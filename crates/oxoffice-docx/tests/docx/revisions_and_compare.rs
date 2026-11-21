//! Tracked changes: the track revisions setting, reading revisions (kind,
//! author, date, story, text), accepting and rejecting them, and comparing two
//! versions of a document into a redline that resolves back to either one.

use std::collections::HashMap;

use oxoffice_docx::{
    Alignment, ComparisonGranularity, ComparisonOptions, ComparisonStoryKind, Document,
    RevisionKind, RunItemRef, StoryKind,
};

use crate::support::{corpus_doc, doc_with, doc_with_body, reopen, texts};

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

#[test]
fn edits_through_the_facade_apply_directly_even_while_tracking_is_on() {
    // The setting asks Word to record the next reviewer's edits. The facade
    // does not record its own: a redline comes from `compare`.
    let mut doc = doc_with(&["first"]);
    doc.set_track_revisions(true).unwrap();
    doc.paragraph_mut(0).unwrap().set_text("changed").unwrap();
    doc.add_paragraph("added");
    let doc = reopen(&mut doc);
    assert_eq!(doc.track_revisions(), Some(true));
    assert!(doc.revisions().is_empty());
    assert_eq!(texts(&doc), ["changed", "added"]);
}

// ---- Reading revisions ----

#[test]
fn each_kind_of_tracked_change_is_read_with_its_author_and_date() {
    let mut doc = doc_with_body(EVERY_KIND);
    let doc = reopen(&mut doc);
    let read: Vec<_> = doc
        .revisions()
        .iter()
        .map(|r| {
            (
                r.id(),
                r.kind(),
                r.author().to_owned(),
                r.timestamp().map(str::to_owned),
            )
        })
        .collect();
    let on = |id, kind, author: &str, day: Option<u32>| {
        let date = day.map(|day| format!("2026-01-0{day}T00:00:00Z"));
        (id, kind, author.to_owned(), date)
    };
    assert_eq!(
        read,
        [
            on(1, RevisionKind::Insertion, "Ada", Some(1)),
            on(2, RevisionKind::Deletion, "Ben", Some(2)),
            on(3, RevisionKind::RunPropertyChange, "Cy", Some(3)),
            // A revision without a date reads as no timestamp.
            on(4, RevisionKind::ParagraphPropertyChange, "Dee", None),
            on(5, RevisionKind::MoveFrom, "Eve", Some(5)),
            on(6, RevisionKind::MoveTo, "Eve", Some(5)),
        ]
    );
}

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

#[test]
fn a_deleted_paragraph_mark_joins_the_paragraph_to_the_next_one() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:pPr><w:rPr><w:del w:id="1" w:author="Ada" w:date="2026-01-01T00:00:00Z"/></w:rPr></w:pPr>"#,
        r#"<w:r><w:t xml:space="preserve">Joined </w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>tail</w:t></w:r></w:p>"#,
    ));
    assert_eq!(doc.text(), "Joined tail\n");
    assert_eq!(texts(&accepted(&mut doc)), ["Joined tail"]);
    assert_eq!(texts(&rejected(&mut doc)), ["Joined ", "tail"]);
}

#[test]
fn revisions_in_headers_and_footnotes_are_listed_with_their_story_and_resolved() {
    let mut original = doc_with_notes(&[("Body.", Some("Old note"))]);
    original.set_header("Old header");
    let mut edited = doc_with_notes(&[("Body.", Some("New note"))]);
    edited.set_header("New header");
    let mut doc = redline(original, &edited);

    // `revisions` reads the main document only; `story_revisions` reads them all.
    assert!(doc.revisions().is_empty());
    let listed: Vec<_> = doc
        .story_revisions()
        .unwrap()
        .into_iter()
        .map(|r| (r.story().kind(), r.kind(), r.text().to_owned()))
        .collect();
    let change = |story, kind, text: &str| (story, kind, text.to_owned());
    assert_eq!(
        listed,
        [
            change(StoryKind::Header, RevisionKind::Deletion, "Old header"),
            change(StoryKind::Header, RevisionKind::Insertion, "New header"),
            change(StoryKind::Footnote, RevisionKind::Deletion, "Old note"),
            change(StoryKind::Footnote, RevisionKind::Insertion, "New note"),
        ]
    );

    let mut copy = reopen(&mut doc);
    assert_eq!(copy.accept_all().unwrap(), 4, "one count for every story");
    let copy = reopen(&mut copy);
    assert_eq!(copy.header_text().as_deref(), Some("New header"));
    assert_eq!(copy.footnotes()[0].1.trim(), "New note");
    let copy = rejected(&mut doc);
    assert_eq!(copy.header_text().as_deref(), Some("Old header"));
    assert_eq!(copy.footnotes()[0].1.trim(), "Old note");
}

