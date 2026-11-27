//! Tracked changes written by edits: each edit of a session against its
//! untracked twin. Accepting every revision gives the twin's result,
//! rejecting every revision or only the session's gives the document back,
//! the session's revisions read as the change, and the document validates
//! and reads the same once saved.

use std::io::Cursor;

use oxoffice_docx::{
    ComparisonGranularity, ComparisonOptions, ContentLocation, Document, ReplaceOptions,
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
/// body each character with its bold state.
fn view(doc: &Document) -> (Vec<String>, Vec<Vec<(char, bool)>>) {
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
            paragraph
                .runs()
                .flat_map(|run| {
                    let bold = run.is_bold();
                    run.text()
                        .chars()
                        .map(move |c| (c, bold))
                        .collect::<Vec<_>>()
                })
                .collect()
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
    /// comparison keeps the words a value repeats, and text that took other
    /// formatting is deleted and inserted where it writes a formatting change.
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
            "formatting alone",
            body(&run("Plain words.")),
            |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.run_mut(0).unwrap().set_bold(true);
                    Ok(())
                })
            },
            not_yet,
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
