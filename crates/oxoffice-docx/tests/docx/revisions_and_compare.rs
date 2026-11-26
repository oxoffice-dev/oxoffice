//! Tracked changes: the track revisions setting, reading revisions (kind,
//! author, date, story, text), accepting and rejecting them, and comparing two
//! versions of a document into a redline that resolves back to either one.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use oxoffice_docx::{
    Alignment, BodyItemRef, ComparisonGranularity, ComparisonOptions, ComparisonStoryKind,
    Document, RenderOptions, RevisionKind, RevisionStyle, RevisionView, RunItemRef, StoryKind,
    StoryParagraph, StyleBuilder, ValidationSeverity,
};

use crate::support::{doc_with, doc_with_body, reopen, texts};

const AUTHOR: &str = "Reviewer";
const DATE: &str = "2026-11-21T09:00:00Z";

/// A new document holding these paragraphs, each with its footnote if any.
fn doc_with_notes(paragraphs: &[(&str, Option<&str>)]) -> Document {
    let mut doc = Document::new();
    for (text, note) in paragraphs {
        let id = note.map(|note| doc.add_footnote(note));
        let mut paragraph = doc.add_paragraph(text);
        if let Some(id) = id {
            paragraph.add_footnote_ref(id);
        }
    }
    doc
}

/// `original` with the tracked changes that turn it into `edited`, by
/// [`AUTHOR`] at [`DATE`]. The comparison reports no diagnostic.
fn redline(mut original: Document, edited: &Document) -> Document {
    let diagnostics = original.compare(edited, AUTHOR, DATE).expect("comparison");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    reopen(&mut original)
}

/// A saved copy of `doc` with every revision accepted, as the next session sees it.
fn accepted(doc: &mut Document) -> Document {
    let mut copy = reopen(doc);
    copy.accept_all().expect("accept all");
    reopen(&mut copy)
}

/// A saved copy of `doc` with every revision rejected, as the next session sees it.
fn rejected(doc: &mut Document) -> Document {
    let mut copy = reopen(doc);
    copy.reject_all().expect("reject all");
    reopen(&mut copy)
}

fn kinds(doc: &Document) -> Vec<RevisionKind> {
    doc.revisions()
        .iter()
        .map(|revision| revision.kind())
        .collect()
}

fn ids(doc: &Document) -> Vec<i32> {
    doc.revisions()
        .iter()
        .map(|revision| revision.id())
        .collect()
}

/// The text of the story revisions of this kind, in order.
fn revision_texts(doc: &Document, kind: RevisionKind) -> Vec<String> {
    doc.story_revisions()
        .expect("story revisions")
        .into_iter()
        .filter(|revision| revision.kind() == kind && !revision.text().is_empty())
        .map(|revision| revision.text().to_owned())
        .collect()
}

/// One paragraph with a kept run, an insertion and a deletion, then a run
/// made bold, a paragraph centred, and a paragraph moved after `middle`.
/// The move leaves its paragraph marks in place.
const EVERY_KIND: &str = concat!(
    r#"<w:p><w:r><w:t xml:space="preserve">kept </w:t></w:r>"#,
    r#"<w:ins w:id="1" w:author="Ada" w:date="2026-01-01T00:00:00Z"><w:r><w:t>added</w:t></w:r></w:ins>"#,
    r#"<w:del w:id="2" w:author="Ben" w:date="2026-01-02T00:00:00Z"><w:r><w:delText>removed</w:delText></w:r></w:del></w:p>"#,
    r#"<w:p><w:r><w:rPr><w:b/><w:rPrChange w:id="3" w:author="Cy" w:date="2026-01-03T00:00:00Z"><w:rPr/></w:rPrChange></w:rPr><w:t>bolded</w:t></w:r></w:p>"#,
    r#"<w:p><w:pPr><w:jc w:val="center"/><w:pPrChange w:id="4" w:author="Dee"><w:pPr/></w:pPrChange></w:pPr><w:r><w:t>centred</w:t></w:r></w:p>"#,
    r#"<w:p><w:moveFrom w:id="5" w:author="Eve" w:date="2026-01-05T00:00:00Z"><w:r><w:t>travelling</w:t></w:r></w:moveFrom></w:p>"#,
    r#"<w:p><w:r><w:t>middle</w:t></w:r></w:p>"#,
    r#"<w:p><w:moveTo w:id="6" w:author="Eve" w:date="2026-01-05T00:00:00Z"><w:r><w:t>travelling</w:t></w:r></w:moveTo></w:p>"#,
);

/// Three insertions in one paragraph by two authors on three days.
const THREE_INSERTIONS: &str = concat!(
    r#"<w:p><w:ins w:id="10" w:author="Alice" w:date="2026-01-01T00:00:00Z"><w:r><w:t>Alpha</w:t></w:r></w:ins>"#,
    r#"<w:ins w:id="20" w:author="Bob" w:date="2026-01-02T00:00:00Z"><w:r><w:t>Bravo</w:t></w:r></w:ins>"#,
    r#"<w:ins w:id="30" w:author="Alice" w:date="2026-01-03T00:00:00Z"><w:r><w:t>Charlie</w:t></w:r></w:ins></w:p>"#,
);

// ---- The track revisions setting ----

#[test]
fn the_tracking_settings_are_unset_on_a_new_document_and_survive_a_save() {
    let mut doc = Document::new();
    assert_eq!(doc.track_revisions(), None);
    assert_eq!(doc.do_not_track_moves(), None);
    assert_eq!(doc.do_not_track_formatting(), None);

    doc.set_track_revisions(true).unwrap();
    doc.set_do_not_track_moves(true).unwrap();
    doc.set_do_not_track_formatting(true).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.track_revisions(), Some(true));
    assert_eq!(doc.do_not_track_moves(), Some(true));
    assert_eq!(doc.do_not_track_formatting(), Some(true));

    doc.set_track_revisions(false).unwrap();
    assert_eq!(reopen(&mut doc).track_revisions(), Some(false));
    assert_eq!(doc.remove_track_revisions().unwrap(), Some(false));
    assert_eq!(reopen(&mut doc).track_revisions(), None);
}

// ---- Reading revisions ----

#[test]
fn the_text_of_a_document_shows_insertions_and_hides_deletions() {
    let mut doc = doc_with_body(EVERY_KIND);
    let doc = reopen(&mut doc);
    assert_eq!(doc.paragraph(0).unwrap().text(), "kept added");
    assert_eq!(
        texts(&doc),
        [
            "kept added",
            "bolded",
            "centred",
            "",
            "middle",
            "travelling"
        ]
    );
    assert!(doc.text().starts_with("kept added\nbolded\n"));
    assert_eq!(doc.text_report().revision_view, "accepted");
    let revisions = doc.revisions();
    assert_eq!(
        revisions[0]
            .insertion_paragraph()
            .map(|p| p.text())
            .as_deref(),
        Some("added")
    );
    assert!(revisions[1].insertion_paragraph().is_none(), "a deletion");
}

#[test]
fn story_revisions_give_the_text_each_change_covers() {
    let doc = doc_with_body(EVERY_KIND);
    let listed: Vec<_> = doc
        .story_revisions()
        .unwrap()
        .into_iter()
        .map(|r| (r.id(), r.story().kind(), r.text().to_owned()))
        .collect();
    let body = |id, text: &str| (id, StoryKind::Body, text.to_owned());
    assert_eq!(
        listed,
        [
            body(1, "added"),
            body(2, "removed"),
            body(3, "bolded"),
            body(4, "centred"),
            body(5, "travelling"),
            body(6, "travelling"),
        ]
    );
}

// ---- Accepting and rejecting ----

#[test]
fn rejecting_all_restores_the_text_before_the_changes() {
    let mut doc = doc_with_body(EVERY_KIND);
    let mut copy = reopen(&mut doc);
    assert_eq!(copy.reject_all().unwrap(), 6);
    let copy = reopen(&mut copy);
    assert!(copy.revisions().is_empty());
    assert_eq!(
        texts(&copy),
        [
            "kept removed",
            "bolded",
            "centred",
            "travelling",
            "middle",
            ""
        ]
    );
}

#[test]
fn one_revision_is_accepted_or_rejected_by_its_id() {
    let mut doc = doc_with_body(THREE_INSERTIONS);
    assert_eq!(doc.accept_revision_id(99).unwrap(), 0, "no such id");
    assert_eq!(doc.reject_revision_id(99).unwrap(), 0, "no such id");
    assert_eq!(doc.reject_revision_id(20).unwrap(), 1);
    assert_eq!(ids(&doc), [10, 30]);
    assert_eq!(doc.accept_revision_id(10).unwrap(), 1);
    let doc = reopen(&mut doc);
    assert_eq!(ids(&doc), [30]);
    assert_eq!(doc.text(), "AlphaCharlie\n");
}

