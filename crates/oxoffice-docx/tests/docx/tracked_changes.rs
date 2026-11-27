//! Tracked changes written by edits: each edit of a session against its
//! untracked twin. Accepting every revision gives the twin's result,
//! rejecting every revision or only the session's gives the document back,
//! the session's revisions read as the change, and the document validates
//! and reads the same once saved.

use oxoffice_docx::{
    ComparisonGranularity, ComparisonOptions, Document, RevisionDate, RevisionKind, TrackedChanges,
    TrackedDocument,
};

use crate::support::{doc_with_body, reopen};

const AUTHOR: &str = "Rev";
const DATE: &str = "2026-11-27T10:00:00Z";

fn changes() -> TrackedChanges {
    TrackedChanges::new(AUTHOR, RevisionDate::At(DATE)).unwrap()
}

fn run(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
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

/// The session's revisions: kind and text, in order.
fn session_revisions(doc: &Document) -> Vec<(RevisionKind, String)> {
    doc.story_revisions()
        .unwrap()
        .into_iter()
        .filter(|revision| revision.author() == AUTHOR)
        .map(|revision| (revision.kind(), revision.text().to_owned()))
        .collect()
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
            .filter(|revision| revision.author() == AUTHOR && !held.contains(&revision.id()))
            .map(|revision| revision.id())
            .collect::<Vec<_>>();
        written.sort();
        written.dedup();
        let mut session_ids = ids.clone();
        session_ids.sort();
        assert_eq!(session_ids, written, "{shape}");
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

/// What a session cannot write yet is refused, the document left as it was.
#[test]
fn an_edit_a_session_cannot_write_is_refused_without_a_change() {
    type Edit = fn(&mut TrackedDocument<'_>) -> oxoffice_docx::Result<()>;
    let field = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
    let rows: [(&str, String, Edit); 3] = [
        ("formatting alone", paragraph(&run("Plain words.")), |doc| {
            doc.edit_body_paragraph(0, |p| {
                p.run_mut(0).unwrap().set_bold(true);
                Ok(())
            })
        }),
        (
            "a field removed",
            paragraph(&(run("Page ") + field)),
            |doc| doc.edit_body_paragraph(0, |p| p.set_text("Page")),
        ),
        (
            "text put in a new link",
            paragraph(&run("Plain words.")),
            |doc| {
                doc.edit_body_paragraph(0, |p| {
                    p.add_hyperlink(" More", "https://example.com/");
                    Ok(())
                })
            },
        ),
    ];
    for (shape, body, edit) in rows {
        let mut doc = doc_with_body(&body);
        let before = doc.to_bytes().unwrap();
        let changes = changes();
        let mut session = doc.track_changes(&changes);
        let error = edit(&mut session).expect_err(shape);
        assert!(session.revision_ids().is_empty(), "{shape}");
        assert!(
            error
                .to_string()
                .contains("is not recorded as a tracked change yet"),
            "{shape}: {error}"
        );
        assert_eq!(doc.to_bytes().unwrap(), before, "{shape}");
    }
}