// ---- Accepting and rejecting ----

#[test]
fn accepting_all_keeps_insertions_and_drops_deletions() {
    let mut doc = doc_with_body(EVERY_KIND);
    let mut copy = reopen(&mut doc);
    assert_eq!(copy.accept_all().unwrap(), 6);
    let copy = reopen(&mut copy);
    assert!(copy.revisions().is_empty());
    assert!(copy.story_revisions().unwrap().is_empty());
    assert_eq!(
        texts(&copy),
        [
            "kept added",
            "bolded",
            "centred",
            "",
            "middle",
            "travelling"
        ]
    );

    // A document without revisions resolves none.
    let mut plain = doc_with(&["nothing tracked"]);
    assert_eq!(plain.accept_all().unwrap(), 0);
    assert_eq!(plain.reject_all().unwrap(), 0);
    assert_eq!(texts(&plain), ["nothing tracked"]);
}

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
fn accepting_a_format_change_keeps_the_new_format_and_rejecting_restores_the_old() {
    let mut doc = doc_with_body(EVERY_KIND);
    let bold = |doc: &Document| doc.paragraph(1).unwrap().runs().next().unwrap().is_bold();

    let copy = accepted(&mut doc);
    assert!(bold(&copy));
    assert_eq!(
        copy.paragraph(2).unwrap().alignment(),
        Some(Alignment::Center)
    );

    let copy = rejected(&mut doc);
    assert!(!bold(&copy));
    assert_eq!(copy.paragraph(2).unwrap().alignment(), None);
}

#[test]
fn accepting_a_move_keeps_the_text_at_its_destination_and_rejecting_puts_it_back() {
    // A whole paragraph moved after `middle`, its paragraph marks included.
    let mark = |kind: &str, id: u32| {
        format!(
            r#"<w:pPr><w:rPr><w:{kind} w:id="{id}" w:author="Eve" w:date="{DATE}"/></w:rPr></w:pPr>"#
        )
    };
    let mut doc = doc_with_body(&format!(
        concat!(
            r#"<w:p>{}<w:moveFromRangeStart w:id="10" w:author="Eve" w:date="{date}" w:name="move1"/>"#,
            r#"<w:moveFrom w:id="1" w:author="Eve" w:date="{date}"><w:r><w:t>travelling</w:t></w:r></w:moveFrom>"#,
            r#"<w:moveFromRangeEnd w:id="10"/></w:p>"#,
            r#"<w:p><w:r><w:t>middle</w:t></w:r></w:p>"#,
            r#"<w:p>{}<w:moveToRangeStart w:id="11" w:author="Eve" w:date="{date}" w:name="move1"/>"#,
            r#"<w:moveTo w:id="3" w:author="Eve" w:date="{date}"><w:r><w:t>travelling</w:t></w:r></w:moveTo>"#,
            r#"<w:moveToRangeEnd w:id="11"/></w:p>"#,
            r#"<w:p><w:r><w:t>end</w:t></w:r></w:p>"#,
        ),
        mark("moveFrom", 2),
        mark("moveTo", 4),
        date = DATE,
    ));
    assert!(kinds(&doc).contains(&RevisionKind::MoveFrom));
    assert!(kinds(&doc).contains(&RevisionKind::MoveTo));
    assert_eq!(texts(&accepted(&mut doc)), ["middle", "travelling", "end"]);
    assert_eq!(texts(&rejected(&mut doc)), ["travelling", "middle", "end"]);
}