/// A run made bold, a paragraph centred, a table given a width and a
/// section given a page size, each from no properties: Word writes the empty
/// prior value (`<w:rPr/>`), LibreOffice writes none (`EMPTY` is `></w:...>`
/// or `/>`).
fn changes_from_no_properties(word: bool, form: &str) -> String {
    let attributes = r#"w:author="Ada" w:date="2026-01-01T00:00:00Z""#;
    let change = |local: &str, id: u32, prior: &str| {
        if word {
            format!(r#"<w:{local}Change w:id="{id}" {attributes}><w:{prior}/></w:{local}Change>"#)
        } else if form == "/>" {
            format!(r#"<w:{local}Change w:id="{id}" {attributes}/>"#)
        } else {
            format!(r#"<w:{local}Change w:id="{id}" {attributes}></w:{local}Change>"#)
        }
    };
    format!(
        concat!(
            r#"<w:p><w:r><w:rPr><w:b/>{}</w:rPr><w:t>bolded</w:t></w:r></w:p>"#,
            r#"<w:p><w:pPr><w:jc w:val="center"/>{}</w:pPr><w:r><w:t>centred</w:t></w:r></w:p>"#,
            r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/>{}</w:tblPr>"#,
            r#"<w:tblGrid><w:gridCol w:w="9000"/></w:tblGrid>"#,
            r#"<w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
            r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="11906" w:h="16838"/>{}</w:sectPr></w:pPr>"#,
            r#"<w:r><w:t>sized</w:t></w:r></w:p>"#,
        ),
        change("rPr", 1, "rPr"),
        change("pPr", 2, "pPr"),
        change("tblPr", 3, "tblPr"),
        change("sectPr", 4, "sectPr"),
    )
}

#[test]
fn a_property_change_with_no_prior_value_resolves_as_one_from_no_properties() {
    for form in ["></w:...>", "/>"] {
        let resolved = |word: bool, accept: bool, id: Option<i32>| {
            let mut doc = doc_with_body(&changes_from_no_properties(word, form));
            let count = match (accept, id) {
                (true, None) => doc.accept_all(),
                (false, None) => doc.reject_all(),
                (true, Some(id)) => doc.accept_revision_id(id),
                (false, Some(id)) => doc.reject_revision_id(id),
            }
            .unwrap_or_else(|error| {
                panic!("{form} word={word} accept={accept} id={id:?}: {error}")
            });
            (count, main_part_xml(&mut reopen(&mut doc)))
        };
        // The changes left unresolved keep the form they were written in.
        let libreoffice_form = |(count, mut xml): (usize, String)| {
            for local in ["rPr", "pPr", "tblPr", "sectPr"] {
                let end = if form == "/>" {
                    "/>".to_owned()
                } else {
                    format!("></w:{local}Change>")
                };
                xml = xml.replace(&format!("><w:{local}/></w:{local}Change>"), &end);
            }
            (count, xml)
        };
        for accept in [true, false] {
            for id in [None, Some(1), Some(2), Some(3), Some(4)] {
                assert_eq!(
                    resolved(false, accept, id),
                    libreoffice_form(resolved(true, accept, id)),
                    "{form} accept={accept} id={id:?}"
                );
            }
        }

        let mut doc = doc_with_body(&changes_from_no_properties(false, form));
        assert_eq!(doc.reject_all().unwrap(), 4, "{form}");
        let doc = reopen(&mut doc);
        assert!(doc.revisions().is_empty(), "{form}");
        assert!(
            !doc.paragraph(0).unwrap().runs().any(|run| run.is_bold()),
            "{form}"
        );
        assert_eq!(doc.paragraph(1).unwrap().alignment(), None, "{form}");
    }
}

#[test]
fn a_range_marker_between_merged_paragraphs_stays_where_they_join() {
    let attributes = r#"w:author="Ada" w:date="2026-01-01T00:00:00Z""#;
    // A paragraph added before a heading whose bookmark starts between the
    // two, as Google Docs writes a heading's bookmark: rejecting it merges
    // the added paragraph into the heading across the bookmark start.
    let added = format!(
        concat!(
            r#"<w:p><w:pPr><w:rPr><w:ins w:id="1" {0}/></w:rPr></w:pPr>"#,
            r#"<w:ins w:id="2" {0}><w:r><w:t>added</w:t></w:r></w:ins></w:p>"#,
            r#"<w:bookmarkStart w:id="7" w:name="_Toc1"/>"#,
            r#"<w:p><w:r><w:t>Heading</w:t></w:r><w:bookmarkEnd w:id="7"/></w:p>"#,
            r#"<w:p><w:r><w:t>after</w:t></w:r></w:p>"#,
        ),
        attributes
    );
    // A paragraph mark removed with a bookmark ending after it: accepting
    // merges the two paragraphs across the bookmark end.
    let removed = format!(
        concat!(
            r#"<w:bookmarkStart w:id="8" w:name="kept"/>"#,
            r#"<w:p><w:pPr><w:rPr><w:del w:id="3" {0}/></w:rPr></w:pPr><w:r><w:t>first</w:t></w:r></w:p>"#,
            r#"<w:bookmarkEnd w:id="8"/><w:p><w:r><w:t>second</w:t></w:r></w:p>"#,
        ),
        attributes
    );
    for (body, accept, text, bookmark, expected) in [
        (
            &added,
            false,
            vec!["Heading", "after"],
            "_Toc1",
            r#"<w:bookmarkStart w:id="7" w:name="_Toc1"/><w:r><w:t>Heading</w:t>"#,
        ),
        (
            &removed,
            true,
            vec!["firstsecond"],
            "kept",
            r#"<w:t>first</w:t></w:r><w:bookmarkEnd w:id="8"/><w:r><w:t>second</w:t>"#,
        ),
    ] {
        let mut doc = doc_with_body(body);
        if accept {
            doc.accept_all()
        } else {
            doc.reject_all()
        }
        .unwrap_or_else(|error| panic!("{body}: {error}"));
        let mut doc = reopen(&mut doc);
        assert!(doc.revisions().is_empty(), "{body}");
        assert_eq!(texts(&doc), text, "{body}");
        let names = doc
            .bookmarks()
            .iter()
            .filter_map(|bookmark| bookmark.name().map(str::to_owned))
            .collect::<Vec<_>>();
        assert_eq!(names, [bookmark], "{body}");
        let xml = main_part_xml(&mut doc);
        assert!(xml.contains(expected), "{xml}");
    }
}

#[test]
fn a_range_marker_crossed_by_a_merge_does_not_stand_for_a_crossed_table() {
    // An inserted last paragraph mark cannot go; a bookmark start crossed
    // on the way changes nothing to that.
    let attributes = r#"w:author="Ada" w:date="2026-01-01T00:00:00Z""#;
    let body = |marker: &str| {
        format!(
            concat!(
                r#"<w:p><w:r><w:t>kept</w:t></w:r></w:p>"#,
                r#"<w:p><w:pPr><w:rPr><w:ins w:id="1" {0}/></w:rPr></w:pPr></w:p>{1}"#,
                r#"<w:p><w:pPr><w:rPr><w:ins w:id="2" {0}/></w:rPr></w:pPr></w:p>"#,
            ),
            attributes, marker
        )
    };
    let outcome = |marker: &str| {
        let mut doc = doc_with_body(&body(marker));
        doc.reject_all()
            .map(|_| texts(&reopen(&mut doc)))
            .map_err(|error| error.to_string())
    };
    assert_eq!(
        outcome(r#"<w:bookmarkStart w:id="7" w:name="mark"/>"#),
        outcome("")
    );
}

#[test]
fn every_producer_copy_with_tracked_changes_accepts_and_rejects_them_all() {
    // The LibreOffice contract on a pull request (empty prior run
    // properties); every contract and record copy with `OXOFFICE_WORKFLOWS=full`.
    let mut copies = crate::support::every_tracked_copy();
    if !crate::support::every_copy() {
        copies.truncate(1);
    }
    crate::support::run_workflow("resolve-all", &copies, |path| {
        let source = crate::support::workflow_source(path);
        let doc = Document::from_bytes(&source).expect("corpus copy opens");
        assert!(!doc.revisions().is_empty(), "{path} has tracked changes");
        let shown = doc.text();
        // Each body insertion and deletion, by its text: rejecting takes the
        // inserted text out of the text shown and brings the deleted back.
        let changes = doc
            .story_revisions()
            .expect("story revisions")
            .into_iter()
            .filter(|revision| {
                revision.story().kind() == StoryKind::Body
                    && matches!(
                        revision.kind(),
                        RevisionKind::Insertion | RevisionKind::Deletion
                    )
                    && !revision.text().trim().is_empty()
            })
            .map(|revision| (revision.kind(), revision.text().to_owned()))
            .collect::<Vec<_>>();
        assert!(!changes.is_empty(), "{path} has body text changes");
        for accept in [true, false] {
            let mut copy = Document::from_bytes(&source).unwrap();
            let resolved = if accept {
                copy.accept_all()
            } else {
                copy.reject_all()
            };
            resolved.unwrap_or_else(|error| panic!("{path} accept={accept}: {error}"));
            let copy = reopen(&mut copy);
            assert!(copy.revisions().is_empty(), "{path} accept={accept}");
            let text = copy.text();
            if accept {
                assert_eq!(text, shown, "{path}: accepting keeps the text shown");
                continue;
            }
            for (kind, change) in &changes {
                let (before, after) = (
                    shown.matches(change.as_str()).count(),
                    text.matches(change.as_str()).count(),
                );
                if *kind == RevisionKind::Insertion {
                    assert!(after < before, "{path}: rejecting takes out {change:?}");
                } else {
                    assert!(after > before, "{path}: rejecting brings back {change:?}");
                }
            }
        }
    });
}

#[test]
fn revisions_are_resolved_by_author() {
    let mut doc = doc_with_body(THREE_INSERTIONS);
    assert_eq!(doc.accept_revisions_by_author("Nobody").unwrap(), 0);
    assert_eq!(doc.reject_revisions_by_author("Alice").unwrap(), 2);
    assert_eq!(ids(&doc), [20]);
    assert_eq!(doc.accept_revisions_by_author("Bob").unwrap(), 1);
    let doc = reopen(&mut doc);
    assert!(doc.revisions().is_empty());
    assert_eq!(doc.text(), "Bravo\n");
}

#[test]
fn revisions_are_resolved_by_an_inclusive_date_range_of_instants() {
    let mut doc = doc_with_body(THREE_INSERTIONS);
    let error = doc
        .accept_revisions_in_date_range("yesterday", "2026-01-02T00:00:00Z")
        .unwrap_err();
    assert!(error.to_string().contains("yesterday"), "{error}");
    let reversed =
        doc.reject_revisions_in_date_range("2026-01-03T00:00:00Z", "2026-01-01T00:00:00Z");
    assert!(reversed.is_err(), "a range that ends before it starts");
    assert_eq!(ids(&doc), [10, 20, 30], "a refused range resolves nothing");

    let day = "2026-01-02T00:00:00Z";
    assert_eq!(doc.accept_revisions_in_date_range(day, day).unwrap(), 1);
    assert_eq!(ids(&doc), [10, 30]);
    // The same instant as 2026-01-03T00:00:00Z, written with an offset.
    let offset = "2026-01-03T01:00:00+01:00";
    assert_eq!(
        doc.reject_revisions_in_date_range(offset, offset).unwrap(),
        1
    );
    assert_eq!(ids(&doc), [10]);
    assert_eq!(reopen(&mut doc).text(), "AlphaBravo\n");
}

// ---- Comparing two versions ----

#[test]
fn word_and_character_granularity_mark_only_what_changed() {
    let original = ["Lorem ipsum dolor sit amet."];
    let edited = doc_with(&["Lorem ipsum DOLOR sit amet."]);
    for granularity in [
        ComparisonGranularity::Word,
        ComparisonGranularity::Character,
    ] {
        let mut doc = doc_with(&original);
        let options = ComparisonOptions {
            granularity,
            ..Default::default()
        };
        doc.compare_with_options(&edited, AUTHOR, DATE, &options)
            .unwrap();
        let deleted = revision_texts(&doc, RevisionKind::Deletion);
        assert_eq!(deleted, ["dolor"], "{granularity:?}");
        let inserted = revision_texts(&doc, RevisionKind::Insertion);
        assert_eq!(inserted, ["DOLOR"], "{granularity:?}");
        assert_eq!(texts(&rejected(&mut doc)), original, "{granularity:?}");
    }
    // Character granularity keeps the letters both words share.
    let mut doc = doc_with(&["colour"]);
    let options = ComparisonOptions {
        granularity: ComparisonGranularity::Character,
        ..Default::default()
    };
    doc.compare_with_options(&doc_with(&["color"]), AUTHOR, DATE, &options)
        .unwrap();
    assert_eq!(revision_texts(&doc, RevisionKind::Deletion), ["u"]);
    assert!(revision_texts(&doc, RevisionKind::Insertion).is_empty());
}

#[test]
fn a_format_change_is_recorded_unless_formatting_is_ignored() {
    let mut edited = Document::new();
    edited.add_paragraph("").add_run("plain").set_bold(true);
    let mut doc = redline(doc_with(&["plain"]), &edited);
    assert_eq!(kinds(&doc), [RevisionKind::RunPropertyChange]);
    let bold = |doc: &Document| doc.paragraph(0).unwrap().runs().all(|run| run.is_bold());
    assert!(bold(&accepted(&mut doc)));
    assert!(!bold(&rejected(&mut doc)));

    let mut ignored = doc_with(&["plain"]);
    let options = ComparisonOptions {
        ignore_formatting: true,
        ..Default::default()
    };
    ignored
        .compare_with_options(&edited, AUTHOR, DATE, &options)
        .unwrap();
    let ignored = reopen(&mut ignored);
    assert!(ignored.revisions().is_empty());
    assert!(!bold(&ignored), "the original side is kept");
}

#[test]
fn a_format_change_of_a_run_holding_a_raw_child_is_recorded_unless_formatting_is_ignored() {
    // Properties added or removed do not move the raw child after them (a
    // VML picture, kept as written): a format change, or none with
    // formatting ignored.
    let picture = r#"<w:pict xmlns:v="urn:schemas-microsoft-com:vml"><v:rect style="width:9pt;height:9pt"/></w:pict>"#;
    let version = |properties: &str| {
        doc_with_body(&format!(
            "<w:p><w:r><w:t>Picture </w:t></w:r><w:r>{properties}{picture}</w:r></w:p>"
        ))
    };
    // Whether the run holding the picture, which stays in it, is bold.
    let bold = |doc: &mut Document| {
        let xml = main_xml(doc);
        let at = xml.find("<w:pict").unwrap();
        let run = &xml[xml[..at].rfind("<w:r>").unwrap()..at];
        assert!(!run.contains("</w:r>") && !run.contains("Picture"), "{xml}");
        run.contains("<w:b/>")
    };
    for (from, to) in [("", "<w:rPr><w:b/></w:rPr>"), ("<w:rPr><w:b/></w:rPr>", "")] {
        for ignore_formatting in [false, true] {
            let case = format!("{from} to {to}, ignore formatting {ignore_formatting}");
            let options = ComparisonOptions {
                ignore_formatting,
                ..ComparisonOptions::default()
            };
            let mut doc = version(from);
            doc.compare_with_options(&version(to), AUTHOR, DATE, &options)
                .unwrap_or_else(|error| panic!("{case}: {error}"));
            let mut doc = reopen(&mut doc);
            assert!(doc.validate().unwrap().is_valid(), "{case}");
            if ignore_formatting {
                assert_eq!(kinds(&doc), [], "{case}");
                assert_eq!(bold(&mut doc), !from.is_empty(), "{case}");
            } else {
                assert_eq!(kinds(&doc), [RevisionKind::RunPropertyChange], "{case}");
                assert_eq!(bold(&mut accepted(&mut doc)), !to.is_empty(), "{case}");
                assert_eq!(bold(&mut rejected(&mut doc)), !from.is_empty(), "{case}");
            }
        }
    }
}

#[test]
fn alignment_and_style_changes_are_recorded_as_paragraph_property_changes() {
    let mut edited = doc_with(&["centre me", "style me"]);
    edited
        .paragraph_mut(0)
        .unwrap()
        .set_alignment(Alignment::Center);
    edited.paragraph_mut(1).unwrap().set_style("Heading1");
    let mut doc = redline(doc_with(&["centre me", "style me"]), &edited);
    assert_eq!(
        kinds(&doc),
        [
            RevisionKind::ParagraphPropertyChange,
            RevisionKind::ParagraphPropertyChange
        ]
    );
    let copy = accepted(&mut doc);
    assert_eq!(
        copy.paragraph(0).unwrap().alignment(),
        Some(Alignment::Center)
    );
    assert_eq!(copy.paragraph(1).unwrap().style_id(), Some("Heading1"));
    let copy = rejected(&mut doc);
    assert_eq!(copy.paragraph(0).unwrap().alignment(), None);
    assert_eq!(copy.paragraph(1).unwrap().style_id(), None);
}

#[test]
fn whitespace_changes_are_recorded_unless_whitespace_is_ignored() {
    let edited = doc_with(&["old tail"]);
    let mut tracked = doc_with(&["old  tail"]);
    tracked.compare(&edited, AUTHOR, DATE).unwrap();
    assert!(!tracked.revisions().is_empty());

    let mut ignored = doc_with(&["old  tail"]);
    let options = ComparisonOptions {
        ignore_whitespace: true,
        ..Default::default()
    };
    ignored
        .compare_with_options(&edited, AUTHOR, DATE, &options)
        .unwrap();
    assert!(ignored.revisions().is_empty());
    assert_eq!(texts(&ignored), ["old  tail"]);
}

#[test]
fn header_changes_are_compared_unless_headers_are_ignored() {
    let with_header = |text: &str| {
        let mut doc = doc_with(&["body"]);
        doc.set_header(text);
        doc
    };
    let edited = with_header("New header");
    let doc = redline(with_header("Old header"), &edited);
    assert_eq!(
        revision_texts(&doc, RevisionKind::Insertion),
        ["New header"]
    );

    let mut ignored = with_header("Old header");
    let options = ComparisonOptions {
        ignored_stories: vec![ComparisonStoryKind::Header],
        ..Default::default()
    };
    ignored
        .compare_with_options(&edited, AUTHOR, DATE, &options)
        .unwrap();
    let ignored = reopen(&mut ignored);
    assert!(ignored.story_revisions().unwrap().is_empty());
    assert_eq!(ignored.header_text().as_deref(), Some("Old header"));
}

#[test]
fn author_names_with_markup_characters_tabs_and_line_ends_and_offset_dates_survive_the_redline() {
    let author = "Ada & \"Bob\"\t<QA>\nteam\r";
    let date = "2026-11-21T09:00:00+01:00";
    let mut doc = doc_with(&["old"]);
    doc.compare(&doc_with(&["new"]), author, date).unwrap();
    let doc = reopen(&mut doc);
    assert!(!doc.revisions().is_empty());
    for revision in doc.revisions() {
        assert_eq!(revision.author(), author);
        assert_eq!(revision.timestamp(), Some(date));
    }
}

#[test]
fn rtl_and_cjk_text_compare_by_character() {
    let original = ["مرحبا بالعالم", "שלום עולם", "東京は大きい都市です"];
    let edited = ["مرحبا بالجميع", "שלום לכולם", "大阪は大きい都市です"];
    let mut doc = doc_with(&original);
    let options = ComparisonOptions {
        granularity: ComparisonGranularity::Character,
        ..Default::default()
    };
    doc.compare_with_options(&doc_with(&edited), AUTHOR, DATE, &options)
        .unwrap();
    let mut doc = reopen(&mut doc);
    let deleted = revision_texts(&doc, RevisionKind::Deletion);
    assert!(deleted.contains(&"東京".to_owned()), "{deleted:?}");
    assert!(revision_texts(&doc, RevisionKind::Insertion).contains(&"大阪".to_owned()));
    assert_eq!(texts(&accepted(&mut doc)), edited);
    assert_eq!(texts(&rejected(&mut doc)), original);
}

/// A document with one paragraph per id of `links`, each a hyperlink through
/// that relationship id, the last one followed by the text `tail`, and one
/// relationship to `address` per distinct id.
fn doc_with_links(links: &[&str], address: &str, tail: &str) -> Document {
    let body = links
        .iter()
        .enumerate()
        .map(|(index, id)| {
            format!(
                r#"<w:p><w:hyperlink xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="{id}"><w:r><w:t>site {index}</w:t></w:r></w:hyperlink>{}</w:p>"#,
                if index + 1 == links.len() && !tail.is_empty() {
                    format!(r#"<w:r><w:t xml:space="preserve">{tail}</w:t></w:r>"#)
                } else {
                    String::new()
                }
            )
        })
        .collect::<String>();
    let mut doc = doc_with_body(&body);
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    let relationships = package.get_or_create_part_rels("/word/document.xml");
    let mut ids = links.to_vec();
    ids.dedup();
    for id in ids {
        relationships.items.push(oxoffice_opc::Relationship {
            id: id.to_owned(),
            rel_type:
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink"
                    .to_owned(),
            target: address.to_owned(),
            target_mode: Some("External".to_owned()),
        });
    }
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

#[test]
fn links_reaching_one_address_through_other_relationships_are_no_change() {
    const SITE: &str = "https://example.com/";
    for (original, edited) in [
        (
            &["rIdShared", "rIdShared"][..],
            &["rIdShared", "rIdOther"][..],
        ),
        (&["rIdA", "rIdB"][..], &["rIdC", "rIdC"][..]),
    ] {
        let mut doc = redline(
            doc_with_links(original, SITE, ""),
            &doc_with_links(edited, SITE, ""),
        );
        assert_eq!(kinds(&doc), [], "{original:?} against {edited:?}");
        let urls = |doc: &Document| {
            doc.links()
                .into_iter()
                .map(|link| link.url)
                .collect::<Vec<_>>()
        };
        assert_eq!(urls(&doc), [Some(SITE.to_owned()), Some(SITE.to_owned())]);
        assert_eq!(urls(&accepted(&mut doc)), urls(&rejected(&mut doc)));
    }
}

#[test]
fn text_added_after_a_link_reaching_its_address_through_another_relationship_is_the_only_change() {
    const SITE: &str = "https://example.com/";
    let changes = |doc: &Document| {
        (
            kinds(doc),
            revision_texts(doc, RevisionKind::Deletion),
            revision_texts(doc, RevisionKind::Insertion),
        )
    };
    for (original, edited) in [
        (
            &["rIdShared", "rIdShared"][..],
            &["rIdShared", "rIdOther"][..],
        ),
        (&["rIdA", "rIdB"][..], &["rIdC", "rIdC"][..]),
    ] {
        // The same edit with the relationships unchanged.
        let wanted = redline(
            doc_with_links(original, SITE, " end"),
            &doc_with_links(original, SITE, " end and more"),
        );
        let mut doc = redline(
            doc_with_links(original, SITE, " end"),
            &doc_with_links(edited, SITE, " end and more"),
        );
        assert_eq!(
            changes(&doc),
            changes(&wanted),
            "{original:?} against {edited:?}"
        );
        assert!(
            !revision_texts(&doc, RevisionKind::Insertion)
                .concat()
                .contains("site")
        );
        assert_eq!(
            texts(&accepted(&mut doc)),
            ["site 0", "site 1 end and more"]
        );
        assert_eq!(texts(&rejected(&mut doc)), ["site 0", "site 1 end"]);
    }
}

// ---- Comparing editors' files ----
// A comparison of the corpus report costs seconds in a debug build, so these
// tests compare once and read the redline back through its resolved copy.

#[test]
fn a_held_paragraph_whose_dropped_value_changed_keeps_the_refusal() {
    // Automatic spacing turned from on to off: the held paragraph changed,
    // and the comparison may not replace it.
    let body = |value: &str| {
        doc_with_body(&format!(
            concat!(
                r#"<w:p><w:r><w:t>Term.</w:t></w:r></w:p>"#,
                r#"<w:p><w:pPr><w:spacing w:beforeAutospacing="{}"/></w:pPr><w:r><w:t xml:space="preserve">Kept </w:t></w:r>"#,
                r#"<w:ins w:id="1" w:author="Earlier" w:date="2026-01-01T00:00:00Z"><w:r><w:t>clause.</w:t></w:r></w:ins></w:p>"#,
            ),
            value
        ))
    };
    let mut original = body("on");
    let error = original.compare(&body("0"), AUTHOR, DATE).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("holds an existing tracked revision"),
        "{error}"
    );
    // Two spellings of on are the same value: the held paragraph is unchanged.
    let mut original = body("on");
    original.compare(&body("1"), AUTHOR, DATE).unwrap();
    // A value the reader drops by design: the residual keeps it.
    let body = |properties: &str| {
        doc_with_body(&format!(
            concat!(
                r#"<w:p><w:r><w:t>Term.</w:t></w:r></w:p>"#,
                r#"<w:p><w:pPr>{}</w:pPr><w:r><w:t xml:space="preserve">Kept </w:t></w:r>"#,
                r#"<w:ins w:id="1" w:author="Earlier" w:date="2026-01-01T00:00:00Z"><w:r><w:t>clause.</w:t></w:r></w:ins></w:p>"#,
            ),
            properties
        ))
    };
    let mut original = body(r#"<w:jc w:val="bogus"/>"#);
    let error = original.compare(&body(""), AUTHOR, DATE).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("holds an existing tracked revision"),
        "{error}"
    );
}

#[test]
fn a_held_paragraph_whose_blank_math_text_changed_keeps_the_refusal() {
    // Blank text is content in a math run: the held paragraph changed, and
    // the comparison may not replace it.
    let body = |text: &str| {
        doc_with_body(&format!(
            concat!(
                r#"<w:p><w:ins w:id="1" w:author="Earlier" w:date="2026-01-01T00:00:00Z">"#,
                r#"<w:r><w:t>Area </w:t></w:r></w:ins>"#,
                r#"<m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">"#,
                r#"<m:r><m:t xml:space="preserve">{}</m:t></m:r></m:oMath></w:p>"#,
            ),
            text
        ))
    };
    let mut original = body(" ");
    let error = original.compare(&body("  "), AUTHOR, DATE).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("holds an existing tracked revision"),
        "{error}"
    );
}

// ---- LibreOffice files and final tables ----

// A textless run whose properties are written `<w:rPr></w:rPr>`, as
// LibreOffice writes its empty paragraphs, reads as the run the model writes
// back.
#[test]
fn an_empty_run_written_by_libreoffice_does_not_block_the_comparison() {
    let body = r#"<w:p><w:r><w:t>first</w:t></w:r></w:p><w:p><w:pPr><w:rPr></w:rPr></w:pPr><w:r><w:rPr></w:rPr></w:r></w:p>"#;
    let mut edited = doc_with_body(body);
    edited.add_paragraph("added");
    let mut doc = redline(doc_with_body(body), &edited);
    assert!(
        kinds(&doc)
            .iter()
            .all(|kind| *kind == RevisionKind::Insertion),
        "only the added paragraph is a change: {:?}",
        kinds(&doc)
    );
    assert_eq!(texts(&accepted(&mut doc)), ["first", "", "added"]);
}

// LibreOffice writes `<w:rPr></w:rPr>` on its runs, here on the run that
// closes its table of contents in the paragraph after the entries: it reads
// as `<w:rPr/>`, which carries no formatting, so the field's end keeps its
// place among the run's children.
#[test]
fn a_field_closed_by_a_run_with_empty_properties_does_not_block_the_comparison() {
    let body = concat!(
        r#"<w:p><w:pPr><w:pStyle w:val="TOC1"/><w:rPr></w:rPr></w:pPr>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"></w:fldChar></w:r>"#,
        r#"<w:r><w:rPr><w:webHidden/></w:rPr><w:instrText xml:space="preserve"> TOC \z \o &quot;1-3&quot; \u \h</w:instrText></w:r>"#,
        r#"<w:r><w:rPr><w:webHidden/></w:rPr><w:fldChar w:fldCharType="separate"/></w:r>"#,
        r#"<w:hyperlink w:anchor="_Toc217000001">"#,
        r#"<w:r><w:rPr><w:webHidden/></w:rPr><w:fldChar w:fldCharType="begin"></w:fldChar></w:r>"#,
        r#"<w:r><w:rPr><w:webHidden/></w:rPr><w:instrText xml:space="preserve">PAGEREF _Toc217000001 \h</w:instrText></w:r>"#,
        r#"<w:r><w:rPr><w:webHidden/></w:rPr><w:fldChar w:fldCharType="separate"/></w:r>"#,
        r#"<w:r><w:rPr><w:webHidden/></w:rPr><w:t>1</w:t><w:tab/><w:t>Introduction</w:t><w:tab/><w:t>1</w:t></w:r>"#,
        r#"<w:r><w:rPr><w:webHidden/></w:rPr><w:fldChar w:fldCharType="end"/></w:r></w:hyperlink></w:p>"#,
        r#"<w:p><w:pPr><w:rPr></w:rPr></w:pPr><w:r><w:rPr></w:rPr></w:r>"#,
        r#"<w:r><w:rPr></w:rPr><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    for spelling in [
        "<w:rPr></w:rPr>",
        "<w:rPr>\n</w:rPr>",
        "<w:rPr><w:rFonts/></w:rPr>",
    ] {
        let body = body.replace("<w:rPr></w:rPr>", spelling);
        let original = texts(&doc_with_body(&body));
        let mut edited = doc_with_body(&body);
        edited.add_paragraph("Added");
        let edited = reopen(&mut edited);
        let mut doc = redline(doc_with_body(&body), &edited);
        assert_eq!(revision_texts(&doc, RevisionKind::Insertion), ["Added"]);
        assert!(
            !kinds(&doc).contains(&RevisionKind::Deletion),
            "{:?}",
            kinds(&doc)
        );
        let mut expected = original.clone();
        expected.push("Added".to_owned());
        assert_eq!(texts(&accepted(&mut doc)), expected);
        assert_eq!(texts(&rejected(&mut doc)), original);
    }
}

// Google Docs writes `<w:tcPr/>` in every cell and LibreOffice
// `<w:trPr></w:trPr>` in every row: empty properties are no formatting.
#[test]
fn empty_row_and_cell_properties_compare_as_no_formatting() {
    let table = |row: &str, cell: &str| {
        doc_with_body(&format!(
            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr>{row}<w:tc>{cell}<w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#
        ))
    };
    for (original, edited) in [
        (table("<w:trPr/>", "<w:tcPr/>"), table("", "")),
        (
            table("", ""),
            table("<w:trPr></w:trPr>", "<w:tcPr></w:tcPr>"),
        ),
    ] {
        let mut doc = original;
        let diagnostics = doc.compare(&edited, AUTHOR, DATE).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert!(doc.revisions().is_empty());
    }
}

/// A new document holding these body owners in order: a paragraph for each
/// text, and a one-cell table for a text written `[cell text]`.
fn doc_with_owners(owners: &[&str]) -> Document {
    let mut doc = Document::new();
    for owner in owners {
        if let Some(cell) = owner.strip_prefix('[').and_then(|o| o.strip_suffix(']')) {
            doc.add_table(1, 1).cell(0, 0).unwrap().set_text(cell);
        } else {
            doc.add_paragraph(owner);
        }
    }
    doc
}

/// The body owners of `doc` in order, written as [`doc_with_owners`] takes
/// them.
fn owners(doc: &Document) -> Vec<String> {
    doc.body_items()
        .map(|item| match item {
            BodyItemRef::Paragraph(paragraph) => paragraph.text(),
            BodyItemRef::Table(table) => format!("[{}]", table.cell(0, 0).unwrap().text()),
            _ => panic!("the body holds only paragraphs and tables"),
        })
        .collect()
}

/// Whether the main part of `doc` holds a table row marked as moved away
/// and one marked as moved here. Row move markers are not listed among
/// the revisions.
fn table_moved(doc: &mut Document) -> bool {
    let xml = main_part_xml(doc);
    xml.contains("<w:trPr><w:moveFrom ") && xml.contains("<w:trPr><w:moveTo ")
}

/// The saved main part of `doc`.
fn main_part_xml(doc: &mut Document) -> String {
    let bytes = doc.to_bytes().expect("document saves");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a ZIP package");
    let mut xml = String::new();
    std::io::Read::read_to_string(
        &mut zip.by_name("word/document.xml").expect("main part"),
        &mut xml,
    )
    .expect("main part is UTF-8");
    xml
}

/// `owners` redlined into `edited` by an earlier comparison, saved and
/// reopened: its tracked changes are existing ones for the next comparison.
fn earlier_redline(owners: &[&str], edited: &[&str]) -> Document {
    redline(doc_with_owners(owners), &doc_with_owners(edited))
}

/// The text and alignment of each body paragraph, in order.
fn aligned_texts(doc: &Document) -> Vec<(String, Option<Alignment>)> {
    doc.paragraphs()
        .iter()
        .map(|paragraph| (paragraph.text(), paragraph.alignment()))
        .collect()
}

/// These paragraphs, the one at `centred` centred.
fn doc_with_centred(paragraphs: &[&str], centred: usize) -> Document {
    let mut doc = doc_with(paragraphs);
    doc.paragraph_mut(centred)
        .unwrap()
        .set_alignment(Alignment::Center);
    doc
}

#[test]
fn the_final_paragraph_deleted_and_the_one_before_moved_away_are_compared() {
    let original = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "closing",
        "Alpha paragraph.",
    ];
    let edited = ["Alpha paragraph.", "Beta paragraph.", "Gamma paragraph."];
    let mut doc = redline(
        doc_with_centred(&original, 1),
        &doc_with_centred(&edited, 2),
    );
    assert_eq!(
        revision_texts(&doc, RevisionKind::MoveFrom),
        ["Alpha paragraph."]
    );
    assert_eq!(
        aligned_texts(&accepted(&mut doc)),
        aligned_texts(&doc_with_centred(&edited, 2))
    );
    assert_eq!(
        aligned_texts(&rejected(&mut doc)),
        aligned_texts(&doc_with_centred(&original, 1))
    );
}

#[test]
fn a_paragraph_moved_to_the_end_after_a_moved_table_is_compared() {
    let original = [
        "Alpha paragraph.",
        "[cell]",
        "Beta paragraph.",
        "Gamma paragraph.",
    ];
    let edited = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "added",
        "[cell]",
        "Alpha paragraph.",
    ];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    assert!(table_moved(&mut doc), "the table is moved");
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

/// The distinct revision ids of `doc`, in order.
fn distinct_ids(doc: &Document) -> Vec<i32> {
    let mut seen = Vec::new();
    for id in ids(doc) {
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    seen
}

/// The body owners after resolving revision `id` alone, accepting it or
/// rejecting it, as the text shows them: the other revisions stay tracked.
fn resolved_alone(doc: &mut Document, id: i32, accept: bool) -> Vec<String> {
    let mut copy = reopen(doc);
    let resolved = if accept {
        copy.accept_revision_id(id)
    } else {
        copy.reject_revision_id(id)
    };
    assert!(resolved.unwrap() > 0, "revision {id} resolves");
    let copy = reopen(&mut copy);
    assert!(
        copy.revisions().iter().all(|revision| revision.id() != id),
        "revision {id} is resolved whole"
    );
    owners(&copy)
}

#[test]
fn paragraphs_moved_together_to_the_end_move_as_one_change() {
    let original = [
        "Alpha paragraph.",
        "Beta paragraph.",
        "Gamma paragraph.",
        "Delta paragraph.",
    ];
    let edited = [
        "Gamma paragraph.",
        "Delta paragraph.",
        "Alpha paragraph.",
        "Beta paragraph.",
    ];
    let mut doc = redline(doc_with(&original), &doc_with(&edited));
    let moves = distinct_ids(&doc);
    assert_eq!(moves.len(), 1, "{:?}", kinds(&doc));
    assert_eq!(resolved_alone(&mut doc, moves[0], true), edited);
    assert_eq!(resolved_alone(&mut doc, moves[0], false), original);
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_block_moved_to_the_end_after_an_added_paragraph_resolves_alone_to_either_version() {
    let original = [
        "Alpha paragraph.",
        "Beta paragraph.",
        "Gamma paragraph.",
        "Delta paragraph.",
    ];
    let edited = [
        "Gamma paragraph.",
        "Delta paragraph.",
        "Added paragraph.",
        "Alpha paragraph.",
        "Beta paragraph.",
    ];
    let mut doc = redline(doc_with(&original), &doc_with(&edited));
    let moved = doc
        .revisions()
        .iter()
        .filter(|revision| revision.kind() == RevisionKind::MoveTo)
        .map(|revision| revision.id())
        .collect::<Vec<_>>();
    assert_eq!(moved.len(), 2, "both paragraphs are moved");
    assert_eq!(moved[0], moved[1], "they move as one block");
    let id = moved[0];
    assert_eq!(resolved_alone(&mut doc, id, true), edited);
    // The added paragraph stays, still tracked.
    assert_eq!(
        resolved_alone(&mut doc, id, false),
        [
            "Alpha paragraph.",
            "Beta paragraph.",
            "Gamma paragraph.",
            "Delta paragraph.",
            "Added paragraph."
        ]
    );
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_paragraph_moved_to_the_end_after_an_added_table_resolves_alone() {
    let original = ["Alpha paragraph.", "Gamma paragraph.", "Delta paragraph."];
    let edited = [
        "Gamma paragraph.",
        "Delta paragraph.",
        "Added paragraph.",
        "[cell]",
        "Alpha paragraph.",
    ];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    let id = doc
        .revisions()
        .iter()
        .find(|revision| revision.kind() == RevisionKind::MoveTo)
        .map(|revision| revision.id())
        .expect("the paragraph is moved");
    assert_eq!(resolved_alone(&mut doc, id, true), edited);
    // Undone alone, the move leaves the body's last paragraph empty after
    // the added table, as Word ends a body with a paragraph after a table.
    assert_eq!(
        resolved_alone(&mut doc, id, false),
        [
            "Alpha paragraph.",
            "Gamma paragraph.",
            "Delta paragraph.",
            "Added paragraph.",
            "[cell]",
            ""
        ]
    );
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_paragraph_moved_to_the_end_right_after_an_added_table_resolves_alone() {
    let original = ["Alpha paragraph.", "Gamma paragraph.", "Delta paragraph."];
    let edited = [
        "Gamma paragraph.",
        "Delta paragraph.",
        "[cell]",
        "Alpha paragraph.",
    ];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    let id = doc
        .revisions()
        .iter()
        .find(|revision| revision.kind() == RevisionKind::MoveTo)
        .map(|revision| revision.id())
        .expect("the paragraph is moved");
    assert_eq!(resolved_alone(&mut doc, id, true), edited);
    // Undone alone, the move leaves the last paragraph empty after the
    // added table, still tracked.
    assert_eq!(
        resolved_alone(&mut doc, id, false),
        [
            "Alpha paragraph.",
            "Gamma paragraph.",
            "Delta paragraph.",
            "[cell]",
            ""
        ]
    );
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn paragraphs_moved_together_from_the_end_move_as_one_change() {
    let original = [
        "Gamma paragraph.",
        "Delta paragraph.",
        "Alpha paragraph.",
        "Beta paragraph.",
    ];
    let edited = [
        "Alpha paragraph.",
        "Beta paragraph.",
        "Gamma paragraph.",
        "Delta paragraph.",
    ];
    let mut doc = redline(doc_with(&original), &doc_with(&edited));
    let moves = distinct_ids(&doc);
    assert_eq!(moves.len(), 1, "{:?}", kinds(&doc));
    assert_eq!(resolved_alone(&mut doc, moves[0], true), edited);
    assert_eq!(resolved_alone(&mut doc, moves[0], false), original);
}

#[test]
fn a_paragraph_moved_away_from_the_end_right_after_a_removed_table_resolves_alone() {
    let original = [
        "Gamma paragraph.",
        "Delta paragraph.",
        "[cell]",
        "Alpha paragraph.",
    ];
    let edited = ["Alpha paragraph.", "Gamma paragraph.", "Delta paragraph."];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    let id = doc
        .revisions()
        .iter()
        .find(|revision| revision.kind() == RevisionKind::MoveFrom)
        .map(|revision| revision.id())
        .expect("the paragraph is moved");
    // Done alone, the move leaves the last paragraph empty after the
    // removed table, still tracked.
    assert_eq!(
        resolved_alone(&mut doc, id, true),
        [
            "Alpha paragraph.",
            "Gamma paragraph.",
            "Delta paragraph.",
            "[]",
            ""
        ]
    );
    // Undone alone, the move leaves the removed table tracked: the text
    // shown hides its cell.
    assert_eq!(
        resolved_alone(&mut doc, id, false),
        [
            "Gamma paragraph.",
            "Delta paragraph.",
            "[]",
            "Alpha paragraph."
        ]
    );
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn paragraphs_moved_to_the_end_from_separate_places_keep_their_own_moves() {
    let original = [
        "Alpha paragraph.",
        "Xray paragraph.",
        "Beta paragraph.",
        "Yankee paragraph.",
    ];
    let edited = [
        "Xray paragraph.",
        "Yankee paragraph.",
        "Alpha paragraph.",
        "Beta paragraph.",
    ];
    let mut doc = redline(doc_with(&original), &doc_with(&edited));
    let moved = doc
        .revisions()
        .iter()
        .filter(|revision| revision.kind() == RevisionKind::MoveTo)
        .map(|revision| revision.id())
        .collect::<Vec<_>>();
    assert_eq!(moved.len(), 2, "{:?}", kinds(&doc));
    assert_ne!(moved[0], moved[1], "each paragraph keeps its own move");
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_table_added_at_the_end_keeps_its_change_off_the_last_paragraph_mark() {
    let original = ["kept"];
    let edited = ["kept", "[cell]", ""];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    let xml = main_part_xml(&mut doc);
    let last = &xml[xml.rfind("<w:p>").or_else(|| xml.rfind("<w:p ")).unwrap()..];
    assert!(
        !last.contains("<w:ins "),
        "the last mark is not tracked: {last}"
    );
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_paragraph_deleted_before_a_final_table_deleted_by_an_earlier_redline_is_compared() {
    let mut redlined = earlier_redline(&["intro", "[cell]"], &["intro"]);
    let mut original = reopen(&mut redlined);
    original.insert_paragraph(1, "removed");
    // The cell's text is a tracked deletion.
    assert_eq!(owners(&original), ["intro", "removed", "[]"]);
    let mut doc = redline(original, &reopen(&mut redlined));
    assert_eq!(owners(&accepted(&mut doc)), ["intro"]);
    assert_eq!(owners(&rejected(&mut doc)), ["intro", "removed", "[cell]"]);
}

// ---- Equations ----

const MATH_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

/// The main story as saved.
fn main_xml(doc: &mut Document) -> String {
    let bytes = doc.to_bytes().expect("document saves");
    String::from_utf8(crate::support::package_parts(&bytes)["word/document.xml"].clone())
        .expect("main part is UTF-8")
}

/// The text of every `m:t` of the main story, in order.
fn math_texts(doc: &mut Document) -> Vec<String> {
    main_xml(doc)
        .split("<m:t")
        .skip(1)
        .filter(|rest| rest.starts_with(['>', ' ']))
        .map(|rest| {
            let content = &rest[rest.find('>').map_or(rest.len(), |end| end + 1)..];
            content
                .split("</m:t>")
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

// Empty paragraph properties that hold only an attribute, then an equation
// at the same run boundary: the equation still reads as itself once the
// comparison's check leaves those properties out.
#[test]
fn an_equation_after_empty_paragraph_properties_is_no_change() {
    let body = format!(
        concat!(
            r#"<w:p><w:pPr xmlns:x="urn:x" x:a="1"/><m:oMath xmlns:m="{}"><m:f><m:fPr><m:ctrlPr><w:rPr><w:i/></w:rPr></m:ctrlPr></m:fPr>"#,
            r#"<m:num><m:r><m:t>1</m:t></m:r></m:num><m:den><m:r><m:t>2</m:t></m:r></m:den></m:f></m:oMath></w:p>"#,
            r#"<w:p><w:r><w:t>small</w:t></w:r></w:p>"#,
        ),
        MATH_NS
    );
    let mut edited = doc_with_body(&body);
    assert_eq!(edited.replace_all(&HashMap::from([("small", "large")])), 1);
    let edited = reopen(&mut edited);
    let mut doc = redline(doc_with_body(&body), &edited);
    assert_eq!(revision_texts(&doc, RevisionKind::Deletion), ["small"]);
    assert_eq!(math_texts(&mut accepted(&mut doc)), ["1", "2"]);
}

// A paragraph another reviewer changed holds an equation whose fraction
// leaves its bar type out, as Word writes it; a save writes the equation
// again with the type spelled out: the paragraph is unchanged.
#[test]
fn a_held_paragraph_holding_an_equation_a_save_wrote_again_is_unchanged() {
    let body = format!(
        concat!(
            r#"<w:p><w:ins w:id="1" w:author="Ada" w:date="2026-01-01T00:00:00Z"><w:r><w:t xml:space="preserve">held </w:t></w:r></w:ins>"#,
            r#"<m:oMath xmlns:m="{}"><m:f><m:fPr><m:ctrlPr><w:rPr><w:i/></w:rPr></m:ctrlPr></m:fPr>"#,
            r#"<m:num><m:r><m:t>1</m:t></m:r></m:num><m:den><m:r><m:t>2</m:t></m:r></m:den></m:f></m:oMath></w:p>"#,
            r#"<w:p><w:r><w:t>small</w:t></w:r></w:p>"#,
        ),
        MATH_NS
    );
    let mut edited = doc_with_body(&body);
    assert_eq!(edited.replace_all(&HashMap::from([("small", "large")])), 1);
    let mut edited = reopen(&mut edited);
    assert!(
        main_xml(&mut edited).contains(r#"<m:type m:val="bar"/>"#),
        "the save no longer spells the bar out: this test no longer tests the comparison"
    );
    let mut doc = redline(doc_with_body(&body), &edited);
    let others = doc
        .revisions()
        .iter()
        .filter(|revision| revision.author() != AUTHOR)
        .map(|revision| (revision.kind(), revision.author().to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(others, [(RevisionKind::Insertion, "Ada".to_owned())]);
    assert_eq!(revision_texts(&doc, RevisionKind::Deletion), ["small"]);
    assert_eq!(math_texts(&mut accepted(&mut doc)), ["1", "2"]);
}

// ---- Bookmarks, tables of contents and notes ----

/// A table of contents as Word writes it, each entry a link to its heading's
/// `_Toc` bookmark with a page reference field, then the headings, each
/// followed by its text. Word renames the bookmarks when it rebuilds the
/// table: here they are `_Toc{base + n}` for the n-th heading. A section
/// whose heading is empty is its text alone.
fn report_with_toc(base: u32, sections: &[(&str, &str)]) -> Document {
    let mut body = String::from(
        r#"<w:p><w:r><w:t>Contents</w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-1" \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p>"#,
    );
    let headings = sections
        .iter()
        .filter(|(heading, _)| !heading.is_empty())
        .collect::<Vec<_>>();
    for (n, (heading, _)) in headings.iter().enumerate() {
        let name = format!("_Toc{}", base + n as u32);
        body.push_str(&format!(
            r#"<w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:hyperlink w:anchor="{name}"><w:r><w:t>{heading}</w:t></w:r><w:r><w:tab/></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGEREF {name} \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:hyperlink></w:p>"#
        ));
    }
    body.push_str(r#"<w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#);
    let mut n = 0;
    for (heading, text) in sections {
        if !heading.is_empty() {
            let name = format!("_Toc{}", base + n);
            body.push_str(&format!(
                r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="{n}" w:name="{name}"/><w:r><w:t>{heading}</w:t></w:r><w:bookmarkEnd w:id="{n}"/></w:p>"#
            ));
            n += 1;
        }
        body.push_str(&format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"));
    }
    doc_with_body(&body)
}

fn bookmark_names(doc: &Document) -> Vec<String> {
    doc.bookmarks()
        .iter()
        .map(|bookmark| bookmark.name().unwrap_or_default().to_owned())
        .collect()
}

/// Each link as (its text, the text of the bookmark it names, if any).
fn link_targets(doc: &Document) -> Vec<(String, Option<String>)> {
    let bookmarks = doc.bookmarks();
    doc.links()
        .into_iter()
        .map(|link| {
            let target = bookmarks
                .iter()
                .find(|bookmark| bookmark.name() == link.anchor.as_deref())
                .map(|bookmark| bookmark.text().to_owned());
            (link.text, target)
        })
        .collect()
}

fn assert_every_entry_links_its_heading(doc: &Document, headings: &[&str]) {
    let targets = link_targets(doc);
    assert_eq!(targets.len(), headings.len(), "{targets:?}");
    for ((entry, target), heading) in targets.iter().zip(headings) {
        assert!(entry.starts_with(heading), "{targets:?}");
        assert_eq!(target.as_deref(), Some(*heading), "{targets:?}");
    }
}

#[test]
fn a_heading_added_before_a_rebuilt_toc_is_the_only_heading_change() {
    let sections = [("Scope", "What we cover."), ("Method", "How we work.")];
    let original = report_with_toc(1, &sections);
    // The rebuild numbered the bookmarks from the new first heading, so
    // Context holds the name and the id Scope held.
    let edited = report_with_toc(
        1,
        &[
            ("Context", "Why now."),
            ("Scope", "What we cover."),
            ("Method", "How we work, step by step."),
        ],
    );
    let mut doc = redline(report_with_toc(1, &sections), &edited);

    let inserted = revision_texts(&doc, RevisionKind::Insertion);
    assert!(
        inserted.iter().any(|text| text.contains("Context")),
        "{inserted:?}"
    );
    let revised = revision_texts(&doc, RevisionKind::Deletion)
        .into_iter()
        .chain(inserted)
        .collect::<Vec<_>>();
    assert!(
        !revised.iter().any(|text| text.contains("Scope")),
        "an unchanged heading is not revised: {revised:?}"
    );

    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_every_entry_links_its_heading(&copy, &["Context", "Scope", "Method"]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&original));
    assert_eq!(bookmark_names(&copy), bookmark_names(&original));
    assert_every_entry_links_its_heading(&copy, &["Scope", "Method"]);
}

#[test]
fn a_heading_renamed_after_a_changed_table_keeps_its_bookmark() {
    // The bookmark is the same in both versions: the heading is revised in
    // place, whatever the table before it gained.
    let document = |rows: &[&str], heading: &str| {
        let rows = rows
            .iter()
            .map(|text| {
                format!("<w:tr><w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc></w:tr>")
            })
            .collect::<String>();
        doc_with_body(&format!(
            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>{rows}</w:tbl><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="5" w:name="_Toc5"/><w:r><w:t>{heading}</w:t></w:r><w:bookmarkEnd w:id="5"/></w:p><w:p><w:r><w:t>Body.</w:t></w:r></w:p>"#
        ))
    };
    let mut doc = redline(
        document(&["r1", "r2"], "Results"),
        &document(&["r1", "r2", "r3"], "Findings"),
    );
    assert_eq!(revision_texts(&doc, RevisionKind::Deletion), ["Results"]);
    assert!(
        revision_texts(&doc, RevisionKind::Insertion).contains(&"Findings".to_owned()),
        "{:?}",
        revision_texts(&doc, RevisionKind::Insertion)
    );
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Findings", "Body."]);
    assert_eq!(bookmark_names(&copy), ["_Toc5"]);
    assert_eq!(bookmark_names(&rejected(&mut doc)), ["_Toc5"]);
}

/// The instruction of every field of the body, in order.
fn field_instructions(doc: &Document) -> Vec<String> {
    let mut instructions = Vec::new();
    for paragraph in doc.paragraphs() {
        for run in paragraph.runs() {
            for item in run.items() {
                if let RunItemRef::Field(field) = item {
                    instructions.push(field.instruction().trim().to_owned());
                }
            }
        }
    }
    instructions
}

#[test]
fn simple_fields_and_hyperlink_fields_follow_a_renamed_toc_bookmark() {
    let document = |name: &str, text: &str| {
        doc_with_body(&format!(
            concat!(
                r#"<w:p><w:r><w:t xml:space="preserve">See page </w:t></w:r><w:fldSimple w:instr=" PAGEREF {name} \h "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>"#,
                r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK \l &quot;{name}&quot; </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Intro</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
                r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="{name}"/><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
                r#"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"#,
            ),
            name = name,
            text = text,
        ))
    };
    let mut doc = redline(document("_Toc1", "Old."), &document("_Toc7", "New."));
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy)[2..], ["Intro", "New."]);
    assert_eq!(bookmark_names(&copy), ["_Toc1"]);
    assert_eq!(
        field_instructions(&copy),
        [r#"PAGEREF _Toc1 \h"#, r#"HYPERLINK \l "_Toc1""#]
    );
}

#[test]
fn a_reference_in_another_case_follows_a_renamed_toc_bookmark_but_a_link_to_another_file_does_not()
{
    let document = |name: &str, text: &str| {
        doc_with_body(&format!(
            concat!(
                r#"<w:p><w:fldSimple w:instr=" PAGEREF {upper} \h "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>"#,
                r#"<w:p><w:fldSimple w:instr=" HYPERLINK \o &quot;tip&quot; &quot;other.docx&quot; \l &quot;_Toc7&quot; "><w:r><w:t>Other</w:t></w:r></w:fldSimple></w:p>"#,
                // The address is a nested field.
                r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK </w:instrText></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Url </w:instrText></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>url</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
                r#"<w:r><w:instrText xml:space="preserve"> \l &quot;_Toc7&quot; </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
                r#"<w:r><w:t>Away</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
                r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="{name}"/><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
                r#"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"#,
            ),
            name = name,
            upper = name.to_uppercase(),
            text = text,
        ))
    };
    let mut doc = redline(document("_Toc1", "Old."), &document("_Toc7", "New."));
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy)[3..], ["Intro", "New."]);
    assert_eq!(bookmark_names(&copy), ["_Toc1"]);
    assert_eq!(
        field_instructions(&copy),
        [
            r#"PAGEREF _Toc1 \h"#,
            r#"HYPERLINK \o "tip" "other.docx" \l "_Toc7""#,
            r#"HYPERLINK  \l "_Toc7""#,
        ]
    );
}

/// Each body paragraph's text with its footnote references shown as
/// `[note text]`, whatever their ids.
fn noted_texts(doc: &Document) -> Vec<String> {
    let notes = doc.footnotes().into_iter().collect::<HashMap<_, _>>();
    doc.paragraphs()
        .iter()
        .map(|paragraph| {
            let mut text = String::new();
            for run in paragraph.runs() {
                for item in run.items() {
                    match item {
                        RunItemRef::Text(part) => text.push_str(part),
                        RunItemRef::FootnoteReference(id) => {
                            let note = notes.get(&id).map_or("?", |note| note.trim());
                            text.push_str(&format!("[{note}]"));
                        }
                        _ => {}
                    }
                }
            }
            text
        })
        .collect()
}

#[test]
fn a_moved_footnote_whose_text_changed_is_compared() {
    let original = doc_with_notes(&[("Claim one.", Some("Source")), ("Claim two.", None)]);
    let edited = doc_with_notes(&[("Claim one.", None), ("Claim two.", Some("Source, p. 4"))]);
    let mut doc = redline(original, &edited);
    assert_eq!(
        noted_texts(&accepted(&mut doc)),
        ["Claim one.", "Claim two.[Source, p. 4]"]
    );
    assert_eq!(
        noted_texts(&rejected(&mut doc)),
        ["Claim one.[Source]", "Claim two."]
    );
}

/// A new document holding these paragraphs, each with its footnote if any,
/// the notes created in paragraph order as Word numbers them when it saves.
fn doc_with_notes_in_order(paragraphs: &[(&str, Option<&str>)]) -> Document {
    let mut doc = Document::new();
    let ids = paragraphs
        .iter()
        .map(|(_, note)| note.map(|note| doc.add_footnote(note)))
        .collect::<Vec<_>>();
    for ((text, _), id) in paragraphs.iter().zip(ids) {
        let mut paragraph = doc.add_paragraph(text);
        if let Some(id) = id {
            paragraph.add_footnote_ref(id);
        }
    }
    doc
}

#[test]
fn a_note_added_before_others_is_the_only_note_change() {
    // Word numbers notes by position: the new first note takes the id of
    // the old first one, and every later note moves up one.
    let sections = [
        ("One.", Some("A")),
        ("Two.", Some("B")),
        ("Three.", Some("C")),
    ];
    let edited = doc_with_notes_in_order(&[
        ("Zero.", Some("N")),
        ("One.", Some("A")),
        ("Two.", Some("B")),
        ("Three.", Some("C")),
    ]);
    let mut doc = redline(doc_with_notes_in_order(&sections), &edited);
    assert_eq!(
        revision_texts(&doc, RevisionKind::Deletion),
        Vec::<String>::new()
    );
    let inserted = revision_texts(&doc, RevisionKind::Insertion);
    assert!(
        !inserted
            .iter()
            .any(|text| ["A", "B", "C"].contains(&text.as_str())),
        "an unchanged note is not revised: {inserted:?}"
    );
    assert_eq!(
        noted_texts(&accepted(&mut doc)),
        ["Zero.[N]", "One.[A]", "Two.[B]", "Three.[C]"]
    );
    assert_eq!(
        noted_texts(&rejected(&mut doc)),
        ["One.[A]", "Two.[B]", "Three.[C]"]
    );
}

#[test]
fn a_note_the_original_does_not_reference_keeps_the_id_a_pair_would_leave() {
    // The original holds a draft note it does not reference before the
    // referenced one; the edited version, numbered by position, holds the
    // referenced note under the draft's id. The draft is revised in place.
    let original = {
        let mut doc = Document::new();
        doc.add_footnote("Draft");
        let id = doc.add_footnote("A");
        doc.add_paragraph("Claim.").add_footnote_ref(id);
        doc
    };
    let edited = {
        let mut doc = Document::new();
        let id = doc.add_footnote("A");
        doc.add_paragraph("Claim.").add_footnote_ref(id);
        doc
    };
    let notes = |doc: &Document| {
        doc.footnotes()
            .into_iter()
            .map(|(id, text)| (id, text.trim().to_owned()))
            .collect::<Vec<_>>()
    };
    let before = notes(&original);
    let mut doc = redline(original, &edited);
    let copy = accepted(&mut doc);
    assert_eq!(noted_texts(&copy), ["Claim.[A]"]);
    assert_eq!(notes(&copy), [(before[0].0, "A".to_owned())]);
    let copy = rejected(&mut doc);
    assert_eq!(noted_texts(&copy), ["Claim.[A]"]);
    assert_eq!(notes(&copy), before);
}

/// Two body paragraphs, an endnote at the end of each listed one.
fn doc_with_endnotes(at: &[usize]) -> Document {
    let mut doc = Document::new();
    doc.add_paragraph("Claim one.");
    doc.add_paragraph("Claim two.");
    for &index in at {
        let location = doc
            .paragraph_story_location(index)
            .expect("paragraph location resolves")
            .expect("paragraph exists");
        doc.create_endnote(&location, "Source")
            .expect("endnote is created");
    }
    doc
}

/// Each endnote reference of the body: its paragraph and the endnote id.
fn endnote_references(doc: &Document) -> Vec<(usize, i32)> {
    let mut references = Vec::new();
    for (index, paragraph) in doc.paragraphs().iter().enumerate() {
        for run in paragraph.runs() {
            for item in run.items() {
                if let RunItemRef::EndnoteReference(id) = item {
                    references.push((index, id));
                }
            }
        }
    }
    references
}

#[test]
fn a_moved_footnote_reference_with_footnotes_ignored_is_refused() {
    // Ignored notes keep the original's, so the moved reference cannot get
    // its own note: the redline would name one note from both places.
    let mut doc = doc_with_notes(&[("Claim one.", Some("Source")), ("Claim two.", None)]);
    let edited = doc_with_notes(&[("Claim one.", None), ("Claim two.", Some("Source"))]);
    let options = ComparisonOptions {
        ignored_stories: vec![ComparisonStoryKind::Footnote],
        ..Default::default()
    };
    let error = doc
        .compare_with_options(&edited, AUTHOR, DATE, &options)
        .unwrap_err();
    assert!(
        error.to_string().contains("footnote") && error.to_string().contains("moved"),
        "{error}"
    );
    assert!(doc.revisions().is_empty());
}

/// A new document holding these paragraphs, each with a footnote when the
/// first note is given and an endnote when the second is. The notes of each
/// kind are created in paragraph order, as Word numbers them when it saves.
fn doc_with_both_notes(paragraphs: &[(&str, Option<&str>, Option<&str>)]) -> Document {
    let mut doc = Document::new();
    for (text, footnote, _) in paragraphs {
        let id = footnote.map(|note| doc.add_footnote(note));
        let mut paragraph = doc.add_paragraph(text);
        if let Some(id) = id {
            paragraph.add_footnote_ref(id);
        }
    }
    for (index, (_, _, endnote)) in paragraphs.iter().enumerate() {
        if let Some(note) = endnote {
            let location = doc
                .paragraph_story_location(index)
                .expect("paragraph location resolves")
                .expect("paragraph exists");
            doc.create_endnote(&location, note)
                .expect("endnote is created");
        }
    }
    doc
}

/// A new document holding these paragraphs, each with its endnote if any,
/// created in paragraph order.
fn doc_with_endnote_texts(paragraphs: &[(&str, Option<&str>)]) -> Document {
    doc_with_both_notes(
        &paragraphs
            .iter()
            .map(|&(text, note)| (text, None, note))
            .collect::<Vec<_>>(),
    )
}

/// The text of endnote `id`, or `?` when the document has none.
fn endnote_text(doc: &Document, id: i32) -> String {
    let Some(story) = doc.endnote_story(id).expect("endnote lookup succeeds") else {
        return "?".to_owned();
    };
    doc.story_items(&story)
        .expect("endnote story reads")
        .iter()
        .map(|item| item.text().expect("endnote item reads").unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Each body paragraph's text with its note references shown where they sit,
/// whatever their ids: a footnote as `[text]`, an endnote as `{text}`.
fn note_marked_texts(doc: &Document) -> Vec<String> {
    let footnotes = doc.footnotes().into_iter().collect::<HashMap<_, _>>();
    doc.paragraphs()
        .iter()
        .map(|paragraph| {
            let mut text = String::new();
            for run in paragraph.runs() {
                for item in run.items() {
                    match item {
                        RunItemRef::Text(part) => text.push_str(part),
                        RunItemRef::FootnoteReference(id) => {
                            let note = footnotes.get(&id).map_or("?", |note| note.trim());
                            text.push_str(&format!("[{note}]"));
                        }
                        RunItemRef::EndnoteReference(id) => {
                            text.push_str(&format!("{{{}}}", endnote_text(doc, id)));
                        }
                        _ => {}
                    }
                }
            }
            text
        })
        .collect()
}

/// The texts of every normal footnote and endnote the document holds, in
/// file order, so that a note left behind without its reference shows.
fn held_note_texts(doc: &Document) -> (Vec<String>, Vec<String>) {
    let footnotes = doc
        .footnotes()
        .into_iter()
        .map(|(_, text)| text.trim().to_owned())
        .collect();
    let endnotes = doc
        .stories()
        .expect("stories are listed")
        .into_iter()
        .filter(|story| story.kind() == StoryKind::Endnote)
        .map(|story| {
            doc.story_items(&story)
                .expect("endnote story reads")
                .iter()
                .map(|item| item.text().expect("endnote item reads").unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_owned()
        })
        .collect();
    (footnotes, endnotes)
}

/// The redline of `original` into `edited`, checked: accepting it gives the
/// edited version and rejecting it the original, each body paragraph with
/// its notes, and the notes each version holds.
fn checked_note_redline(original: Document, edited: &Document) -> Document {
    let wanted = |doc: &Document| (note_marked_texts(doc), held_note_texts(doc));
    let (before, after) = (wanted(&original), wanted(edited));
    let mut doc = redline(original, edited);
    assert_eq!(wanted(&accepted(&mut doc)), after, "accepting the redline");
    assert_eq!(wanted(&rejected(&mut doc)), before, "rejecting the redline");
    doc
}

#[test]
fn an_added_endnote_is_inserted_with_its_reference() {
    let original = doc_with_endnote_texts(&[("Claim one.", Some("A")), ("Claim two.", None)]);
    let edited = doc_with_endnote_texts(&[("Claim one.", Some("A")), ("Claim two.", Some("B"))]);
    let doc = checked_note_redline(original, &edited);
    // The note's text is inserted, and nothing is deleted.
    let inserted = revision_texts(&doc, RevisionKind::Insertion);
    assert!(
        inserted.iter().any(|text| text.trim() == "B"),
        "{inserted:?}"
    );
    assert_eq!(
        revision_texts(&doc, RevisionKind::Deletion),
        Vec::<String>::new()
    );
}

#[test]
fn a_removed_endnote_is_deleted_with_its_reference() {
    let original = doc_with_endnote_texts(&[("Claim one.", Some("A")), ("Claim two.", Some("B"))]);
    let edited = doc_with_endnote_texts(&[("Claim one.", Some("A")), ("Claim two.", None)]);
    checked_note_redline(original, &edited);
}

#[test]
fn the_first_endnote_of_a_document_creates_its_endnotes_part() {
    // The original has no endnotes part: accepting creates it, rejecting
    // leaves none.
    let original = doc_with_endnote_texts(&[("Claim one.", None), ("Claim two.", None)]);
    let edited = doc_with_endnote_texts(&[("Claim one.", None), ("Claim two.", Some("A"))]);
    let mut doc = checked_note_redline(original, &edited);
    assert_eq!(
        endnote_parts(&mut accepted(&mut doc)),
        ["word/endnotes.xml"]
    );
    assert!(endnote_parts(&mut rejected(&mut doc)).is_empty());
}

/// The names of the endnotes parts of the saved package.
fn endnote_parts(doc: &mut Document) -> Vec<String> {
    crate::support::package_parts(&doc.to_bytes().expect("document saves"))
        .into_keys()
        .filter(|name| name.contains("endnotes"))
        .collect()
}

#[test]
fn the_last_endnote_of_a_document_removed_takes_its_endnotes_part() {
    let original = doc_with_endnote_texts(&[("Claim one.", Some("A")), ("Claim two.", None)]);
    let edited = doc_with_endnote_texts(&[("Claim one.", None), ("Claim two.", None)]);
    let mut doc = checked_note_redline(original, &edited);
    assert!(endnote_parts(&mut accepted(&mut doc)).is_empty());
    assert_eq!(
        endnote_parts(&mut rejected(&mut doc)),
        ["word/endnotes.xml"]
    );
}

#[test]
fn an_endnote_reference_moved_to_another_paragraph_is_compared() {
    let original = doc_with_endnote_texts(&[("Claim one.", Some("Source")), ("Claim two.", None)]);
    let edited = doc_with_endnote_texts(&[("Claim one.", None), ("Claim two.", Some("Source"))]);
    checked_note_redline(original, &edited);
}

#[test]
fn an_endnote_reference_moved_within_its_paragraph_is_compared() {
    let document = |run_index: usize| {
        let mut doc = Document::new();
        doc.add_paragraph("Claim ").add_run("one.");
        let location = doc.paragraph_story_location(0).unwrap().unwrap();
        doc.create_endnote_at(
            &oxoffice_docx::StoryRunPosition {
                location,
                run_index,
            },
            "Source",
        )
        .expect("endnote is created");
        doc
    };
    let original = document(1);
    let edited = document(2);
    assert_eq!(note_marked_texts(&original), ["Claim {Source}one."]);
    checked_note_redline(original, &edited);
}

#[test]
fn a_moved_endnote_whose_text_changed_is_compared() {
    let original = doc_with_endnote_texts(&[("Claim one.", Some("Source")), ("Claim two.", None)]);
    let edited =
        doc_with_endnote_texts(&[("Claim one.", None), ("Claim two.", Some("Source, p. 4"))]);
    checked_note_redline(original, &edited);
}

#[test]
fn an_endnote_added_before_others_is_the_only_endnote_change() {
    // Endnotes are numbered by position: the new first endnote takes the id
    // of the old first one, and every later one moves up one.
    let original = doc_with_endnote_texts(&[
        ("One.", Some("A")),
        ("Two.", Some("B")),
        ("Three.", Some("C")),
    ]);
    let edited = doc_with_endnote_texts(&[
        ("Zero.", Some("N")),
        ("One.", Some("A")),
        ("Two.", Some("B")),
        ("Three.", Some("C")),
    ]);
    let doc = checked_note_redline(original, &edited);
    assert_eq!(
        revision_texts(&doc, RevisionKind::Deletion),
        Vec::<String>::new()
    );
    let inserted = revision_texts(&doc, RevisionKind::Insertion);
    assert!(
        !inserted
            .iter()
            .any(|text| ["A", "B", "C"].contains(&text.trim())),
        "an unchanged endnote is not revised: {inserted:?}"
    );
}

#[test]
fn a_paragraph_moved_with_its_endnote_is_compared() {
    let document = |swapped: bool| {
        let paragraphs = [("Claim.", Some("Source")), ("Other.", None)];
        let mut ordered = paragraphs.to_vec();
        if swapped {
            ordered.reverse();
        }
        doc_with_endnote_texts(&ordered)
    };
    checked_note_redline(document(false), &document(true));
}

#[test]
fn a_table_replaced_whole_with_the_endnote_it_holds_is_compared() {
    // A column added replaces the table whole: its endnote reference is
    // deleted with the old table and inserted with the new one.
    let document = |columns: usize| {
        let mut doc = doc_with_endnote_texts(&[("Cell.", Some("Source")), ("After.", None)]);
        with_part(&mut doc, "document.xml", |xml| {
            // The paragraph holding the endnote reference.
            let at = xml.find("<w:endnoteReference").expect("endnote reference");
            let start = xml[..at]
                .rfind("<w:p>")
                .max(xml[..at].rfind("<w:p "))
                .expect("its paragraph");
            let end = start + xml[start..].find("</w:p>").expect("its end") + "</w:p>".len();
            let grid = "<w:gridCol w:w=\"2000\"/>".repeat(columns);
            let extra = "<w:tc><w:p><w:r><w:t>More.</w:t></w:r></w:p></w:tc>".repeat(columns - 1);
            format!(
                "{}<w:tbl><w:tblPr/><w:tblGrid>{grid}</w:tblGrid><w:tr><w:tc>{}</w:tc>{extra}</w:tr></w:tbl>{}",
                &xml[..start],
                &xml[start..end],
                &xml[end..]
            )
        })
    };
    // The cells, the endnote references and the notes held.
    let shape = |doc: &mut Document| {
        let xml = main_part_xml(doc);
        (
            xml.matches("<w:tc>").count() + xml.matches("<w:tc ").count(),
            xml.matches("<w:endnoteReference ").count(),
            held_note_texts(doc),
        )
    };
    let (mut original, mut edited) = (document(1), document(2));
    let (before, after) = (shape(&mut original), shape(&mut edited));
    assert_eq!(before, (1, 1, (Vec::new(), vec!["Source".to_owned()])));
    let mut doc = redline(original, &edited);
    // Both tables are in the redline, each with its own reference.
    assert_eq!(shape(&mut doc).1, 2);
    assert_eq!(shape(&mut accepted(&mut doc)), after);
    assert_eq!(shape(&mut rejected(&mut doc)), before);
}

#[test]
fn a_footnote_and_an_endnote_changed_at_once_are_both_compared() {
    let original = doc_with_both_notes(&[
        ("Claim one.", Some("Foot A"), Some("End A")),
        ("Claim two.", None, Some("End B")),
        ("Claim three.", Some("Foot C"), None),
    ]);
    let edited = doc_with_both_notes(&[
        ("Claim one.", Some("Foot A"), None),
        ("Claim two.", Some("Foot B"), Some("End B")),
        ("Claim three.", Some("Foot C"), Some("End C")),
    ]);
    let mut doc = checked_note_redline(original, &edited);
    // Each kind's custody pairs carry its own letter.
    let xml = main_part_xml(&mut doc);
    assert!(xml.contains("_rdocxCmpNoteF"), "{xml}");
    assert!(xml.contains("_rdocxCmpNoteE"), "{xml}");
}

#[test]
fn a_footnote_and_an_endnote_moved_at_once_are_both_compared() {
    // Each moved note makes the comparison start again with it under a new
    // id; the ids an earlier pass gave are kept per kind.
    let original = doc_with_both_notes(&[
        ("Claim one.", Some("Foot"), Some("End")),
        ("Claim two.", None, None),
    ]);
    let edited = doc_with_both_notes(&[
        ("Claim one.", None, None),
        ("Claim two.", Some("Foot"), Some("End")),
    ]);
    checked_note_redline(original, &edited);
}

#[test]
fn endnotes_ignored_keep_the_original_endnotes() {
    // The edited version rewrites the endnote and the body: ignoring
    // endnotes compares the body and keeps the original's endnote whole.
    let mut doc = doc_with_endnote_texts(&[("Claim one.", Some("Source")), ("Claim two.", None)]);
    let edited =
        doc_with_endnote_texts(&[("Claim one.", Some("Other")), ("Claim two, edited.", None)]);
    let options = ComparisonOptions {
        ignored_stories: vec![ComparisonStoryKind::Endnote],
        ..Default::default()
    };
    let diagnostics = doc
        .compare_with_options(&edited, AUTHOR, DATE, &options)
        .expect("comparison");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let mut doc = reopen(&mut doc);
    assert_eq!(
        note_marked_texts(&accepted(&mut doc)),
        ["Claim one.{Source}", "Claim two, edited."]
    );
    assert_eq!(
        note_marked_texts(&rejected(&mut doc)),
        ["Claim one.{Source}", "Claim two."]
    );
}

#[test]
fn an_endnote_the_original_lacks_is_refused_while_endnotes_are_ignored() {
    // The original holds endnotes, but not the one the edited version adds.
    let mut doc = doc_with_endnotes(&[0]);
    let options = ComparisonOptions {
        ignored_stories: vec![ComparisonStoryKind::Endnote],
        ..Default::default()
    };
    let edited = doc_with_endnotes(&[0, 1]);
    let (_, added) = endnote_references(&edited)[1];
    let error = doc
        .compare_with_options(&edited, AUTHOR, DATE, &options)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("ignored endnotes cannot supply reference {added}")
    );
    assert!(doc.revisions().is_empty());
}

#[test]
fn a_moved_endnote_reference_with_endnotes_ignored_is_refused() {
    // Ignored endnotes keep the original's, so the moved reference cannot
    // get its own endnote: the redline would name one endnote from both
    // places.
    let mut doc = doc_with_endnote_texts(&[("Claim one.", Some("Source")), ("Claim two.", None)]);
    let edited = doc_with_endnote_texts(&[("Claim one.", None), ("Claim two.", Some("Source"))]);
    let options = ComparisonOptions {
        ignored_stories: vec![ComparisonStoryKind::Endnote],
        ..Default::default()
    };
    let error = doc
        .compare_with_options(&edited, AUTHOR, DATE, &options)
        .unwrap_err();
    assert!(
        error.to_string().contains("endnote") && error.to_string().contains("moved"),
        "{error}"
    );
    assert!(doc.revisions().is_empty());
}

/// `doc` saved with `edit` applied to the XML of `part`, a package path
/// such as `word/endnotes.xml`, then reopened: markup Word writes that the
/// facade would not.
fn with_part_edited(doc: &mut Document, part: &str, edit: impl FnOnce(&str) -> String) -> Document {
    let bytes = doc.to_bytes().expect("document saves");
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("package opens");
    let name = format!("/{part}");
    let xml = std::str::from_utf8(package.get_part(&name).expect("part exists"))
        .expect("part is UTF-8")
        .to_owned();
    package.set_part(&name, edit(&xml).into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("edited document opens")
}

/// `xml` with `edit` applied to the content of its note `<w:{owner} w:id="{id}">`.
fn with_note_edited(xml: &str, owner: &str, id: i32, edit: impl FnOnce(&str) -> String) -> String {
    let start = xml
        .find(&format!(r#"<w:{owner} w:id="{id}""#))
        .expect("note owner");
    let open = start + xml[start..].find('>').expect("owner start tag") + 1;
    let close = open
        + xml[open..]
            .find(&format!("</w:{owner}>"))
            .expect("owner end tag");
    format!(
        "{}{}{}",
        &xml[..open],
        edit(&xml[open..close]),
        &xml[close..]
    )
}

/// `xml` with every run wrapped in its own tracked insertion, ids from `id`.
fn runs_inserted(xml: &str, id: &mut i32) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(start) = ["<w:r>", "<w:r "]
        .iter()
        .filter_map(|tag| rest.find(tag))
        .min()
    {
        let end = start + rest[start..].find("</w:r>").expect("run end") + "</w:r>".len();
        out.push_str(&rest[..start]);
        out.push_str(&format!(
            r#"<w:ins w:id="{id}" w:author="{AUTHOR}" w:date="{DATE}">{}</w:ins>"#,
            &rest[start..end]
        ));
        *id += 1;
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// `doc` with note `id` of this kind (`footnote` or `endnote`) inserted as
/// Word tracks a note typed with track changes on: its reference run and
/// every run of the note in a tracked insertion, the note's paragraph mark
/// inserted.
fn with_tracked_note_insertion(doc: &mut Document, owner: &str, id: i32) -> Document {
    let mut revision = 900;
    let reference = format!(r#"<w:{owner}Reference w:id="{id}"/></w:r>"#);
    let mut doc = with_part_edited(doc, "word/document.xml", |xml| {
        let end = xml.find(&reference).expect("note reference") + reference.len();
        let start = xml[..end].rfind("<w:r>").expect("reference run");
        format!(
            "{}{}{}",
            &xml[..start],
            runs_inserted(&xml[start..end], &mut revision),
            &xml[end..]
        )
    });
    with_part_edited(&mut doc, &format!("word/{owner}s.xml"), |xml| {
        with_note_edited(xml, owner, id, |content| {
            let content = runs_inserted(content, &mut revision);
            content.replacen(
                "<w:p>",
                &format!(
                    r#"<w:p><w:pPr><w:rPr><w:ins w:id="{revision}" w:author="{AUTHOR}" w:date="{DATE}"/></w:rPr></w:pPr>"#
                ),
                1,
            )
        })
    })
}

#[test]
fn rejecting_a_tracked_endnote_insertion_removes_the_endnote_whole() {
    // Not a comparison: Word tracked the endnote typed at the end of the
    // second claim. Rejecting removes its definition with its reference.
    let mut doc = doc_with_endnote_texts(&[("Claim one.", Some("A")), ("Claim two.", Some("B"))]);
    let (_, id) = endnote_references(&doc)[1];
    let mut doc = with_tracked_note_insertion(&mut doc, "endnote", id);
    let wanted = |doc: &Document| (note_marked_texts(doc), held_note_texts(doc));
    assert_eq!(
        wanted(&accepted(&mut doc)),
        (
            vec!["Claim one.{A}".to_owned(), "Claim two.{B}".to_owned()],
            (vec![], vec!["A".to_owned(), "B".to_owned()])
        )
    );
    assert_eq!(
        wanted(&rejected(&mut doc)),
        (
            vec!["Claim one.{A}".to_owned(), "Claim two.".to_owned()],
            (vec![], vec!["A".to_owned()])
        )
    );
}

#[test]
fn rejecting_a_tracked_footnote_insertion_removes_the_footnote_whole() {
    // The footnote twin of the endnote case above.
    let mut doc = doc_with_notes_in_order(&[("Claim one.", Some("A")), ("Claim two.", Some("B"))]);
    let id = doc.footnotes().last().expect("second footnote").0;
    let mut doc = with_tracked_note_insertion(&mut doc, "footnote", id);
    let wanted = |doc: &Document| (note_marked_texts(doc), held_note_texts(doc));
    assert_eq!(
        wanted(&accepted(&mut doc)),
        (
            vec!["Claim one.[A]".to_owned(), "Claim two.[B]".to_owned()],
            (vec!["A".to_owned(), "B".to_owned()], vec![])
        )
    );
    assert_eq!(
        wanted(&rejected(&mut doc)),
        (
            vec!["Claim one.[A]".to_owned(), "Claim two.".to_owned()],
            (vec!["A".to_owned()], vec![])
        )
    );
}

#[test]
fn a_tracked_change_beside_an_endnote_reference_without_endnotes_resolves() {
    // No endnotes part, so no endnote to settle: the stray reference stays
    // and the insertion beside it resolves as in any paragraph.
    let body = format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Claim </w:t></w:r><w:ins w:id="1" w:author="{AUTHOR}" w:date="{DATE}"><w:r><w:t>added</w:t></w:r></w:ins><w:r><w:endnoteReference w:id="2"/></w:r></w:p>"#
    );
    let mut doc = doc_with_body(&body);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Claim added"]);
    assert_eq!(endnote_references(&copy), [(0, 2)]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), ["Claim "]);
    assert_eq!(endnote_references(&copy), [(0, 2)]);
}

#[test]
fn a_tracked_change_in_a_text_box_of_an_added_note_resolves_with_the_note() {
    // The added note holds a text box, and the text box a tracked insertion
    // of its own, in both the DrawingML and the VML copy. Accepting keeps
    // the note, rejecting removes it whole, and one revision alone is a
    // partial selection of the note, for either kind.
    for owner in ["footnote", "endnote"] {
        let (original, mut edited) = if owner == "footnote" {
            (
                doc_with_notes_in_order(&[("Claim one.", Some("A")), ("Claim two.", None)]),
                doc_with_notes_in_order(&[("Claim one.", Some("A")), ("Claim two.", Some("B"))]),
            )
        } else {
            (
                doc_with_endnote_texts(&[("Claim one.", Some("A")), ("Claim two.", None)]),
                doc_with_endnote_texts(&[("Claim one.", Some("A")), ("Claim two.", Some("B"))]),
            )
        };
        let part = format!("word/{owner}s.xml");
        let inside = "<w:r><w:t>Inside</w:t></w:r>";
        let edited = with_part_edited(&mut edited, &part, |xml| {
            with_note_edited(xml, owner, 3, |content| {
                let text_box = crate::support::text_box_run(7, &format!("<w:p>{inside}</w:p>"));
                content.replacen("</w:p>", &format!("{text_box}</w:p>"), 1)
            })
        });
        let mut doc = redline(original, &edited);
        let mut doc = with_part_edited(&mut doc, &part, |xml| {
            xml.replace(
                inside,
                &format!(
                    r#"<w:ins w:id="901" w:author="{AUTHOR}" w:date="{DATE}">{inside}</w:ins>"#
                ),
            )
        });
        let mark = |text: &str| {
            if owner == "footnote" {
                format!("[{text}]")
            } else {
                format!("{{{text}}}")
            }
        };
        let notes = |texts: &[&str]| {
            let texts = texts.iter().map(|text| (*text).to_owned()).collect();
            if owner == "footnote" {
                (texts, vec![])
            } else {
                (vec![], texts)
            }
        };
        let wanted = |doc: &Document| (note_marked_texts(doc), held_note_texts(doc));
        let note_part = |doc: &Document| doc.part_xml(&part).expect("note part reads");
        let copy = accepted(&mut doc);
        assert_eq!(
            wanted(&copy),
            (
                vec![
                    format!("Claim one.{}", mark("A")),
                    format!("Claim two.{}", mark("B"))
                ],
                notes(&["A", "B"])
            ),
            "{owner}"
        );
        assert!(
            copy.story_revisions().expect("revisions").is_empty(),
            "{owner}"
        );
        let xml = note_part(&copy);
        assert_eq!(xml.matches(">Inside<").count(), 2, "{owner}: {xml}");
        // Footnotes leave the VML copy's insertion in the part, outside
        // the revisions they list; endnotes resolve every copy, as the body.
        if owner == "endnote" {
            assert!(!xml.contains("<w:ins "), "{xml}");
        }
        let copy = rejected(&mut doc);
        assert_eq!(
            wanted(&copy),
            (
                vec![format!("Claim one.{}", mark("A")), "Claim two.".to_owned()],
                notes(&["A"])
            ),
            "{owner}"
        );
        assert!(!note_part(&copy).contains("Inside"), "{owner}");
        let ids = doc
            .story_revisions()
            .expect("revisions")
            .iter()
            .map(|revision| revision.id())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(ids.contains(&901), "{owner}: {ids:?}");
        for id in ids {
            for accept in [true, false] {
                let mut copy = reopen(&mut doc);
                let error = if accept {
                    copy.accept_revision_id(id)
                } else {
                    copy.reject_revision_id(id)
                }
                .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains(&format!("partial {owner} custody selection")),
                    "{owner} {id}: {error}"
                );
            }
        }
    }
}

/// `body` with every revision accepted, and the main part the resolution
/// wrote.
fn accepted_main_part(body: &str) -> String {
    let mut doc = doc_with_body(body);
    doc.accept_all().expect("accept all");
    main_part_xml(&mut doc)
}

const DELETED_MARK: &str = r#"<w:pPr><w:rPr><w:del w:id="1" w:author="Eve" w:date="2026-01-01T00:00:00Z"/></w:rPr></w:pPr>"#;

#[test]
fn an_element_with_its_own_default_namespace_moves_without_an_undeclaration() {
    // The first paragraph declares a default namespace; the run moved into
    // it holds an element that declares its own, so the run needs no
    // `xmlns=""`.
    let xml = accepted_main_part(&format!(
        r#"<w:p xmlns="urn:example:tasks">{DELETED_MARK}<w:r><w:t xml:space="preserve">one </w:t></w:r></w:p><w:p><w:r><w:t>two</w:t><extra xmlns="urn:example:extra"/></w:r></w:p>"#
    ));
    assert!(
        xml.contains(r#"<extra xmlns="urn:example:extra"/>"#),
        "{xml}"
    );
    assert!(!xml.contains(r#"xmlns="""#), "{xml}");
}

#[test]
fn an_element_taking_the_inherited_default_moves_with_an_undeclaration() {
    // The moved element has no prefix and no declaration: it takes no
    // namespace in its own paragraph, and keeps none in the first one.
    let xml = accepted_main_part(&format!(
        r#"<w:p xmlns="urn:example:tasks">{DELETED_MARK}<w:r><w:t xml:space="preserve">one </w:t></w:r></w:p><w:p><w:r><w:t>two</w:t></w:r><extra/></w:p>"#
    ));
    assert!(xml.contains(r#"<extra xmlns=""/>"#), "{xml}");
}

#[test]
fn an_element_taking_its_paragraph_default_keeps_it_when_moved() {
    // The moved element has no prefix and takes its paragraph's default
    // namespace: it keeps that namespace in the first paragraph.
    let xml = accepted_main_part(&format!(
        r#"<w:p>{DELETED_MARK}<w:r><w:t xml:space="preserve">one </w:t></w:r></w:p><w:p xmlns="urn:example:tasks"><w:r><w:t>two</w:t></w:r><extra/></w:p>"#
    ));
    assert!(
        xml.contains(r#"<extra xmlns="urn:example:tasks"/>"#),
        "{xml}"
    );
}

// ---- The redline of two versions of a long document ----
// A tool keeps each version of a thesis, edits the next one, then hands the
// reader a redline: by words, fields and formatting left out, accepted to the
// new version or rejected to the old one, and printed in the colours of
// legal comparison tools.

/// A thesis copy as its producer saved it, and its next version: a section
/// added before the last one, the first paragraph of the discussion moved
/// below the second, the running header retitled and a footnote added. With `rebuild_toc`,
/// both versions carry the contents oxoffice rebuilt, as a tool leaves them
/// after each pass.
fn thesis_versions(path: &str, rebuild_toc: bool) -> (Document, Document) {
    let mut original = Document::from_bytes(&crate::support::workflow_source(path)).unwrap();
    if rebuild_toc {
        original.rebuild_toc().unwrap();
        original = reopen(&mut original);
    }
    let mut edited = reopen(&mut original);
    let body_paragraph = |doc: &Document, text: &str| {
        let paragraphs = doc.story_paragraphs().unwrap();
        let at = paragraphs
            .iter()
            .position(|p| p.kind == StoryKind::Body && p.text == text && p.style.is_some())
            .unwrap_or_else(|| panic!("{path}: a heading reads {text:?}"));
        (paragraphs, at)
    };
    // The discussion's first paragraph goes below its second.
    let (paragraphs, discussion) = body_paragraph(&edited, "Discussion");
    let [first, second] = [&paragraphs[discussion + 1], &paragraphs[discussion + 2]];
    assert!(
        first.style == second.style && !first.text.is_empty() && !second.text.is_empty(),
        "{path}: two body paragraphs follow the discussion heading"
    );
    edited
        .move_content(&first.location, &paragraphs[discussion + 3].location)
        .unwrap();
    // A section heading before the last section, in its heading style.
    let (paragraphs, future) = body_paragraph(&edited, "Future work");
    let style = paragraphs[future].style.clone().unwrap();
    let index = (0..edited.paragraph_count())
        .rfind(|&index| edited.paragraph(index).unwrap().text() == "Future work")
        .and_then(|index| edited.content_index_of_paragraph(index))
        .unwrap_or_else(|| panic!("{path}: the last section heading is a body paragraph"));
    edited.insert_paragraph(index, "Limitations").style(&style);
    let (paragraphs, future) = body_paragraph(&edited, "Future work");
    assert_eq!(paragraphs[future - 1].text, "Limitations", "{path}");
    let retitled = edited.replace_all(&HashMap::from([("MSc thesis", "MSc dissertation")]));
    assert!(retitled > 0, "{path}: the header names the degree");
    // A footnote on the paragraph that opens the last section.
    let note = edited.add_footnote("Agreed with the site managers.");
    let opening = (0..edited.paragraph_count())
        .rfind(|&index| edited.paragraph(index).unwrap().text() == "Future work")
        .unwrap()
        + 1;
    edited
        .paragraph_mut(opening)
        .unwrap()
        .add_footnote_ref(note);
    if rebuild_toc {
        edited.rebuild_toc().unwrap();
    }
    (original, reopen(&mut edited))
}

fn compare_versions(original: &mut Document, edited: &Document) {
    let options = ComparisonOptions {
        granularity: ComparisonGranularity::Word,
        ignore_formatting: true,
        ignore_fields: true,
        ..Default::default()
    };
    let diagnostics = original
        .compare_with_options(edited, AUTHOR, DATE, &options)
        .unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

/// Every paragraph text of every story, as a reader sees it.
fn story_texts(doc: &Document) -> Vec<(StoryKind, String)> {
    doc.story_paragraphs()
        .unwrap()
        .into_iter()
        .map(|paragraph| (paragraph.kind, paragraph.text))
        .collect()
}

/// One render of a document: the PDF, for where text is drawn, and every
/// page image at 72 DPI, from one layout.
struct View {
    pdf: oxoffice_render_test::pdf::Pdf,
    images: Vec<oxoffice_render_test::fingerprint::Pixels>,
}

impl View {
    fn of(doc: &Document, options: RenderOptions) -> Self {
        let pdf = oxoffice_render_test::pdf::Pdf::read(
            &doc.to_pdf_deterministic_with_options(options).unwrap(),
        );
        let images = doc
            .render_all_pages_with_options(72.0, options)
            .unwrap()
            .iter()
            .map(|png| oxoffice_render_test::fingerprint::Pixels::decode(png))
            .collect::<Vec<_>>();
        assert_eq!(images.len(), pdf.page_count());
        Self { pdf, images }
    }

    /// The pages whose drawn text holds `text`.
    fn pages_drawing(&self, text: &str) -> Vec<usize> {
        (0..self.pdf.page_count())
            .filter(|&page| {
                self.pdf
                    .drawn(page)
                    .iter()
                    .any(|piece| piece.text.contains(text))
            })
            .collect()
    }

    /// The page and the first piece drawn holding `text`.
    fn piece(&self, text: &str) -> (usize, oxoffice_render_test::pdf::Drawn) {
        let page = *self
            .pages_drawing(text)
            .first()
            .unwrap_or_else(|| panic!("{text} is drawn"));
        let piece = self
            .pdf
            .drawn(page)
            .into_iter()
            .find(|piece| piece.text.contains(text));
        (page, piece.unwrap())
    }

    /// The first piece drawn on `page` holding `text`.
    fn piece_on(
        &self,
        page: usize,
        text: &str,
    ) -> Option<(usize, oxoffice_render_test::pdf::Drawn)> {
        let piece = self
            .pdf
            .drawn(page)
            .into_iter()
            .find(|piece| piece.text.contains(text));
        piece.map(|piece| (page, piece))
    }

    /// The piece holding `text` drawn on the line of `anchor`, the first
    /// piece drawn holding that.
    fn piece_beside(&self, anchor: &str, text: &str) -> (usize, oxoffice_render_test::pdf::Drawn) {
        let (page, anchor) = self.piece(anchor);
        let piece = self
            .pdf
            .drawn(page)
            .into_iter()
            .find(|piece| piece.text.contains(text) && (piece.y - anchor.y).abs() < 1.0)
            .unwrap_or_else(|| panic!("{text} is drawn beside {}", anchor.text));
        (page, piece)
    }

    /// How many pixels inside the box of `piece` on `page`, from a little
    /// under its baseline to its font size above it, pass `colour`.
    fn pixels_in(
        &self,
        (page, piece): (usize, oxoffice_render_test::pdf::Drawn),
        colour: impl Fn([u8; 3]) -> bool,
    ) -> usize {
        let image = &self.images[page];
        let height = f64::from(image.height);
        let rows = (height - piece.y - piece.size).max(0.0) as u32
            ..((height - piece.y + piece.size * 0.4) as u32).min(image.height);
        let mut count = 0;
        for y in rows {
            for x in piece.x as u32..(piece.end as u32).min(image.width) {
                let at = ((y * image.width + x) * 4) as usize;
                if colour([image.rgba[at], image.rgba[at + 1], image.rgba[at + 2]]) {
                    count += 1;
                }
            }
        }
        count
    }

    /// How many pixels of page `page` pass `colour`.
    fn page_pixels(&self, page: usize, colour: impl Fn([u8; 3]) -> bool) -> usize {
        self.images[page]
            .rgba
            .chunks_exact(4)
            .filter(|pixel| colour([pixel[0], pixel[1], pixel[2]]))
            .count()
    }
}

#[test]
fn a_redline_of_two_thesis_versions_resolves_to_either_and_prints_in_legal_colours() {
    let copies = crate::support::thesis_copies();
    crate::support::run_workflow("redline", &copies, |path| {
        let (mut doc, edited) = thesis_versions(path, false);
        let original = story_texts(&doc);
        compare_versions(&mut doc, &edited);
        let mut doc = reopen(&mut doc);

        assert!(
            doc.revisions()
                .iter()
                .all(|r| r.author() == AUTHOR && r.timestamp() == Some(DATE)),
            "{path}"
        );
        let changes = doc.story_revisions().unwrap();
        let covers = |kind: RevisionKind, story: StoryKind, text: &str| {
            changes.iter().any(|revision| {
                revision.kind() == kind
                    && revision.story().kind() == story
                    && revision.text().contains(text)
            })
        };
        assert!(
            covers(RevisionKind::Insertion, StoryKind::Body, "Limitations"),
            "{path}"
        );
        assert!(
            covers(RevisionKind::Deletion, StoryKind::Header, "thesis"),
            "{path}"
        );
        assert!(
            covers(RevisionKind::Insertion, StoryKind::Header, "dissertation"),
            "{path}"
        );
        assert!(
            covers(
                RevisionKind::Insertion,
                StoryKind::Footnote,
                "site managers"
            ),
            "{path}"
        );
        assert!(
            kinds(&doc).contains(&RevisionKind::MoveFrom)
                && kinds(&doc).contains(&RevisionKind::MoveTo),
            "{path}: the paragraph is recorded as moved, {:?}",
            kinds(&doc)
        );
        assert_eq!(
            story_texts(&accepted(&mut doc)),
            story_texts(&edited),
            "{path}"
        );
        assert_eq!(story_texts(&rejected(&mut doc)), original, "{path}");

        // Printed in the legal style, on the copies the walks render:
        // insertions blue, deletions red, the moved paragraph green, in the
        // body and in the header alike, and the text around in its own colour.
        if !crate::support::walk_renders(path) {
            return;
        }
        let legal = View::of(
            &doc,
            RenderOptions::default()
                .with_revision_view(RevisionView::Tracked)
                .with_revision_style(RevisionStyle::Legal),
        );
        let blue = |[r, g, b]: [u8; 3]| b > 180 && r < 90 && g < 90;
        let red = |[r, g, b]: [u8; 3]| r > 180 && g < 90 && b < 90;
        let green = |[r, g, b]: [u8; 3]| g > 120 && r < 60 && b < 60;
        let (page, inserted) = legal.piece("Limitations");
        assert!(legal.pixels_in((page, inserted), blue) > 10, "{path}");
        let next = legal
            .piece_on(page, "Future")
            .expect("the last section heading follows the new one");
        assert_eq!(
            legal.pixels_in(next.clone(), blue) + legal.pixels_in(next, red),
            0,
            "{path}: the heading after the insertion keeps its colour"
        );
        assert!(
            legal.pixels_in(legal.piece("dissertation"), blue) > 10,
            "{path}"
        );
        let deleted = legal.piece_beside("dissertation", "thesis");
        assert!(legal.pixels_in(deleted, red) > 10, "{path}");
        let kept = legal.piece_beside("dissertation", "Laurent");
        assert_eq!(
            legal.pixels_in(kept.clone(), blue) + legal.pixels_in(kept, red),
            0,
            "{path}"
        );
        // The moved paragraph sits below the discussion heading (drawn last,
        // after its TOC entries): its text is green where it left and where
        // it went, and green shows nowhere else, change bars included.
        let heading = *legal.pages_drawing("Discussion").last().unwrap();
        let paragraphs = edited.story_paragraphs().unwrap();
        let discussion = paragraphs
            .iter()
            .rposition(|p| p.text == "Discussion")
            .unwrap();
        let moved = &paragraphs[discussion + 2].text;
        let word = moved
            .split(|c: char| !c.is_alphanumeric())
            .max_by_key(|word| word.len())
            .unwrap();
        // Where the moved word is drawn on those pages, in reading order
        // (page, then top to bottom), and whether it is green: green above
        // the kept paragraph, where the text left, and below it, where it went.
        let on_pages = |text: &str| {
            [heading, heading + 1]
                .into_iter()
                .filter(|&page| page < legal.images.len())
                .flat_map(|page| {
                    legal
                        .pdf
                        .drawn(page)
                        .into_iter()
                        .filter(|piece| piece.text.contains(text))
                        .map(move |piece| (page, piece))
                })
                .collect::<Vec<_>>()
        };
        let drawn = on_pages(word)
            .into_iter()
            .map(|(page, piece)| {
                let is_green = legal.pixels_in((page, piece.clone()), green) > 5;
                (page, -piece.y, is_green)
            })
            .collect::<Vec<_>>();
        // The kept paragraph's own longest word, drawn in the text colour.
        let kept_word = paragraphs[discussion + 1]
            .text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|kept| !moved.contains(kept))
            .max_by_key(|kept| kept.len())
            .unwrap_or_else(|| panic!("{path}: the kept paragraph has a word of its own"));
        let (page, piece) = on_pages(kept_word)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{path}: {kept_word} is drawn"));
        let kept_at = (page, -piece.y);
        let green_before = drawn
            .iter()
            .any(|&(page, y, is_green)| is_green && (page, y) < kept_at);
        let green_after = drawn
            .iter()
            .any(|&(page, y, is_green)| is_green && (page, y) > kept_at);
        assert!(
            green_before && green_after,
            "{path}: {word} is green where it left and where it went, {drawn:?} around {kept_at:?}"
        );
        let pages = (0..legal.images.len())
            .filter(|&page| legal.page_pixels(page, green) > 0)
            .collect::<Vec<_>>();
        assert!(
            pages
                .iter()
                .all(|&page| page == heading || page == heading + 1),
            "{path}: green on {pages:?}, the discussion starts on {heading}"
        );
    });
}

// ---- A whole paragraph's simple field ----
//
// Word never writes `w:fldSimple`, and a `w:fldSimple` cannot carry a
// revision mark of its own: a paragraph inserted or deleted whole writes its
// simple field as a complex field, all of it in one `w:ins` or `w:del`, as
// Word's own deletion does. Resolving the paragraph then leaves no field.

/// A paragraph that shows `Page 1` through a simple `PAGE` field.
const PAGE_LINE: &str = r#"<w:p><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>"#;

fn plain(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

/// The name, without its leading slash, of the saved part ending with `suffix`.
fn part_name(package: &oxoffice_opc::OpcPackage, suffix: &str) -> String {
    package
        .zip_entries()
        .unwrap()
        .into_iter()
        .map(|(name, _)| name)
        .find(|name| name.ends_with(suffix))
        .unwrap_or_else(|| panic!("no part ends with {suffix}"))
}

/// The XML of the saved part ending with `suffix`.
fn part_xml(doc: &mut Document, suffix: &str) -> String {
    let package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    let name = part_name(&package, suffix);
    String::from_utf8(package.get_part(&format!("/{name}")).unwrap().to_vec()).unwrap()
}

/// `doc` saved with the part ending with `suffix` rewritten by `edit`, as
/// another producer would write it.
fn with_part(doc: &mut Document, suffix: &str, edit: impl FnOnce(&str) -> String) -> Document {
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    let name = format!("/{}", part_name(&package, suffix));
    let xml = String::from_utf8(package.get_part(&name).unwrap().to_vec()).unwrap();
    let edited = edit(&xml);
    assert_ne!(edited, xml, "the edit changes {name}");
    package.set_part(&name, edited.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

/// Whether this part XML holds a field of either form.
fn holds_field(xml: &str) -> bool {
    xml.contains("fldSimple") || xml.contains("fldChar")
}

#[test]
fn a_paragraph_with_a_simple_field_added_to_a_word_2019_file_is_compared() {
    // Word 2019 declares a default namespace on the document root that no
    // name uses, and the edited copy, from another producer, does not: the
    // inserted field's facts are read under the bindings they use only.
    let without = format!("{}{}", plain("Before."), plain("After."));
    let with = format!("{}{PAGE_LINE}{}", plain("Before."), plain("After."));
    let mut original = doc_with_body(&without);
    let original = with_part(&mut original, "document.xml", |xml| {
        xml.replacen(
            "<w:document ",
            r#"<w:document xmlns="http://schemas.microsoft.com/office/tasks/2019/documenttasks" "#,
            1,
        )
    });
    let mut doc = redline(original, &doc_with_body(&with));
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Before.", "Page 1", "After."]);
    assert_eq!(field_instructions(&copy), ["PAGE"]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), ["Before.", "After."]);
}

/// Compare `original` with `edited` both ways, and check that each redline
/// resolves to the side it should, with a field in the part ending with
/// `suffix` only on the side `field` says.
fn check_both_ways(
    original: impl Fn() -> Document,
    edited: impl Fn() -> Document,
    suffix: &str,
    field: [bool; 2],
) {
    for (from, to, held) in [
        (
            &original as &dyn Fn() -> Document,
            &edited as &dyn Fn() -> Document,
            field,
        ),
        (&edited, &original, [field[1], field[0]]),
    ] {
        let mut doc = redline(from(), &to());
        let mut copy = accepted(&mut doc);
        assert_eq!(part_texts(&mut copy, suffix), part_texts(&mut to(), suffix));
        assert_eq!(holds_field(&part_xml(&mut copy, suffix)), held[1]);
        let mut copy = rejected(&mut doc);
        assert_eq!(
            part_texts(&mut copy, suffix),
            part_texts(&mut from(), suffix)
        );
        assert_eq!(holds_field(&part_xml(&mut copy, suffix)), held[0]);
    }
}

/// The text of each `w:p` of the saved part ending with `suffix`, read from
/// its `w:t` elements; the part holds no pending deletion.
fn part_texts(doc: &mut Document, suffix: &str) -> Vec<String> {
    let xml = part_xml(doc, suffix);
    assert!(!xml.contains("<w:del "), "resolved: {xml}");
    let starts = xml
        .match_indices("<w:p")
        .map(|(at, _)| at)
        .filter(|at| matches!(xml.as_bytes().get(at + 4), Some(b'>' | b' ')));
    starts
        .map(|at| {
            let paragraph = &xml[at..];
            let paragraph = &paragraph[..paragraph.find("</w:p>").unwrap_or(paragraph.len())];
            paragraph
                .match_indices("<w:t")
                .filter_map(|(at, _)| {
                    let text = &paragraph[at + 4..];
                    let text = match text.as_bytes().first() {
                        Some(b'>') => &text[1..],
                        Some(b' ') => &text[text.find('>')? + 1..],
                        _ => return None,
                    };
                    Some(&text[..text.find("</w:t>")?])
                })
                .collect::<String>()
        })
        .collect()
}

#[test]
fn a_simple_field_paragraph_in_a_footnote_a_comment_or_a_cell_is_tracked_whole() {
    let note = |page: bool| {
        let mut doc = doc_with_notes(&[("Claim.", Some("Source.")), ("After.", None)]);
        if page {
            with_part(&mut doc, "footnotes.xml", |xml| {
                let at = xml.find("Source.").unwrap();
                let end = at + xml[at..].find("</w:footnote>").unwrap();
                format!("{}{PAGE_LINE}{}", &xml[..end], &xml[end..])
            })
        } else {
            reopen(&mut doc)
        }
    };
    check_both_ways(
        || note(false),
        || note(true),
        "footnotes.xml",
        [false, true],
    );

    let comment = |page: bool| {
        let mut doc = doc_with(&["Claim.", "After."]);
        let range = oxoffice_docx::RunRange {
            start: oxoffice_docx::RunPosition {
                body_index: 0,
                run_index: 0,
            },
            end: oxoffice_docx::RunPosition {
                body_index: 0,
                run_index: 1,
            },
        };
        doc.add_comment_with_date(
            range,
            "Ada",
            None,
            "Note.",
            oxoffice_docx::CommentDate::Undated,
        )
        .unwrap();
        if page {
            with_part(&mut doc, "comments.xml", |xml| {
                xml.replacen("</w:comment>", &format!("{PAGE_LINE}</w:comment>"), 1)
            })
        } else {
            reopen(&mut doc)
        }
    };
    check_both_ways(
        || comment(false),
        || comment(true),
        "comments.xml",
        [false, true],
    );

    // First and last paragraph of a kept cell.
    let cell = |content: &str| {
        doc_with_body(&format!(
            r#"<w:tbl><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc>{content}</w:tc></w:tr></w:tbl>{}"#,
            plain("After.")
        ))
    };
    check_both_ways(
        || cell(&plain("Cell.")),
        || cell(&format!("{PAGE_LINE}{}", plain("Cell."))),
        "document.xml",
        [false, true],
    );
    check_both_ways(
        || cell(&plain("Cell.")),
        || cell(&format!("{}{PAGE_LINE}", plain("Cell."))),
        "document.xml",
        [false, true],
    );
}

#[test]
fn a_simple_field_paragraph_in_a_content_control_is_tracked_whole() {
    // First and last paragraph of a block content control.
    let control = |content: &str| {
        doc_with_body(&format!(
            r#"<w:sdt><w:sdtPr><w:alias w:val="Box"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>{}"#,
            plain("After.")
        ))
    };
    check_both_ways(
        || control(&plain("Inside.")),
        || control(&format!("{PAGE_LINE}{}", plain("Inside."))),
        "document.xml",
        [false, true],
    );
    check_both_ways(
        || control(&plain("Inside.")),
        || control(&format!("{}{PAGE_LINE}", plain("Inside."))),
        "document.xml",
        [false, true],
    );
}

#[test]
fn a_simple_field_the_reader_keeps_raw_keeps_a_refusal_when_its_paragraph_goes_whole() {
    // A simple field with an empty instruction is kept as raw XML, with no
    // complex form to take: the comparison refuses its paragraph and writes
    // nothing, rather than leave its shell.
    let raw = r#"<w:p><w:r><w:t>x</w:t></w:r><w:fldSimple w:instr=" "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>"#;
    let after = plain("After.");
    for (original, edited) in [
        (after.clone(), format!("{raw}{after}")),
        (format!("{raw}{after}"), after.clone()),
    ] {
        let mut doc = doc_with_body(&original);
        doc.compare(&doc_with_body(&edited), AUTHOR, DATE)
            .expect_err("refused");
        assert!(doc.revisions().is_empty());
        assert_eq!(texts(&reopen(&mut doc)), texts(&doc_with_body(&original)));
    }
}

#[test]
fn a_simple_field_inside_a_hyperlink_is_tracked_whole_with_its_paragraph() {
    let linked = r#"<w:p><w:hyperlink w:anchor="_Top"><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:hyperlink></w:p>"#;
    let after = plain("After.");
    // Inserted and deleted whole, before a paragraph and as the last one.
    check_both_ways(
        || doc_with_body(&after),
        || doc_with_body(&format!("{linked}{after}")),
        "document.xml",
        [false, true],
    );
    check_both_ways(
        || doc_with_body(&plain("Before.")),
        || doc_with_body(&format!("{}{linked}", plain("Before."))),
        "document.xml",
        [false, true],
    );
    // Paired, its hyperlink pointing elsewhere: written as a deleted and an
    // inserted paragraph.
    let relinked = linked.replace("_Top", "_Bottom");
    check_both_ways(
        || doc_with_body(&format!("{linked}{after}")),
        || doc_with_body(&format!("{relinked}{after}")),
        "document.xml",
        [true, true],
    );
    // The field, from begin to end, in one wrapper inside the hyperlink.
    let mut doc = redline(
        doc_with_body(&after),
        &doc_with_body(&format!("{linked}{after}")),
    );
    let xml = main_part_xml(&mut doc);
    assert!(!xml.contains("fldSimple"), "{xml}");
    let link = &xml[xml.find("<w:hyperlink").unwrap()..xml.find("</w:hyperlink>").unwrap()];
    let field = &link
        [link.find(r#"fldCharType="begin""#).unwrap()..link.rfind(r#"fldCharType="end""#).unwrap()];
    assert!(
        !field.contains("<w:ins ") && !field.contains("</w:ins>"),
        "{xml}"
    );
}

#[test]
fn a_simple_field_nested_in_another_is_tracked_whole_with_its_paragraph() {
    // The nested field is written as a complex field too, in the result of
    // the outer one: Word nests complex fields only.
    let nested = r#"<w:p><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:fldSimple w:instr=" IF 1 = 1 "><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:fldSimple></w:p>"#;
    let after = plain("After.");
    // Inserted before a paragraph, and as the last paragraph of the body.
    check_both_ways(
        || doc_with_body(&after),
        || doc_with_body(&format!("{nested}{after}")),
        "document.xml",
        [false, true],
    );
    check_both_ways(
        || doc_with_body(&plain("Before.")),
        || doc_with_body(&format!("{}{nested}", plain("Before."))),
        "document.xml",
        [false, true],
    );
    // Paired, its text now linked: written as a deleted and an inserted
    // paragraph.
    let linked = nested.replace(
        r#"<w:r><w:t xml:space="preserve">Page </w:t></w:r>"#,
        r#"<w:hyperlink w:anchor="_Top"><w:r><w:t xml:space="preserve">Page </w:t></w:r></w:hyperlink>"#,
    );
    check_both_ways(
        || doc_with_body(&format!("{nested}{after}")),
        || doc_with_body(&format!("{linked}{after}")),
        "document.xml",
        [true, true],
    );
    // Both fields from the first begin to the last end in one wrapper.
    let mut doc = redline(
        doc_with_body(&after),
        &doc_with_body(&format!("{nested}{after}")),
    );
    let xml = main_part_xml(&mut doc);
    assert!(!xml.contains("fldSimple"), "{xml}");
    let field = &xml
        [xml.find(r#"fldCharType="begin""#).unwrap()..xml.rfind(r#"fldCharType="end""#).unwrap()];
    assert!(
        !field.contains("<w:ins ") && !field.contains("</w:ins>"),
        "{xml}"
    );
    assert_eq!(field.matches(r#"fldCharType="begin""#).count(), 2, "{xml}");
    assert!(
        field.find(" IF 1 = 1 ").unwrap() < field.find(" PAGE ").unwrap(),
        "{xml}"
    );
    // Deleted, every text of both fields is deleted text, which Word
    // requires.
    let mut doc = redline(
        doc_with_body(&format!("{nested}{after}")),
        &doc_with_body(&after),
    );
    let xml = main_part_xml(&mut doc);
    let field = &xml
        [xml.find(r#"fldCharType="begin""#).unwrap()..xml.rfind(r#"fldCharType="end""#).unwrap()];
    assert_eq!(field.matches("<w:delInstrText").count(), 2, "{xml}");
    assert!(
        !field.contains("<w:instrText") && !field.contains("<w:t>") && !field.contains("<w:t "),
        "{xml}"
    );
}

/// A document of these Heading 1 paragraphs under a table of contents that
/// `rebuild_toc` filled, so that its page numbers are simple `PAGEREF` fields.
fn rebuilt_toc(headings: &[&str]) -> Document {
    let mut doc = Document::new();
    for heading in headings {
        doc.add_paragraph(heading).style("Heading1");
    }
    doc.insert_toc(0, 1).unwrap();
    doc.rebuild_toc().unwrap();
    reopen(&mut doc)
}

#[test]
fn a_rebuilt_table_of_contents_removed_whole_keeps_its_simple_page_fields() {
    // The table of contents is carried: replaced whole inside its own field,
    // its simple fields are written as before.
    let with_toc = || rebuilt_toc(&["Scope", "Method"]);
    let without = || {
        let mut doc = Document::new();
        for heading in ["Scope", "Method"] {
            doc.add_paragraph(heading).style("Heading1");
        }
        reopen(&mut doc)
    };
    let mut doc = redline(with_toc(), &without());
    assert!(part_xml(&mut doc, "document.xml").contains("PAGEREF"));
    assert!(part_xml(&mut doc, "document.xml").contains("<w:fldSimple"));
    assert_eq!(texts(&accepted(&mut doc)), texts(&without()));
    assert_eq!(texts(&rejected(&mut doc)), texts(&with_toc()));
}

#[test]
fn a_comparison_tracks_a_break_that_only_changes_where_its_text_restarts() {
    // The clear is part of the break: a break that starts clearing the
    // floats is a change, as a page break replacing a line break is.
    let body = |br: &str| {
        doc_with_body(&format!(
            "<w:p><w:r><w:t>x</w:t>{br}<w:t>y</w:t></w:r></w:p>"
        ))
    };
    let cleared = r#"<w:br w:type="textWrapping" w:clear="all"/>"#;
    let doc = redline(body("<w:br/>"), &body(cleared));
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(xml.contains("<w:delText>x</w:delText><w:br/>"), "{xml}");
    assert!(xml.contains(&format!("<w:t>x</w:t>{cleared}")), "{xml}");
    let doc = redline(body(cleared), &body(cleared));
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(!xml.contains("<w:ins") && !xml.contains("<w:del"), "{xml}");
}

/// The same redline once both versions carry the contents oxoffice rebuilt,
/// as a tool that rebuilds them on each pass compares its versions. The
/// rebuild writes page references as simple fields, which the new heading's
/// entry carries as one complex field in its tracked paragraph. In Google's
/// copy the heading after the new one gains a contents bookmark beside its
/// own, which a bookmark both versions hold lets the comparison replace.
#[test]
fn a_redline_of_two_thesis_versions_with_rebuilt_contents_resolves_to_either() {
    let copies = crate::support::thesis_copies();
    crate::support::run_workflow("redline-rebuilt-toc", &copies, |path| {
        let (mut doc, edited) = thesis_versions(path, true);
        let original = story_texts(&doc);
        compare_versions(&mut doc, &edited);
        let mut doc = reopen(&mut doc);
        assert_eq!(
            story_texts(&accepted(&mut doc)),
            story_texts(&edited),
            "{path}"
        );
        assert_eq!(story_texts(&rejected(&mut doc)), original, "{path}");
    });
}

/// A heading holding these bookmarks, as (id, name).
fn heading_with(text: &str, marks: &[(u32, &str)]) -> String {
    let starts = marks
        .iter()
        .map(|(id, name)| format!(r#"<w:bookmarkStart w:id="{id}" w:name="{name}"/>"#))
        .collect::<String>();
    let ends = marks
        .iter()
        .map(|(id, _)| format!(r#"<w:bookmarkEnd w:id="{id}"/>"#))
        .collect::<String>();
    format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{starts}<w:r><w:t>{text}</w:t></w:r>{ends}</w:p>"#
    )
}

// ---- A bookmark both versions hold ----
//
// No tracked change records a bookmark. A paragraph whose bookmarks differ is
// replaced whole, the insertion first, and a bookmark both versions hold is
// written once, outside the revision marks: its start in the insertion, its
// end in the deletion, so that Word's Reject All keeps it. Both resolutions
// keep it over the same text.

fn plain_paragraph(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

/// Check that a redline's part XML writes the bookmark `name` both versions
/// hold as Word's Reject All keeps it: its start in an inserted paragraph,
/// its end in a deleted one after it, or, for a point, both in the inserted
/// paragraph.
fn assert_shared_bookmark_layout(xml: &str, name: &str, point: bool) {
    let paragraphs = xml.split("</w:p>").collect::<Vec<_>>();
    // The revision marks on a paragraph's mark.
    let marked = |paragraph: &str, kind: &str| {
        paragraph.rfind("<w:pPr").is_some_and(|at| {
            paragraph[at..at + paragraph[at..].find("</w:pPr>").unwrap()]
                .contains(&format!("<w:{kind} "))
        })
    };
    let named = format!(r#"w:name="{name}""#);
    let start = paragraphs
        .iter()
        .position(|paragraph| paragraph.contains(&named))
        .unwrap_or_else(|| panic!("no start of {name}: {xml}"));
    let tag = &paragraphs[start][..paragraphs[start].find(&named).unwrap()];
    let tag = &tag[tag.rfind("<w:bookmarkStart").unwrap()..];
    let id = tag[tag.find(r#"w:id=""#).unwrap() + 6..]
        .split('"')
        .next()
        .unwrap();
    let end = paragraphs
        .iter()
        .position(|paragraph| paragraph.contains(&format!(r#"<w:bookmarkEnd w:id="{id}"/>"#)))
        .unwrap_or_else(|| panic!("no end of {name}: {xml}"));
    assert!(marked(paragraphs[start], "ins"), "{name} starts in: {xml}");
    if point {
        assert_eq!(end, start, "{name} is a point: {xml}");
    } else {
        // A story's last paragraph keeps its mark, unmarked: its deleted
        // content holds the end then.
        let ending = paragraphs[end];
        assert!(end > start, "{name} ends after the insertion: {xml}");
        assert!(
            !marked(ending, "ins") && (marked(ending, "del") || ending.contains("<w:del ")),
            "{name} ends in: {xml}"
        );
    }
}

/// The body index where each bookmark starts, by name.
fn bookmark_starts(doc: &Document) -> Vec<(String, usize)> {
    doc.bookmarks()
        .iter()
        .map(|bookmark| {
            (
                bookmark.name().unwrap_or_default().to_owned(),
                bookmark
                    .range()
                    .expect("a bookmark in the body")
                    .start
                    .body_index,
            )
        })
        .collect()
}

#[test]
fn a_heading_that_keeps_its_bookmark_and_gains_another_is_replaced_whole() {
    let original = || {
        doc_with_body(&format!(
            "{}{}{}",
            plain_paragraph("One"),
            heading_with("Heading", &[(1, "Kept")]),
            plain_paragraph("Two")
        ))
    };
    let edited = doc_with_body(&format!(
        "{}{}{}",
        plain_paragraph("One"),
        heading_with("Heading", &[(2, "Added"), (1, "Kept")]),
        plain_paragraph("Two")
    ));
    let mut doc = redline(original(), &edited);
    assert_eq!(revision_texts(&doc, RevisionKind::Deletion), ["Heading"]);
    assert_eq!(revision_texts(&doc, RevisionKind::Insertion), ["Heading"]);
    let names = |doc: &Document| {
        let mut names = bookmark_names(doc);
        names.sort();
        names
    };
    assert_eq!(
        names(&doc),
        ["Added", "Kept"],
        "each bookmark is written once"
    );
    assert_shared_bookmark_layout(&main_part_xml(&mut doc), "Kept", false);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(bookmark_starts(&copy), bookmark_starts(&edited));
    // As in Word, the end the deletion held lands just after the inserted
    // paragraph's mark.
    assert!(
        copy.bookmarks()
            .iter()
            .all(|bookmark| bookmark.text().trim_end_matches('\n') == "Heading")
    );
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&original()));
    assert_eq!(bookmark_starts(&copy), [("Kept".to_owned(), 1)]);
}

#[test]
fn a_paragraph_inserted_before_a_heading_that_gains_a_bookmark_is_compared() {
    // As a contents rebuilt after the insertion gives the heading a new
    // bookmark beside the one the producer wrote (Google's copy of #448).
    let original = || {
        doc_with_body(&format!(
            "{}{}{}",
            plain_paragraph("One"),
            heading_with("Future work", &[(1, "_hjerlw5ntnls")]),
            plain_paragraph("Two")
        ))
    };
    let edited = doc_with_body(&format!(
        "{}{}{}{}",
        plain_paragraph("One"),
        heading_with("Limitations", &[(2, "_Toc7")]),
        heading_with("Future work", &[(3, "_Toc8"), (1, "_hjerlw5ntnls")]),
        plain_paragraph("Two")
    ));
    let mut doc = redline(original(), &edited);
    let inserted = revision_texts(&doc, RevisionKind::Insertion);
    assert!(
        inserted.iter().any(|text| text == "Limitations"),
        "{inserted:?}"
    );
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(bookmark_starts(&copy), bookmark_starts(&edited));
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&original()));
    assert_eq!(bookmark_starts(&copy), [("_hjerlw5ntnls".to_owned(), 1)]);
}

#[test]
fn a_cell_paragraph_that_keeps_its_bookmark_and_gains_another_is_compared() {
    let table = |marks: &[(u32, &str)]| {
        doc_with_body(&format!(
            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>{}{}</w:tc></w:tr></w:tbl>{}"#,
            heading_with("Cell", marks),
            plain_paragraph("Below"),
            plain_paragraph("After")
        ))
    };
    let edited = table(&[(2, "Added"), (1, "Kept")]);
    let mut doc = redline(table(&[(1, "Kept")]), &edited);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    let mut names = bookmark_names(&copy);
    names.sort();
    assert_eq!(names, ["Added", "Kept"]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&table(&[(1, "Kept")])));
    assert_eq!(bookmark_names(&copy), ["Kept"]);
}

#[test]
fn a_last_paragraph_that_keeps_its_bookmark_and_gains_another_is_compared() {
    let document = |marks: &[(u32, &str)]| {
        doc_with_body(&format!(
            "{}{}",
            plain_paragraph("One"),
            heading_with("Last", marks)
        ))
    };
    let edited = document(&[(2, "Added"), (1, "Kept")]);
    let mut doc = redline(document(&[(1, "Kept")]), &edited);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(bookmark_starts(&copy), bookmark_starts(&edited));
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), ["One", "Last"]);
    assert_eq!(bookmark_names(&copy), ["Kept"]);
}

/// `xml` with these bookmarks, as (id, name), around the run that shows
/// `text`.
fn bookmarked_run(xml: &str, text: &str, marks: &[(u32, &str)]) -> String {
    let at = xml.find(&format!(">{text}</w:t>")).expect("the run's text");
    let start = xml[..at].rfind("<w:r>").expect("the run");
    let end = at + xml[at..].find("</w:r>").expect("the run's end") + "</w:r>".len();
    let starts = marks
        .iter()
        .map(|(id, name)| format!(r#"<w:bookmarkStart w:id="{id}" w:name="{name}"/>"#))
        .collect::<String>();
    let ends = marks
        .iter()
        .map(|(id, _)| format!(r#"<w:bookmarkEnd w:id="{id}"/>"#))
        .collect::<String>();
    format!(
        "{}{starts}{}{ends}{}",
        &xml[..start],
        &xml[start..end],
        &xml[end..]
    )
}

/// `xml` with these bookmarks, as (id, name), around the whole content of
/// the paragraph that shows `text`.
fn bookmarked_paragraph(xml: &str, text: &str, marks: &[(u32, &str)]) -> String {
    let at = xml
        .find(&format!(">{text}</w:t>"))
        .expect("the paragraph's text");
    let paragraph = xml[..at]
        .rfind("<w:p>")
        .into_iter()
        .chain(xml[..at].rfind("<w:p "))
        .max()
        .expect("the paragraph");
    let start = match xml[paragraph..at].find("</w:pPr>") {
        Some(properties) => paragraph + properties + "</w:pPr>".len(),
        None => paragraph + xml[paragraph..].find('>').expect("the paragraph's tag") + 1,
    };
    let end = at + xml[at..].find("</w:p>").expect("the paragraph's end");
    let starts = marks
        .iter()
        .map(|(id, name)| format!(r#"<w:bookmarkStart w:id="{id}" w:name="{name}"/>"#))
        .collect::<String>();
    let ends = marks
        .iter()
        .map(|(id, _)| format!(r#"<w:bookmarkEnd w:id="{id}"/>"#))
        .collect::<String>();
    format!(
        "{}{starts}{}{ends}{}",
        &xml[..start],
        &xml[start..end],
        &xml[end..]
    )
}

/// How many bookmark starts of the part ending with `suffix` take each name.
fn part_bookmark_counts(doc: &mut Document, suffix: &str, names: &[&str]) -> Vec<usize> {
    let xml = part_xml(doc, suffix);
    names
        .iter()
        .map(|name| xml.matches(&format!(r#"w:name="{name}""#)).count())
        .collect()
}

#[test]
fn a_header_paragraph_that_keeps_its_bookmark_and_gains_another_is_compared() {
    let document = |marks: &[(u32, &str)]| {
        let mut doc = doc_with(&["body"]);
        doc.set_header("Running title");
        with_part(&mut doc, "header1.xml", |xml| {
            bookmarked_run(xml, "Running title", marks)
        })
    };
    let mut edited = document(&[(2, "Added"), (1, "Kept")]);
    let mut doc = redline(document(&[(1, "Kept")]), &edited);
    let names = ["Kept", "Added"];
    assert_eq!(
        part_bookmark_counts(&mut doc, "header1.xml", &names),
        [1, 1]
    );
    assert_shared_bookmark_layout(&part_xml(&mut doc, "header1.xml"), "Kept", false);
    let mut copy = accepted(&mut doc);
    assert_eq!(copy.header_text(), edited.header_text());
    assert_eq!(
        part_bookmark_counts(&mut copy, "header1.xml", &names),
        part_bookmark_counts(&mut edited, "header1.xml", &names)
    );
    let mut copy = rejected(&mut doc);
    assert_eq!(copy.header_text().as_deref(), Some("Running title"));
    assert_eq!(
        part_bookmark_counts(&mut copy, "header1.xml", &names),
        [1, 0]
    );
}

#[test]
fn a_footnote_paragraph_that_keeps_its_bookmark_and_gains_another_is_compared() {
    let document = |marks: &[(u32, &str)]| {
        let mut doc = doc_with_notes(&[("Text", Some("A note.")), ("After", None)]);
        with_part(&mut doc, "footnotes.xml", |xml| {
            bookmarked_paragraph(xml, "A note.", marks)
        })
    };
    let mut edited = document(&[(42, "Added"), (41, "Kept")]);
    let mut doc = redline(document(&[(41, "Kept")]), &edited);
    let names = ["Kept", "Added"];
    assert_eq!(
        part_bookmark_counts(&mut doc, "footnotes.xml", &names),
        [1, 1]
    );
    assert_shared_bookmark_layout(&part_xml(&mut doc, "footnotes.xml"), "Kept", false);
    let mut copy = accepted(&mut doc);
    assert_eq!(story_texts(&copy), story_texts(&edited));
    assert_eq!(
        part_bookmark_counts(&mut copy, "footnotes.xml", &names),
        part_bookmark_counts(&mut edited, "footnotes.xml", &names)
    );
    let mut copy = rejected(&mut doc);
    assert_eq!(story_texts(&copy), story_texts(&document(&[(41, "Kept")])));
    assert_eq!(
        part_bookmark_counts(&mut copy, "footnotes.xml", &names),
        [1, 0]
    );
}

#[test]
fn a_heading_whose_link_changed_keeps_its_bookmark_on_rejection() {
    let document = |anchor: &str| {
        doc_with_body(&format!(
            r#"{}<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="1" w:name="Kept"/><w:hyperlink w:anchor="{anchor}"><w:r><w:t>Heading</w:t></w:r></w:hyperlink><w:bookmarkEnd w:id="1"/></w:p>{}"#,
            plain_paragraph("One"),
            plain_paragraph("Two")
        ))
    };
    let edited = document("Second");
    let mut doc = redline(document("First"), &edited);
    assert_eq!(bookmark_names(&doc), ["Kept"]);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(bookmark_starts(&copy), bookmark_starts(&edited));
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), ["One", "Heading", "Two"]);
    assert_eq!(bookmark_starts(&copy), [("Kept".to_owned(), 1)]);
}

#[test]
fn a_bookmark_held_under_another_id_is_refused_rather_than_written_twice() {
    // A header keeps the ids its producer wrote, which Word renumbers on a
    // save: the bookmark could not be written once.
    let document = |marks: &[(u32, &str)]| {
        let mut doc = doc_with(&["body"]);
        doc.set_header("Running title");
        with_part(&mut doc, "header1.xml", |xml| {
            bookmarked_run(xml, "Running title", marks)
        })
    };
    let mut doc = document(&[(1, "Kept")]);
    let error = doc
        .compare(&document(&[(2, "Added"), (7, "Kept")]), AUTHOR, DATE)
        .unwrap_err();
    assert!(error.to_string().contains("boundary structures"), "{error}");
}

#[test]
fn a_bookmark_inside_a_hyperlink_is_refused_rather_than_written_twice() {
    let document = |added: bool| {
        let (start, end) = if added {
            (
                r#"<w:bookmarkStart w:id="2" w:name="Added"/>"#,
                r#"<w:bookmarkEnd w:id="2"/>"#,
            )
        } else {
            ("", "")
        };
        doc_with_body(&format!(
            r#"{}<w:p>{start}<w:hyperlink w:anchor="Target"><w:bookmarkStart w:id="1" w:name="Kept"/><w:r><w:t>Linked</w:t></w:r><w:bookmarkEnd w:id="1"/></w:hyperlink>{end}</w:p>{}"#,
            plain_paragraph("One"),
            plain_paragraph("Two")
        ))
    };
    let mut doc = document(false);
    let error = doc.compare(&document(true), AUTHOR, DATE).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("boundary structures at body/paragraph[1]"),
        "{error}"
    );
}

/// A heading holding `Kept` around a link to `anchor`.
fn linked_heading(anchor: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="1" w:name="Kept"/><w:hyperlink w:anchor="{anchor}"><w:r><w:t>Heading</w:t></w:r></w:hyperlink><w:bookmarkEnd w:id="1"/></w:p>"#
    )
}

#[test]
fn a_heading_whose_link_changed_before_a_new_or_removed_last_paragraph_is_compared() {
    // The paragraph marks of a trailing change close on the heading's own.
    for (original, edited) in [
        (
            format!("{}{}", plain_paragraph("A"), linked_heading("First")),
            format!(
                "{}{}{}",
                plain_paragraph("A"),
                linked_heading("Second"),
                plain_paragraph("Tail")
            ),
        ),
        (
            format!(
                "{}{}{}",
                plain_paragraph("A"),
                linked_heading("First"),
                plain_paragraph("Gone")
            ),
            format!("{}{}", plain_paragraph("A"), linked_heading("Second")),
        ),
    ] {
        let edited = doc_with_body(&edited);
        let mut doc = redline(doc_with_body(&original), &edited);
        let copy = accepted(&mut doc);
        assert_eq!(texts(&copy), texts(&edited));
        assert_eq!(bookmark_starts(&copy), bookmark_starts(&edited));
        let copy = rejected(&mut doc);
        assert_eq!(texts(&copy), texts(&doc_with_body(&original)));
        assert_eq!(bookmark_starts(&copy), [("Kept".to_owned(), 1)]);
    }
}

#[test]
fn a_bookmark_half_inside_a_hyperlink_is_refused_rather_than_written_twice() {
    let document = |added: bool| {
        let (start, end) = if added {
            (
                r#"<w:bookmarkStart w:id="2" w:name="Added"/>"#,
                r#"<w:bookmarkEnd w:id="2"/>"#,
            )
        } else {
            ("", "")
        };
        doc_with_body(&format!(
            r#"{}<w:p>{start}<w:hyperlink w:anchor="Target"><w:bookmarkStart w:id="1" w:name="Kept"/><w:r><w:t>Linked</w:t></w:r></w:hyperlink><w:bookmarkEnd w:id="1"/>{end}</w:p>{}"#,
            plain_paragraph("One"),
            plain_paragraph("Two")
        ))
    };
    let mut doc = document(false);
    let error = doc.compare(&document(true), AUTHOR, DATE).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("boundary structures at body/paragraph[1]"),
        "{error}"
    );
}

#[test]
fn a_bookmark_ending_in_a_replaced_paragraph_still_covers_it_on_rejection() {
    let document = |anchor: &str| {
        doc_with_body(&format!(
            r#"<w:p><w:bookmarkStart w:id="1" w:name="Span"/><w:r><w:t>From</w:t></w:r></w:p><w:p><w:hyperlink w:anchor="{anchor}"><w:r><w:t>Heading</w:t></w:r></w:hyperlink><w:bookmarkEnd w:id="1"/></w:p>{}"#,
            plain_paragraph("Two")
        ))
    };
    let edited = document("Second");
    let mut doc = redline(document("First"), &edited);
    let span = |doc: &Document| {
        let bookmarks = doc.bookmarks();
        let [bookmark] = bookmarks.as_slice() else {
            panic!("one bookmark: {bookmarks:?}")
        };
        (
            bookmark.text().to_owned(),
            bookmark.range().unwrap().end.body_index,
        )
    };
    // Accepting leaves the end the deletion held at the start of the next
    // paragraph, as Word does: over the same text.
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(span(&copy), ("From\nHeading\n".to_owned(), 2));
    // Rejecting restores it exactly.
    let original = document("First");
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&original));
    assert_eq!(span(&copy), span(&original));
}

#[test]
fn a_heading_that_keeps_its_point_bookmark_and_gains_another_is_compared() {
    // Google writes its heading bookmarks as points before the text.
    let heading = |added: &str| {
        doc_with_body(&format!(
            r#"{}<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{added}<w:bookmarkStart w:id="1" w:name="_hjerlw5ntnls"/><w:bookmarkEnd w:id="1"/><w:r><w:t>Heading</w:t></w:r></w:p>{}"#,
            plain_paragraph("One"),
            plain_paragraph("Two")
        ))
    };
    let edited = heading(r#"<w:bookmarkStart w:id="2" w:name="_Toc2"/><w:bookmarkEnd w:id="2"/>"#);
    let mut doc = redline(heading(""), &edited);
    assert_shared_bookmark_layout(&main_part_xml(&mut doc), "_hjerlw5ntnls", true);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(bookmark_starts(&copy), bookmark_starts(&edited));
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&heading("")));
    assert_eq!(bookmark_starts(&copy), [("_hjerlw5ntnls".to_owned(), 1)]);
}

#[test]
fn a_point_bookmark_both_versions_hold_after_text_in_a_replaced_paragraph_is_refused() {
    // Rejecting would leave it at the start of the restored paragraph, and
    // its position is all a point has.
    let paragraph = |added: &str| {
        doc_with_body(&format!(
            r#"{}<w:p>{added}<w:r><w:t xml:space="preserve">Hello </w:t></w:r><w:bookmarkStart w:id="1" w:name="B"/><w:bookmarkEnd w:id="1"/><w:r><w:t>world</w:t></w:r></w:p>{}"#,
            plain_paragraph("One"),
            plain_paragraph("Two")
        ))
    };
    let edited =
        paragraph(r#"<w:bookmarkStart w:id="2" w:name="Added"/><w:bookmarkEnd w:id="2"/>"#);
    let error = paragraph("")
        .compare(&edited, AUTHOR, DATE)
        .expect_err("the point cannot be kept after the text");
    assert!(error.to_string().contains("over the same text"), "{error}");
}

#[test]
fn a_bookmark_both_versions_hold_that_would_cross_a_picture_is_refused() {
    // The end after a line break and before the paragraph's text: accepting
    // the deletion would move it after all of it, across no counted text
    // but the break.
    let paragraph = |added: &str| {
        doc_with_body(&format!(
            r#"{}<w:p>{added}<w:bookmarkStart w:id="1" w:name="B"/><w:r><w:br/></w:r><w:bookmarkEnd w:id="1"/><w:r><w:sym w:font="Symbol" w:char="F0B7"/></w:r></w:p>{}"#,
            plain_paragraph("One"),
            plain_paragraph("Two")
        ))
    };
    let edited =
        paragraph(r#"<w:bookmarkStart w:id="2" w:name="Added"/><w:bookmarkEnd w:id="2"/>"#);
    let error = paragraph("")
        .compare(&edited, AUTHOR, DATE)
        .expect_err("the end cannot be kept before the symbol");
    assert!(error.to_string().contains("over the same text"), "{error}");
}

#[test]
fn a_rewritten_heading_that_keeps_its_bookmark_goes_before_the_deletion_that_held_it() {
    // A paragraph inserted before the heading takes its place in the pair,
    // and the rewritten heading is inserted after the deletion: the
    // comparison moves it before that deletion, so that the bookmark's start
    // lands in it.
    let original = || {
        doc_with_body(&format!(
            "{}{}{}",
            plain_paragraph("One"),
            heading_with("Future work", &[(1, "Kept")]),
            plain_paragraph("Two")
        ))
    };
    let edited = doc_with_body(&format!(
        "{}{}{}{}",
        plain_paragraph("One"),
        plain_paragraph("Limitations"),
        heading_with("Future work, revised", &[(1, "Kept")]),
        plain_paragraph("Two")
    ));
    let mut doc = redline(original(), &edited);
    assert_shared_bookmark_layout(&main_part_xml(&mut doc), "Kept", false);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(bookmark_starts(&copy), [("Kept".to_owned(), 2)]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&original()));
    assert_eq!(bookmark_starts(&copy), [("Kept".to_owned(), 1)]);
}

#[test]
fn a_bookmark_both_versions_hold_ending_inside_a_replaced_paragraph_is_refused() {
    // Neither resolution can keep the end inside the text: the insertion
    // cannot hold it (Word's Reject All drops it with the paragraph), and
    // accepting the deletion that holds it leaves it after the paragraph.
    let caption = |marks: &str| {
        doc_with_body(&format!(
            r#"{}<w:p>{marks}<w:bookmarkStart w:id="1" w:name="_Ref1"/><w:r><w:t>Figure 1</w:t></w:r><w:bookmarkEnd w:id="1"/><w:r><w:t xml:space="preserve">: results</w:t></w:r></w:p>{}"#,
            plain_paragraph("One"),
            plain_paragraph("Two")
        ))
    };
    let edited = caption(r#"<w:bookmarkStart w:id="2" w:name="Added"/><w:bookmarkEnd w:id="2"/>"#);
    let error = caption("")
        .compare(&edited, AUTHOR, DATE)
        .expect_err("the end cannot be kept inside the text");
    assert!(error.to_string().contains("over the same text"), "{error}");
}

#[test]
fn a_changed_field_holding_a_nested_field_is_deleted_as_deleted_text() {
    // The outer instruction changes: the field is deleted and inserted
    // whole, and the deletion holds the nested field's text too.
    let field = |known: &str| {
        format!(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF "</w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Name </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Smith</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve">" = "Smith" "{known}" "Unknown" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{known}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
        )
    };
    let edited = doc_with_body(&field("Yes"));
    let mut doc = redline(doc_with_body(&field("Known")), &edited);
    let xml = main_part_xml(&mut doc);
    let deletions = xml
        .split("<w:del ")
        .skip(1)
        .map(|tail| &tail[..tail.find("</w:del>").unwrap()])
        .collect::<Vec<_>>();
    assert!(
        deletions
            .iter()
            .any(|deletion| deletion.matches("<w:delInstrText").count() == 3),
        "the deletion holds both instructions: {xml}"
    );
    for deletion in deletions {
        assert!(
            !deletion.contains("<w:instrText")
                && !deletion.contains("<w:t>")
                && !deletion.contains("<w:t "),
            "{xml}"
        );
    }
    assert!(xml.contains("MERGEFIELD"), "{xml}");
    assert_eq!(texts(&accepted(&mut doc)), texts(&edited));
    assert_eq!(
        texts(&rejected(&mut doc)),
        texts(&doc_with_body(&field("Known")))
    );
}

#[test]
fn a_field_whose_result_changes_beside_a_nested_field_is_compared() {
    // The result of the outer field starts after its own separate, not
    // after the one of a field nested in its instruction, and ends at its
    // own end, not at the one of a field nested in its result.
    let begin = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#;
    let separate = r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#;
    let end = r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
    let instruction = |text: &str| {
        format!(r#"<w:r><w:instrText xml:space="preserve">{text}</w:instrText></w:r>"#)
    };
    let text = |text: &str| format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#);
    let merge = format!(
        "{begin}{}{separate}{}{end}",
        instruction(" MERGEFIELD Name "),
        text("Smith")
    );
    let in_instruction = |shown: &str| {
        format!(
            "<w:p>{begin}{}{merge}{}{separate}{}{end}</w:p>",
            instruction(r#" IF ""#),
            instruction(r#"" = "Smith" "Known" "Unknown" "#),
            text(shown)
        )
    };
    let in_result = |shown: &str| {
        format!(
            "<w:p>{begin}{}{separate}{}{merge}{end}</w:p>",
            instruction(" QUOTE "),
            text(shown)
        )
    };
    for field in [&in_instruction as &dyn Fn(&str) -> String, &in_result] {
        let (original, edited) = (field("Unknown"), field("Known"));
        let mut doc = redline(doc_with_body(&original), &doc_with_body(&edited));
        assert_eq!(texts(&accepted(&mut doc)), texts(&doc_with_body(&edited)));
        assert_eq!(texts(&rejected(&mut doc)), texts(&doc_with_body(&original)));
    }
}

#[test]
fn a_simple_field_nested_through_a_control_in_another_keeps_a_refusal_when_its_paragraph_goes_whole()
 {
    // Nested through an inline content control, it would stay a simple
    // field inside the deletion, whose text Word requires as deleted text.
    let nested = r#"<w:p><w:fldSimple w:instr=" IF 1 = 1 "><w:sdt><w:sdtPr/><w:sdtContent><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:sdtContent></w:sdt></w:fldSimple></w:p>"#;
    let after = plain("After.");
    for (original, edited) in [
        (after.clone(), format!("{nested}{after}")),
        (format!("{nested}{after}"), after.clone()),
    ] {
        let mut doc = doc_with_body(&original);
        let error = doc
            .compare(&doc_with_body(&edited), AUTHOR, DATE)
            .expect_err("refused");
        assert!(error.to_string().contains("simple field nested"), "{error}");
        assert!(doc.revisions().is_empty());
    }
}

#[test]
fn a_paragraph_merged_on_rejection_keeps_its_properties_when_formatting_is_ignored() {
    // A bookmark start between the paragraphs makes the paragraph before the
    // inserted heading carry the insertion on its mark. Rejecting it in Word
    // merges that paragraph into the heading's mark, so the heading records
    // the paragraph's own properties in a change, formatting ignored or not.
    let before =
        r#"<w:p><w:pPr><w:jc w:val="both"/></w:pPr><w:r><w:t>Body text.</w:t></w:r></w:p>"#;
    let next = r#"<w:bookmarkStart w:id="54" w:name="_Toc23"/><w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Future work</w:t></w:r><w:bookmarkEnd w:id="54"/></w:p>"#;
    let inserted = r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Limitations</w:t></w:r></w:p>"#;
    let original = || doc_with_body(&format!("{before}{next}"));
    let edited = doc_with_body(&format!("{before}{inserted}{next}"));
    let options = ComparisonOptions {
        ignore_formatting: true,
        ..Default::default()
    };
    let mut doc = original();
    doc.compare_with_options(&edited, AUTHOR, DATE, &options)
        .unwrap();
    let xml = main_part_xml(&mut doc);
    let heading = &xml[..xml.find("Limitations").unwrap()];
    let heading = &heading[heading
        .rfind("<w:p>")
        .into_iter()
        .chain(heading.rfind("<w:p "))
        .max()
        .unwrap()..];
    assert!(
        heading.contains("<w:pPrChange") && heading.contains(r#"<w:jc w:val="both"/>"#),
        "{xml}"
    );
    assert_eq!(texts(&accepted(&mut doc)), texts(&edited));
    let mut copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&original()));
    let xml = main_part_xml(&mut copy);
    let body = &xml[..xml.find("Body text.").unwrap()];
    let body = &body[body
        .rfind("<w:p>")
        .into_iter()
        .chain(body.rfind("<w:p "))
        .max()
        .unwrap()..];
    assert!(!body.contains("Heading2"), "{xml}");
}

#[test]
fn a_last_paragraph_deleted_after_a_restyled_one_keeps_the_original_formatting_when_it_is_ignored()
{
    // The paragraph before carries the deletion on its mark, and accepting
    // merges the last one into it: with formatting ignored, it keeps the
    // original's properties on both sides, as the paragraph itself does.
    let original = doc_with_body(&format!("{}{}", plain("Intro"), plain("Gone")));
    let edited = doc_with_body(
        r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>Intro</w:t></w:r></w:p>"#,
    );
    let options = ComparisonOptions {
        ignore_formatting: true,
        ..Default::default()
    };
    let mut doc = original;
    doc.compare_with_options(&edited, AUTHOR, DATE, &options)
        .unwrap();
    let xml = main_part_xml(&mut doc);
    assert!(
        !xml.contains("center") && !xml.contains("pPrChange"),
        "{xml}"
    );
    let mut copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Intro"]);
    assert!(!main_part_xml(&mut copy).contains("center"));
}

// ---- Block content controls ----

const INTRO: &str = "<w:p><w:r><w:t>Intro</w:t></w:r></w:p>";
const BODY: &str = "<w:p><w:r><w:t>Body.</w:t></w:r></w:p>";

/// A block content control named `alias` holding `content`.
fn block_control(alias: &str, content: &str) -> String {
    format!(
        r#"<w:sdt><w:sdtPr><w:alias w:val="{alias}"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"#
    )
}

/// A one-cell table holding `content`.
fn one_cell_table(content: &str) -> String {
    format!(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc>{content}</w:tc></w:tr></w:tbl>"#
    )
}

/// The body owners of `doc` in order: a paragraph's text, a block control
/// as `alias{text}`, a table as `[text of its first cell]`, other markup as
/// its XML.
fn body_owners(doc: &Document) -> Vec<String> {
    doc.body_items()
        .map(|item| match item {
            BodyItemRef::Paragraph(paragraph) => paragraph.text(),
            BodyItemRef::Table(table) => format!("[{}]", table.cell(0, 0).unwrap().text()),
            BodyItemRef::ContentControl(control) => {
                format!("{}{{{}}}", control.alias().unwrap_or(""), control.text())
            }
            BodyItemRef::UnsupportedXml(xml) => String::from_utf8_lossy(xml).into_owned(),
        })
        .collect()
}

/// What a resolved body must give back: its owners, the alias of every
/// content control in document order, nested ones included, and its
/// bookmark names.
fn resolved_shape(doc: &mut Document) -> (Vec<String>, Vec<String>, Vec<String>) {
    let aliases = main_xml(doc)
        .split(r#"<w:alias w:val=""#)
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap_or_default().to_owned())
        .collect();
    (body_owners(doc), aliases, bookmark_names(doc))
}

/// Compare the body `original` with the body `edited` both ways, at word
/// and character granularity: each redline resolves to the edited body
/// once accepted and to the original once rejected (see
/// [`resolved_shape`]), with the revisions `check` expects.
fn check_bodies_both_ways(original: &str, edited: &str, check: impl Fn(&mut Document, bool)) {
    for (from, to, forward) in [(original, edited, true), (edited, original, false)] {
        for granularity in [
            ComparisonGranularity::Word,
            ComparisonGranularity::Character,
        ] {
            let mut doc = doc_with_body(from);
            let options = ComparisonOptions {
                granularity,
                ..Default::default()
            };
            let diagnostics = doc
                .compare_with_options(&doc_with_body(to), AUTHOR, DATE, &options)
                .unwrap_or_else(|error| panic!("{granularity:?} {forward}: {error}"));
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let mut doc = reopen(&mut doc);
            check(&mut doc, forward);
            let context = format!("{granularity:?}, forward {forward}");
            assert_eq!(
                resolved_shape(&mut accepted(&mut doc)),
                resolved_shape(&mut doc_with_body(to)),
                "{context}"
            );
            assert_eq!(
                resolved_shape(&mut rejected(&mut doc)),
                resolved_shape(&mut doc_with_body(from)),
                "{context}"
            );
        }
    }
}

#[test]
fn a_block_content_control_added_or_removed_whole_is_tracked_and_resolves_to_either_version() {
    let control = block_control("Box", &plain("Inside"));
    for (original, edited) in [
        (format!("{INTRO}{control}{BODY}"), format!("{INTRO}{BODY}")),
        (format!("{INTRO}{control}"), INTRO.to_owned()),
        (control.clone(), INTRO.to_owned()),
        (
            format!("{INTRO}{control}{}", block_control("Two", &plain("Second"))),
            INTRO.to_owned(),
        ),
    ] {
        check_bodies_both_ways(&original, &edited, |doc, forward| {
            let kind = if forward {
                RevisionKind::Deletion
            } else {
                RevisionKind::Insertion
            };
            assert!(
                revision_texts(doc, kind)
                    .iter()
                    .any(|text| text == "Inside"),
                "{:?}",
                revision_texts(doc, kind)
            );
            // The control and its properties stay in the redline.
            assert!(main_xml(doc).contains(r#"<w:alias w:val="Box"/>"#));
        });
    }
    // After a control both versions hold, ending the body.
    check_bodies_both_ways(
        &format!("{INTRO}{control}{}", block_control("Two", &plain("Second"))),
        &format!("{INTRO}{control}"),
        |_, _| {},
    );
}

#[test]
fn a_block_content_control_holding_a_table_or_another_control_goes_whole() {
    let nested = block_control(
        "Outer",
        &format!(
            "{}{}",
            one_cell_table(&plain("cell")),
            block_control("Inner", &plain("Inside"))
        ),
    );
    check_bodies_both_ways(
        &format!("{INTRO}{nested}{BODY}"),
        &format!("{INTRO}{BODY}"),
        |_, _| {},
    );
}

#[test]
fn a_paragraph_appended_after_a_final_block_content_control_is_compared() {
    let control = block_control("Box", &plain("Inside"));
    check_bodies_both_ways(
        &format!("{INTRO}{control}"),
        &format!("{INTRO}{control}{BODY}"),
        |doc, forward| {
            let kind = if forward {
                RevisionKind::Insertion
            } else {
                RevisionKind::Deletion
            };
            assert_eq!(revision_texts(doc, kind), ["Body."]);
            // As Word marks it: the change is on the mark of the control's
            // last paragraph, and the body's final mark is untracked.
            let xml = main_xml(doc);
            let tag = if forward { "w:ins" } else { "w:del" };
            let control =
                &xml[xml.find("<w:sdtContent").unwrap()..xml.find("</w:sdtContent>").unwrap()];
            assert!(control.contains(&format!("<w:rPr><{tag} ")), "{xml}");
            let last = &xml[xml.rfind("<w:p>").unwrap()..];
            assert!(!last.contains("<w:pPr>"), "{xml}");
        },
    );
    // A centred paragraph appended after a control whose paragraph is not:
    // each resolution keeps each paragraph's alignment.
    let centred = "<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:t>Body.</w:t></w:r></w:p>";
    check_bodies_both_ways(
        &format!("{INTRO}{control}"),
        &format!("{INTRO}{control}{centred}"),
        |doc, forward| {
            let centred_after =
                |doc: &mut Document| main_xml(doc).matches("w:val=\"center\"").count();
            let (accept, reject) = if forward { (1, 0) } else { (0, 1) };
            assert_eq!(centred_after(&mut accepted(doc)), accept);
            assert_eq!(centred_after(&mut rejected(doc)), reject);
        },
    );
    // The control's own text changes too.
    check_bodies_both_ways(
        &format!("{INTRO}{control}"),
        &format!(
            "{INTRO}{}{BODY}",
            block_control("Box", &plain("Inside now"))
        ),
        |_, _| {},
    );
}

#[test]
fn a_paragraph_removed_next_to_a_block_content_control_is_compared() {
    let first = block_control("Box", &plain("Inside"));
    let second = block_control("Two", &format!("{}{}", plain("Second"), plain("Third")));
    let removed = plain("Removed");
    for (original, edited) in [
        // First in the body, before a control.
        (format!("{removed}{first}{BODY}"), format!("{first}{BODY}")),
        // Between two controls.
        (
            format!("{first}{removed}{second}{BODY}"),
            format!("{first}{second}{BODY}"),
        ),
        // Before a control that ends the body.
        (
            format!("{INTRO}{removed}{first}"),
            format!("{INTRO}{first}"),
        ),
        // With the control after it, ending the body.
        (format!("{INTRO}{removed}{first}"), INTRO.to_owned()),
        // Before a control both versions hold empty, in the middle and
        // ending the body.
        (
            format!("{INTRO}{removed}{}{BODY}", block_control("Empty", "<w:p/>")),
            format!("{INTRO}{}{BODY}", block_control("Empty", "<w:p/>")),
        ),
        (
            format!("{INTRO}{removed}{}", block_control("Empty", "<w:p/>")),
            format!("{INTRO}{}", block_control("Empty", "<w:p/>")),
        ),
        // In a cell, before a control.
        (
            one_cell_table(&format!("{removed}{first}{INTRO}")),
            one_cell_table(&format!("{first}{INTRO}")),
        ),
        // Inside a control, before a nested control.
        (
            block_control("Outer", &format!("{removed}{first}{INTRO}")),
            block_control("Outer", &format!("{first}{INTRO}")),
        ),
    ] {
        check_bodies_both_ways(&original, &edited, |_, _| {});
    }
}

#[test]
fn a_moved_block_content_control_is_tracked_as_a_move() {
    let control = block_control("Box", &plain("Inside"));
    check_bodies_both_ways(
        &format!("{control}{INTRO}{BODY}"),
        &format!("{INTRO}{BODY}{control}"),
        |doc, _| {
            assert_eq!(revision_texts(doc, RevisionKind::MoveFrom), ["Inside"]);
            assert_eq!(revision_texts(doc, RevisionKind::MoveTo), ["Inside"]);
        },
    );
}

#[test]
fn a_block_content_control_added_in_a_cell_or_in_another_control_is_compared() {
    let control = block_control("Box", &plain("Inside"));
    check_bodies_both_ways(
        &format!("{INTRO}{}{BODY}", one_cell_table(INTRO)),
        &format!(
            "{INTRO}{}{BODY}",
            one_cell_table(&format!("{control}{INTRO}"))
        ),
        |_, _| {},
    );
    check_bodies_both_ways(
        &format!("{INTRO}{}{BODY}", block_control("Outer", INTRO)),
        &format!(
            "{INTRO}{}{BODY}",
            block_control("Outer", &format!("{control}{INTRO}"))
        ),
        |_, _| {},
    );
}

#[test]
fn a_word_table_of_contents_removed_whole_is_compared() {
    // Word's References > Table of Contents: a gallery control holding a
    // heading and a TOC field across paragraphs, with hyperlinked entries.
    let toc = concat!(
        r#"<w:sdt><w:sdtPr><w:id w:val="-1"/><w:docPartObj><w:docPartGallery w:val="Table of Contents"/>"#,
        r#"<w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="TOCHeading"/></w:pPr><w:r><w:t>Contents</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
        r#"<w:hyperlink w:anchor="_Toc1" w:history="1"><w:r><w:t>Intro</w:t></w:r><w:r><w:tab/></w:r>"#,
        r#"<w:r><w:t>1</w:t></w:r></w:hyperlink></w:p>"#,
        r#"<w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        r#"</w:sdtContent></w:sdt>"#,
    );
    let heading = r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="_Toc1"/><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#;
    check_bodies_both_ways(
        &format!("{toc}{heading}{BODY}"),
        &format!("{heading}{BODY}"),
        |doc, forward| {
            let kind = if forward {
                RevisionKind::Deletion
            } else {
                RevisionKind::Insertion
            };
            assert!(revision_texts(doc, kind).contains(&"Contents".to_owned()));
        },
    );
}

#[test]
fn a_block_content_control_whose_content_is_all_tracked_goes_with_that_content() {
    // A control deleted or inserted under tracking: every run and
    // paragraph mark inside it marked, not the control itself.
    let marked = |kind: &str, text: &str| {
        block_control(
            "Box",
            &format!(
                r#"<w:p><w:pPr><w:rPr><w:{kind} w:id="1" w:author="Ada" w:date="{DATE}"/></w:rPr></w:pPr><w:{kind} w:id="2" w:author="Ada" w:date="{DATE}"><w:r>{text}</w:r></w:{kind}></w:p>"#
            ),
        )
    };
    let deleted = marked("del", "<w:delText>Inside</w:delText>");
    let inserted = marked("ins", "<w:t>Inside</w:t>");
    for (body, goes_on_accept) in [
        (format!("{INTRO}{deleted}{BODY}"), true),
        (format!("{INTRO}{deleted}"), true),
        (format!("{INTRO}{inserted}{BODY}"), false),
        (format!("{INTRO}{inserted}"), false),
    ] {
        let mut doc = doc_with_body(&body);
        let kept = body_owners(&doc_with_body(
            &body.replace(&deleted, "").replace(&inserted, ""),
        ));
        let with_control = {
            let mut owners = kept.clone();
            owners.insert(1, "Box{Inside}".to_owned());
            owners
        };
        let (on_accept, on_reject) = if goes_on_accept {
            (kept, with_control)
        } else {
            (with_control, kept)
        };
        assert_eq!(body_owners(&accepted(&mut doc)), on_accept, "{body}");
        assert_eq!(body_owners(&rejected(&mut doc)), on_reject, "{body}");
    }
}

#[test]
fn an_existing_paragraph_wrapped_in_a_block_content_control_is_compared() {
    check_bodies_both_ways(
        &format!("{INTRO}{BODY}"),
        &format!("{INTRO}{}", block_control("Box", BODY)),
        |_, _| {},
    );
}

#[test]
fn a_block_content_control_removed_whole_takes_the_bookmarks_it_encloses() {
    // A heading bookmark and Word's `_GoBack` inside the control, and a
    // pretty-printed control with whitespace and a proofing mark between
    // its paragraphs.
    let clause = block_control(
        "Clause",
        &format!(
            concat!(
                r#"<w:p><w:bookmarkStart w:id="7" w:name="_Toc9"/><w:r><w:t>Clause</w:t></w:r><w:bookmarkEnd w:id="7"/></w:p>"#,
                "\n  ",
                r#"<w:proofErr w:type="spellStart"/>"#,
                "\n  ",
                r#"<w:p><w:bookmarkStart w:id="8" w:name="_GoBack"/><w:bookmarkEnd w:id="8"/><w:r><w:t>Text</w:t></w:r></w:p>"#,
                "{}"
            ),
            "\n"
        ),
    );
    check_bodies_both_ways(
        &format!("{INTRO}{clause}{BODY}"),
        &format!("{INTRO}{BODY}"),
        |_, _| {},
    );
}

#[test]
fn a_bookmark_that_only_ends_in_a_removed_control_keeps_both_ends() {
    // A Word redline: the control deleted under tracking holds the end of a
    // bookmark that starts before it.
    let body = format!(
        concat!(
            r#"<w:p><w:bookmarkStart w:id="3" w:name="span"/><w:r><w:t>Intro</w:t></w:r></w:p>"#,
            r#"<w:sdt><w:sdtPr><w:alias w:val="Box"/></w:sdtPr><w:sdtContent><w:p><w:pPr><w:rPr>"#,
            r#"<w:del w:id="1" w:author="Ada" w:date="{date}"/></w:rPr></w:pPr><w:del w:id="2" w:author="Ada" w:date="{date}">"#,
            r#"<w:r><w:delText>Inside</w:delText></w:r></w:del><w:bookmarkEnd w:id="3"/></w:p></w:sdtContent></w:sdt>"#,
            "{body}"
        ),
        date = DATE,
        body = BODY
    );
    let mut doc = doc_with_body(&body);
    let accepted = accepted(&mut doc);
    // The end stays at block level, where the control was.
    let owners = body_owners(&accepted);
    assert_eq!(owners.len(), 3, "{owners:?}");
    assert_eq!([&owners[0], &owners[2]], ["Intro", "Body."]);
    assert!(owners[1].starts_with("<w:bookmarkEnd ") && owners[1].contains(r#"w:id="3""#));
    assert_eq!(bookmark_names(&accepted), ["span"]);
    assert_eq!(bookmark_names(&rejected(&mut doc)), ["span"]);
}

#[test]
fn a_block_content_control_ending_the_body_leaves_the_final_mark_untracked() {
    // Word cannot resolve a change on the body's last paragraph mark: the
    // paragraph before the control carries it, as for a final paragraph.
    let control = block_control("Box", &format!("{}{}", plain("Inside"), plain("Last")));
    check_bodies_both_ways(&format!("{INTRO}{control}"), INTRO, |doc, forward| {
        let xml = main_xml(doc);
        let tag = if forward { "<w:del " } else { "<w:ins " };
        let marks = |paragraph: &str| {
            let properties = paragraph.split("</w:pPr>").next().unwrap_or_default();
            paragraph.contains("</w:pPr>") && properties.contains(tag)
        };
        // The first paragraph is Intro's, the last the control's last.
        let intro = &xml[..xml.find("</w:p>").unwrap()];
        let last = &xml[xml.rfind("<w:p>").max(xml.rfind("<w:p ")).unwrap()..];
        assert!(intro.contains("Intro") && marks(intro), "{xml}");
        assert!(last.contains("Last<") && !marks(last), "{xml}");
    });
}

#[test]
fn a_range_ending_in_a_removed_final_control_still_ends_after_it_starts() {
    // A Word-style redline of a control removed at the end of the body: the
    // paragraph before it carries the change, and a bookmark that starts
    // there ends in the control's first paragraph.
    let mark = |kind: &str, id: u32| {
        format!(
            r#"<w:pPr><w:rPr><w:{kind} w:id="{id}" w:author="Ada" w:date="{DATE}"/></w:rPr></w:pPr>"#
        )
    };
    let deleted = |id: u32, text: &str| {
        format!(
            r#"<w:del w:id="{id}" w:author="Ada" w:date="{DATE}"><w:r><w:delText>{text}</w:delText></w:r></w:del>"#
        )
    };
    let body = format!(
        concat!(
            r#"<w:p>{intro_mark}<w:bookmarkStart w:id="9" w:name="span"/><w:r><w:t>Intro</w:t></w:r></w:p>"#,
            r#"<w:sdt><w:sdtPr><w:alias w:val="Box"/></w:sdtPr><w:sdtContent>"#,
            r#"<w:p>{first_mark}{first}<w:bookmarkEnd w:id="9"/></w:p><w:p>{last}</w:p>"#,
            r#"</w:sdtContent></w:sdt>"#
        ),
        intro_mark = mark("del", 1),
        first_mark = mark("del", 2),
        first = deleted(3, "Inside"),
        last = deleted(4, "Last"),
    );
    let mut accepted = accepted(&mut doc_with_body(&body));
    // The end stays at block level, after the merged paragraph.
    let owners = body_owners(&accepted);
    assert_eq!(owners.len(), 2, "{owners:?}");
    assert_eq!(owners[0], "Intro");
    assert!(owners[1].starts_with("<w:bookmarkEnd "), "{owners:?}");
    let xml = main_xml(&mut accepted);
    let start = xml.find("<w:bookmarkStart").expect("the bookmark starts");
    let end = xml.find("<w:bookmarkEnd").expect("the bookmark ends");
    assert!(start < end, "{xml}");
    assert_eq!(bookmark_names(&accepted), ["span"]);
}

/// Two numbered items and a third in a table cell, saved with each one's
/// `w:ilvl` element replaced by `level` (empty: none), as producers write it.
fn numbered_items(level: &str) -> Document {
    let mut doc = Document::new();
    doc.add_numbered_list_item("one", 0);
    doc.add_numbered_list_item("two", 0);
    let list = doc.paragraph(0).unwrap().numbering().unwrap().0;
    doc.add_table(1, 1)
        .cell(0, 0)
        .unwrap()
        .add_paragraph("three")
        .numbering(list, 0);
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    let main = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
    assert!(xml.contains(r#"<w:ilvl w:val="0"/>"#), "{xml}");
    package.set_part(
        &main,
        xml.replace(r#"<w:ilvl w:val="0"/>"#, level).into_bytes(),
    );
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

#[test]
fn a_copy_that_writes_the_list_level_its_items_show_is_no_change() {
    // Word or python-docx write a `w:numId` alone; LibreOffice writes the
    // `w:ilvl 0` it shows.
    let doc = redline(
        numbered_items(""),
        &numbered_items(r#"<w:ilvl w:val="0"/>"#),
    );
    assert_eq!(kinds(&doc), []);
    let doc = redline(
        numbered_items(r#"<w:ilvl w:val="0"/>"#),
        &numbered_items(""),
    );
    assert_eq!(kinds(&doc), []);
    // A real change of level is still one.
    let doc = redline(
        numbered_items(""),
        &numbered_items(r#"<w:ilvl w:val="1"/>"#),
    );
    assert!(!kinds(&doc).is_empty());
}

/// The main part of `doc` as saved.
fn saved_main_xml(doc: &mut Document) -> String {
    let package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    let main = package.main_document_part().unwrap();
    String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap()
}

#[test]
fn a_list_item_without_a_level_keeps_that_form_through_a_tracked_property_change() {
    // The edited copy writes the level and centres the first item: rejecting
    // the change gives back the original, still without a `w:ilvl`.
    let mut edited = numbered_items(r#"<w:ilvl w:val="0"/>"#);
    edited
        .paragraph_mut(0)
        .unwrap()
        .set_alignment(Alignment::Center);
    // Both routes: compared by word, and run by run.
    for granularity in [ComparisonGranularity::Word, ComparisonGranularity::Run] {
        let mut doc = numbered_items("");
        let options = ComparisonOptions {
            granularity,
            ..Default::default()
        };
        let diagnostics = doc
            .compare_with_options(&edited, AUTHOR, DATE, &options)
            .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let mut doc = reopen(&mut doc);
        assert_eq!(kinds(&doc), [RevisionKind::ParagraphPropertyChange]);
        let mut back = rejected(&mut doc);
        assert!(
            !saved_main_xml(&mut back).contains("<w:ilvl"),
            "{granularity:?}"
        );
        assert_eq!(
            accepted(&mut doc).paragraph(0).unwrap().alignment(),
            Some(Alignment::Center)
        );
    }
}

#[test]
fn a_list_item_without_a_level_reads_at_its_style_s_level() {
    // The items' style holds `w:ilvl 1`: a copy that writes level 1 is no
    // change, one that writes level 0 is one.
    let styled = |level: &str| {
        let mut doc = numbered_items(level);
        let mut package =
            oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
                .unwrap();
        let main = package.main_document_part().unwrap();
        let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
        package.set_part(
            &main,
            xml.replace("<w:pPr>", r#"<w:pPr><w:pStyle w:val="Deep"/>"#)
                .into_bytes(),
        );
        let styles = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec())
            .unwrap()
            .replace(
                "</w:styles>",
                concat!(
                    r#"<w:style w:type="paragraph" w:styleId="Deep"><w:name w:val="Deep"/>"#,
                    r#"<w:pPr><w:numPr><w:ilvl w:val="1"/></w:numPr></w:pPr></w:style></w:styles>"#
                ),
            );
        package.set_part("/word/styles.xml", styles.into_bytes());
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        Document::from_bytes(&out.into_inner()).unwrap()
    };
    let doc = redline(styled(""), &styled(r#"<w:ilvl w:val="1"/>"#));
    assert_eq!(kinds(&doc), []);
    let doc = redline(styled(""), &styled(r#"<w:ilvl w:val="0"/>"#));
    assert!(!kinds(&doc).is_empty());
}

#[test]
fn a_list_item_without_a_level_keeps_that_form_through_a_tracked_mark_change() {
    // The edited copy writes the level and makes the first item's mark bold:
    // rejecting the change gives back the original, still without a `w:ilvl`.
    let mut edited = numbered_items(r#"<w:ilvl w:val="0"/>"#);
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(edited.to_bytes().unwrap()))
            .unwrap();
    let main = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
    package.set_part(
        &main,
        xml.replacen("</w:numPr>", "</w:numPr><w:rPr><w:b/></w:rPr>", 1)
            .into_bytes(),
    );
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let edited = Document::from_bytes(&out.into_inner()).unwrap();
    for granularity in [ComparisonGranularity::Word, ComparisonGranularity::Run] {
        let mut doc = numbered_items("");
        let options = ComparisonOptions {
            granularity,
            ..Default::default()
        };
        let diagnostics = doc
            .compare_with_options(&edited, AUTHOR, DATE, &options)
            .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let mut doc = reopen(&mut doc);
        assert!(!kinds(&doc).is_empty(), "{granularity:?}");
        let mut back = rejected(&mut doc);
        assert!(
            !saved_main_xml(&mut back).contains("<w:ilvl"),
            "{granularity:?}"
        );
    }
}

// ---- Owners written self-closing ----
// Google Docs leaves empty inline content controls written `<w:sdtContent/>`
// in ordinary paragraphs. An owner written self-closing reads as an empty
// one, keeps that form while nothing is written in it, and gets an end tag
// when the redline writes in it.

/// A paragraph holding an inline content control as Google Docs writes it,
/// with `inside` as its text, or empty and self-closing.
fn paragraph_with_control(word: &str, inside: Option<&str>) -> String {
    let content = inside.map_or("<w:sdtContent/>".to_owned(), |text| {
        format!("<w:sdtContent><w:r><w:t>{text}</w:t></w:r></w:sdtContent>")
    });
    format!(
        concat!(
            r#"<w:p><w:r><w:t xml:space="preserve">Alpha beta </w:t></w:r>"#,
            r#"<w:sdt><w:sdtPr><w:tag w:val="goog_rdk_1"/><w:id w:val="1"/></w:sdtPr>{}</w:sdt>"#,
            r#"<w:r><w:t>{}.</w:t></w:r></w:p>"#
        ),
        content, word
    )
}

/// Compare the body `original` with the body `edited` both ways, at every
/// granularity: each redline resolves to the body it goes to once accepted
/// and to the one it comes from once rejected, and `check` reads its main
/// part.
fn check_paragraphs_both_ways(original: &str, edited: &str, check: impl Fn(&str, bool)) {
    for (from, to, forward) in [(original, edited, true), (edited, original, false)] {
        for granularity in [
            ComparisonGranularity::Run,
            ComparisonGranularity::Word,
            ComparisonGranularity::Character,
        ] {
            let context = format!("{granularity:?}, forward {forward}");
            let mut doc = doc_with_body(from);
            let options = ComparisonOptions {
                granularity,
                ..Default::default()
            };
            let diagnostics = doc
                .compare_with_options(&doc_with_body(to), AUTHOR, DATE, &options)
                .unwrap_or_else(|error| panic!("{context}: {error}"));
            assert!(diagnostics.is_empty(), "{context}: {diagnostics:?}");
            let mut doc = reopen(&mut doc);
            check(&main_xml(&mut doc), forward);
            assert_eq!(
                texts(&accepted(&mut doc)),
                texts(&doc_with_body(to)),
                "{context}"
            );
            assert_eq!(
                texts(&rejected(&mut doc)),
                texts(&doc_with_body(from)),
                "{context}"
            );
        }
    }
}

#[test]
fn a_changed_paragraph_holding_an_empty_control_written_self_closing_is_compared() {
    // The control stays as Google Docs wrote it.
    check_paragraphs_both_ways(
        &paragraph_with_control("gamma", None),
        &paragraph_with_control("delta", None),
        |xml, _| assert!(xml.contains("<w:sdtContent/></w:sdt>"), "{xml}"),
    );
    // Nested in a control that holds text.
    let nested = |word: &str| {
        paragraph_with_control(word, Some("kept")).replace(
            "<w:sdtContent><w:r>",
            "<w:sdtContent><w:sdt><w:sdtPr/><w:sdtContent/></w:sdt><w:r>",
        )
    };
    check_paragraphs_both_ways(&nested("gamma"), &nested("delta"), |xml, _| {
        assert!(xml.contains("<w:sdtPr/><w:sdtContent/></w:sdt>"), "{xml}");
    });
}

#[test]
fn text_typed_in_an_empty_control_written_self_closing_is_tracked_inside_it() {
    for word in ["gamma", "delta"] {
        check_paragraphs_both_ways(
            &paragraph_with_control("gamma", None),
            &paragraph_with_control(word, Some("new")),
            |xml, forward| {
                let content = xml
                    .split_once("<w:sdtContent>")
                    .and_then(|(_, rest)| rest.split_once("</w:sdtContent>"))
                    .map_or("", |(content, _)| content);
                let tag = if forward { "<w:ins " } else { "<w:del " };
                assert!(
                    content.starts_with(tag) && content.contains(">new<"),
                    "{xml}"
                );
            },
        );
    }
}

/// Whether the last `local` Word owner of `xml` is written self-closing.
fn last_owner_is_self_closing(xml: &str, local: &str) -> bool {
    let start = xml
        .rfind(&format!("<w:{local} "))
        .unwrap_or_else(|| panic!("{local} owner in {xml}"));
    let end = start + xml[start..].find('>').expect("owner start tag");
    xml[..end].ends_with('/')
}

#[test]
fn a_changed_paragraph_holding_an_empty_revision_is_compared() {
    // An empty insertion or deletion marks nothing, and a tracked paragraph
    // mark is left as it was by an edit of the text: the paragraph compares,
    // and the earlier revision stays and resolves with the new ones.
    let mark = r#"<w:ins w:id="5" w:author="Earlier" w:date="2026-01-01T00:00:00Z"/>"#;
    for (properties, inside) in [
        ("", mark),
        (
            "",
            r#"<w:del w:id="5" w:author="Earlier" w:date="2026-01-01T00:00:00Z"></w:del>"#,
        ),
        (&*format!("<w:pPr><w:rPr>{mark}</w:rPr></w:pPr>"), ""),
    ] {
        let version = |word: &str| {
            doc_with_body(&format!(
                r#"<w:p>{properties}<w:r><w:t xml:space="preserve">Alpha {word} </w:t></w:r>{inside}<w:r><w:t>gamma.</w:t></w:r></w:p>{}"#,
                plain("Tail.")
            ))
        };
        let mut doc = redline(version("beta"), &version("delta"));
        assert!(
            main_part_xml(&mut doc).contains(r#"w:id="5""#),
            "{properties}{inside}"
        );
        assert_eq!(
            story_texts(&accepted(&mut doc)),
            story_texts(&accepted(&mut version("delta"))),
            "{properties}{inside}"
        );
        assert_eq!(
            story_texts(&rejected(&mut doc)),
            story_texts(&rejected(&mut version("beta"))),
            "{properties}{inside}"
        );
    }
}

/// A run holding a text box, as each producer writes one: Word, LibreOffice
/// and Google write a DrawingML choice and a VML fallback, older files the
/// VML alone, some writers the DrawingML alone.
fn text_box_of_form(form: &str, content: &str) -> String {
    let run = crate::support::text_box_run(7, content);
    let between = |open: &str, close: &str| &run[run.find(open).unwrap()..run.find(close).unwrap()];
    match form {
        "alternate content" => run,
        "drawing" => format!(
            r#"<w:r>{}</w:r>"#,
            between("<w:drawing>", "</mc:Choice>").replacen(
                "<w:drawing>",
                r#"<w:drawing xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">"#,
                1
            )
        ),
        _ => format!(
            r#"<w:r>{}</w:r>"#,
            between("<w:pict>", "</mc:Fallback>").replacen(
                "<w:pict>",
                r#"<w:pict xmlns:v="urn:schemas-microsoft-com:vml">"#,
                1
            )
        ),
    }
}

#[test]
fn a_text_box_edited_inside_is_tracked_inside_each_copy_of_the_box() {
    // An edit inside a box is marked inside it, in each copy an alternate
    // content holds, as Word marks it: the redline is valid and resolves to
    // either version, in the body or a header, formatting compared or
    // ignored. Text typed into an empty box and an edit beside the box as
    // well compare the same.
    let paragraph = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let version = |part: &str, before: &str, form: &str, content: &str| {
        let boxed = text_box_of_form(form, content);
        if part == "document.xml" {
            return doc_with_body(&format!(
                r#"<w:p><w:r><w:t xml:space="preserve">{before} </w:t></w:r>{boxed}</w:p>{}"#,
                plain("Tail.")
            ));
        }
        let mut doc = doc_with(&["Body."]);
        doc.set_header(before);
        with_part(&mut doc, part, |xml| {
            xml.replacen("<w:r>", &format!("{boxed}<w:r>"), 1)
        })
    };
    let (body, header) = ("document.xml", "header1.xml");
    for (form, case, part, ignore_formatting) in [
        ("alternate content", "edit inside", body, false),
        ("alternate content", "typed into an empty box", body, false),
        ("alternate content", "edit beside too", body, false),
        ("alternate content", "edit inside", body, true),
        ("alternate content", "edit inside", header, true),
        ("drawing", "edit inside", body, false),
        ("vml", "edit inside", body, false),
    ] {
        let copies = if form == "alternate content" { 2 } else { 1 };
        let (from, to, text) = if case == "typed into an empty box" {
            ("<w:p/>".to_owned(), paragraph("Typed"), "Typed")
        } else {
            (
                paragraph("Inside the box"),
                paragraph("Within the box"),
                "Within the box",
            )
        };
        let beside = if case == "edit beside too" {
            "After"
        } else {
            "Before"
        };
        let case = format!("{form}, {case}, {part}, ignore formatting {ignore_formatting}");
        let options = ComparisonOptions {
            ignore_formatting,
            ..ComparisonOptions::default()
        };
        let edited = version(part, beside, form, &to);
        let mut doc = version(part, "Before", form, &from);
        let diagnostics = doc
            .compare_with_options(&edited, AUTHOR, DATE, &options)
            .unwrap_or_else(|error| panic!("{case}: {error}"));
        assert!(diagnostics.is_empty(), "{case}: {diagnostics:?}");
        let mut doc = reopen(&mut doc);
        let report = doc.validate().unwrap();
        assert!(report.is_valid(), "{case}: {:?}", report.findings);
        let xml = part_xml(&mut doc, part);
        let marked = xml
            .split("</w:txbxContent>")
            .filter(|piece| {
                piece
                    .rfind("<w:txbxContent")
                    .is_some_and(|at| piece[at..].contains("<w:ins "))
            })
            .count();
        assert_eq!(marked, copies, "{case}: {xml}");
        let (mut accepted, mut rejected) = (accepted(&mut doc), rejected(&mut doc));
        let written =
            |doc: &mut Document| part_xml(doc, part).matches(&format!(">{text}<")).count();
        assert_eq!(written(&mut accepted), copies, "{case}");
        assert_eq!(written(&mut rejected), 0, "{case}");
        assert_eq!(story_texts(&accepted), story_texts(&edited), "{case}");
        assert_eq!(
            story_texts(&rejected),
            story_texts(&version(part, "Before", form, &from)),
            "{case}"
        );
    }
}

#[test]
fn a_text_box_deleted_replaced_or_reframed_is_redlined_with_its_text_deleted_inside() {
    // A box deleted or replaced, or whose frame or run changed as well as
    // its text, is deleted and inserted whole: each copy of the deleted box
    // holds its text deleted inside, as the deletion around it needs. The
    // redline is valid and resolves to either version, in each copy, in the
    // body, a header or a note, formatting compared or ignored, the text box
    // story compared or ignored.
    let paragraph = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let (inside, within) = ("Inside the box", "Within the box");
    let anchored = |run: String| {
        run.replacen(
            r#"<wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="914400"/>"#,
            concat!(
                r#"<wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" "#,
                r#"relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1">"#,
                r#"<wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>0</wp:posOffset>"#,
                r#"</wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset>"#,
                r#"</wp:positionV><wp:extent cx="914400" cy="914400"/><wp:wrapSquare wrapText="bothSides"/>"#,
            ),
            1,
        )
        .replacen("</wp:inline>", "</wp:anchor>", 1)
    };
    // The original box run, the edited one and the edited tail paragraph.
    let shape = |form: &str, shape: &str| -> (String, String, String) {
        let from = text_box_of_form(form, &paragraph(inside));
        let to = text_box_of_form(form, &paragraph(within));
        let tail = "Tail.".to_owned();
        match shape {
            "deleted" => (from, String::new(), "Tails.".to_owned()),
            "replaced by text" => (from, "<w:r><w:t>Instead</w:t></w:r>".to_owned(), tail),
            "resized" => (
                from,
                to.replace(r#"cy="914400""#, r#"cy="1828800""#).replace(
                    r#"<v:shape id="tb7">"#,
                    r#"<v:shape id="tb7" style="height:144pt">"#,
                ),
                tail,
            ),
            "anchored" => (from, anchored(to), tail),
            "run formatted" => (
                from,
                to.replacen("<w:r>", "<w:r><w:rPr><w:b/></w:rPr>", 1),
                tail,
            ),
            "text beside" => (
                from.replacen("<w:r>", "<w:r><w:t>A</w:t>", 1),
                to.replacen("<w:r>", "<w:r><w:t>B</w:t>", 1),
                tail,
            ),
            "moved" => (
                from,
                String::new(),
                format!("Tail.</w:t></w:r>{to}<w:r><w:t>"),
            ),
            "nested" => {
                // A box holding a VML box, both edited inside.
                let nested = |outer: &str, inner: &str| {
                    let inner = text_box_of_form("vml", &paragraph(inner)).replace("tb7", "tb8");
                    text_box_of_form(form, &format!("{}<w:p>{inner}</w:p>", paragraph(outer)))
                };
                (nested(inside, "Inner"), nested(within, "Inner box"), tail)
            }
            _ => (from, to, tail),
        }
    };
    let version = |part: &str, boxed: &str, tail: &str| {
        if part == "document.xml" {
            return doc_with_body(&format!(
                r#"<w:p><w:r><w:t xml:space="preserve">Before </w:t></w:r>{boxed}</w:p>{}"#,
                plain(tail)
            ));
        }
        let mut doc = if part == "header1.xml" {
            let mut doc = doc_with(&[tail]);
            doc.set_header("Before");
            doc
        } else {
            doc_with_notes(&[(tail, Some("Before"))])
        };
        if boxed.is_empty() {
            return reopen(&mut doc);
        }
        with_part(&mut doc, part, |xml| {
            let at = xml.rfind("<w:r>").unwrap();
            format!("{}{boxed}{}", &xml[..at], &xml[at..])
        })
    };
    let (body, header, notes) = ("document.xml", "header1.xml", "footnotes.xml");
    for (form, change, part, ignore_formatting, boxes_ignored) in [
        ("alternate content", "deleted", body, false, false),
        ("alternate content", "replaced by text", body, true, false),
        ("alternate content", "resized", body, false, false),
        ("alternate content", "anchored", body, true, false),
        ("alternate content", "run formatted", body, true, false),
        ("alternate content", "text beside", body, true, false),
        ("alternate content", "moved", body, false, false),
        ("alternate content", "run formatted", body, false, false),
        ("alternate content", "nested", body, false, false),
        ("drawing", "deleted", body, false, false),
        ("drawing", "anchored", body, false, false),
        ("vml", "resized", body, false, false),
        ("alternate content", "deleted", header, false, false),
        ("alternate content", "edited", notes, false, false),
        ("alternate content", "resized", notes, true, false),
        ("alternate content", "deleted", body, false, true),
        ("alternate content", "nested", body, false, true),
        ("vml", "run formatted", body, true, true),
    ] {
        let case = format!(
            "{form}, {change}, {part}, ignore formatting {ignore_formatting}, boxes ignored {boxes_ignored}"
        );
        let (from, to, tail) = shape(form, change);
        let original = version(part, &from, "Tail.");
        let edited = version(part, &to, &tail);
        let mut options = ComparisonOptions {
            ignore_formatting,
            ..ComparisonOptions::default()
        };
        if boxes_ignored {
            options.ignored_stories.push(ComparisonStoryKind::TextBox);
        }
        let mut doc = version(part, &from, "Tail.");
        let diagnostics = doc
            .compare_with_options(&edited, AUTHOR, DATE, &options)
            .unwrap_or_else(|error| panic!("{case}: {error}"));
        assert!(diagnostics.is_empty(), "{case}: {diagnostics:?}");
        let mut doc = reopen(&mut doc);
        let report = doc.validate().unwrap();
        assert!(report.is_valid(), "{case}: {:?}", report.findings);
        let copies = if form == "alternate content" { 2 } else { 1 };
        let count = |doc: &mut Document, text: &str| part_xml(doc, part).matches(text).count();
        // A box deleted and inserted whole has a drawing id of its own.
        let xml = part_xml(&mut doc, part);
        let mut ids = xml
            .split(r#"<wp:docPr id=""#)
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect::<Vec<_>>();
        let drawings = ids.len();
        assert!(form == "vml" || drawings > 0, "{case}: {xml}");
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), drawings, "{case}: {xml}");
        // The route a row pins: a box compared inside keeps its one
        // alternate content, one replaced whole is deleted and inserted.
        let alternate_contents = match (change, ignore_formatting) {
            ("nested", _) => Some(1),
            ("run formatted", false) => Some(2),
            _ => None,
        };
        if let Some(expected) = alternate_contents {
            let written = xml.matches("<mc:AlternateContent").count();
            assert_eq!(written, expected, "{case}: {xml}");
        }
        if !boxes_ignored {
            let deleted = count(&mut doc, &format!(">{inside}</w:delText>"));
            assert_eq!(deleted, copies, "{case}: {}", part_xml(&mut doc, part));
        }
        // Ignored, a box both versions hold keeps the original's text.
        let texts = |doc: &Document| {
            story_texts(doc)
                .into_iter()
                .filter(|(_, text)| {
                    !boxes_ignored
                        || ![inside, within, "Inner", "Inner box"].contains(&text.as_str())
                })
                .collect::<Vec<_>>()
        };
        let (mut accepted, mut rejected) = (accepted(&mut doc), rejected(&mut doc));
        assert_eq!(texts(&accepted), texts(&edited), "{case}");
        assert_eq!(texts(&rejected), texts(&original), "{case}");
        let mut edited = edited;
        let mut original = original;
        for text in [inside, within] {
            let text = format!(">{text}<");
            if !boxes_ignored {
                assert_eq!(
                    count(&mut accepted, &text),
                    count(&mut edited, &text),
                    "{case}"
                );
            }
            assert_eq!(
                count(&mut rejected, &text),
                count(&mut original, &text),
                "{case}"
            );
        }
    }
}

#[test]
fn a_container_empty_in_one_version_is_refused_with_why() {
    // No tracked change records a block control, a header or a text box
    // itself, and the last mark of their content cannot be taken out, as
    // the body's last cannot: one holding nothing in one version cannot be
    // marked to hold the other's paragraph.
    let control = |content: &str| {
        doc_with_body(&format!(
            r#"<w:sdt><w:sdtPr><w:id w:val="5"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>{}"#,
            plain("After")
        ))
    };
    let header = |content: &str| {
        let mut doc = doc_with(&["Body."]);
        doc.set_header("Head");
        with_part(&mut doc, "header1.xml", |xml| {
            let open = xml.find("<w:hdr").unwrap();
            let open = open + xml[open..].find('>').unwrap() + 1;
            let close = xml.find("</w:hdr>").unwrap();
            format!("{}{content}{}", &xml[..open], &xml[close..])
        })
    };
    let text_box = |content: &str| {
        doc_with_body(&format!(
            "<w:p><w:r><w:t>Boxed </w:t></w:r>{}</w:p>{}",
            crate::support::text_box_run(7, content),
            plain("After")
        ))
    };
    // A note whose reference sits in a tracked change is not paired by id,
    // so it is compared in place, as a story.
    let note = |content: &str| {
        let mut doc = doc_with_notes(&[("Claim.", Some("Source.")), ("Two.", Some("Old"))]);
        let mut doc = with_part_edited(&mut doc, "word/document.xml", |xml| {
            let reference = r#"<w:footnoteReference w:id="2"/></w:r>"#;
            let end = xml.find(reference).unwrap() + reference.len();
            let start = xml[..end].rfind("<w:r>").unwrap();
            format!(
                r#"{}<w:ins w:id="90" w:author="Ada" w:date="{DATE}">{}</w:ins>{}"#,
                &xml[..start],
                &xml[start..end],
                &xml[end..]
            )
        });
        with_part_edited(&mut doc, "word/footnotes.xml", |xml| {
            with_note_edited(xml, "footnote", 3, |_| content.to_owned())
        })
    };
    for version in [
        &control as &dyn Fn(&str) -> Document,
        &header,
        &text_box,
        &note,
    ] {
        for (from, to) in [("", plain("Inside")), (&plain("Inside"), String::new())] {
            let error = version(from)
                .compare(&version(&to), AUTHOR, DATE)
                .unwrap_err();
            assert!(
                error.to_string().contains("docx-compare-empty-container"),
                "{error}"
            );
        }
    }
}

/// Each body paragraph's text with its references to notes of this kind
/// (`footnote` or `endnote`), `[+]` to one holding a paragraph and `[-]`
/// to one holding none; how many normal notes of that kind the part holds;
/// and every story's paragraph texts. Read from the saved XML: the note
/// readers skip a note written self-closing.
fn referenced_notes(
    doc: &mut Document,
    owner: &str,
) -> (Vec<String>, usize, Vec<(StoryKind, String)>) {
    let package = crate::support::package_parts(&doc.to_bytes().unwrap());
    let xml = String::from_utf8_lossy(&package[&format!("word/{owner}s.xml")]).into_owned();
    let notes = xml
        .split(&format!("<w:{owner} "))
        .skip(1)
        .filter(|note| !note[..note.find('>').unwrap()].contains("w:type="))
        .map(|note| {
            let id = note.split(r#"w:id=""#).nth(1).unwrap();
            let id = id[..id.find('"').unwrap()].parse::<i32>().unwrap();
            (id, note.contains("<w:p"))
        })
        .collect::<HashMap<_, _>>();
    let mut texts = Vec::new();
    for paragraph in doc.paragraphs() {
        let mut text = String::new();
        for run in paragraph.runs() {
            for item in run.items() {
                match item {
                    RunItemRef::Text(part) => text.push_str(part),
                    RunItemRef::FootnoteReference(id) | RunItemRef::EndnoteReference(id) => {
                        text.push_str(if notes[&id] { "[+]" } else { "[-]" });
                    }
                    _ => {}
                }
            }
        }
        texts.push(text);
    }
    (texts, notes.len(), story_texts(doc))
}

#[test]
fn a_note_with_no_paragraph_in_one_version_is_compared_whole() {
    // A note written with nothing inside, self-closing or open and empty,
    // added or removed with its reference, or both versions' note empty in
    // one and holding a paragraph in the other: the note goes whole, its
    // custody pair at block level when it has no paragraph to hold it.
    for owner in ["footnote", "endnote"] {
        for written in [String::from("/>"), format!(">\n  </w:{owner}>")] {
            let version = |second: Option<&str>| {
                let paragraphs = [("Claim.", Some("Source.")), ("Two.", second)];
                let mut doc = if owner == "footnote" {
                    doc_with_notes(&paragraphs)
                } else {
                    doc_with_endnote_texts(&paragraphs)
                };
                if second != Some("") {
                    return doc;
                }
                with_part(&mut doc, &format!("{owner}s.xml"), |xml| {
                    let start = xml.rfind(&format!("<w:{owner} ")).unwrap();
                    let open = start + xml[start..].find('>').unwrap();
                    let end = xml.rfind(&format!("</w:{owner}>")).unwrap() + owner.len() + 5;
                    format!("{}{written}{}", &xml[..open], &xml[end..])
                })
            };
            for (case, from, to) in [
                ("added", None, Some("")),
                ("removed", Some(""), None),
                ("emptied", Some("Text."), Some("")),
                ("filled", Some(""), Some("Text.")),
            ] {
                let (mut original, mut edited) = (version(from), version(to));
                let case = format!("{owner} {written:?} {case}");
                let mut doc = redline(reopen(&mut original), &edited);
                let report = doc.validate().unwrap();
                assert!(report.is_valid(), "{case}: {:?}", report.findings);
                for (outcome, mut resolved, wanted) in [
                    ("accepted", accepted(&mut doc), &mut edited),
                    ("rejected", rejected(&mut doc), &mut original),
                ] {
                    assert_eq!(
                        referenced_notes(&mut resolved, owner),
                        referenced_notes(wanted, owner),
                        "{case} {outcome}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_redline_holds_the_styles_and_the_comments_its_content_names() {
    // Content taken from the edited version names styles the original may
    // lack, and a removed comment's markers stay in the deleted content
    // until the deletion is accepted: the redline brings those styles, with
    // the styles they are based on, and keeps that comment.
    let styled = |styled: bool| {
        let mut doc = doc_with(&["Claim."]);
        if styled {
            doc.add_style(StyleBuilder::paragraph("Base", "Base"))
                .unwrap();
            doc.add_style(StyleBuilder::paragraph("Clause", "Clause").based_on("Base"))
                .unwrap();
            doc.add_paragraph("Added.").style("Clause");
        }
        doc
    };
    // A header paragraph in a style whose next style the original lacks too.
    let headed = |styled: bool| {
        let mut doc = doc_with(&["Claim."]);
        doc.set_header("Head");
        if !styled {
            return doc;
        }
        doc.add_style(StyleBuilder::paragraph("After", "After"))
            .unwrap();
        doc.add_style(StyleBuilder::paragraph("Banner", "Banner").next_style("After"))
            .unwrap();
        with_part(&mut doc, "header1.xml", |xml| {
            xml.replacen(
                "<w:r>",
                r#"<w:pPr><w:pStyle w:val="Banner"/></w:pPr><w:r>"#,
                1,
            )
        })
    };
    let commented = |commented: bool| {
        let mut doc = doc_with(&["Claim.", "Two."]);
        if commented {
            let at = |run_index| oxoffice_docx::RunPosition {
                body_index: 0,
                run_index,
            };
            let range = oxoffice_docx::RunRange {
                start: at(0),
                end: at(1),
            };
            doc.add_comment_with_date(
                range,
                "Ada",
                None,
                "Note.",
                oxoffice_docx::CommentDate::Undated,
            )
            .unwrap();
        }
        doc
    };
    for (case, mut original, edited) in [
        ("a style the original lacks", styled(false), styled(true)),
        (
            "a header style and its next style",
            headed(false),
            headed(true),
        ),
        ("the first comment", commented(false), commented(true)),
        ("a comment removed whole", commented(true), commented(false)),
    ] {
        let mut doc = redline(reopen(&mut original), &edited);
        let report = doc.validate().unwrap();
        assert!(report.is_valid(), "{case}: {:?}", report.findings);
        assert_eq!(
            story_texts(&accepted(&mut doc)),
            story_texts(&edited),
            "{case}"
        );
        assert_eq!(
            story_texts(&rejected(&mut doc)),
            story_texts(&original),
            "{case}"
        );
    }
}

#[test]
fn an_empty_text_box_comment_or_note_written_self_closing_is_compared_around() {
    const EMPTY_BOX: &str = concat!(
        r#"<w:r><w:pict xmlns:v="urn:schemas-microsoft-com:vml"><v:shape id="box" style="width:120pt;height:40pt">"#,
        r#"<v:textbox><w:txbxContent/></v:textbox></v:shape></w:pict></w:r>"#
    );
    // Each pair of versions differs by the text of a paragraph around the
    // empty owner, which `part` keeps self-closing once compared.
    let in_body = |text: &str| doc_with_body(&format!("<w:p>{EMPTY_BOX}</w:p>{}", plain(text)));
    let in_header = |text: &str| {
        let mut doc = doc_with(&["Body."]);
        doc.set_header(text);
        with_part(&mut doc, "header1.xml", |xml| {
            xml.replacen("<w:r>", &format!("{EMPTY_BOX}<w:r>"), 1)
        })
    };
    let commented = |text: &str| {
        let mut doc = doc_with(&["Claim.", text]);
        let at = |run_index| oxoffice_docx::RunPosition {
            body_index: 0,
            run_index,
        };
        let range = oxoffice_docx::RunRange {
            start: at(0),
            end: at(1),
        };
        doc.add_comment_with_date(
            range,
            "Ada",
            None,
            "Note.",
            oxoffice_docx::CommentDate::Undated,
        )
        .unwrap();
        with_part(&mut doc, "comments.xml", |xml| {
            let start = xml.find("<w:comment ").unwrap();
            let open = start + xml[start..].find('>').unwrap();
            let end = xml.find("</w:comment>").unwrap() + "</w:comment>".len();
            format!("{}/>{}", &xml[..open], &xml[end..])
        })
    };
    let noted = |owner: &str, text: &str| {
        let paragraphs = [("Claim.", Some("Source.")), (text, None)];
        let mut doc = if owner == "footnote" {
            doc_with_notes(&paragraphs)
        } else {
            doc_with_endnote_texts(&paragraphs)
        };
        with_part(&mut doc, &format!("{owner}s.xml"), |xml| {
            let start = xml.rfind(&format!("<w:{owner} ")).unwrap();
            let open = start + xml[start..].find('>').unwrap();
            let end = xml.rfind(&format!("</w:{owner}>")).unwrap() + owner.len() + 5;
            format!("{}/>{}", &xml[..open], &xml[end..])
        })
    };
    let version = |part: &str, text: &str| match part {
        "document.xml" => in_body(text),
        "header1.xml" => in_header(text),
        "comments.xml" => commented(text),
        notes => noted(notes.trim_end_matches("s.xml"), text),
    };
    for (part, owner) in [
        ("document.xml", "txbxContent"),
        ("header1.xml", "txbxContent"),
        ("comments.xml", "comment"),
        ("footnotes.xml", "footnote"),
        ("endnotes.xml", "endnote"),
    ] {
        let mut doc = redline(version(part, "Before."), &version(part, "After."));
        let package = crate::support::package_parts(&doc.to_bytes().unwrap());
        let xml = String::from_utf8_lossy(&package[&format!("word/{part}")]).into_owned();
        if owner == "txbxContent" {
            assert!(xml.contains("<w:txbxContent/>"), "{part}: {xml}");
        } else {
            assert!(last_owner_is_self_closing(&xml, owner), "{part}: {xml}");
        }
        assert_eq!(
            story_texts(&accepted(&mut doc)),
            story_texts(&version(part, "After.")),
            "{part}"
        );
        assert_eq!(
            story_texts(&rejected(&mut doc)),
            story_texts(&version(part, "Before.")),
            "{part}"
        );
    }
}

// ---- Hyperlinks of whole paragraphs ----
//
// A hyperlink of a paragraph inserted or deleted whole is written as one
// HYPERLINK complex field in a wrapper of its own, as Word writes it: Word
// keeps an emptied w:hyperlink element when it resolves the paragraph away.

/// A paragraph linking `text` to the bookmark `Target`.
fn linked_line(text: &str) -> String {
    format!(
        r#"<w:p><w:hyperlink w:anchor="Target" w:history="1"><w:r><w:rPr><w:rStyle w:val="Hyperlink"/></w:rPr><w:t>{text}</w:t></w:r></w:hyperlink></w:p>"#
    )
}

/// The XML of the one `w:ins` or `w:del` holding the field whose instruction
/// starts with `HYPERLINK`, in `xml`.
fn hyperlink_field_wrapper<'a>(xml: &'a str, kind: &str) -> &'a str {
    let instruction = xml.find(">HYPERLINK").expect("a HYPERLINK field");
    let start = xml[..instruction].rfind(&format!("<w:{kind} ")).unwrap();
    let end = xml[instruction..].find(&format!("</w:{kind}>")).unwrap() + instruction;
    &xml[start..end]
}

#[test]
fn a_linked_paragraph_added_or_removed_whole_is_tracked_as_a_hyperlink_field() {
    let target = r#"<w:p><w:bookmarkStart w:id="0" w:name="Target"/><w:r><w:t>Target section.</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#;
    let without = format!("{}{target}{}", plain("Before."), plain("After."));
    let with = format!(
        "{}{}{target}{}",
        plain("Before."),
        linked_line("See the target."),
        plain("After.")
    );
    for (from, to, kind) in [(&without, &with, "ins"), (&with, &without, "del")] {
        let mut doc = redline(doc_with_body(from), &doc_with_body(to));
        let xml = main_part_xml(&mut doc);
        assert!(!xml.contains("<w:hyperlink"), "{xml}");
        let field = hyperlink_field_wrapper(&xml, kind);
        assert!(field.contains(r#"HYPERLINK \l "Target""#), "{xml}");
        for marker in ["begin", "separate", "end"] {
            assert_eq!(
                field.matches(&format!(r#"fldCharType="{marker}""#)).count(),
                1,
                "{xml}"
            );
        }
        let inner = &field[field.find('>').unwrap()..];
        assert!(
            !inner.contains("<w:ins ") && !inner.contains("<w:del "),
            "{xml}"
        );
        assert_eq!(
            field.contains("<w:delText>See the target.</w:delText>"),
            kind == "del",
            "{xml}"
        );
        assert_eq!(
            field.contains("<w:t>See the target.</w:t>"),
            kind == "ins",
            "{xml}"
        );
    }
    check_both_ways(
        || doc_with_body(&without),
        || doc_with_body(&with),
        "document.xml",
        [false, true],
    );
}

/// `doc` saved with an external hyperlink relationship `id` to `address`
/// added to its main part.
fn with_link_relationship(doc: &mut Document, id: &str, address: &str) -> Document {
    with_part_link_relationship(doc, "/word/document.xml", id, address)
}

/// `doc` saved with an external hyperlink relationship `id` to `address`
/// added to its part `part`.
fn with_part_link_relationship(
    doc: &mut Document,
    part: &str,
    id: &str,
    address: &str,
) -> Document {
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    package
        .get_or_create_part_rels(part)
        .items
        .push(oxoffice_opc::Relationship {
            id: id.to_owned(),
            rel_type:
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink"
                    .to_owned(),
            target: address.to_owned(),
            target_mode: Some("External".to_owned()),
        });
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

#[test]
fn an_address_link_of_a_whole_paragraph_keeps_its_address_tooltip_and_frame() {
    const SITE: &str = "https://example.com/a?b=1&c=2";
    let link = |id: &str, text: &str| {
        format!(
            r#"<w:p><w:hyperlink xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="{id}" w:tooltip="Say &quot;hi&quot; \ bye" w:tgtFrame="_blank" w:history="1"><w:r><w:t>{text}</w:t></w:r></w:hyperlink></w:p>"#
        )
    };
    // The kept link reaches the address through rIdKept in both versions;
    // the other link reaches it through its own relationship (rIdNew), or
    // a new address (rIdElse), both only in the version holding it.
    let kept = link("rIdKept", "Kept link.");
    let build = |body: &str| {
        let mut doc = doc_with_body(body);
        let mut doc = with_link_relationship(&mut doc, "rIdKept", SITE);
        let mut doc = with_link_relationship(&mut doc, "rIdNew", SITE);
        with_link_relationship(&mut doc, "rIdElse", "https://example.org/")
    };
    for (id, address) in [("rIdNew", SITE), ("rIdElse", "https://example.org/")] {
        let added = link(id, "Added link.");
        // Before a paragraph, and as the last one.
        for (without, with) in [
            (
                format!("{kept}{}", plain("After.")),
                format!("{kept}{added}{}", plain("After.")),
            ),
            (kept.clone(), format!("{kept}{added}")),
        ] {
            for (from, to, kind) in [(&without, &with, "ins"), (&with, &without, "del")] {
                let mut doc = redline(build(from), &build(to));
                let xml = main_part_xml(&mut doc);
                assert_eq!(xml.matches("<w:hyperlink").count(), 1, "{xml}");
                let field = hyperlink_field_wrapper(&xml, kind);
                let instruction = format!(
                    r#"HYPERLINK "{}" \o "Say \"hi\" \\ bye" \t "_blank""#,
                    address.replace('&', "&amp;")
                );
                assert!(field.contains(&instruction), "{instruction} in {xml}");
                let links = |doc: &mut Document| {
                    let mut links = doc
                        .links()
                        .into_iter()
                        .map(|link| link.url)
                        .collect::<Vec<_>>();
                    links.sort();
                    links
                };
                assert_eq!(
                    texts(&accepted(&mut doc)),
                    texts(&build(to)),
                    "{from} -> {to}"
                );
                assert_eq!(texts(&rejected(&mut doc)), texts(&build(from)));
                assert_eq!(
                    links(&mut accepted(&mut doc)).len(),
                    links(&mut build(to)).len()
                );
            }
        }
    }
}

#[test]
fn a_contents_entry_added_or_removed_whole_keeps_its_page_field_in_the_link_field() {
    // A contents entry links its heading and shows its page through a
    // PAGEREF field inside the link: both fields go in one wrapper, nested.
    let entry = concat!(
        r#"<w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:hyperlink w:anchor="_Toc1" w:history="1">"#,
        r#"<w:r><w:t>Method</w:t></w:r><w:r><w:tab/></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> PAGEREF _Toc1 \h </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>2</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:hyperlink></w:p>"#
    );
    let heading = r#"<w:p><w:bookmarkStart w:id="0" w:name="_Toc1"/><w:r><w:t>Method</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#;
    let without = format!("{}{heading}", plain("Contents"));
    let with = format!("{}{entry}{heading}", plain("Contents"));
    for (from, to, kind) in [(&without, &with, "ins"), (&with, &without, "del")] {
        let mut doc = redline(doc_with_body(from), &doc_with_body(to));
        let xml = main_part_xml(&mut doc);
        assert!(!xml.contains("<w:hyperlink"), "{xml}");
        let field = hyperlink_field_wrapper(&xml, kind);
        assert!(field.contains(r#"HYPERLINK \l "_Toc1""#), "{xml}");
        assert!(field.contains("PAGEREF _Toc1"), "{xml}");
        assert_eq!(field.matches(r#"fldCharType="begin""#).count(), 2, "{xml}");
        assert_eq!(field.matches(r#"fldCharType="end""#).count(), 2, "{xml}");
        assert!(!field.contains("<w:t>") || kind == "ins", "{xml}");
    }
    check_both_ways(
        || doc_with_body(&without),
        || doc_with_body(&with),
        "document.xml",
        [false, true],
    );
}

#[test]
fn a_linked_paragraph_in_a_note_a_comment_a_header_a_cell_or_a_control_is_tracked_as_a_field() {
    let line = linked_line("See the target.");
    let target = r#"<w:p><w:bookmarkStart w:id="0" w:name="Target"/><w:r><w:t>Target.</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#;
    let held = |xml: &str| xml.contains("HYPERLINK");
    let note = |linked: bool| {
        let mut doc = doc_with_notes(&[("Claim.", Some("Source.")), ("After.", None)]);
        if linked {
            with_part(&mut doc, "footnotes.xml", |xml| {
                let at = xml.find("Source.").unwrap();
                let end = at + xml[at..].find("</w:footnote>").unwrap();
                format!("{}{line}{}", &xml[..end], &xml[end..])
            })
        } else {
            reopen(&mut doc)
        }
    };
    let comment = |linked: bool| {
        let mut doc = doc_with(&["Claim.", "After."]);
        let range = oxoffice_docx::RunRange {
            start: oxoffice_docx::RunPosition {
                body_index: 0,
                run_index: 0,
            },
            end: oxoffice_docx::RunPosition {
                body_index: 0,
                run_index: 1,
            },
        };
        doc.add_comment_with_date(
            range,
            "Ada",
            None,
            "Note.",
            oxoffice_docx::CommentDate::Undated,
        )
        .unwrap();
        if linked {
            with_part(&mut doc, "comments.xml", |xml| {
                xml.replacen("</w:comment>", &format!("{line}</w:comment>"), 1)
            })
        } else {
            reopen(&mut doc)
        }
    };
    // A link to an address, through the comment part's own relationship,
    // which both versions hold.
    let addressed = |linked: bool| {
        let mut doc = comment(false);
        let mut doc = with_part_link_relationship(
            &mut doc,
            "/word/comments.xml",
            "rIdSite",
            "https://example.com/",
        );
        if linked {
            with_part(&mut doc, "comments.xml", |xml| {
                xml.replacen(
                    "</w:comment>",
                    r#"<w:p><w:hyperlink xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="rIdSite"><w:r><w:t>Site.</w:t></w:r></w:hyperlink></w:p></w:comment>"#,
                    1,
                )
            })
        } else {
            doc
        }
    };
    let header = |linked: bool| {
        let mut doc = doc_with(&["Body."]);
        doc.set_header("Running title");
        if linked {
            with_part(&mut doc, "header1.xml", |xml| {
                xml.replacen("</w:hdr>", &format!("{line}</w:hdr>"), 1)
            })
        } else {
            reopen(&mut doc)
        }
    };
    for (owner, suffix) in [
        (&note as &dyn Fn(bool) -> Document, "footnotes.xml"),
        (&comment, "comments.xml"),
        (&addressed, "comments.xml"),
        (&header, "header1.xml"),
    ] {
        for (from, to) in [(false, true), (true, false)] {
            let mut doc = redline(owner(from), &owner(to));
            let xml = part_xml(&mut doc, suffix);
            assert!(held(&xml) && !xml.contains("<w:hyperlink"), "{xml}");
            assert!(!part_xml(&mut accepted(&mut doc), suffix).contains("<w:hyperlink"));
            assert_eq!(
                part_texts(&mut accepted(&mut doc), suffix),
                part_texts(&mut owner(to), suffix)
            );
            assert_eq!(
                part_texts(&mut rejected(&mut doc), suffix),
                part_texts(&mut owner(from), suffix)
            );
        }
    }

    let cell = |content: &str| {
        doc_with_body(&format!(
            r#"<w:tbl><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc>{content}</w:tc></w:tr></w:tbl>{target}"#
        ))
    };
    let control = |content: &str| {
        doc_with_body(&format!(
            r#"<w:sdt><w:sdtPr><w:alias w:val="Box"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>{target}"#
        ))
    };
    for owner in [&cell as &dyn Fn(&str) -> Document, &control] {
        // First and last paragraph of a kept cell or control.
        for with in [
            format!("{line}{}", plain("Inside.")),
            format!("{}{line}", plain("Inside.")),
        ] {
            let mut doc = redline(owner(&plain("Inside.")), &owner(&with));
            let xml = main_part_xml(&mut doc);
            assert!(held(&xml) && !xml.contains("<w:hyperlink"), "{xml}");
            check_both_ways(
                || owner(&plain("Inside.")),
                || owner(&with),
                "document.xml",
                [false, true],
            );
        }
    }
}

#[test]
fn a_whole_paragraph_link_no_field_can_carry_is_refused() {
    // w:docLocation has no HYPERLINK switch: the comparison refuses rather
    // than drop it, and writes nothing.
    let located = r#"<w:p><w:hyperlink w:anchor="Target" w:docLocation="table"><w:r><w:t>See.</w:t></w:r></w:hyperlink></w:p>"#;
    let after = plain("After.");
    for (original, edited) in [
        (after.clone(), format!("{located}{after}")),
        (format!("{located}{after}"), after.clone()),
    ] {
        let mut doc = doc_with_body(&original);
        let error = doc
            .compare(&doc_with_body(&edited), AUTHOR, DATE)
            .expect_err("refused");
        assert!(error.to_string().contains("w:docLocation"), "{error}");
        assert!(doc.revisions().is_empty());
        assert_eq!(texts(&reopen(&mut doc)), texts(&doc_with_body(&original)));
    }
}

/// `Kept four tail` with a comment on `Kept four`, whose end and comment
/// mark are then wrapped in a tracked `wrapper` (`del` or `ins`), as
/// LibreOffice writes a comment that ends on changed text, with `four` in a
/// second wrapper of the same kind.
fn comment_ending_inside_a_revision(wrapper: &str) -> Document {
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t xml:space="preserve">Kept </w:t></w:r><w:r><w:t>four</w:t></w:r><w:r><w:t xml:space="preserve"> tail</w:t></w:r></w:p>"#,
    );
    let range = oxoffice_docx::RunRange {
        start: oxoffice_docx::RunPosition {
            body_index: 0,
            run_index: 0,
        },
        end: oxoffice_docx::RunPosition {
            body_index: 0,
            run_index: 2,
        },
    };
    doc.add_comment_with_date(
        range,
        "Reviewer",
        None,
        "Check",
        oxoffice_docx::CommentDate::Undated,
    )
    .unwrap();
    let bytes = doc.to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    let open = |id: u32| {
        format!(r#"<w:{wrapper} w:id="{id}" w:author="A" w:date="2026-01-01T00:00:00Z">"#)
    };
    let text = if wrapper == "del" { "delText" } else { "t" };
    let mark_start = xml.find("<w:commentRangeEnd").unwrap();
    let mark_end = xml[mark_start..].find("</w:r>").unwrap() + mark_start + "</w:r>".len();
    let four = xml.find("<w:r><w:t>four</w:t></w:r>").unwrap();
    let xml = format!(
        "{}{}<w:r><w:{text}>four</w:{text}></w:r></w:{wrapper}>{}{}</w:{wrapper}>{}",
        &xml[..four],
        open(90),
        open(91),
        &xml[mark_start..mark_end],
        &xml[mark_end..],
    );
    package.set_part(&part, xml.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

/// The text the only comment of `doc` covers, after `resolve`.
fn comment_after(
    doc: &mut Document,
    resolve: fn(&mut Document) -> oxoffice_docx::Result<usize>,
) -> String {
    let mut doc = reopen(doc);
    resolve(&mut doc).unwrap_or_else(|e| panic!("{e}"));
    let doc = reopen(&mut doc);
    let comments = doc.comments();
    assert_eq!(comments.len(), 1);
    comments[0].anchor_text().unwrap().unwrap_or_default()
}

#[test]
fn a_comment_ending_inside_an_accepted_deletion_stays_on_the_text_left() {
    let mut doc = comment_ending_inside_a_revision("del");
    assert_eq!(comment_after(&mut doc, Document::accept_all), "Kept ");
    assert_eq!(comment_after(&mut doc, Document::reject_all), "Kept four");
}

#[test]
fn a_comment_ending_inside_a_rejected_insertion_stays_on_the_text_left() {
    let mut doc = comment_ending_inside_a_revision("ins");
    assert_eq!(comment_after(&mut doc, Document::accept_all), "Kept four");
    assert_eq!(comment_after(&mut doc, Document::reject_all), "Kept ");
}

#[test]
fn every_revision_of_libreoffices_record_is_accepted_with_its_comments_kept() {
    // LibreOffice writes the end and the mark of a comment inside a deletion.
    let mut doc = crate::support::corpus_doc("libreoffice/record.docx");
    let before = doc.comments().len();
    doc.accept_all().unwrap_or_else(|e| panic!("{e}"));
    let doc = reopen(&mut doc);
    assert_eq!(doc.comments().len(), before);
    assert!(
        doc.comments()
            .iter()
            .all(|comment| comment.anchor_text().is_ok_and(|text| text.is_some())),
    );
}

/// `Kept 30 days tail` with a comment on `30 days`, its main part then
/// rewritten by `edit`.
fn comment_on_thirty_days(edit: impl FnOnce(&str) -> String) -> Document {
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t xml:space="preserve">Kept </w:t></w:r><w:r><w:t>30</w:t></w:r><w:r><w:t xml:space="preserve"> days</w:t></w:r><w:r><w:t xml:space="preserve"> tail</w:t></w:r></w:p>"#,
    );
    let range = oxoffice_docx::RunRange {
        start: oxoffice_docx::RunPosition {
            body_index: 0,
            run_index: 1,
        },
        end: oxoffice_docx::RunPosition {
            body_index: 0,
            run_index: 3,
        },
    };
    doc.add_comment_with_date(
        range,
        "Reviewer",
        None,
        "Check",
        oxoffice_docx::CommentDate::Undated,
    )
    .unwrap();
    let bytes = doc.to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    package.set_part(&part, edit(&xml).into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

/// `xml` with `from..to` wrapped in a tracked `wrapper` with this id, its
/// text turned into deleted text for a deletion.
fn wrapped(xml: &str, from: usize, to: usize, wrapper: &str, id: u32) -> String {
    let inner = &xml[from..to];
    let inner = if wrapper == "del" {
        inner
            .replace("<w:t>", "<w:delText>")
            .replace(
                "<w:t xml:space=\"preserve\">",
                "<w:delText xml:space=\"preserve\">",
            )
            .replace("</w:t>", "</w:delText>")
    } else {
        inner.to_owned()
    };
    format!(
        r#"{}<w:{wrapper} w:id="{id}" w:author="A" w:date="2026-01-01T00:00:00Z">{inner}</w:{wrapper}>{}"#,
        &xml[..from],
        &xml[to..]
    )
}

/// The start of the comment through `30`, and ` days` through the comment
/// mark, as two deletions 90 and 91.
fn thirty_days_in_two_deletions(xml: &str) -> String {
    let start = xml.find("<w:commentRangeStart").unwrap();
    let thirty = xml.find("<w:t>30</w:t></w:r>").unwrap() + "<w:t>30</w:t></w:r>".len();
    let days = xml.find("<w:r><w:t xml:space=\"preserve\"> days").unwrap();
    let reference = xml.find("<w:commentReference").unwrap();
    let mark = xml[reference..].find("</w:r>").unwrap() + reference + "</w:r>".len();
    let xml = wrapped(xml, days, mark, "del", 91);
    wrapped(&xml, start, thirty, "del", 90)
}

#[test]
fn a_comment_whose_range_two_accepted_deletions_hold_goes_with_them() {
    let mut doc = comment_on_thirty_days(thirty_days_in_two_deletions);
    doc.accept_all().unwrap_or_else(|e| panic!("{e}"));
    let doc = reopen(&mut doc);
    assert!(doc.comments().is_empty());
    assert_eq!(texts(&doc), ["Kept  tail"]);
}

#[test]
fn accepting_only_the_deletion_with_a_comment_end_keeps_the_comment() {
    // The start stays in the other deletion, which is not resolved.
    let mut doc = comment_on_thirty_days(thirty_days_in_two_deletions);
    doc.accept_revision_id(91).unwrap_or_else(|e| panic!("{e}"));
    let doc = reopen(&mut doc);
    assert_eq!(doc.comments().len(), 1);
    assert!(doc.comments()[0].anchor_text().is_ok());
}

#[test]
fn a_comment_end_in_a_deletion_inside_a_rejected_insertion_stays() {
    // ` days`, the end and the mark were inserted, then the end and the mark
    // deleted by someone else.
    let mut doc = comment_on_thirty_days(|xml| {
        let end = xml.find("<w:commentRangeEnd").unwrap();
        let reference = xml.find("<w:commentReference").unwrap();
        let mark = xml[reference..].find("</w:r>").unwrap() + reference + "</w:r>".len();
        let xml = wrapped(xml, end, mark, "del", 93);
        let days = xml.find("<w:r><w:t xml:space=\"preserve\"> days").unwrap();
        let close = xml.find("</w:del>").unwrap() + "</w:del>".len();
        wrapped(&xml, days, close, "ins", 92)
    });
    assert_eq!(comment_after(&mut doc, Document::reject_all), "30");
}

// ---- Compare after an edit, on every producer's copy ----
// A tool edits one paragraph of a file a producer saved, then hands the
// reader a redline of the two versions. Comparing a file with itself passes
// whatever it holds: a refusal shows only once a paragraph near the construct
// that triggers it changed. So each kind of paragraph a copy holds is edited
// on its own, and each edit is compared, accepted, rejected and validated.

/// The kinds of paragraph the walk edits, in the order it reports them.
const PARAGRAPH_KINDS: [&str; 14] = [
    "plain",
    "field",
    "hyperlink",
    "inline control",
    "block control",
    "table of contents",
    "table cell",
    "header",
    "footer",
    "footnote",
    "endnote",
    "comment anchor",
    "bookmark",
    "text box",
];

/// The word every edit writes, in no corpus file.
const EDITED_WORD: &str = "zephyr";

/// The kinds of a paragraph, from where it sits and the XML it holds. A
/// paragraph holding a tracked change is left out: the edit walks compare
/// what a producer wrote, not revisions a reviewer left pending.
fn paragraph_kinds(doc: &Document, paragraph: &StoryParagraph) -> Vec<&'static str> {
    let Ok(xml) = doc.paragraph_element_xml(&paragraph.location) else {
        return Vec::new();
    };
    let xml = String::from_utf8_lossy(&xml);
    let holds =
        |tag: &str| xml.contains(&format!("<w:{tag} ")) || xml.contains(&format!("<w:{tag}>"));
    if ["ins", "del", "moveFrom", "moveTo", "rPrChange", "pPrChange"]
        .iter()
        .any(|tag| holds(tag))
    {
        return Vec::new();
    }
    let story = paragraph.location.story().kind();
    let in_block_control = paragraph.location.index_path().len() > 1
        && matches!(story, StoryKind::Body | StoryKind::TableCell);
    let toc = xml.contains("PAGEREF") || (in_block_control && xml.contains("w:anchor="));
    let field = holds("fldChar") || holds("fldSimple");
    let hyperlink = holds("hyperlink");
    let inline_control = holds("sdt");
    let comment = holds("commentRangeStart") || holds("commentReference");
    let bookmark = holds("bookmarkStart");
    let mut kinds = Vec::new();
    let structural = [
        (story == StoryKind::TableCell, "table cell"),
        (story == StoryKind::Header, "header"),
        (story == StoryKind::Footer, "footer"),
        (story == StoryKind::Footnote, "footnote"),
        (story == StoryKind::Endnote, "endnote"),
        (story == StoryKind::TextBox, "text box"),
        (toc, "table of contents"),
        (in_block_control && !toc, "block control"),
        (field && !toc, "field"),
        (hyperlink && !toc, "hyperlink"),
        (inline_control, "inline control"),
        (comment, "comment anchor"),
        (bookmark && !toc, "bookmark"),
    ];
    kinds.extend(
        structural
            .iter()
            .filter(|(has, _)| *has)
            .map(|(_, kind)| *kind),
    );
    let other = [
        "footnoteReference",
        "endnoteReference",
        "drawing",
        "pict",
        "object",
    ]
    .iter()
    .any(|tag| holds(tag))
        || xml.contains("<m:oMath");
    if kinds.is_empty() && story == StoryKind::Body && !other {
        kinds.push("plain");
    }
    kinds
}

/// The words of `text` an edit may replace: letters only, four or more, each
/// found once in the paragraph so that a counted replacement takes it.
fn replaceable_words(text: &str) -> Vec<&str> {
    let mut words = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|word| word.chars().count() >= 4 && text.matches(*word).count() == 1)
        .collect::<Vec<_>>();
    words.dedup();
    words
}

/// A corpus copy the walk edits: its bytes, its text with every revision it
/// holds rejected, and the validation errors it already has.
struct EditedCopy {
    source: Vec<u8>,
    rejected: Vec<(StoryKind, String)>,
    errors: Vec<String>,
}

impl EditedCopy {
    fn new(source: Vec<u8>) -> Self {
        let mut doc = Document::from_bytes(&source).expect("copy opens");
        let errors = validation_errors(&doc);
        let rejected = story_texts(&rejected(&mut doc));
        Self {
            source,
            rejected,
            errors,
        }
    }
}

fn validation_errors(doc: &Document) -> Vec<String> {
    doc.validate()
        .expect("validation runs")
        .findings
        .into_iter()
        .filter(|finding| finding.severity == ValidationSeverity::Error)
        .map(|finding| finding.message)
        .collect()
}

/// One edit of the walk: the word replaced in a paragraph, by its place in
/// the story paragraphs. Its location is read again before the edit: a
/// location goes stale once its story changed.
type Edit = (usize, String);

/// The first edit of a paragraph of this kind that a counted replacement
/// takes, among the first few paragraphs and words. `paragraphs` come with
/// their kinds.
fn edit_of_kind(
    copy: &EditedCopy,
    paragraphs: &[(StoryParagraph, Vec<&'static str>)],
    kind: &str,
) -> Option<Edit> {
    paragraphs
        .iter()
        .enumerate()
        .filter(|(_, (_, kinds))| kinds.contains(&kind))
        .take(4)
        .flat_map(|(at, (paragraph, _))| {
            replaceable_words(&paragraph.text)
                .into_iter()
                .take(4)
                .map(move |word| (at, paragraph.location.clone(), word.to_owned()))
        })
        .find(|(_, location, word)| {
            let mut edited = Document::from_bytes(&copy.source).expect("copy opens");
            matches!(
                edited.try_replace_text_at(location, word, EDITED_WORD, Some(1)),
                Ok(Ok(1))
            )
        })
        .map(|(at, _, word)| (at, word))
}

/// Makes `edits` on the copy, compares the original with the edited copy
/// and checks the redline: it is as valid as the copy, and resolves to
/// either version (the revisions the copy already holds resolve
/// with the new ones). What failed, if anything.
fn compare_after_edits(copy: &EditedCopy, edits: &[&Edit]) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(|| {
        let mut edited = Document::from_bytes(&copy.source).expect("copy opens");
        for (at, word) in edits {
            let location = edited.story_paragraphs().expect("story paragraphs")[*at]
                .location
                .clone();
            let replaced = edited.try_replace_text_at(&location, word, EDITED_WORD, Some(1));
            assert!(
                matches!(replaced, Ok(Ok(1))),
                "the edit of {word:?} is refused: {replaced:?}"
            );
        }
        let mut edited = reopen(&mut edited);
        let mut original = Document::from_bytes(&copy.source).unwrap();
        let diagnostics = original
            .compare(&edited, AUTHOR, DATE)
            .unwrap_or_else(|error| panic!("compare: {error}"));
        assert!(
            diagnostics.is_empty(),
            "compare diagnostics: {diagnostics:?}"
        );
        // Accepting gives the edited copy, so the redline holds the edit.
        let mut redline = reopen(&mut original);
        assert!(
            !redline
                .story_revisions()
                .expect("revisions read")
                .is_empty(),
            "the redline holds no change"
        );
        let new = validation_errors(&redline)
            .into_iter()
            .filter(|error| !copy.errors.contains(error))
            .collect::<Vec<_>>();
        assert!(new.is_empty(), "the redline is invalid: {new:?}");
        assert!(
            story_texts(&accepted(&mut redline)) == story_texts(&accepted(&mut edited)),
            "accepting every change does not give the edited copy"
        );
        assert!(
            story_texts(&rejected(&mut redline)) == copy.rejected,
            "rejecting every change does not give the original"
        );
    }))
    .map_err(|panic| {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_default()
    })
}

#[test]
fn a_paragraph_of_each_kind_edited_in_a_producer_copy_compares_and_resolves_to_either_version() {
    // One copy on a pull request; every Word copy of every producer but the
    // long ones with `OXOFFICE_WORKFLOWS=full`.
    let mut copies = crate::support::every_short_docx_copy();
    if !crate::support::every_copy() {
        copies.retain(|copy| copy == COMPARE_AFTER_EDIT_PR_COPY);
    }
    crate::support::run_workflow("compare-after-edit", &copies, |path| {
        let copy = EditedCopy::new(crate::support::workflow_source(path));
        let doc = Document::from_bytes(&copy.source).expect("copy opens");
        let paragraphs = doc
            .story_paragraphs()
            .expect("story paragraphs")
            .into_iter()
            .map(|paragraph| {
                let kinds = paragraph_kinds(&doc, &paragraph);
                (paragraph, kinds)
            })
            .collect::<Vec<_>>();
        let edits = PARAGRAPH_KINDS
            .iter()
            .filter_map(|kind| edit_of_kind(&copy, &paragraphs, kind).map(|edit| (*kind, edit)))
            .collect::<Vec<_>>();
        assert!(!edits.is_empty(), "{path}: no paragraph was edited");
        // Every kind's edit in one comparison; each on its own only when that
        // one fails, to name the kinds that fail. A comparison reads around
        // what an edit touched, so the edits do not hide each other.
        let mut distinct = Vec::<&Edit>::new();
        for (_, edit) in &edits {
            if !distinct.contains(&edit) {
                distinct.push(edit);
            }
        }
        let joint = compare_after_edits(&copy, &distinct);
        let failures = if joint.is_ok() {
            Vec::new()
        } else {
            edits
                .iter()
                .filter_map(|(kind, edit)| {
                    compare_after_edits(&copy, &[edit])
                        .err()
                        .map(|error| format!("{kind} ({error})"))
                })
                .collect::<Vec<_>>()
        };
        let kinds = edits.iter().map(|(kind, _)| *kind).collect::<Vec<_>>();
        assert!(
            failures.is_empty(),
            "{path}: kinds failing: {}. Edited: {}.",
            failures.join("; "),
            kinds.join(", ")
        );
        assert!(
            joint.is_ok(),
            "{path}: the edits of every kind fail together: {joint:?}"
        );
    });
}

/// The copy a pull request walks: a page written in Google Docs, with a
/// paragraph of most kinds, which compares in seconds.
const COMPARE_AFTER_EDIT_PR_COPY: &str = "google-docs/review.docx";
