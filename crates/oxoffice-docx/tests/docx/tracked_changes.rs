//! Tracked changes written by edits: each edit of a session against its
//! untracked twin. Accepting every revision gives the twin's result,
//! rejecting every revision or only the session's gives the document back,
//! the session's revisions read as the change, and the document validates
//! and reads the same once saved.

use std::io::Cursor;

use oxoffice_docx::{
    Alignment, ComparisonGranularity, ComparisonOptions, ContentLocation, Document, ReplaceOptions,
    RevisionDate, RevisionKind, RunPosition, RunRange, StoryKind, StoryRevision, TrackedChanges,
    TrackedDocument,
};
use oxoffice_opc::OpcPackage;

use crate::support::{doc_with_body, reopen, text_box_run};

const AUTHOR: &str = "Rev";
const DATE: &str = "2026-11-27T10:00:00Z";

fn changes() -> TrackedChanges {
    TrackedChanges::new(AUTHOR, RevisionDate::At(DATE)).unwrap()
}

fn run(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn bold(text: &str) -> String {
    format!(r#"<w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn paragraph(content: &str) -> String {
    format!("<w:p>{content}</w:p>")
}

fn by(author: &str, kind: &str, id: i32, content: &str) -> String {
    format!(
        r#"<w:{kind} w:id="{id}" w:author="{author}" w:date="2026-11-20T09:00:00Z">{content}</w:{kind}>"#
    )
}

/// What a reader sees of each story: every paragraph's text, and for the
/// body each paragraph's style and alignment and each character with its
/// bold and italic states.
type View = (Vec<String>, Vec<(String, Vec<(char, bool, bool)>)>);

fn view(doc: &Document) -> View {
    let texts = doc
        .story_paragraphs()
        .unwrap()
        .into_iter()
        .map(|paragraph| format!("{:?}: {}", paragraph.kind, paragraph.text))
        .collect();
    let body = doc
        .paragraphs()
        .iter()
        .map(|paragraph| {
            let format = format!("{:?} {:?}", paragraph.style_id(), paragraph.alignment());
            let characters = paragraph
                .runs()
                .flat_map(|run| {
                    let (bold, italic) = (run.is_bold(), run.is_italic());
                    run.text()
                        .chars()
                        .map(move |c| (c, bold, italic))
                        .collect::<Vec<_>>()
                })
                .collect();
            (format, characters)
        })
        .collect();
    (texts, body)
}

/// Every revision once: the `mc:Fallback` copy of a text box repeats the
/// revisions of its `mc:Choice`, ids included, as it repeats its bookmarks,
/// and the reader reports both.
fn revisions(doc: &Document) -> Vec<StoryRevision> {
    let mut seen = Vec::new();
    doc.story_revisions()
        .unwrap()
        .into_iter()
        .filter(|revision| {
            let key = (
                revision.id(),
                revision.kind(),
                revision.author().to_owned(),
                revision.text().to_owned(),
            );
            let fresh = !seen.contains(&key);
            seen.push(key);
            fresh
        })
        .collect()
}

/// The session's revisions: kind and text, in order.
fn session_revisions(doc: &Document) -> Vec<(RevisionKind, String)> {
    revisions(doc)
        .into_iter()
        .filter(|revision| revision.author() == AUTHOR)
        .map(|revision| (revision.kind(), revision.text().to_owned()))
        .collect()
}

/// The location of the first paragraph of a story of this kind.
fn located(doc: &Document, kind: StoryKind) -> ContentLocation {
    doc.story_paragraphs()
        .unwrap()
        .into_iter()
        .find(|paragraph| paragraph.location.story().kind() == kind)
        .unwrap_or_else(|| panic!("a {kind:?} paragraph"))
        .location
}

/// A body whose first paragraph holds a bookmark (id 0) and a comment (id 0),
/// and a header whose text is another author's insertion (id 2): a new
/// revision takes neither id.
fn annotated() -> Document {
    let mut doc = doc_with_body(&paragraph(&format!(
        r#"<w:bookmarkStart w:id="0" w:name="mark"/>{}<w:bookmarkEnd w:id="0"/>{}"#,
        run("Ship four "),
        run("units now.")
    )));
    doc.set_header("Heading line");
    let range = RunRange {
        start: RunPosition {
            body_index: 0,
            run_index: 0,
        },
        end: RunPosition {
            body_index: 0,
            run_index: 1,
        },
    };
    doc.add_comment(range, "Ann", None, "a note").unwrap();
    let doc = with_header(&mut doc, |xml| {
        let start = xml.find("<w:r>").expect("a header run");
        let end = xml[start..].find("</w:r>").unwrap() + start + "</w:r>".len();
        format!(
            "{}{}{}",
            &xml[..start],
            by("Ann", "ins", 2, &xml[start..end]),
            &xml[end..]
        )
    });
    assert!(
        doc.story_revisions()
            .unwrap()
            .iter()
            .any(|revision| revision.id() == 2),
        "the header holds Ann's insertion"
    );
    doc
}

/// `doc` saved with its header part's XML edited.
fn with_header(doc: &mut Document, edit: impl FnOnce(&str) -> String) -> Document {
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let header = package
        .parts
        .keys()
        .find(|name| name.contains("header"))
        .cloned()
        .unwrap();
    let xml = String::from_utf8(package.get_part(&header).unwrap().to_vec()).unwrap();
    package.set_part(&header, edit(&xml).into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    Document::from_bytes(&bytes.into_inner()).unwrap()
}

fn run_text(doc: &mut Document, index: usize, text: &'static str) {
    let mut paragraph = doc.paragraph_mut(0).unwrap();
    let path = paragraph.run_path(index).unwrap();
    paragraph.edit_run(&path, |run| run.set_text(text)).unwrap();
}

type Tracked = fn(&mut TrackedDocument<'_>);

/// The session's revisions of a run made bold, then text typed inside it:
/// the formatting change on each part of the run, the insertion between.
const REVISIONS_FORMAT_THEN_INSERT: &[(RevisionKind, &str)] = &[
    (RevisionKind::RunPropertyChange, "Plain "),
    (RevisionKind::Insertion, "new "),
    (RevisionKind::RunPropertyChange, "words."),
];
type Untracked = fn(&mut Document);

struct Row {
    shape: &'static str,
    document: fn() -> Document,
    tracked: Tracked,
    untracked: Untracked,
    revisions: &'static [(RevisionKind, &'static str)],
    /// Whether rejecting the session's revisions alone gives the document
    /// back: not when it dropped text its author had inserted before.
    reject_own: bool,
    /// Whether the word comparison of the original with the untracked edit
    /// writes other changes: a replacement deletes the whole match where the
    /// comparison keeps the words a value repeats, and a property change is
    /// written per run element where the comparison writes its own runs.
    unlike_compare: bool,
}

#[test]
fn every_tracked_text_edit_resolves_to_its_untracked_twin_and_to_the_original() {
    let rows = [
        Row {
            shape: "a word replaced",
            document: || doc_with_body(&paragraph(&run("The quick brown fox jumps."))),
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.set_text("The slow brown fox jumps."))
                    .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .set_text("The slow brown fox jumps.")
                    .unwrap()
            },
            revisions: &[
                (RevisionKind::Deletion, "quick"),
                (RevisionKind::Insertion, "slow"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a word inserted in a run",
            document: || doc_with_body(&paragraph(&run("The quick fox jumps."))),
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.set_text("The quick brown fox jumps."))
                    .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .set_text("The quick brown fox jumps.")
                    .unwrap()
            },
            revisions: &[(RevisionKind::Insertion, "brown ")],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "two words deleted, the space between them with them",
            document: || doc_with_body(&paragraph(&run("The quick brown fox jumps."))),
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.set_text("The fox jumps."))
                    .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .set_text("The fox jumps.")
                    .unwrap()
            },
            revisions: &[(RevisionKind::Deletion, "quick brown ")],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a replacement across two runs keeps the first run's formatting",
            document: || doc_with_body(&paragraph(&(run("Pay within 30 ") + &bold("days net.")))),
            tracked: |doc| {
                doc.try_replace_text("30 days", "45 days").unwrap();
            },
            untracked: |doc| {
                doc.try_replace_text("30 days", "45 days").unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "30 days"),
                (RevisionKind::Insertion, "45 days"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "inside a link",
            document: || {
                doc_with_body(&paragraph(&format!(
                    r#"{}<w:hyperlink w:anchor="target">{}</w:hyperlink>{}"#,
                    run("Pay "),
                    run("within 30 days"),
                    run(".")
                )))
            },
            tracked: |doc| {
                doc.try_replace_text("30 days", "45 days").unwrap();
            },
            untracked: |doc| {
                doc.try_replace_text("30 days", "45 days").unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "30 days"),
                (RevisionKind::Insertion, "45 days"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "inside an inline control",
            document: || {
                doc_with_body(&paragraph(&format!(
                    r#"{}<w:sdt><w:sdtPr><w:tag w:val="party"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>"#,
                    run("Between "),
                    run("Acme Ltd")
                )))
            },
            tracked: |doc| {
                doc.try_replace_text("Acme Ltd", "Zenith Ltd").unwrap();
            },
            untracked: |doc| {
                doc.try_replace_text("Acme Ltd", "Zenith Ltd").unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "Acme Ltd"),
                (RevisionKind::Insertion, "Zenith Ltd"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a tab added to a run",
            document: || doc_with_body(&paragraph(&run("Name"))),
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.run_mut(0).unwrap().add_tab();
                    Ok(())
                })
                .unwrap()
            },
            untracked: |doc| doc.paragraph_mut(0).unwrap().run_mut(0).unwrap().add_tab(),
            revisions: &[(RevisionKind::Insertion, "\t")],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a second edit of the session's own insertion",
            document: || doc_with_body(&paragraph(&run("Ship four units."))),
            tracked: |doc| {
                doc.try_replace_text("four", "six").unwrap();
                doc.try_replace_text("six", "seven").unwrap();
            },
            untracked: |doc| {
                doc.try_replace_text("four", "six").unwrap();
                doc.try_replace_text("six", "seven").unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "four"),
                (RevisionKind::Insertion, "seven"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "inside another author's insertion",
            document: || {
                doc_with_body(&paragraph(&format!(
                    "{}{}",
                    run("Kept "),
                    by("Ann", "ins", 1, &run("new words here"))
                )))
            },
            tracked: |doc| {
                doc.try_replace_text("words", "terms").unwrap();
            },
            untracked: |doc| {
                doc.try_replace_text("words", "terms").unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "words"),
                (RevisionKind::Insertion, "terms"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "around another author's deletion",
            document: || {
                doc_with_body(&paragraph(&format!(
                    "{}{}{}",
                    run("Keep "),
                    by(
                        "Ann",
                        "del",
                        1,
                        r#"<w:r><w:delText xml:space="preserve">gone </w:delText></w:r>"#
                    ),
                    run("this text.")
                )))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.set_text("Keep that text."))
                    .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .set_text("Keep that text.")
                    .unwrap()
            },
            revisions: &[
                (RevisionKind::Deletion, "this"),
                (RevisionKind::Insertion, "that"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a deletion another author's deletion cuts, one id per element",
            document: || {
                doc_with_body(&paragraph(&format!(
                    "{}{}{}",
                    run("Pay within 30 "),
                    by(
                        "Ann",
                        "del",
                        1,
                        r#"<w:r><w:delText xml:space="preserve">old </w:delText></w:r>"#
                    ),
                    run("days net.")
                )))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.set_text("Pay within net."))
                    .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .set_text("Pay within net.")
                    .unwrap()
            },
            revisions: &[
                (RevisionKind::Deletion, "30 "),
                (RevisionKind::Deletion, "days "),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a word made bold",
            document: || doc_with_body(&paragraph(&(run("Plain ") + &run("words.")))),
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.run_mut(1).unwrap().set_bold(true);
                    Ok(())
                })
                .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .run_mut(1)
                    .unwrap()
                    .set_bold(true)
            },
            revisions: &[(RevisionKind::RunPropertyChange, "words.")],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "the author's own inserted text made bold, inside the insertion (W3)",
            document: || {
                doc_with_body(&paragraph(&format!(
                    "{}{}",
                    run("Kept "),
                    by(AUTHOR, "ins", 1, &run("new words"))
                )))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    let path = p.run_path(1).unwrap();
                    p.edit_run(&path, |run| run.set_bold(true))
                })
                .unwrap()
            },
            untracked: |doc| {
                let mut p = doc.paragraph_mut(0).unwrap();
                let path = p.run_path(1).unwrap();
                p.edit_run(&path, |run| run.set_bold(true)).unwrap()
            },
            revisions: &[
                (RevisionKind::Insertion, "new words"),
                (RevisionKind::RunPropertyChange, "new words"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a second change of a run's properties keeps the oldest (W4)",
            document: || {
                doc_with_body(&paragraph(
                    r#"<w:r><w:rPr><w:b/><w:rPrChange w:id="1" w:author="Ann" w:date="2026-11-20T09:00:00Z"><w:rPr/></w:rPrChange></w:rPr><w:t>Bold words</w:t></w:r>"#,
                ))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.run_mut(0).unwrap().set_italic(true);
                    Ok(())
                })
                .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .run_mut(0)
                    .unwrap()
                    .set_italic(true)
            },
            revisions: &[(RevisionKind::RunPropertyChange, "Bold words")],
            // Rejecting the one change brings back the run before Ann's.
            reject_own: false,
            unlike_compare: true,
        },
        Row {
            shape: "a run made bold, then text typed inside it, one id per element",
            document: || doc_with_body(&paragraph(&run("Plain words."))),
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.run_mut(0).unwrap().set_bold(true);
                    Ok(())
                })
                .unwrap();
                doc.edit_body_paragraph(0, |p| {
                    p.run_mut(0).unwrap().set_text("Plain new words.");
                    Ok(())
                })
                .unwrap();
            },
            untracked: |doc| {
                let mut p = doc.paragraph_mut(0).unwrap();
                p.run_mut(0).unwrap().set_bold(true);
                p.run_mut(0).unwrap().set_text("Plain new words.");
            },
            revisions: REVISIONS_FORMAT_THEN_INSERT,
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "text typed inside a run another author made bold",
            document: || {
                doc_with_body(&paragraph(
                    r#"<w:r><w:rPr><w:b/><w:rPrChange w:id="1" w:author="Ann" w:date="2026-11-20T09:00:00Z"><w:rPr/></w:rPrChange></w:rPr><w:t>Plain words.</w:t></w:r>"#,
                ))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.run_mut(0).unwrap().set_text("Plain new words.");
                    Ok(())
                })
                .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .run_mut(0)
                    .unwrap()
                    .set_text("Plain new words.")
            },
            revisions: &[(RevisionKind::Insertion, "new ")],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a paragraph centred and given a style, one change (W5)",
            document: || doc_with_body(&paragraph(&run("A title"))),
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.set_alignment(Alignment::Center);
                    p.set_style("Heading1");
                    Ok(())
                })
                .unwrap()
            },
            untracked: |doc| {
                let mut p = doc.paragraph_mut(0).unwrap();
                p.set_alignment(Alignment::Center);
                p.set_style("Heading1");
            },
            revisions: &[(RevisionKind::ParagraphPropertyChange, "A title")],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a field added, as its fldChar runs",
            document: || doc_with_body(&paragraph(&run("Page "))),
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.add_run("").add_complex_field("PAGE", "1"))
                    .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .add_run("")
                    .add_complex_field("PAGE", "1")
                    .unwrap()
            },
            revisions: &[(RevisionKind::Insertion, "1")],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a paragraph inserted between two",
            document: || doc_with_body(&(paragraph(&run("First.")) + &paragraph(&run("Last.")))),
            tracked: |doc| {
                doc.insert_paragraph(1, "Middle.").unwrap();
            },
            untracked: |doc| {
                doc.insert_paragraph(1, "Middle.");
            },
            revisions: &[
                (RevisionKind::Insertion, ""),
                (RevisionKind::Insertion, "Middle."),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a paragraph appended, the final mark unmarked (W12)",
            document: || doc_with_body(&paragraph(&run("First."))),
            tracked: |doc| {
                doc.add_paragraph("End.").unwrap();
            },
            untracked: |doc| {
                doc.add_paragraph("End.");
            },
            revisions: &[
                (RevisionKind::Insertion, ""),
                (RevisionKind::Insertion, "End."),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a paragraph appended after a styled one: its properties a change of that one's",
            document: || {
                doc_with_body(
                    r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/><w:spacing w:after="120"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>"#,
                )
            },
            tracked: |doc| {
                doc.add_paragraph("Body.").unwrap();
            },
            untracked: |doc| {
                doc.add_paragraph("Body.");
            },
            // The mark before, the new properties, the new text.
            revisions: &[
                (RevisionKind::Insertion, ""),
                (RevisionKind::ParagraphPropertyChange, "Body."),
                (RevisionKind::Insertion, "Body."),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a table inserted, each row marked",
            document: || doc_with_body(&(paragraph(&run("First.")) + &paragraph(&run("Last.")))),
            tracked: |doc| {
                doc.insert_table(1, 1, 2).unwrap();
            },
            untracked: |doc| {
                doc.insert_table(1, 1, 2);
            },
            // The row and each cell's mark.
            revisions: &[
                (RevisionKind::Insertion, ""),
                (RevisionKind::Insertion, ""),
                (RevisionKind::Insertion, ""),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a paragraph removed between two",
            document: || {
                doc_with_body(
                    &(paragraph(&run("First."))
                        + &paragraph(&run("Middle."))
                        + &paragraph(&run("Last."))),
                )
            },
            tracked: |doc| doc.remove_content_range(1, 2).unwrap(),
            untracked: |doc| {
                doc.remove_content_range(1, 2).unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, ""),
                (RevisionKind::Deletion, "Middle."),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a heading removed with its bookmark start between blocks, which stays (Google)",
            document: || {
                doc_with_body(&format!(
                    r#"{}<w:bookmarkStart w:id="1" w:name="h.1"/><w:p><w:r><w:t>Heading</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>{}"#,
                    paragraph(&run("First.")),
                    paragraph(&run("Last."))
                ))
            },
            tracked: |doc| doc.remove_content_range(1, 3).unwrap(),
            untracked: |doc| {
                doc.remove_content_range(1, 3).unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, ""),
                (RevisionKind::Deletion, "Heading"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "the last paragraphs removed, the final mark kept (W12)",
            document: || {
                doc_with_body(
                    &(paragraph(&run("First."))
                        + &paragraph(&run("Second."))
                        + r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>Last.</w:t></w:r></w:p>"#),
                )
            },
            tracked: |doc| doc.remove_content_range(1, 3).unwrap(),
            untracked: |doc| {
                doc.remove_content_range(1, 3).unwrap();
            },
            // The mark before them, theirs but the final one, their content,
            // and the final paragraph's properties, which take the first's.
            revisions: &[
                (RevisionKind::Deletion, ""),
                (RevisionKind::Deletion, ""),
                (RevisionKind::Deletion, "Second."),
                (RevisionKind::ParagraphPropertyChange, ""),
                (RevisionKind::Deletion, "Last."),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a table removed, each row marked",
            document: || {
                doc_with_body(&format!(
                    r#"{}<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>{}"#,
                    paragraph(&run("First.")),
                    paragraph(&run("Last."))
                ))
            },
            tracked: |doc| doc.remove_content_range(1, 2).unwrap(),
            untracked: |doc| {
                doc.remove_content_range(1, 2).unwrap();
            },
            // The row, its cell's mark and content.
            revisions: &[
                (RevisionKind::Deletion, ""),
                (RevisionKind::Deletion, ""),
                (RevisionKind::Deletion, "cell"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a cell paragraph",
            document: || {
                doc_with_body(&format!(
                    r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr>{}</w:tc></w:tr></w:tbl><w:p/>"#,
                    paragraph(&run("Old cell text"))
                ))
            },
            tracked: |doc| {
                doc.edit_cell_paragraph(0, 0, 0, 0, |p| p.set_text("New cell text"))
                    .unwrap()
            },
            untracked: |doc| {
                doc.edit_cell_paragraph(0, 0, 0, 0, |p| p.set_text("New cell text"))
                    .unwrap()
            },
            revisions: &[
                (RevisionKind::Deletion, "Old"),
                (RevisionKind::Insertion, "New"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a header paragraph",
            document: || {
                let mut doc = doc_with_body(&paragraph(&run("Body.")));
                doc.set_header("Draft for review");
                reopen(&mut doc)
            },
            tracked: |doc| {
                doc.try_replace_text("Draft for", "Final for").unwrap();
            },
            untracked: |doc| {
                doc.try_replace_text("Draft for", "Final for").unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "Draft for"),
                (RevisionKind::Insertion, "Final for"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a footnote paragraph",
            document: || {
                let mut doc = doc_with_body(&paragraph(&run("Body.")));
                let note = doc.add_footnote("Source: the 2025 survey.");
                doc.paragraph_mut(0).unwrap().add_footnote_ref(note);
                reopen(&mut doc)
            },
            tracked: |doc| {
                doc.try_replace_all_checked(
                    &[("2025 survey", "2026 survey", Some(1))],
                    ReplaceOptions::default(),
                )
                .unwrap()
                .unwrap();
            },
            untracked: |doc| {
                doc.try_replace_all_checked(
                    &[("2025 survey", "2026 survey", Some(1))],
                    ReplaceOptions::default(),
                )
                .unwrap()
                .unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "2025 survey"),
                (RevisionKind::Insertion, "2026 survey"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a regular expression replacement",
            document: || doc_with_body(&paragraph(&run("Pay within 30 days, or 60 days late."))),
            tracked: |doc| {
                doc.try_replace_all_regex_expected(&[(r"(\d+) days", "$1 weeks", Some(2))])
                    .unwrap()
                    .unwrap();
            },
            untracked: |doc| {
                doc.try_replace_all_regex_expected(&[(r"(\d+) days", "$1 weeks", Some(2))])
                    .unwrap()
                    .unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "30 days"),
                (RevisionKind::Insertion, "30 weeks"),
                (RevisionKind::Deletion, "60 days"),
                (RevisionKind::Insertion, "60 weeks"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a replacement at a location deletes the whole match too",
            document: || doc_with_body(&paragraph(&run("Pay within 30 days net."))),
            tracked: |doc| {
                let location = located(doc.document(), StoryKind::Body);
                doc.try_replace_text_at(&location, "30 days", "45 days", Some(1))
                    .unwrap()
                    .unwrap();
            },
            untracked: |doc| {
                let location = located(doc, StoryKind::Body);
                doc.try_replace_text_at(&location, "30 days", "45 days", Some(1))
                    .unwrap()
                    .unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "30 days"),
                (RevisionKind::Insertion, "45 days"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "each occurrence the replacement reached, the other kept",
            document: || {
                doc_with_body(&paragraph(&format!(
                    "{}{}{}",
                    run("Pay 30 days, then 30 "),
                    by("Ann", "ins", 1, &run("days")),
                    run(".")
                )))
            },
            tracked: |doc| {
                doc.try_replace_text("30 days", "45 days").unwrap();
            },
            untracked: |doc| {
                doc.try_replace_text("30 days", "45 days").unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "30 days"),
                (RevisionKind::Insertion, "45 days"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "a replacement the matches do not explain, aligned word by word",
            document: || {
                doc_with_body(&paragraph(&format!(
                    "{}{}{}",
                    run("Pay 30 "),
                    by("Ann", "ins", 1, &run("days")),
                    run(" and 30 days.")
                )))
            },
            tracked: |doc| {
                doc.try_replace_text("30 days", "30").unwrap();
            },
            untracked: |doc| {
                doc.try_replace_text("30 days", "30").unwrap();
            },
            revisions: &[(RevisionKind::Deletion, " days")],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a replacement deletes the whole match, the words its value repeats too",
            document: || doc_with_body(&paragraph(&run("Pay within 30 days net."))),
            tracked: |doc| {
                doc.try_replace_text("30 days", "45 days").unwrap();
            },
            untracked: |doc| {
                doc.try_replace_text("30 days", "45 days").unwrap();
            },
            revisions: &[
                (RevisionKind::Deletion, "30 days"),
                (RevisionKind::Insertion, "45 days"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "typing inside the author's insertion of an earlier session splits it",
            document: || {
                doc_with_body(&paragraph(&format!(
                    "{}{}",
                    run("Kept "),
                    by(AUTHOR, "ins", 1, &run("alpha beta gamma"))
                )))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.set_text("Kept alpha XYZ beta gamma"))
                    .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .set_text("Kept alpha XYZ beta gamma")
                    .unwrap()
            },
            revisions: &[
                (RevisionKind::Insertion, "alpha "),
                (RevisionKind::Insertion, "XYZ "),
                (RevisionKind::Insertion, "beta gamma"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "deleting inside the author's insertion of an earlier session drops the text",
            document: || {
                doc_with_body(&paragraph(&format!(
                    "{}{}",
                    run("Kept "),
                    by(AUTHOR, "ins", 1, &run("alpha beta gamma"))
                )))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.set_text("Kept alpha beta"))
                    .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .set_text("Kept alpha beta")
                    .unwrap()
            },
            revisions: &[(RevisionKind::Insertion, "alpha beta")],
            reject_own: false,
            unlike_compare: false,
        },
        Row {
            shape: "a header paragraph, by its location",
            document: || {
                let mut doc = doc_with_body(&paragraph(&run("Body.")));
                doc.set_header("Draft for review");
                reopen(&mut doc)
            },
            tracked: |doc| {
                let location = located(doc.document(), StoryKind::Header);
                doc.edit_paragraph(&location, |p| p.set_text("Final for review"))
                    .unwrap()
            },
            untracked: |doc| {
                let location = located(doc, StoryKind::Header);
                doc.edit_story_paragraph(&location, |p| p.set_text("Final for review"))
                    .unwrap()
            },
            revisions: &[
                (RevisionKind::Deletion, "Draft"),
                (RevisionKind::Insertion, "Final"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a footnote paragraph, by its location",
            document: || {
                let mut doc = doc_with_body(&paragraph(&run("Body.")));
                let note = doc.add_footnote("Source: the 2025 survey.");
                doc.paragraph_mut(0).unwrap().add_footnote_ref(note);
                reopen(&mut doc)
            },
            tracked: |doc| {
                let location = located(doc.document(), StoryKind::Footnote);
                doc.edit_paragraph(&location, |p| p.set_text("Source: the 2026 survey."))
                    .unwrap()
            },
            untracked: |doc| {
                let location = located(doc, StoryKind::Footnote);
                doc.edit_story_paragraph(&location, |p| p.set_text("Source: the 2026 survey."))
                    .unwrap()
            },
            // `set_text` also drops the space Word writes after the reference.
            revisions: &[
                (RevisionKind::Deletion, " "),
                (RevisionKind::Deletion, "2025"),
                (RevisionKind::Insertion, "2026"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "a text box paragraph, by its location",
            document: || {
                doc_with_body(&paragraph(
                    &(run("See ") + &text_box_run(1, &paragraph(&run("Boxed old text")))),
                ))
            },
            tracked: |doc| {
                let location = located(doc.document(), StoryKind::TextBox);
                doc.edit_paragraph(&location, |p| p.set_text("Boxed new text"))
                    .unwrap()
            },
            untracked: |doc| {
                let location = located(doc, StoryKind::TextBox);
                doc.edit_story_paragraph(&location, |p| p.set_text("Boxed new text"))
                    .unwrap()
            },
            revisions: &[
                (RevisionKind::Deletion, "old"),
                (RevisionKind::Insertion, "new"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
        Row {
            shape: "set_story_text keeps the first run's formatting, as the untracked call",
            document: || {
                doc_with_body(&paragraph(
                    r#"<w:r><w:rPr><w:b/><w:color w:val="FF0000"/></w:rPr><w:t xml:space="preserve">Old title here</w:t></w:r>"#,
                ))
            },
            tracked: |doc| {
                let location = located(doc.document(), StoryKind::Body);
                doc.set_story_text(&location, "Old title there").unwrap()
            },
            untracked: |doc| {
                let location = located(doc, StoryKind::Body);
                doc.set_story_text(&location, "Old title there").unwrap()
            },
            revisions: &[
                (RevisionKind::Deletion, "here"),
                (RevisionKind::Insertion, "there"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "set_story_text in a header, as the untracked call",
            document: || {
                let mut doc = doc_with_body(&paragraph(&run("Body.")));
                doc.set_header("Draft for review");
                reopen(&mut doc)
            },
            tracked: |doc| {
                let location = located(doc.document(), StoryKind::Header);
                doc.set_story_text(&location, "Final for review").unwrap()
            },
            untracked: |doc| {
                let location = located(doc, StoryKind::Header);
                doc.set_story_text(&location, "Final for review").unwrap()
            },
            revisions: &[
                (RevisionKind::Deletion, "Draft"),
                (RevisionKind::Insertion, "Final"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "two edits of one session, the second inside the first's insertion",
            document: || doc_with_body(&paragraph(&run("The quick fox jumps."))),
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.set_text("The quick red fox jumps."))
                    .unwrap();
                doc.edit_body_paragraph(0, |p| p.set_text("The quick reddish fox jumps."))
                    .unwrap();
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .set_text("The quick reddish fox jumps.")
                    .unwrap()
            },
            revisions: &[(RevisionKind::Insertion, "reddish ")],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "inside another author's insertion, which splits",
            document: || {
                doc_with_body(&paragraph(&format!(
                    "{}{}",
                    run("Kept "),
                    by("Ann", "ins", 1, &run("new words here"))
                )))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| p.set_text("Kept new terms here"))
                    .unwrap()
            },
            untracked: |doc| {
                doc.paragraph_mut(0)
                    .unwrap()
                    .set_text("Kept new terms here")
                    .unwrap()
            },
            revisions: &[
                (RevisionKind::Deletion, "words"),
                (RevisionKind::Insertion, "terms"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "text in a link",
            document: || {
                doc_with_body(&paragraph(&format!(
                    r#"{}<w:hyperlink w:anchor="target">{}</w:hyperlink>{}"#,
                    run("See "),
                    run("the old section"),
                    run(".")
                )))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    let path = p.run_path(1).unwrap();
                    p.edit_run(&path, |run| run.set_text("the new section"))
                })
                .unwrap()
            },
            untracked: |doc| run_text(doc, 1, "the new section"),
            revisions: &[
                (RevisionKind::Deletion, "old"),
                (RevisionKind::Insertion, "new"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "text in an inline control",
            document: || {
                doc_with_body(&paragraph(&format!(
                    r#"{}<w:sdt><w:sdtPr><w:tag w:val="party"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>"#,
                    run("Between "),
                    run("Acme Ltd")
                )))
            },
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    let path = p.run_path(1).unwrap();
                    p.edit_run(&path, |run| run.set_text("Zenith Ltd"))
                })
                .unwrap()
            },
            untracked: |doc| run_text(doc, 1, "Zenith Ltd"),
            revisions: &[
                (RevisionKind::Deletion, "Acme"),
                (RevisionKind::Insertion, "Zenith"),
            ],
            reject_own: true,
            unlike_compare: false,
        },
        Row {
            shape: "beside a bookmark and a comment, ids clear of theirs and of another part's revision",
            document: annotated,
            tracked: |doc| {
                doc.edit_body_paragraph(0, |p| {
                    let path = p.run_path(0).unwrap();
                    p.edit_run(&path, |run| run.set_text("Ship six "))
                })
                .unwrap()
            },
            untracked: |doc| run_text(doc, 0, "Ship six "),
            revisions: &[
                (RevisionKind::Deletion, "four"),
                (RevisionKind::Insertion, "six"),
            ],
            reject_own: true,
            unlike_compare: true,
        },
    ];
    for row in rows {
        let shape = row.shape;
        let original = (row.document)();
        let mut untracked = (row.document)();
        (row.untracked)(&mut untracked);
        let mut tracked = (row.document)();
        let ids = {
            let changes = changes();
            let mut session = tracked.track_changes(&changes);
            (row.tracked)(&mut session);
            session.revision_ids()
        };
        let mut tracked = reopen(&mut tracked);
        assert_eq!(
            session_revisions(&tracked),
            row.revisions
                .iter()
                .map(|(k, t)| (*k, t.to_string()))
                .collect::<Vec<_>>(),
            "{shape}"
        );
        let held = original
            .story_revisions()
            .unwrap()
            .into_iter()
            .map(|revision| revision.id())
            .collect::<Vec<_>>();
        let mut written = tracked
            .story_revisions()
            .unwrap()
            .into_iter()
            .filter(|revision| revision.author() == AUTHOR && revision.timestamp() == Some(DATE))
            .map(|revision| revision.id())
            .collect::<Vec<_>>();
        written.sort();
        written.dedup();
        let mut session_ids = ids.clone();
        session_ids.sort();
        assert_eq!(session_ids, written, "{shape}");
        // A new revision takes no id a revision of any story, a comment or a
        // bookmark held, and no two revisions share one.
        let taken = held
            .iter()
            .copied()
            .chain(original.comments().iter().map(|comment| comment.id()))
            .chain(
                original
                    .bookmarks()
                    .iter()
                    .filter_map(|bookmark| bookmark.id()),
            )
            .collect::<Vec<_>>();
        assert!(
            ids.iter().all(|id| !taken.contains(id)),
            "{shape}: {ids:?} {taken:?}"
        );
        let mut all = revisions(&tracked)
            .into_iter()
            .map(|revision| revision.id())
            .collect::<Vec<_>>();
        let count = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), count, "{shape}: a revision id is repeated");
        assert!(
            tracked
                .story_revisions()
                .unwrap()
                .iter()
                .filter(|revision| ids.contains(&revision.id()))
                .all(|revision| revision.author() == AUTHOR && revision.timestamp() == Some(DATE)),
            "{shape}"
        );
        assert!(tracked.validate().unwrap().is_valid(), "{shape}");

        let mut accepted = reopen(&mut tracked);
        accepted.accept_all().unwrap();
        let mut untracked_accepted = reopen(&mut untracked);
        untracked_accepted.accept_all().unwrap();
        assert_eq!(
            view(&reopen(&mut accepted)),
            view(&untracked_accepted),
            "{shape}: accept all"
        );

        let mut rejected = reopen(&mut tracked);
        rejected.reject_all().unwrap();
        let mut original_rejected = (row.document)();
        original_rejected.reject_all().unwrap();
        assert_eq!(
            view(&reopen(&mut rejected)),
            view(&original_rejected),
            "{shape}: reject all"
        );

        // On a document without revisions, the comparison of the original
        // with the untracked edit, word by word, writes the same changes.
        if original.story_revisions().unwrap().is_empty() && !row.unlike_compare {
            let mut compared = (row.document)();
            let options = ComparisonOptions {
                granularity: ComparisonGranularity::Word,
                ..ComparisonOptions::default()
            };
            compared
                .compare_with_options(&reopen(&mut untracked), AUTHOR, DATE, &options)
                .unwrap();
            assert_eq!(
                session_revisions(&reopen(&mut compared)),
                session_revisions(&tracked),
                "{shape}: compare"
            );
        }

        if row.reject_own {
            let mut own = reopen(&mut tracked);
            for id in &ids {
                own.reject_revision_id(*id).unwrap();
            }
            assert_eq!(
                view(&reopen(&mut own)),
                view(&original),
                "{shape}: reject the session's"
            );
        }
    }
}

/// What a session cannot write yet is refused, the document left as it was
/// and no revision id spent.
#[test]
fn an_edit_a_session_cannot_write_is_refused_without_a_change() {
    type Edit = fn(&mut TrackedDocument<'_>) -> oxoffice_docx::Result<()>;
    fn body(content: &str) -> Document {
        doc_with_body(&paragraph(content))
    }
    fn boxed() -> Document {
        body(&(run("See ") + &text_box_run(1, &paragraph(&run("Boxed old text")))))
    }
    /// A count refusal, as an error.
    fn refused<T, E: std::fmt::Display>(
        result: oxoffice_docx::Result<Result<T, E>>,
    ) -> oxoffice_docx::Result<()> {
        result.and_then(|counted| {
            counted
                .map(|_| ())
                .map_err(|refusal| oxoffice_docx::Error::Other(refusal.to_string()))
        })
    }
    let field = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
    let not_yet = "is not recorded as a tracked change yet";
    let cannot = "this edit cannot be written as a tracked change";
    let end = |p: &mut oxoffice_docx::Paragraph<'_>| {
        p.add_run(" End.");
        Ok(())
    };
    let rows: Vec<(&str, Document, Edit, &str)> = vec![
        (
            "a section break removed",
            doc_with_body(&format!(
                r#"{}<w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:pPr><w:r><w:t>Section.</w:t></w:r></w:p>{}"#,
                paragraph(&run("First.")),
                paragraph(&run("Last."))
            )),
            |doc| doc.remove_content_range(1, 2),
            not_yet,
        ),
        (
            "a content control removed",
            doc_with_body(&format!(
                r#"<w:sdt><w:sdtPr/><w:sdtContent>{}</w:sdtContent></w:sdt>{}"#,
                paragraph(&run("Inside.")),
                paragraph(&run("Last."))
            )),
            |doc| doc.remove_content_range(0, 1),
            not_yet,
        ),
        (
            "blocks removed, a later one holding a revision: none takes an id",
            doc_with_body(&format!(
                "{}{}{}{}",
                paragraph(&run("First.")),
                paragraph(&run("Second.")),
                paragraph(&by("Other", "ins", 9, &run("Third."))),
                paragraph(&run("Last."))
            )),
            |doc| doc.remove_content_range(1, 3),
            not_yet,
        ),
        (
            "a field removed",
            doc_with_body(&format!(
                r#"<w:p><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>{}"#,
                paragraph(&run("Last."))
            )),
            |doc| doc.remove_content_range(0, 1),
            not_yet,
        ),
        (
            "the last paragraph removed after a table",
            doc_with_body(&format!(
                r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>{}"#,
                paragraph(&run("cell")),
                paragraph(&run("Last."))
            )),
            |doc| doc.remove_content_range(1, 2),
            not_yet,
        ),
        (
            "a paragraph added after a section break",
            doc_with_body(
                r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:pPr><w:r><w:t>Section.</w:t></w:r></w:p>"#,
            ),
            |doc| doc.add_paragraph("End.").map(|_| ()),
            not_yet,
        ),
        (
            "the paragraph mark made bold",
            body(&run("Plain words.")),
            |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.mark().set_bold(true);
                    Ok(())
                })
            },
            "a change of the paragraph mark's formatting or its section",
        ),
        (
            "a field removed",
            body(&(run("Page ") + field)),
            |doc| doc.edit_body_paragraph(0, |p| p.set_text("Page")),
            not_yet,
        ),
        (
            "text put in a new link",
            body(&run("Plain words.")),
            |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.add_hyperlink(" More", "https://example.com/");
                    Ok(())
                })
            },
            not_yet,
        ),
        (
            "a bookmark start between two deleted words",
            body(&format!(
                r#"{}<w:bookmarkStart w:id="0" w:name="tail"/>{}<w:bookmarkEnd w:id="0"/>"#,
                run("Lead "),
                run("old tail")
            )),
            |doc| doc.edit_body_paragraph(0, |p| p.set_text("new tail")),
            cannot,
        ),
        (
            "a bookmark edge inside a match",
            body(&format!(
                r#"{}<w:bookmarkStart w:id="0" w:name="due"/>{}<w:bookmarkEnd w:id="0"/>{}"#,
                run("Pay 30 "),
                run("days"),
                run(" net.")
            )),
            |doc| doc.try_replace_text("30 days", "45 days").map(|_| ()),
            cannot,
        ),
        (
            "a text box, literal",
            boxed(),
            |doc| doc.try_replace_text("old text", "new text").map(|_| ()),
            "a replacement in a text box",
        ),
        (
            "a text box, regular expression",
            boxed(),
            |doc| refused(doc.try_replace_all_regex_expected(&[("old (text)", "new $1", None)])),
            "a replacement in a text box",
        ),
        (
            "a text box, at its location",
            boxed(),
            |doc| {
                let location = located(doc.document(), StoryKind::TextBox);
                refused(doc.try_replace_text_at(&location, "old text", "new text", None))
            },
            "a replacement in a text box",
        ),
        (
            "a cell paragraph of a table in a text box, at its location",
            body(
                &(run("See ")
                    + &text_box_run(
                        1,
                        &format!(
                            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>{}"#,
                            paragraph(&run("Boxed old text")),
                            paragraph("")
                        ),
                    )),
            ),
            |doc| {
                let location = located(doc.document(), StoryKind::TableCell);
                refused(doc.try_replace_text_at(&location, "old text", "new text", None))
            },
            "a replacement in a text box",
        ),
        (
            "a table in a header",
            {
                let mut doc = body(&run("Body."));
                doc.set_header("Heading line");
                with_header(&mut doc, |xml| {
                    let table = r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>Draft cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
                    xml.replacen("<w:p>", &format!("{table}<w:p>"), 1)
                })
            },
            |doc| doc.try_replace_text("Draft cell", "Final cell").map(|_| ()),
            "a replacement in a header or footer table or control",
        ),
        (
            "a whole table at its location",
            doc_with_body(&format!(
                r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>{}"#,
                paragraph(&run("Old cell text")),
                paragraph(&run("Body."))
            )),
            |doc| {
                let story = doc.document().stories().unwrap().remove(0);
                let table = doc.document().story_tables(&story).unwrap().remove(0);
                refused(doc.try_replace_text_at(&table, "Old", "New", None))
            },
            "a replacement in a whole table or control",
        ),
        (
            "a checked batch whose second pair refuses",
            body(&run("The quick brown fox.")),
            |doc| {
                refused(doc.try_replace_all_checked(
                    &[("quick", "slow", Some(1)), ("nonexistent", "x", Some(1))],
                    ReplaceOptions::default(),
                ))
            },
            "expected 1 replacement(s)",
        ),
        (
            "a regular expression batch whose second pair refuses",
            body(&run("The quick brown fox.")),
            |doc| {
                refused(doc.try_replace_all_regex_expected(&[
                    ("quick", "slow", Some(1)),
                    ("nonexistent", "x", Some(1)),
                ]))
            },
            "expected 1 replacement(s)",
        ),
    ];
    for (shape, mut doc, edit, message) in rows {
        let before = doc.to_bytes().unwrap();
        let changes = changes();
        let mut session = doc.track_changes(&changes);
        let error = edit(&mut session).expect_err(shape);
        assert!(session.revision_ids().is_empty(), "{shape}");
        assert!(error.to_string().contains(message), "{shape}: {error}");
        // The refusal spent no id: the next edit takes the ids a fresh
        // session would.
        let paragraph = session.document().paragraphs().len() - 1;
        session.edit_body_paragraph(paragraph, end).unwrap();
        let after = session.revision_ids();
        drop(session);
        let mut fresh = Document::from_bytes(&before).unwrap();
        let mut again = fresh.track_changes(&changes);
        again.edit_body_paragraph(paragraph, end).unwrap();
        assert_eq!(
            after,
            again.revision_ids(),
            "{shape}: ids after the refusal"
        );
        drop(again);
        assert_eq!(
            doc.to_bytes().unwrap(),
            fresh.to_bytes().unwrap(),
            "{shape}"
        );
    }
}

/// A session refuses an author or a date Word cannot read.
#[test]
fn a_session_refuses_an_author_or_date_it_cannot_write() {
    for (author, date) in [
        ("", RevisionDate::At(DATE)),
        ("Rev\u{1}", RevisionDate::At(DATE)),
        (AUTHOR, RevisionDate::At("27 November 2026")),
    ] {
        assert!(TrackedChanges::new(author, date).is_err(), "{author:?}");
    }
}

/// A panic inside a tracked edit ends the session's hold on the document:
/// the next plain edit writes no revision.
#[test]
fn a_panic_in_a_tracked_edit_leaves_later_edits_untracked() {
    let mut doc = doc_with_body(&paragraph(&run("Plain words.")));
    let changes = changes();
    let mut session = doc.track_changes(&changes);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = session.edit_body_paragraph(0, |_| -> oxoffice_docx::Result<()> {
            panic!("an edit that fails")
        });
    }));
    assert!(caught.is_err());
    drop(session);
    let location = located(&doc, StoryKind::Body);
    doc.edit_story_paragraph(&location, |p| p.set_text("Other words."))
        .unwrap();
    let doc = reopen(&mut doc);
    assert!(doc.story_revisions().unwrap().is_empty());
    assert_eq!(doc.paragraphs()[0].text(), "Other words.");
}

/// Table, row, cell and section property edits under a session, as their
/// routes make them: each writes the changes Word writes (W8), accepting
/// them gives the untracked edit, and rejecting gives the document back.
#[test]
fn every_table_and_section_property_edit_writes_a_property_change() {
    type Edit = fn(&mut Document);
    let table = || {
        doc_with_body(&format!(
            r#"<w:tbl><w:tblPr><w:tblW w:w="4000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr>{0}</w:tc><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr>{0}</w:tc></w:tr></w:tbl>{0}"#,
            paragraph(&run("cell"))
        ))
    };
    use RevisionKind::*;
    let rows: [(&str, Edit, &[RevisionKind]); 5] = [
        (
            "a table's width",
            |doc| {
                doc.edit_table_properties_in_place(0, |table| {
                    table.set_width(oxoffice_docx::Length::twips(5000));
                    Ok(())
                })
                .unwrap()
                .unwrap()
            },
            &[TablePropertyChange],
        ),
        (
            "a column's width",
            |doc| {
                doc.edit_table_properties_in_place(0, |table| {
                    table.set_column_width(0, oxoffice_docx::Length::twips(3000));
                    Ok(())
                })
                .unwrap()
                .unwrap()
            },
            // Its cell and the table's width follow it.
            &[TablePropertyChange, TableGridChange, CellPropertyChange],
        ),
        (
            "a cell's shading",
            |doc| {
                doc.edit_table_cell_properties_in_place(0, 0, 0, |cell| {
                    cell.set_shading("FF0000");
                    Ok(())
                })
                .unwrap()
                .unwrap()
            },
            &[CellPropertyChange],
        ),
        (
            "a row's height, as Word writes it (W8)",
            |doc| {
                doc.edit_table_row_properties_in_place(0, 0, |row| {
                    row.set_height(oxoffice_docx::Length::twips(800));
                    Ok(())
                })
                .unwrap()
                .unwrap()
            },
            &[
                TablePropertyChange,
                TableGridChange,
                RowPropertyChange,
                CellPropertyChange,
                CellPropertyChange,
            ],
        ),
        (
            "a section's page size",
            |doc| {
                let before = doc.section_properties_before_edit(0);
                doc.section_mut(0)
                    .unwrap()
                    .set_page_size(
                        oxoffice_docx::Length::twips(15000),
                        oxoffice_docx::Length::twips(15840),
                    )
                    .unwrap();
                if let Some(before) = before {
                    doc.redline_section_edit(0, before).unwrap();
                }
            },
            &[SectionPropertyChange],
        ),
    ];
    let shown = |doc: &mut Document| {
        let xml = reopen(doc).part_xml("/word/document.xml").unwrap();
        let start = xml.find("<w:body>").unwrap();
        xml[start..].to_owned()
    };
    for (shape, edit, kinds) in rows {
        let mut tracked = table();
        let original = shown(&mut tracked);
        let changes = changes();
        tracked.start_tracked_changes(&changes).unwrap();
        edit(&mut tracked);
        let session = tracked.stop_tracked_changes().unwrap();
        let ids = session.revision_ids(&tracked).unwrap();
        let mut written = tracked
            .story_revisions()
            .unwrap()
            .into_iter()
            .filter(|revision| ids.contains(&revision.id()))
            .map(|revision| revision.kind())
            .collect::<Vec<_>>();
        written.sort_by_key(|kind| format!("{kind:?}"));
        let mut expected = kinds.to_vec();
        expected.sort_by_key(|kind| format!("{kind:?}"));
        assert_eq!(written, expected, "{shape}");
        let mut untracked = table();
        edit(&mut untracked);
        let mut accepted = reopen(&mut tracked);
        accepted.accept_all().unwrap();
        assert_eq!(
            shown(&mut accepted),
            shown(&mut untracked),
            "{shape}: accept all"
        );
        let mut rejected = reopen(&mut tracked);
        rejected.reject_all().unwrap();
        assert_eq!(shown(&mut rejected), original, "{shape}: reject all");
    }
}

/// A section's change rejected keeps its header reference, which its old
/// value leaves out, first, as the schema orders it.
#[test]
fn a_rejected_section_change_keeps_its_references_first() {
    let mut doc = doc_with_body(&paragraph(&run("Body.")));
    doc.set_header("Head");
    let mut doc = reopen(&mut doc);
    let changes = changes();
    doc.start_tracked_changes(&changes).unwrap();
    let before = doc.section_properties_before_edit(0).unwrap();
    doc.section_mut(0)
        .unwrap()
        .set_page_size(
            oxoffice_docx::Length::twips(15000),
            oxoffice_docx::Length::twips(15840),
        )
        .unwrap();
    doc.redline_section_edit(0, before).unwrap();
    doc.stop_tracked_changes().unwrap();
    let mut rejected = reopen(&mut doc);
    rejected.reject_all().unwrap();
    let xml = reopen(&mut rejected)
        .part_xml("/word/document.xml")
        .unwrap();
    let section = &xml[xml.rfind("<w:sectPr").unwrap()..];
    let first = section[1..]
        .find("<w:")
        .map(|at| &section[at + 4..])
        .unwrap();
    assert!(first.starts_with("headerReference"), "{section}");
    assert!(
        !section.contains("sectPrChange") && !section.contains("15000"),
        "{section}"
    );
}

/// A copy that ends the body, whose properties a session rewrites, gives
/// every revision it brings an id of its own, one in its mark's properties
/// too: the source keeps its ids.
#[test]
fn a_copy_ending_the_body_gives_its_revisions_ids_of_their_own() {
    let source = r#"<w:p><w:pPr><w:jc w:val="center"/><w:rPr><w:b/><w:rPrChange w:id="7" w:author="Other" w:date="2026-11-01T00:00:00Z"><w:rPr/></w:rPrChange></w:rPr></w:pPr><w:ins w:id="8" w:author="Other" w:date="2026-11-01T00:00:00Z"><w:r><w:t>Copied.</w:t></w:r></w:ins></w:p>"#;
    let mut doc = doc_with_body(&(source.to_owned() + &paragraph(&run("Last."))));
    let changes = changes();
    doc.start_tracked_changes(&changes).unwrap();
    let copy = doc.body_children()[0].clone();
    let slot = doc.content_count();
    let marked = doc.marked_inserted_block(copy, slot, true).unwrap();
    doc.insert_body_block(slot, marked);
    doc.stop_tracked_changes();
    let xml = reopen(&mut doc).part_xml("/word/document.xml").unwrap();
    let ids = xml
        .match_indices(r#" w:id=""#)
        .map(|(at, _)| xml[at + 7..].split('"').next().unwrap().to_owned())
        .collect::<Vec<_>>();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
}

/// A paragraph moved under a session, in Word's shape (W7): the source keeps
/// its text in `w:moveFrom`, the copy goes in `w:moveTo`, each side covered
/// by a range named alike; accepting gives the untracked move, rejecting the
/// original, and one id resolves it whole.
#[test]
fn a_moved_paragraph_is_written_as_word_writes_a_move() {
    let body = || {
        doc_with_body(
            &["Alpha.", "Beta.", "Gamma.", "Delta."]
                .map(|text| paragraph(&run(text)))
                .concat(),
        )
    };
    let texts = |doc: &mut Document| {
        reopen(doc)
            .paragraphs()
            .into_iter()
            .map(|p| p.text())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
    };
    let locations = |doc: &Document| {
        (
            doc.direct_body_item_location(1).unwrap().unwrap(),
            doc.direct_body_item_location(3).unwrap().unwrap(),
        )
    };
    let mut untracked = body();
    let (source, destination) = locations(&untracked);
    untracked.move_content(&source, &destination).unwrap();
    let mut tracked = body();
    let (source, destination) = locations(&tracked);
    let changes = changes();
    tracked
        .track_changes(&changes)
        .move_content(&source, &destination)
        .unwrap();
    let mut tracked = reopen(&mut tracked);
    let xml = tracked.part_xml("/word/document.xml").unwrap();
    for part in [
        "moveFromRangeStart",
        "moveFromRangeEnd",
        "moveToRangeStart",
        "moveToRangeEnd",
    ] {
        assert_eq!(xml.matches(part).count(), 1, "{part}: {xml}");
    }
    let names = xml.match_indices(r#"w:name="move"#).count();
    assert_eq!(names, 2, "{xml}");
    let mut ids = xml
        .match_indices(r#" w:id=""#)
        .map(|(at, _)| xml[at + 7..].split('"').next().unwrap().to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    // Two wrappers and two marks, two ranges whose start and end share an id.
    assert_eq!(ids.len(), 6, "{xml}");
    let mut accepted = reopen(&mut tracked);
    accepted.accept_all().unwrap();
    assert_eq!(texts(&mut accepted), texts(&mut untracked));
    let mut rejected = reopen(&mut tracked);
    rejected.reject_all().unwrap();
    assert_eq!(texts(&mut rejected), texts(&mut body()));
    let first = xml[xml.find("<w:moveFrom ").unwrap()..]
        .split('"')
        .nth(1)
        .unwrap()
        .parse::<i32>()
        .unwrap();
    let mut by_id = reopen(&mut tracked);
    by_id.accept_revision_id(first).unwrap();
    assert_eq!(texts(&mut by_id), texts(&mut untracked));
    assert!(
        !reopen(&mut by_id)
            .part_xml("/word/document.xml")
            .unwrap()
            .contains("w:move")
    );
}

/// What a session cannot move as Word does is refused, the document left as
/// it was: a block outside the body or inside another, a move to the body's
/// end, a section break, the last paragraph, a table, a block declaring
/// namespaces of its own or holding a revision, and a block that the copy
/// would put twice in the document (a bookmark, a comment, a note
/// reference, a drawing).
#[test]
fn a_move_word_would_not_write_is_refused() {
    let table = r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>Cell.</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let body = [
        paragraph(&run("First.")),
        table.to_owned(),
        r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:pPr><w:r><w:t>Section.</w:t></w:r></w:p>"#.to_owned(),
        paragraph(&by("Other", "ins", 70, &run("Revised."))),
        paragraph(&format!(
            r#"<w:bookmarkStart w:id="1" w:name="mark"/>{}<w:bookmarkEnd w:id="1"/>"#,
            run("Bookmarked.")
        )),
        paragraph(&run("Commented.")),
        paragraph(&run("Noted.")),
        paragraph(&(run("Drawn.") + &text_box_run(9, &paragraph(&run("Boxed."))))),
        paragraph(&run("Last.")),
    ]
    .concat();
    let mut doc = doc_with_body(&body);
    doc.add_comment_on_text("Commented", "Ada", "Note", Default::default())
        .unwrap();
    let note = doc.add_footnote("Footnote.");
    doc.set_header("Head.");
    let mut doc = reopen(&mut doc);
    let at = |doc: &Document, text: &str| {
        doc.story_range_paragraphs()
            .unwrap()
            .into_iter()
            .find(|(_, xml)| String::from_utf8_lossy(xml).contains(text))
            .unwrap_or_else(|| panic!("{text}"))
            .0
    };
    {
        // The note reference, after the body is read again.
        let slot = (0..doc.content_count())
            .find(|&slot| {
                doc.direct_body_item_location(slot)
                    .unwrap()
                    .is_some_and(|location| {
                        location.index_path() == at(&doc, "Noted.").index_path()
                    })
            })
            .unwrap();
        doc.paragraph_mut(slot - 1).unwrap().add_footnote_ref(note);
    }
    let mut doc = reopen(&mut doc);
    // Before "First.", which no source is next to.
    let kept = at(&doc, "First.");
    let end = ContentLocation::end(kept.story().clone());
    let slot = |doc: &Document, at: usize| doc.direct_body_item_location(at).unwrap().unwrap();
    let rows = [
        ("Head.", "a move outside the body"),
        ("Cell.", "a move inside a block"),
        ("Section.", "the move of a section break"),
        ("Last.", "the move of the body's last paragraph"),
        ("Revised.", "holds a revision"),
        ("Bookmarked.", "holds a bookmark, a comment"),
        ("Commented", "holds a bookmark, a comment"),
        ("Noted.", "holds a bookmark, a comment"),
        ("Drawn.", "holds a bookmark, a comment"),
    ];
    let before = doc.to_bytes().unwrap();
    let changes = changes();
    let mut failed = Vec::new();
    let mut attempt = |doc: &mut Document,
                       row: &str,
                       source: &ContentLocation,
                       destination: &ContentLocation,
                       refusal: &str| {
        let result = doc
            .track_changes(&changes)
            .move_content(source, destination);
        match result {
            Err(error) if error.to_string().contains(refusal) => {}
            other => failed.push(format!("{row}: {other:?}")),
        }
        if doc.to_bytes().unwrap() != before {
            failed.push(format!("{row}: the document changed"));
        }
    };
    for (text, refusal) in rows {
        let source = at(&doc, text);
        attempt(&mut doc, text, &source, &kept, refusal);
    }
    let source = slot(&doc, 0);
    attempt(
        &mut doc,
        "to the end",
        &source,
        &end,
        "a move to the end of the body",
    );
    let source = slot(&doc, 1);
    attempt(&mut doc, "a table", &source, &kept, "the move of a table");
    // A block that declares namespaces of its own, in a document of its own.
    let declares = [
            // As Word may write a text box: the paragraph declares what it uses.
            concat!(
                r#"<w:p xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" "#,
                r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
                r#"xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">"#,
                r#"<w:r><w:t>Declares.</w:t></w:r><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">"#,
                r#"<wp:extent cx="914400" cy="457200"/><wp:docPr id="1" name="Text Box"/><a:graphic><a:graphicData "#,
                r#"uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/>"#,
                r#"<wps:spPr/><wps:txbx><w:txbxContent><w:p><w:r><w:t>Boxed.</w:t></w:r></w:p></w:txbxContent></wps:txbx>"#,
                r#"<wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
            )
            .to_owned(),
        paragraph(&run("Kept.")),
        paragraph(&run("Last.")),
    ]
    .concat();
    let mut doc = doc_with_body(&declares);
    let mut doc = reopen(&mut doc);
    let (source, kept) = (slot(&doc, 0), slot(&doc, 2));
    let before = doc.to_bytes().unwrap();
    match doc.track_changes(&changes).move_content(&source, &kept) {
        Err(error) if error.to_string().contains("declares namespaces of its own") => {}
        other => failed.push(format!("declares: {other:?}")),
    }
    if doc.to_bytes().unwrap() != before {
        failed.push("declares: the document changed".to_owned());
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// An insertion the in-place route does not mark (a paragraph inside a
/// header's control, a header holding a text box's two copies) is refused
/// with nothing changed and no id taken.
#[test]
fn a_story_insertion_not_marked_in_place_is_refused_whole() {
    let mut doc = doc_with_body(&paragraph(&run("Body.")));
    doc.set_header("Head");
    let mut doc = reopen(&mut doc);
    let in_control = |xml: &str| {
        let at = xml.find("<w:p>").or_else(|| xml.find("<w:p ")).unwrap();
        format!(
            "{}<w:sdt><w:sdtContent><w:p><w:r><w:t>In.</w:t></w:r></w:p></w:sdtContent></w:sdt>{}",
            &xml[..at],
            &xml[at..]
        )
    };
    let boxed = |xml: &str| {
        let at = xml.find("<w:r>").unwrap();
        format!(
            r#"{}<w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="wps"/><mc:Fallback/></mc:AlternateContent></w:r>{}"#,
            &xml[..at],
            &xml[at..]
        )
    };
    for (case, edit, path, message) in [
        (
            "in a control",
            &in_control as &dyn Fn(&str) -> String,
            vec![0, 0],
            "inside a block",
        ),
        ("beside a text box", &boxed, vec![0], "not marked in place"),
    ] {
        let mut doc = with_header(&mut doc, edit);
        let story = doc
            .stories()
            .unwrap()
            .into_iter()
            .find(|story| story.kind() == oxoffice_docx::StoryKind::Header)
            .unwrap();
        let before = doc.to_bytes().unwrap();
        let session = doc.start_tracked_changes(&changes()).unwrap();
        let location = oxoffice_docx::ContentLocation::new(
            story,
            oxoffice_docx::StoryItemKind::Paragraph,
            path,
        );
        let error = doc.mark_story_insertion_in_place(&location, 1).unwrap_err();
        assert!(error.to_string().contains(message), "{case}: {error}");
        assert_eq!(
            session.revision_ids(&doc).unwrap(),
            Vec::<i32>::new(),
            "{case}"
        );
        doc.stop_tracked_changes().unwrap();
        assert_eq!(doc.to_bytes().unwrap(), before, "{case}");
    }
}

/// What Word writes beside a revision it writes, on each revision oxoffice
/// writes, in a session or a comparison, in any story and through any output
/// (a save, a part read): `w16du:dateUtc`, the date in UTC, the part's root
/// declaring `w16du` ignorable, and the author in `people.xml` when the file
/// has that part (Q8). A producer's revisions, an undated one, the range
/// marks of a move (W7) and a revision rejected before the save are left
/// out.
#[test]
fn a_written_revision_carries_its_utc_date_and_its_author_as_word_writes_them() {
    const PEOPLE: &str = concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<w15:people xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml">"#,
        r#"<w15:person w15:author="Ann"><w15:presenceInfo w15:providerId="Windows Live" w15:userId="1"/></w15:person>"#,
        "</w15:people>"
    );
    const MAIN: &str = "/word/document.xml";
    const REV: &str = r#"<w15:person w15:author="Rev"><w15:presenceInfo w15:providerId="None" w15:userId="Rev"/></w15:person>"#;
    // `body` as the main part's body, `people` as `people.xml` when given.
    let document = |body: &str, people: Option<&str>| {
        let mut doc = doc_with_body(body);
        doc.set_header("Draft for review");
        let note = doc.add_footnote("Old note");
        doc.paragraph_mut(0).unwrap().add_footnote_ref(note);
        let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
        if let Some(people) = people {
            package.set_part("/word/people.xml", people.as_bytes().to_vec());
            package.content_types.add_override(
                "/word/people.xml",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.people+xml",
            );
            package.get_or_create_part_rels(MAIN).add(
                "http://schemas.microsoft.com/office/2011/relationships/people",
                "people.xml",
            );
        }
        let mut out = Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        Document::from_bytes(&out.into_inner()).unwrap()
    };
    let plain =
        paragraph(&run("Alpha beta.")) + &paragraph(&run("Gamma.")) + &paragraph(&run("Last."));
    let produced = paragraph(&(run("Alpha beta.") + &by("Other", "ins", 70, &run(" Theirs."))))
        + &paragraph(&run("Last."));
    let replaced = |doc: &mut Document, date: RevisionDate<'static>| {
        let changes = TrackedChanges::new(AUTHOR, date).unwrap();
        doc.track_changes(&changes)
            .try_replace_text("beta", "delta")
            .unwrap();
    };
    let story = |kind: StoryKind, text: &'static str| {
        move |doc: &mut Document| {
            let location = located(doc, kind);
            let changes = changes();
            doc.track_changes(&changes)
                .set_story_text(&location, text)
                .unwrap();
        }
    };
    type Edit = Box<dyn Fn(&mut Document)>;
    // Name, body, people, edit, part, UTC date, Rev listed, read without a save.
    type Row<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        Edit,
        &'a str,
        Option<&'a str>,
        bool,
        bool,
    );
    let rows: Vec<Row> = vec![
        (
            "a session, its date in UTC already",
            &plain,
            None,
            Box::new(move |doc| replaced(doc, RevisionDate::At(DATE))),
            MAIN,
            Some("2026-11-27T10:00:00Z"),
            false,
            false,
        ),
        (
            "a session dated with an offset",
            &plain,
            None,
            Box::new(move |doc| replaced(doc, RevisionDate::At("2026-11-27T12:30:00+02:00"))),
            MAIN,
            Some("2026-11-27T10:30:00Z"),
            false,
            false,
        ),
        (
            "an undated session",
            &plain,
            None,
            Box::new(move |doc| replaced(doc, RevisionDate::Undated)),
            MAIN,
            None,
            false,
            false,
        ),
        (
            "a comparison",
            &plain,
            None,
            Box::new(|doc| {
                let mut edited = reopen(doc);
                edited
                    .paragraph_mut(1)
                    .unwrap()
                    .set_text("Gamma ray.")
                    .unwrap();
                let edited = reopen(&mut edited);
                doc.compare(&edited, AUTHOR, DATE).unwrap();
            }),
            MAIN,
            Some("2026-11-27T10:00:00Z"),
            false,
            false,
        ),
        (
            "a move, its range marks dated by w:date alone",
            &plain,
            None,
            Box::new(|doc| {
                // "Gamma." before the first paragraph, which holds the note.
                let (source, destination) = (
                    doc.direct_body_item_location(1).unwrap().unwrap(),
                    doc.direct_body_item_location(0).unwrap().unwrap(),
                );
                let changes = changes();
                doc.track_changes(&changes)
                    .move_content(&source, &destination)
                    .unwrap();
            }),
            MAIN,
            Some("2026-11-27T10:00:00Z"),
            false,
            false,
        ),
        (
            "a header",
            &plain,
            None,
            Box::new(story(StoryKind::Header, "Final for review")),
            "/word/header1.xml",
            Some("2026-11-27T10:00:00Z"),
            false,
            false,
        ),
        (
            "a footnote",
            &plain,
            None,
            Box::new(story(StoryKind::Footnote, "New note")),
            "/word/footnotes.xml",
            Some("2026-11-27T10:00:00Z"),
            false,
            false,
        ),
        (
            "read through part_xml, before any save",
            &plain,
            Some(PEOPLE),
            Box::new(move |doc| replaced(doc, RevisionDate::At(DATE))),
            MAIN,
            Some("2026-11-27T10:00:00Z"),
            true,
            true,
        ),
        (
            "a session still open at the save",
            &plain,
            Some(PEOPLE),
            Box::new(|doc| {
                let changes = changes();
                doc.start_tracked_changes(&changes).unwrap();
                doc.try_replace_text("beta", "delta").unwrap();
            }),
            MAIN,
            Some("2026-11-27T10:00:00Z"),
            true,
            false,
        ),
        (
            "a session stopped before the save",
            &plain,
            Some(PEOPLE),
            Box::new(|doc| {
                let changes = changes();
                doc.start_tracked_changes(&changes).unwrap();
                doc.try_replace_text("beta", "delta").unwrap();
                doc.stop_tracked_changes();
            }),
            MAIN,
            Some("2026-11-27T10:00:00Z"),
            true,
            false,
        ),
        (
            "a session, then a producer's revision accepted (a staged reopen)",
            &produced,
            Some(PEOPLE),
            Box::new(move |doc| {
                replaced(doc, RevisionDate::At(DATE));
                doc.accept_revision_id(70).unwrap();
            }),
            MAIN,
            Some("2026-11-27T10:00:00Z"),
            true,
            false,
        ),
        (
            "a session rejected before the save",
            &plain,
            Some(PEOPLE),
            Box::new(move |doc| {
                replaced(doc, RevisionDate::At(DATE));
                doc.reject_all().unwrap();
            }),
            MAIN,
            None,
            false,
            false,
        ),
    ];
    let mut failed = Vec::new();
    for (row, body, people, edit, part, utc, listed, unsaved) in rows {
        let mut doc = document(body, people);
        edit(&mut doc);
        let mut doc = match unsaved {
            true => doc,
            false => reopen(&mut doc),
        };
        let xml = doc.part_xml(part).unwrap();
        let opens = xml.find("?>").map_or(0, |at| at + 2);
        let root = &xml[opens..opens + xml[opens..].find('>').unwrap() + 1];
        let mut problems = Vec::new();
        // The revisions written, the range marks of a move left out.
        let starts = xml.match_indices("RangeStart ").collect::<Vec<_>>();
        let written = xml.matches(&format!(r#"w:author="{AUTHOR}""#)).count() - starts.len();
        if starts
            .iter()
            .any(|(at, _)| xml[*at..at + xml[*at..].find('>').unwrap()].contains("dateUtc"))
        {
            problems.push("a move's range mark dated");
        }
        match utc {
            Some(utc) => {
                if written == 0
                    || xml.matches(&format!(r#"w16du:dateUtc="{utc}""#)).count() != written
                    || xml.matches("w16du:dateUtc").count() != written
                    || !root.contains(
                        r#"xmlns:w16du="http://schemas.microsoft.com/office/word/2023/wordml/word16du""#,
                    )
                    || !root.contains("w16du\"")
                {
                    problems.push("utc date");
                }
            }
            None => {
                if xml.contains("w16du:dateUtc") {
                    problems.push("dated");
                }
            }
        }
        let people_xml = doc.part_xml("/word/people.xml").ok();
        match (people, &people_xml) {
            (None, None) => {}
            (Some(_), Some(people_xml)) if people_xml.contains(REV) == listed => {
                // Saved again, after another session, the author is listed once.
                if listed {
                    let mut again = reopen(&mut doc);
                    let changes = changes();
                    again
                        .track_changes(&changes)
                        .try_replace_text("Last", "Final")
                        .unwrap();
                    let people_xml = reopen(&mut again).part_xml("/word/people.xml").unwrap();
                    if people_xml.matches(REV).count() != 1 {
                        problems.push("an author listed twice");
                    }
                }
            }
            _ => problems.push("people.xml"),
        }
        if !doc.validate().unwrap().is_valid() {
            problems.push("validation");
        }
        if !problems.is_empty() {
            failed.push(format!("{row}: {problems:?}: {xml} {people_xml:?}"));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