#[test]
fn a_deleted_row_goes_on_accept_and_an_inserted_row_goes_on_reject() {
    let row = |mark: &str, text: &str| {
        let (property, content) = match mark {
            "del" => (
                format!(r#"<w:trPr><w:del w:id="1" w:author="Ada" w:date="{DATE}"/></w:trPr>"#),
                format!(
                    r#"<w:del w:id="2" w:author="Ada" w:date="{DATE}"><w:r><w:delText>{text}</w:delText></w:r></w:del>"#
                ),
            ),
            "ins" => (
                format!(r#"<w:trPr><w:ins w:id="3" w:author="Ada" w:date="{DATE}"/></w:trPr>"#),
                format!(
                    r#"<w:ins w:id="4" w:author="Ada" w:date="{DATE}"><w:r><w:t>{text}</w:t></w:r></w:ins>"#
                ),
            ),
            _ => (String::new(), format!("<w:r><w:t>{text}</w:t></w:r>")),
        };
        format!("<w:tr>{property}<w:tc><w:p>{content}</w:p></w:tc></w:tr>")
    };
    let mut doc = doc_with_body(&format!(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>{}{}{}</w:tbl><w:p/>"#,
        row("", "KEEP"),
        row("del", "GONE"),
        row("ins", "NEW"),
    ));
    let cells = |doc: &Document| {
        let table = doc.table(0).unwrap();
        (0..table.row_count())
            .map(|row| table.cell(row, 0).unwrap().text())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        doc.text(),
        "KEEP\t\nNEW\t\n\n",
        "the reader skips the deleted row"
    );
    assert_eq!(cells(&accepted(&mut doc)), ["KEEP", "NEW"]);
    assert_eq!(cells(&rejected(&mut doc)), ["KEEP", "GONE"]);
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
fn comparing_identical_documents_records_nothing() {
    let mut doc = doc_with(&["same", "text"]);
    let diagnostics = doc
        .compare(&doc_with(&["same", "text"]), AUTHOR, DATE)
        .unwrap();
    assert!(diagnostics.is_empty());
    let doc = reopen(&mut doc);
    assert!(doc.story_revisions().unwrap().is_empty());
    assert_eq!(texts(&doc), ["same", "text"]);
}

#[test]
fn a_changed_paragraph_is_recorded_as_a_deletion_and_an_insertion_by_the_given_author() {
    let doc = redline(
        doc_with(&["The quick brown fox", "jumps"]),
        &doc_with(&["The quick red fox", "jumps"]),
    );
    assert_eq!(
        kinds(&doc),
        [RevisionKind::Deletion, RevisionKind::Insertion]
    );
    assert!(
        doc.revisions()
            .iter()
            .all(|r| r.author() == AUTHOR && r.timestamp() == Some(DATE))
    );
    // The default granularity replaces the whole run.
    assert_eq!(
        revision_texts(&doc, RevisionKind::Deletion),
        ["The quick brown fox"]
    );
    assert_eq!(
        revision_texts(&doc, RevisionKind::Insertion),
        ["The quick red fox"]
    );
    assert_eq!(texts(&doc), ["The quick red fox", "jumps"]);
}

#[test]
fn a_redline_accepts_to_the_edited_version_and_rejects_to_the_original() {
    let original = [
        "Keep this.",
        "Change this sentence.",
        "Remove this.",
        "Keep that.",
    ];
    let edited = [
        "Keep this.",
        "Change this phrase.",
        "Keep that.",
        "Add this.",
    ];
    let mut doc = redline(doc_with(&original), &doc_with(&edited));
    assert!(kinds(&doc).contains(&RevisionKind::Insertion));
    assert!(kinds(&doc).contains(&RevisionKind::Deletion));
    assert_eq!(texts(&accepted(&mut doc)), edited);
    assert_eq!(texts(&rejected(&mut doc)), original);
}

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
fn a_moved_paragraph_is_recorded_as_a_move() {
    let original = [
        "Alpha paragraph.",
        "Beta paragraph.",
        "Gamma paragraph.",
        "Delta paragraph.",
    ];
    let edited = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "Delta paragraph.",
        "Alpha paragraph.",
    ];
    let mut doc = redline(doc_with(&original), &doc_with(&edited));
    assert_eq!(kinds(&doc), [RevisionKind::MoveFrom, RevisionKind::MoveTo]);
    assert_eq!(
        revision_texts(&doc, RevisionKind::MoveTo),
        ["Alpha paragraph."]
    );
    assert_eq!(texts(&accepted(&mut doc)), edited);
    assert_eq!(texts(&rejected(&mut doc)), original);
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
fn a_changed_cell_and_an_added_row_are_tracked_in_the_table() {
    let table_doc = |rows: &[[&str; 2]]| {
        let mut doc = doc_with(&["before"]);
        let mut table = doc.add_table(rows.len(), 2);
        for (r, row) in rows.iter().enumerate() {
            for (c, text) in row.iter().enumerate() {
                table.cell(r, c).unwrap().set_text(text);
            }
        }
        doc.add_paragraph("after");
        doc
    };
    let original = [["a", "b"], ["c", "d"]];
    let edited = [["a", "B"], ["c", "d"], ["e", "f"]];
    let mut doc = redline(table_doc(&original), &table_doc(&edited));
    let stories: Vec<_> = doc
        .story_revisions()
        .unwrap()
        .iter()
        .map(|r| r.story().kind())
        .collect();
    assert!(
        stories.iter().all(|kind| *kind == StoryKind::Body),
        "{stories:?}"
    );
    assert_eq!(revision_texts(&doc, RevisionKind::Deletion), ["b"]);
    assert_eq!(
        revision_texts(&doc, RevisionKind::Insertion),
        ["B", "e", "f"]
    );
    assert_eq!(
        accepted(&mut doc).text(),
        "before\na\tB\t\nc\td\t\ne\tf\t\nafter\n"
    );
    assert_eq!(rejected(&mut doc).text(), "before\na\tb\t\nc\td\t\nafter\n");
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
fn an_invalid_timestamp_is_refused_and_leaves_the_document_unchanged() {
    let mut doc = doc_with(&["old"]);
    let before = doc.to_bytes().unwrap();
    let error = doc
        .compare(&doc_with(&["new"]), AUTHOR, "yesterday")
        .unwrap_err();
    assert!(error.to_string().contains("timestamp"), "{error}");
    assert!(doc.revisions().is_empty());
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn author_names_with_markup_characters_and_offset_dates_survive_the_redline() {
    let author = "Ada & \"Bob\" <QA>";
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

#[test]
fn comparing_a_redline_again_keeps_its_earlier_revisions() {
    let mut first = redline(
        doc_with(&["Reviewed once.", "Edited later."]),
        &doc_with(&["Reviewed twice.", "Edited later."]),
    );
    let mut edited = reopen(&mut first);
    assert_eq!(
        edited.replace_all(&HashMap::from([("Edited later.", "Edited again.")])),
        1
    );
    first.compare(&edited, "Second reviewer", DATE).unwrap();
    let mut doc = reopen(&mut first);
    let authors: Vec<_> = doc
        .revisions()
        .iter()
        .map(|r| r.author().to_owned())
        .collect();
    assert_eq!(
        authors,
        [AUTHOR, AUTHOR, "Second reviewer", "Second reviewer"]
    );
    assert_eq!(
        texts(&accepted(&mut doc)),
        ["Reviewed twice.", "Edited again."]
    );
    assert_eq!(
        texts(&rejected(&mut doc)),
        ["Reviewed once.", "Edited later."]
    );
}

// ---- Comparing editors' files ----
// A comparison of the corpus report costs seconds in a debug build, so these
// tests compare once and read the redline back through its resolved copy.

/// The Word-saved report and a copy with "Lyon" renamed and a paragraph
/// added, already compared into a redline of the first.
fn word_report_redline() -> (Document, Document) {
    let mut original = corpus_doc("word/report.docx");
    let mut edited = corpus_doc("word/report.docx");
    let renamed = edited.replace_all(&HashMap::from([("Lyon", "Marseille")]));
    assert_eq!(renamed, 3);
    edited.add_paragraph("A closing line.");
    let diagnostics = original.compare(&edited, AUTHOR, DATE).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    (original, edited)
}

#[test]
fn a_redline_of_two_word_saved_reports_accepts_to_the_edited_copy() {
    let (mut doc, edited) = word_report_redline();
    assert!(
        doc.revisions()
            .iter()
            .all(|r| r.author() == AUTHOR && r.timestamp() == Some(DATE))
    );
    let changes = doc.story_revisions().unwrap();
    let covers = |kind, word| {
        changes
            .iter()
            .any(|r| r.kind() == kind && r.text().contains(word))
    };
    assert!(covers(RevisionKind::Deletion, "Lyon"));
    assert!(covers(RevisionKind::Insertion, "Marseille"));
    let copy = accepted(&mut doc);
    assert_eq!(copy.text(), edited.text());
    assert_eq!(
        copy.comments().len(),
        1,
        "the comment of the report is kept"
    );
}

#[test]
fn a_redline_of_two_word_saved_reports_rejects_to_the_original() {
    let (mut doc, _) = word_report_redline();
    let text = corpus_doc("word/report.docx").text();
    assert_eq!(rejected(&mut doc).text(), text);
}

#[test]
fn a_redline_of_two_google_saved_reports_rejects_to_the_original() {
    let mut doc = corpus_doc("google/report.docx");
    let text = doc.text();
    let mut edited = corpus_doc("google/report.docx");
    let replaced = edited.replace_all(&HashMap::from([("unaudited", "audited")]));
    assert_eq!(replaced, 1);
    assert!(doc.compare(&edited, AUTHOR, DATE).unwrap().is_empty());
    assert!(!doc.revisions().is_empty());
    assert_eq!(rejected(&mut doc).text(), text);
}

// ---- Known failures ----

// Fails today: a textless run whose properties are written `<w:rPr></w:rPr>`,
// as LibreOffice writes its empty paragraphs, blocks any comparison of the
// file. Issue: #32
#[test]
#[should_panic(expected = "comparison acceptance does not reproduce the edited stories")]
fn an_empty_run_written_by_libreoffice_does_not_block_the_comparison() {
    let body = r#"<w:p><w:r><w:t>first</w:t></w:r></w:p><w:p><w:r><w:rPr></w:rPr></w:r></w:p>"#;
    let mut edited = doc_with_body(body);
    edited.add_paragraph("added");
    let mut doc = redline(doc_with_body(body), &edited);
    assert_eq!(texts(&accepted(&mut doc)), ["first", "", "added"]);
}

// Fails today: the LibreOffice copy of the corpus report refuses any edited
// version, even one appended paragraph. Issue: #32
#[test]
#[should_panic(expected = "comparison acceptance does not reproduce the edited stories")]
fn a_libreoffice_report_compares_with_an_edited_copy() {
    let original = texts(&corpus_doc("libreoffice/report.docx"));
    let mut edited = corpus_doc("libreoffice/report.docx");
    edited.add_paragraph("A closing remark.");
    let mut doc = redline(corpus_doc("libreoffice/report.docx"), &edited);
    let mut expected = original.clone();
    expected.push("A closing remark.".to_owned());
    assert_eq!(texts(&accepted(&mut doc)), expected);
    assert_eq!(texts(&rejected(&mut doc)), original);
}

// Fails today: a paragraph appended after a table that ends the body is
// refused. Issue: #32
#[test]
#[should_panic(expected = "comparison needs an adjacent paragraph for a final paragraph change")]
fn a_paragraph_appended_after_a_final_table_is_compared() {
    let table_last = || {
        let mut doc = doc_with(&["intro"]);
        doc.add_table(1, 1).cell(0, 0).unwrap().set_text("cell");
        doc
    };
    let mut edited = table_last();
    edited.add_paragraph("after the table");
    let mut doc = redline(table_last(), &edited);
    assert_eq!(texts(&accepted(&mut doc)), ["intro", "after the table"]);
    assert_eq!(texts(&rejected(&mut doc)), ["intro"]);
}

#[test]
fn renamed_toc_heading_bookmarks_do_not_block_the_comparison() {
    let heading = |toc: u32| {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="_Toc{toc}"/><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#
        )
    };
    let body = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let original = doc_with_body(&format!("{}{}", heading(100), body("Old body.")));
    let edited = doc_with_body(&format!("{}{}", heading(200), body("New body.")));
    let mut doc = redline(original, &edited);
    assert_eq!(texts(&accepted(&mut doc)), ["Intro", "New body."]);
    assert_eq!(texts(&rejected(&mut doc)), ["Intro", "Old body."]);
}

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
fn a_toc_rebuilt_under_new_bookmark_names_shows_only_the_text_changes() {
    let original = report_with_toc(217_000_001, &[("Scope", "Old."), ("Method", "Steps.")]);
    let edited = report_with_toc(
        318_000_001,
        &[("Scope and aims", "Old."), ("Method", "Steps.")],
    );
    let mut doc = redline(
        report_with_toc(217_000_001, &[("Scope", "Old."), ("Method", "Steps.")]),
        &edited,
    );
    // The entry and its heading, nothing else.
    assert_eq!(
        revision_texts(&doc, RevisionKind::Deletion),
        ["Scope", "Scope"]
    );
    assert_eq!(
        revision_texts(&doc, RevisionKind::Insertion),
        ["Scope and aims", "Scope and aims"]
    );
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_every_entry_links_its_heading(&copy, &["Scope and aims", "Method"]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&original));
    assert_eq!(bookmark_names(&copy), ["_Toc217000001", "_Toc217000002"]);
}

#[test]
fn bookmark_ids_renumbered_by_a_save_are_no_change() {
    let mark = |id: u32, name: &str, text: &str| {
        format!(
            r#"<w:p><w:bookmarkStart w:id="{id}" w:name="{name}"/><w:r><w:t>{text}</w:t></w:r><w:bookmarkEnd w:id="{id}"/></w:p>"#
        )
    };
    let body = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    // Word numbers bookmarks in document order when it saves, so a bookmark
    // added first moves the id of every bookmark after it.
    let original = doc_with_body(&format!("{}{}", mark(0, "intro", "Intro"), body("Old.")));
    let edited = doc_with_body(&format!(
        "{}{}{}",
        mark(0, "summary", "Summary"),
        mark(1, "intro", "Intro"),
        body("New.")
    ));
    let mut doc = redline(original, &edited);
    assert_eq!(
        revision_texts(&doc, RevisionKind::Insertion),
        ["Summary", "New."]
    );
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Summary", "Intro", "New."]);
    assert_eq!(bookmark_names(&copy), ["summary", "intro"]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), ["Intro", "Old."]);
    assert_eq!(bookmark_names(&copy), ["intro"]);
}

#[test]
fn a_renamed_bookmark_of_the_author_keeps_its_name_on_each_side() {
    let mark = |name: &str| {
        format!(
            r#"<w:p><w:bookmarkStart w:id="0" w:name="{name}"/><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#
        )
    };
    let body = "<w:p><w:r><w:t>Body.</w:t></w:r></w:p>";
    let original = doc_with_body(&format!("{}{body}", mark("draft")));
    let edited = doc_with_body(&format!("{}{body}", mark("final")));
    // No tracked change renames a bookmark: its paragraph is replaced.
    let mut doc = redline(original, &edited);
    assert_eq!(revision_texts(&doc, RevisionKind::Deletion), ["Intro"]);
    assert_eq!(revision_texts(&doc, RevisionKind::Insertion), ["Intro"]);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Intro", "Body."]);
    assert_eq!(bookmark_names(&copy), ["final"]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), ["Intro", "Body."]);
    assert_eq!(bookmark_names(&copy), ["draft"]);
}

#[test]
fn a_toc_reference_split_across_runs_keeps_the_refusal() {
    // The edited link names `_Toc2` in one instruction run, `_To` + `c2` in
    // the other: renaming the bookmark would leave that half pointing at
    // nothing, so the comparison refuses the pair instead.
    let entry = |name: &str, split: bool| {
        let instruction = if split {
            format!(
                r#"<w:r><w:instrText xml:space="preserve"> PAGEREF {}</w:instrText></w:r><w:r><w:instrText xml:space="preserve">{} \h </w:instrText></w:r>"#,
                &name[..3],
                &name[3..]
            )
        } else {
            format!(
                r#"<w:r><w:instrText xml:space="preserve"> PAGEREF {name} \h </w:instrText></w:r>"#
            )
        };
        format!(
            r#"<w:p><w:hyperlink w:anchor="{name}"><w:r><w:t>Intro</w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r>{instruction}<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:hyperlink></w:p>"#
        )
    };
    let heading = |name: &str| {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="{name}"/><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#
        )
    };
    let body = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let document = |name: &str, split: bool, text: &str| {
        doc_with_body(&format!(
            "{}{}{}",
            entry(name, split),
            heading(name),
            body(text)
        ))
    };
    let mut original = document("_Toc1", false, "Old.");
    let error = original
        .compare(&document("_Toc2", true, "New."), AUTHOR, DATE)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot revise paragraph boundary structures"),
        "{error}"
    );
    assert!(original.revisions().is_empty());
    // The same pair with the reference in one run compares.
    let mut doc = redline(
        document("_Toc1", false, "Old."),
        &document("_Toc2", false, "New."),
    );
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Intro1", "Intro", "New."]);
    assert_eq!(
        link_targets(&copy),
        [("Intro1".to_owned(), Some("Intro".to_owned()))]
    );
}

#[test]
fn a_heading_removed_and_another_added_further_down_keep_their_own_bookmarks() {
    // Scope goes but its text stays, and Outlook comes after that text: the
    // two headings do not pair, so Outlook takes a name and an id the
    // original does not hold, rather than Scope's.
    let sections = [("Intro", "Hello."), ("Scope", "Kept."), ("End", "Bye.")];
    let original = report_with_toc(1, &sections);
    let edited = report_with_toc(
        1,
        &[
            ("Intro", "Hello."),
            ("", "Kept."),
            ("Outlook", "Next."),
            ("End", "Bye."),
        ],
    );
    let mut doc = redline(report_with_toc(1, &sections), &edited);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_every_entry_links_its_heading(&copy, &["Intro", "Outlook", "End"]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&original));
    assert_eq!(bookmark_names(&copy), bookmark_names(&original));
    assert_every_entry_links_its_heading(&copy, &["Intro", "Scope", "End"]);
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
fn a_renamed_toc_bookmark_named_in_another_story_keeps_the_refusal() {
    // A footnote names the edited bookmark: renaming it in the body alone
    // would leave that name pointing at nothing.
    let document = |name: &str, text: &str, note: bool| {
        let mut doc = doc_with_body(&format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="{name}"/><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p><w:p><w:r><w:t>{text}</w:t></w:r></w:p>"#
        ));
        if note {
            let location = doc.paragraph_story_location(1).unwrap().unwrap();
            doc.create_footnote(&location, &format!("See {name}."))
                .unwrap();
        }
        doc
    };
    let mut original = document("_Toc1", "Old.", true);
    let error = original
        .compare(&document("_Toc7", "New.", true), AUTHOR, DATE)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot revise paragraph boundary structures"),
        "{error}"
    );
    // Without the note naming it, the same pair compares.
    let mut doc = redline(
        document("_Toc1", "Old.", false),
        &document("_Toc7", "New.", false),
    );
    assert_eq!(bookmark_names(&accepted(&mut doc)), ["_Toc1"]);
}

// Fails today: the same footnote, its reference moved to another paragraph,
// is refused. Known limitation. Issue: #4
#[test]
#[should_panic(expected = "its reference moved to another site")]
fn a_footnote_reference_moved_to_another_paragraph_is_compared() {
    let original = doc_with_notes(&[("Claim one.", Some("Source")), ("Claim two.", None)]);
    let edited = doc_with_notes(&[("Claim one.", None), ("Claim two.", Some("Source"))]);
    let mut doc = redline(original, &edited);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Claim one.", "Claim two."]);
    assert_eq!(copy.footnotes().len(), 1);
}
