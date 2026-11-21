//! Tracked changes: the track revisions setting, reading revisions (kind,
//! author, date, story, text), accepting and rejecting them, and comparing two
//! versions of a document into a redline that resolves back to either one.

use std::collections::HashMap;

use oxoffice_docx::{
    Alignment, BodyItemRef, ComparisonGranularity, ComparisonOptions, ComparisonStoryKind,
    Document, RevisionKind, RunItemRef, StoryKind,
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
fn a_redline_in_one_name_on_a_reviewed_contract_edited_by_oxoffice_is_compared() {
    // The contract holds two reviewers' tracked changes. The edit saves the
    // main part again, which writes some characters differently (an
    // apostrophe as an entity) in paragraphs the edit does not touch.
    let mut edited = corpus_doc("word/contract.docx");
    let replaced = edited.replace_all(&HashMap::from([(
        "thirty-six (36) months",
        "forty-eight (48) months",
    )]));
    assert_eq!(replaced, 1);
    let mut edited = reopen(&mut edited);
    // The held paragraph is written another way, not changed.
    assert!(main_part_xml(&mut edited).contains("days&apos; written"));
    assert!(main_part_xml(&mut corpus_doc("word/contract.docx")).contains("days' written"));
    let original = corpus_doc("word/contract.docx");
    let earlier = original
        .revisions()
        .iter()
        .map(|revision| (revision.kind(), revision.author().to_owned()))
        .collect::<Vec<_>>();
    assert!(!earlier.is_empty());
    let mut doc = redline(original, &edited);
    let mine = doc
        .revisions()
        .iter()
        .filter(|revision| revision.author() == AUTHOR)
        .count();
    assert!(
        mine > 0,
        "the replacement is tracked in the reviewer's name"
    );
    let others = doc
        .revisions()
        .iter()
        .filter(|revision| revision.author() != AUTHOR)
        .map(|revision| (revision.kind(), revision.author().to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(others, earlier, "the other reviewers' revisions stay");
    assert_eq!(texts(&accepted(&mut doc)), texts(&accepted(&mut edited)));
    let mut original = corpus_doc("word/contract.docx");
    assert_eq!(texts(&rejected(&mut doc)), texts(&rejected(&mut original)));
}

/// The reviewed contract with one more comment, on the first run of its
/// body, added in its package so that every other byte stays as Word wrote
/// it.
fn contract_with_a_comment_on_its_first_run() -> Document {
    contract_copy_with_a_comment_on_its_first_run("word")
}

/// [`contract_with_a_comment_on_its_first_run`] on the copy of `producer`.
fn contract_copy_with_a_comment_on_its_first_run(producer: &str) -> Document {
    let bytes = std::fs::read(
        crate::support::corpus_root().join(format!("files/{producer}/contract.docx")),
    )
    .expect("corpus file");
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let body = main.find("<w:body>").unwrap();
    let start = body
        + main[body..]
            .match_indices("<w:r")
            .find(|(at, _)| matches!(main.as_bytes()[body + at + 4], b'>' | b' '))
            .unwrap()
            .0;
    let end = start + main[start..].find("</w:r>").unwrap() + "</w:r>".len();
    let main = format!(
        "{}<w:commentRangeStart w:id=\"90\"/>{}<w:commentRangeEnd w:id=\"90\"/><w:r><w:commentReference w:id=\"90\"/></w:r>{}",
        &main[..start],
        &main[start..end],
        &main[end..]
    );
    package.set_part("/word/document.xml", main.into_bytes());
    let comments =
        String::from_utf8(package.get_part("/word/comments.xml").unwrap().to_vec()).unwrap();
    let close = comments.rfind("</w:comments>").unwrap();
    let comments = format!(
        "{}<w:comment w:id=\"90\" w:author=\"First reviewer\" w:date=\"2026-11-20T10:00:00Z\"><w:p><w:r><w:t>Check the parties.</w:t></w:r></w:p></w:comment>{}",
        &comments[..close],
        &comments[close..]
    );
    package.set_part("/word/comments.xml", comments.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

#[test]
fn a_reply_added_on_a_reviewed_contract_is_compared() {
    // The comparison replays the reply on a copy of the original, which
    // saves the main part again: the paragraphs holding the reviewers'
    // revisions are written another way, and stay the same.
    let mut edited = contract_with_a_comment_on_its_first_run();
    edited
        .reply_to(90, "Second reviewer", "Agreed, it's fine.")
        .unwrap();
    let mut edited = reopen(&mut edited);
    // The replay saves the main part as this copy was saved: the held
    // paragraph is written another way, not changed.
    assert!(main_part_xml(&mut edited).contains("days&apos; written"));
    let mut original = contract_with_a_comment_on_its_first_run();
    assert!(main_part_xml(&mut original).contains("days' written"));
    let held = original.revisions().len();
    let doc = redline(original, &edited);
    assert_eq!(doc.revisions().len(), held, "the reply changes no revision");
    assert_eq!(doc.comments().len(), edited.comments().len());
}

#[test]
fn a_reply_added_on_the_google_and_libreoffice_contracts_is_compared() {
    // The replay's save writes the copies' held paragraphs as an oxoffice
    // save writes them, which the reply path reads as the compare path does.
    for producer in ["google", "libreoffice"] {
        let mut edited = contract_copy_with_a_comment_on_its_first_run(producer);
        edited
            .reply_to(90, "Second reviewer", "Agreed, it's fine.")
            .unwrap();
        let edited = reopen(&mut edited);
        let original = contract_copy_with_a_comment_on_its_first_run(producer);
        let held = original.revisions().len();
        let doc = redline(original, &edited);
        assert_eq!(doc.revisions().len(), held, "{producer}: no revision");
        assert_eq!(doc.comments().len(), edited.comments().len(), "{producer}");
    }
}

#[test]
fn a_held_paragraph_whose_dropped_value_changed_keeps_the_refusal() {
    // The reader reads both values of automatic spacing as off and the writer
    // writes them alike, but the source says another thing: the held
    // paragraph changed, and the comparison may not replace it.
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
fn a_reply_anchored_in_a_held_paragraph_keeps_the_refusal() {
    // The reply's range goes in the paragraph that holds the reviewer's
    // revision, which the comparison may not change.
    let mut edited = corpus_doc("word/contract.docx");
    let parent = edited.comments()[0].id();
    edited
        .reply_to(parent, "Second reviewer", "Agreed.")
        .unwrap();
    let edited = reopen(&mut edited);
    let mut original = corpus_doc("word/contract.docx");
    let error = original.compare(&edited, AUTHOR, DATE).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("holds an existing tracked revision"),
        "{error}"
    );
}

#[test]
fn a_redline_in_one_name_on_the_google_and_libreoffice_contracts_edited_by_oxoffice_is_compared() {
    // The same walk on the copies Google Docs and LibreOffice saved: the
    // save also writes their held paragraphs' property values its own way
    // and declares the part's namespaces differently.
    for path in ["google/contract.docx", "libreoffice/contract.docx"] {
        let mut edited = corpus_doc(path);
        let replaced = edited.replace_all(&HashMap::from([(
            "thirty-six (36) months",
            "forty-eight (48) months",
        )]));
        assert_eq!(replaced, 1, "{path}");
        let mut edited = reopen(&mut edited);
        // The save wrote the part another way: Google's root default
        // namespace is gone, and LibreOffice's empty run properties too.
        let (before, after) = (
            main_part_xml(&mut corpus_doc(path)),
            main_part_xml(&mut edited),
        );
        let written = |xml: &str| {
            (
                xml.contains(
                    r#"xmlns="http://schemas.microsoft.com/office/tasks/2019/documenttasks""#,
                ),
                xml.matches("<w:rPr></w:rPr>").count(),
            )
        };
        assert!(
            written(&before).0 && !written(&after).0 || written(&before).1 > written(&after).1,
            "{path}: the save rewrote the part"
        );
        let original = corpus_doc(path);
        let earlier = original
            .revisions()
            .iter()
            .map(|revision| (revision.kind(), revision.author().to_owned()))
            .collect::<Vec<_>>();
        assert!(!earlier.is_empty(), "{path}");
        let mut doc = redline(original, &edited);
        let others = doc
            .revisions()
            .iter()
            .filter(|revision| revision.author() != AUTHOR)
            .map(|revision| (revision.kind(), revision.author().to_owned()))
            .collect::<Vec<_>>();
        assert_eq!(
            others, earlier,
            "{path}: the other reviewers' revisions stay"
        );
        assert!(
            doc.revisions()
                .iter()
                .any(|revision| revision.author() == AUTHOR),
            "{path}: the replacement is tracked in the reviewer's name"
        );
        // Resolve this reviewer's revisions only: LibreOffice's own property
        // revisions do not resolve today, whatever the comparison does.
        let mut copy = reopen(&mut doc);
        copy.accept_revisions_by_author(AUTHOR).unwrap();
        assert_eq!(texts(&reopen(&mut copy)), texts(&edited), "{path}");
        let mut copy = reopen(&mut doc);
        copy.reject_revisions_by_author(AUTHOR).unwrap();
        assert_eq!(
            texts(&reopen(&mut copy)),
            texts(&corpus_doc(path)),
            "{path}"
        );
    }
}

/// `doc` with its term changed from thirty-six to forty-eight months,
/// saved and opened again.
fn with_term_changed(mut doc: Document) -> Document {
    let replaced = doc.replace_all(&HashMap::from([("thirty-six", "forty-eight")]));
    assert_eq!(replaced, 1);
    reopen(&mut doc)
}

#[test]
fn a_held_revision_under_a_part_namespace_a_save_drops_is_unchanged() {
    // Google Docs declares a default namespace on the part's root, which the
    // save does not write again; the earlier revision reads none of it.
    let with_default_namespace = |doc: &mut Document| {
        let mut package =
            oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
                .unwrap();
        let main = package.main_document_part().unwrap();
        let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
        let xml = xml.replacen(
            "<w:document ",
            r#"<w:document xmlns="http://schemas.microsoft.com/office/tasks/2019/documenttasks" "#,
            1,
        );
        package.set_part(&main, xml.into_bytes());
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        Document::from_bytes(&out.into_inner()).unwrap()
    };
    let body = concat!(
        r#"<w:p><w:r><w:t>Term: thirty-six months.</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t xml:space="preserve">Kept </w:t></w:r>"#,
        r#"<w:ins w:id="1" w:author="Earlier" w:date="2026-01-01T00:00:00Z"><w:r><w:t>clause.</w:t></w:r></w:ins></w:p>"#,
    );
    let mut edited = with_term_changed(with_default_namespace(&mut doc_with_body(body)));
    assert!(!main_part_xml(&mut edited).contains("documenttasks"));
    let original = with_default_namespace(&mut doc_with_body(body));
    let mut doc = redline(original, &edited);
    assert_eq!(
        texts(&accepted(&mut doc)),
        ["Term: forty-eight months.", "Kept clause."]
    );
    assert_eq!(
        texts(&rejected(&mut doc)),
        ["Term: thirty-six months.", "Kept "]
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

#[test]
fn a_libreoffice_report_compares_with_an_edited_copy() {
    let original = texts(&corpus_doc("libreoffice/report.docx"));
    let mut edited = corpus_doc("libreoffice/report.docx");
    edited.add_paragraph("A closing remark.");
    let mut doc = redline(corpus_doc("libreoffice/report.docx"), &edited);
    // The appended paragraph, the mark of the paragraph before it, and the
    // properties that paragraph gets back on rejection (its style and
    // spacing): no run of the report changes format.
    assert_eq!(
        kinds(&doc),
        [
            RevisionKind::Insertion,
            RevisionKind::ParagraphPropertyChange,
            RevisionKind::Insertion,
        ]
    );
    let mut expected = original.clone();
    expected.push("A closing remark.".to_owned());
    assert_eq!(texts(&accepted(&mut doc)), expected);
    assert_eq!(texts(&rejected(&mut doc)), original);
}

#[test]
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
fn a_final_paragraph_deleted_after_a_table_is_compared() {
    let table_last = |closing: Option<&str>| {
        let mut doc = doc_with(&["intro"]);
        doc.add_table(1, 1).cell(0, 0).unwrap().set_text("cell");
        if let Some(text) = closing {
            doc.add_paragraph(text);
        }
        doc
    };
    let mut doc = redline(table_last(Some("closing")), &table_last(None));
    assert_eq!(revision_texts(&doc, RevisionKind::Deletion), ["closing"]);
    assert_eq!(texts(&accepted(&mut doc)), ["intro"]);
    assert_eq!(texts(&rejected(&mut doc)), ["intro", "closing"]);
}

#[test]
fn two_paragraphs_appended_after_a_final_table_are_compared() {
    let table_last = || {
        let mut doc = doc_with(&["intro"]);
        doc.add_table(1, 1).cell(0, 0).unwrap().set_text("cell");
        doc
    };
    let mut edited = table_last();
    edited.add_paragraph("one");
    edited.add_paragraph("two");
    let mut doc = redline(table_last(), &edited);
    assert_eq!(
        revision_texts(&doc, RevisionKind::Insertion),
        ["one", "two"]
    );
    assert_eq!(texts(&accepted(&mut doc)), ["intro", "one", "two"]);
    assert_eq!(texts(&rejected(&mut doc)), ["intro"]);
}

#[test]
fn a_table_and_a_paragraph_appended_after_a_final_table_are_compared() {
    let table_last = || {
        let mut doc = doc_with(&["intro"]);
        doc.add_table(1, 1).cell(0, 0).unwrap().set_text("cell");
        doc
    };
    let mut edited = table_last();
    edited
        .add_table(1, 1)
        .cell(0, 0)
        .unwrap()
        .set_text("second");
    edited.add_paragraph("closing");
    let mut doc = redline(table_last(), &edited);
    assert_eq!(texts(&accepted(&mut doc)), ["intro", "closing"]);
    assert_eq!(texts(&rejected(&mut doc)), ["intro"]);
}

#[test]
fn a_paragraph_appended_after_a_final_table_whose_grid_changed_is_compared() {
    // The table is written as a deleted one and an inserted one: each
    // resolution keeps one of them before the paragraph.
    let table_last = |columns: usize| {
        let mut doc = doc_with(&["intro"]);
        let mut table = doc.add_table(1, columns);
        for column in 0..columns {
            table.cell(0, column).unwrap().set_text("cell");
        }
        doc
    };
    let mut edited = table_last(2);
    edited.add_paragraph("closing");
    let mut doc = redline(table_last(1), &edited);
    assert_eq!(texts(&accepted(&mut doc)), ["intro", "closing"]);
    assert_eq!(texts(&rejected(&mut doc)), ["intro"]);
}

#[test]
fn a_final_table_replaced_by_a_paragraph_is_compared() {
    let mut original = doc_with(&["intro"]);
    original
        .add_table(1, 1)
        .cell(0, 0)
        .unwrap()
        .set_text("cell");
    let edited = doc_with(&["intro", "closing"]);
    let mut doc = redline(original, &edited);
    let copy = accepted(&mut doc);
    assert_eq!(
        (texts(&copy), copy.tables().len()),
        (vec!["intro".to_owned(), "closing".to_owned()], 0)
    );
    let copy = rejected(&mut doc);
    assert_eq!(
        (texts(&copy), copy.tables().len()),
        (vec!["intro".to_owned()], 1)
    );
}

#[test]
fn a_final_paragraph_replaced_by_a_table_is_compared() {
    let original = doc_with(&["intro", "closing"]);
    let mut edited = doc_with(&["intro"]);
    edited.add_table(1, 1).cell(0, 0).unwrap().set_text("cell");
    let mut doc = redline(original, &edited);
    let copy = accepted(&mut doc);
    assert_eq!(
        (texts(&copy), copy.tables().len()),
        (vec!["intro".to_owned()], 1)
    );
    let copy = rejected(&mut doc);
    assert_eq!(
        (texts(&copy), copy.tables().len()),
        (vec!["intro".to_owned(), "closing".to_owned()], 0)
    );
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

/// Whether a paragraph mark of `doc` holds two revision markers of this
/// kind, which the schema allows once.
fn a_mark_holds_two(doc: &mut Document, kind: &str) -> bool {
    let marker = format!("<w:{kind} ");
    main_part_xml(doc)
        .split("<w:pPr")
        .skip(1)
        .filter(|properties| properties.starts_with(['>', ' ']))
        .filter_map(|properties| properties.split("</w:pPr>").next())
        .any(|properties| properties.matches(&marker).count() > 1)
}

/// `owners` redlined into `edited` by an earlier comparison, saved and
/// reopened: its tracked changes are existing ones for the next comparison.
fn earlier_redline(owners: &[&str], edited: &[&str]) -> Document {
    redline(doc_with_owners(owners), &doc_with_owners(edited))
}

#[test]
fn a_paragraph_moved_after_a_final_table_is_compared() {
    let original = [
        "Alpha paragraph.",
        "Beta paragraph.",
        "Gamma paragraph.",
        "[cell]",
    ];
    let edited = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "[cell]",
        "Alpha paragraph.",
    ];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    assert_eq!(
        revision_texts(&doc, RevisionKind::MoveTo),
        ["Alpha paragraph."]
    );
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_paragraph_moved_after_a_table_added_at_the_end_is_compared() {
    // Rejecting removes the new table with the move, so the mark of the
    // paragraph before the table carries the move.
    let original = ["Alpha paragraph.", "Beta paragraph.", "Gamma paragraph."];
    let edited = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "[cell]",
        "Alpha paragraph.",
    ];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    assert_eq!(
        revision_texts(&doc, RevisionKind::MoveTo),
        ["Alpha paragraph."]
    );
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_paragraph_moved_away_from_after_a_final_table_is_compared() {
    let original = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "[cell]",
        "Alpha paragraph.",
    ];
    let edited = [
        "Alpha paragraph.",
        "Beta paragraph.",
        "Gamma paragraph.",
        "[cell]",
    ];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    assert_eq!(
        revision_texts(&doc, RevisionKind::MoveFrom),
        ["Alpha paragraph."]
    );
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_paragraph_appended_after_a_table_moved_to_the_end_is_compared() {
    let original = ["[cell]", "Beta paragraph.", "Gamma paragraph."];
    let edited = ["Beta paragraph.", "Gamma paragraph.", "[cell]", "closing"];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    assert!(table_moved(&mut doc), "the table is moved");
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_final_paragraph_deleted_after_a_table_moved_to_the_end_is_compared() {
    let original = ["[cell]", "Beta paragraph.", "Gamma paragraph.", "closing"];
    let edited = ["Beta paragraph.", "Gamma paragraph.", "[cell]"];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    assert!(table_moved(&mut doc), "the table is moved");
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
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
fn a_paragraph_moved_to_the_end_then_one_appended_is_compared() {
    // The paragraph before the moved one keeps its own properties when
    // rejecting merges the two ends of the body into it.
    let original = ["Alpha paragraph.", "Beta paragraph.", "Gamma paragraph."];
    let edited = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "Alpha paragraph.",
        "closing",
    ];
    let mut doc = redline(
        doc_with_centred(&original, 2),
        &doc_with_centred(&edited, 1),
    );
    assert_eq!(
        revision_texts(&doc, RevisionKind::MoveTo),
        ["Alpha paragraph."]
    );
    assert_eq!(
        aligned_texts(&accepted(&mut doc)),
        aligned_texts(&doc_with_centred(&edited, 1))
    );
    assert_eq!(
        aligned_texts(&rejected(&mut doc)),
        aligned_texts(&doc_with_centred(&original, 2))
    );
}

#[test]
fn a_paragraph_appended_then_one_moved_to_the_end_is_compared() {
    let original = ["Alpha paragraph.", "Beta paragraph.", "Gamma paragraph."];
    let edited = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "closing",
        "Alpha paragraph.",
    ];
    let mut doc = redline(
        doc_with_centred(&original, 2),
        &doc_with_centred(&edited, 1),
    );
    assert_eq!(
        revision_texts(&doc, RevisionKind::MoveTo),
        ["Alpha paragraph."]
    );
    assert_eq!(
        aligned_texts(&accepted(&mut doc)),
        aligned_texts(&doc_with_centred(&edited, 1))
    );
    assert_eq!(
        aligned_texts(&rejected(&mut doc)),
        aligned_texts(&doc_with_centred(&original, 2))
    );
}

#[test]
fn two_paragraphs_moved_to_the_end_are_compared() {
    let original = [
        "Alpha paragraph.",
        "Delta paragraph.",
        "Beta paragraph.",
        "Gamma paragraph.",
    ];
    let edited = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "Alpha paragraph.",
        "Delta paragraph.",
    ];
    let mut doc = redline(
        doc_with_centred(&original, 3),
        &doc_with_centred(&edited, 1),
    );
    assert_eq!(
        revision_texts(&doc, RevisionKind::MoveTo),
        ["Alpha paragraph.", "Delta paragraph."]
    );
    assert!(!a_mark_holds_two(&mut doc, "moveTo"));
    assert_eq!(
        aligned_texts(&accepted(&mut doc)),
        aligned_texts(&doc_with_centred(&edited, 1))
    );
    assert_eq!(
        aligned_texts(&rejected(&mut doc)),
        aligned_texts(&doc_with_centred(&original, 3))
    );
}

#[test]
fn two_paragraphs_moved_away_from_the_end_are_compared() {
    let original = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "Epsilon paragraph.",
        "Alpha paragraph.",
        "Delta paragraph.",
    ];
    let edited = [
        "Alpha paragraph.",
        "Delta paragraph.",
        "Beta paragraph.",
        "Gamma paragraph.",
        "Epsilon paragraph.",
    ];
    let mut doc = redline(
        doc_with_centred(&original, 2),
        &doc_with_centred(&edited, 4),
    );
    assert_eq!(
        revision_texts(&doc, RevisionKind::MoveFrom),
        ["Alpha paragraph.", "Delta paragraph."]
    );
    assert!(!a_mark_holds_two(&mut doc, "moveFrom"));
    assert_eq!(
        aligned_texts(&accepted(&mut doc)),
        aligned_texts(&doc_with_centred(&edited, 4))
    );
    assert_eq!(
        aligned_texts(&rejected(&mut doc)),
        aligned_texts(&doc_with_centred(&original, 2))
    );
}

#[test]
fn a_paragraph_moved_away_from_the_end_with_the_final_paragraph_deleted_is_compared() {
    let original = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "Alpha paragraph.",
        "closing",
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

/// `doc` saved with only the revision of this kind resolved, by its id:
/// both ends of a move share theirs.
fn with_one_resolved(doc: &mut Document, kind: RevisionKind, accept: bool) -> Document {
    let id = doc
        .revisions()
        .iter()
        .find(|revision| revision.kind() == kind)
        .map(|revision| revision.id())
        .expect("a revision of that kind");
    let mut copy = reopen(doc);
    if accept {
        copy.accept_revision_id(id).unwrap();
    } else {
        copy.reject_revision_id(id).unwrap();
    }
    reopen(&mut copy)
}

#[test]
fn rejecting_only_a_move_to_the_end_keeps_the_paragraph_appended_before_it() {
    let original = ["Alpha paragraph.", "Beta paragraph.", "Gamma paragraph."];
    let edited = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "closing",
        "Alpha paragraph.",
    ];
    let mut doc = redline(doc_with(&original), &doc_with(&edited));
    let mut moved_back = with_one_resolved(&mut doc, RevisionKind::MoveTo, false);
    // The appended paragraph is still a tracked insertion.
    assert_eq!(
        texts(&accepted(&mut moved_back)),
        [&original[..], &["closing"]].concat()
    );
    assert_eq!(texts(&rejected(&mut moved_back)), original);
}

#[test]
fn accepting_only_a_move_away_from_the_end_keeps_the_deleted_paragraph_before_it() {
    let original = [
        "Beta paragraph.",
        "Gamma paragraph.",
        "closing",
        "Alpha paragraph.",
    ];
    let edited = ["Alpha paragraph.", "Beta paragraph.", "Gamma paragraph."];
    let mut doc = redline(doc_with(&original), &doc_with(&edited));
    let mut moved_up = with_one_resolved(&mut doc, RevisionKind::MoveFrom, true);
    // The final paragraph is still a tracked deletion.
    assert_eq!(texts(&accepted(&mut moved_up)), edited);
    assert_eq!(
        texts(&rejected(&mut moved_up)),
        [&edited[..], &["closing"]].concat()
    );
}

#[test]
fn a_paragraph_a_table_and_a_paragraph_appended_are_compared() {
    let original = ["intro"];
    let edited = ["intro", "added", "[cell]", "closing"];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
}

#[test]
fn a_paragraph_a_table_and_a_paragraph_deleted_at_the_end_are_compared() {
    let original = ["intro", "removed", "[cell]", "closing"];
    let edited = ["intro"];
    let mut doc = redline(doc_with_owners(&original), &doc_with_owners(&edited));
    assert_eq!(owners(&accepted(&mut doc)), edited);
    assert_eq!(owners(&rejected(&mut doc)), original);
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

#[test]
fn a_paragraph_inserted_before_a_final_table_inserted_by_an_earlier_redline_is_compared() {
    // Rejecting every revision removes the table and the paragraph before
    // it: the body ends with the paragraph before both.
    let mut redlined = earlier_redline(&["intro"], &["intro", "[cell]"]);
    let mut edited = reopen(&mut redlined);
    edited.insert_paragraph(1, "added");
    assert_eq!(owners(&edited), ["intro", "added", "[cell]"]);
    let mut doc = redline(reopen(&mut redlined), &edited);
    assert_eq!(owners(&accepted(&mut doc)), ["intro", "added", "[cell]"]);
    assert_eq!(owners(&rejected(&mut doc)), ["intro"]);
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

#[test]
fn a_paragraph_appended_after_a_final_table_with_tracked_cell_text_is_compared() {
    // The earlier redline changes the text of the cell: no resolution
    // removes the table, so the new paragraph carries its own mark.
    let mut redlined = earlier_redline(&["intro", "[cell]"], &["intro", "[changed]"]);
    let mut edited = reopen(&mut redlined);
    edited.add_paragraph("closing");
    let mut doc = redline(reopen(&mut redlined), &edited);
    assert_eq!(
        owners(&accepted(&mut doc)),
        ["intro", "[changed]", "closing"]
    );
    assert_eq!(owners(&rejected(&mut doc)), ["intro", "[cell]"]);
}

#[test]
fn a_final_paragraph_deleted_after_a_table_with_tracked_cell_text_is_compared() {
    let mut redlined = earlier_redline(&["intro", "[cell]"], &["intro", "[changed]"]);
    let mut original = reopen(&mut redlined);
    original.add_paragraph("closing");
    let mut doc = redline(original, &reopen(&mut redlined));
    assert_eq!(owners(&accepted(&mut doc)), ["intro", "[changed]"]);
    assert_eq!(owners(&rejected(&mut doc)), ["intro", "[cell]", "closing"]);
}

#[test]
fn a_paragraph_appended_after_a_final_table_inserted_by_an_earlier_redline_is_compared() {
    // Rejecting only the comparison's revisions keeps the table, so the new
    // paragraph carries its own mark; rejecting every revision removes the
    // table too, and the paragraph goes with it.
    let mut redlined = doc_with_owners(&["intro"]);
    redlined
        .compare(&doc_with_owners(&["intro", "[cell]"]), "Earlier", DATE)
        .unwrap();
    let mut edited = reopen(&mut redlined);
    edited.add_paragraph("closing");
    let mut doc = redline(reopen(&mut redlined), &edited);
    assert_eq!(owners(&accepted(&mut doc)), ["intro", "[cell]", "closing"]);
    assert_eq!(owners(&rejected(&mut doc)), ["intro"]);
    let mut own = reopen(&mut doc);
    own.reject_revisions_by_author(AUTHOR).unwrap();
    assert_eq!(owners(&reopen(&mut own)), ["intro", "[cell]"]);
}

#[test]
fn a_paragraph_appended_after_a_lone_table_inserted_by_an_earlier_redline_is_compared() {
    // Rejecting every revision removes all the body holds, as it does for
    // the original.
    let mut redlined = earlier_redline(&[], &["[cell]"]);
    let mut edited = reopen(&mut redlined);
    edited.add_paragraph("closing");
    let mut doc = redline(reopen(&mut redlined), &edited);
    assert_eq!(owners(&accepted(&mut doc)), ["[cell]", "closing"]);
    assert_eq!(owners(&rejected(&mut doc)), Vec::<String>::new());
    assert_eq!(owners(&rejected(&mut redlined)), Vec::<String>::new());
}

#[test]
fn a_paragraph_appended_after_a_final_table_deleted_by_an_earlier_redline_is_compared() {
    // Rejecting keeps the table, which then ends the body again.
    let mut redlined = earlier_redline(&["intro", "[cell]"], &["intro"]);
    let mut edited = reopen(&mut redlined);
    edited.add_paragraph("closing");
    let mut doc = redline(reopen(&mut redlined), &edited);
    assert_eq!(owners(&accepted(&mut doc)), ["intro", "closing"]);
    assert_eq!(owners(&rejected(&mut doc)), ["intro", "[cell]"]);
}

#[test]
fn a_final_paragraph_deleted_after_a_final_table_inserted_by_an_earlier_redline_is_compared() {
    // Accepting keeps the table, which then ends the body.
    let mut redlined = earlier_redline(&["intro"], &["intro", "[cell]"]);
    let mut original = reopen(&mut redlined);
    original.add_paragraph("closing");
    let mut doc = redline(original, &reopen(&mut redlined));
    assert_eq!(owners(&accepted(&mut doc)), ["intro", "[cell]"]);
    assert_eq!(owners(&rejected(&mut doc)), ["intro", "closing"]);
}

#[test]
fn a_paragraph_appended_after_a_table_both_resolutions_remove_is_compared() {
    // Every row of the table is an inserted row that is deleted again: each
    // resolution of every revision removes the table and the new paragraph
    // with it.
    let body = concat!(
        r#"<w:p><w:r><w:t>intro</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:trPr>"#,
        r#"<w:ins w:id="1" w:author="Earlier" w:date="2026-01-01T00:00:00Z"/>"#,
        r#"<w:del w:id="2" w:author="Earlier" w:date="2026-01-01T00:00:00Z"/>"#,
        r#"</w:trPr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
    );
    let mut edited = doc_with_body(body);
    edited.add_paragraph("closing");
    let mut doc = redline(doc_with_body(body), &edited);
    assert_eq!(owners(&accepted(&mut doc)), ["intro", "closing"]);
    assert_eq!(owners(&rejected(&mut doc)), ["intro"]);
}

#[test]
fn rejecting_a_final_paragraph_inserted_with_the_table_before_it_ends_the_body_with_the_paragraph_before()
 {
    // Rejecting removes the table and the paragraph after it, which has
    // nothing left: the body ends with the paragraph before the table.
    let body = concat!(
        r#"<w:p><w:r><w:t>intro</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:trPr>"#,
        r#"<w:ins w:id="1" w:author="Eve" w:date="2026-01-01T00:00:00Z"/>"#,
        r#"</w:trPr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:p><w:pPr><w:rPr><w:ins w:id="2" w:author="Eve" w:date="2026-01-01T00:00:00Z"/></w:rPr></w:pPr>"#,
        r#"<w:ins w:id="3" w:author="Eve" w:date="2026-01-01T00:00:00Z"><w:r><w:t>closing</w:t></w:r></w:ins></w:p>"#,
    );
    let mut doc = doc_with_body(body);
    assert_eq!(owners(&rejected(&mut doc)), ["intro"]);
    assert_eq!(owners(&accepted(&mut doc)), ["intro", "[cell]", "closing"]);
}

#[test]
fn empty_border_containers_against_none_are_no_format_change() {
    // The original saved by LibreOffice, with empty table and cell border
    // containers, the edited copy saved by Word without them.
    let table = |borders: &str, cell_borders: &str, text: &str| {
        format!(
            concat!(
                r#"<w:tbl><w:tblPr>{}</w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>"#,
                r#"<w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/>{}</w:tcPr>"#,
                r#"<w:p><w:r><w:t>{}</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#
            ),
            borders, cell_borders, text
        )
    };
    for (original, edited) in [
        (
            table(
                "<w:tblBorders></w:tblBorders>",
                "<w:tcBorders></w:tcBorders>",
                "cell",
            ),
            table("", "", "cell edited"),
        ),
        (
            table("", "", "cell"),
            table("<w:tblBorders/>", "<w:tcBorders/>", "cell edited"),
        ),
    ] {
        let doc = redline(doc_with_body(&original), &doc_with_body(&edited));
        assert!(
            kinds(&doc)
                .iter()
                .all(|kind| matches!(kind, RevisionKind::Insertion | RevisionKind::Deletion)),
            "no table or cell format change: {:?}",
            kinds(&doc)
        );
    }
}

#[test]
fn empty_table_properties_written_by_libreoffice_are_no_format_change() {
    // LibreOffice writes `<w:trPr></w:trPr>` and `<w:tcBorders></w:tcBorders>`;
    // the comparison reads them as no properties.
    let body = concat!(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>"#,
        r#"<w:tr><w:trPr></w:trPr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/><w:tcBorders></w:tcBorders></w:tcPr>"#,
        r#"<w:p><w:r><w:rPr></w:rPr><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:p><w:r><w:t>after</w:t></w:r></w:p>"#,
    );
    let mut edited = doc_with_body(body);
    edited.add_paragraph("added");
    let mut doc = redline(doc_with_body(body), &edited);
    assert!(
        kinds(&doc)
            .iter()
            .all(|kind| *kind == RevisionKind::Insertion),
        "no row, cell or run format change: {:?}",
        kinds(&doc)
    );
    assert_eq!(revision_texts(&doc, RevisionKind::Insertion), ["added"]);
    assert_eq!(texts(&accepted(&mut doc)), ["after", "added"]);
    assert_eq!(texts(&rejected(&mut doc)), ["after"]);
}

// Google Docs declares a default namespace on its root that nothing uses,
// and a save leaves it out: a field the edit did not touch is unchanged.
#[test]
fn a_field_is_no_change_once_a_save_drops_an_unused_root_declaration() {
    let body = concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">Page </w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    let google_root = || {
        let bytes = doc_with_body(body).to_bytes().expect("document saves");
        let mut package =
            oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("opens");
        let part = package.main_document_part().expect("main part");
        let xml = String::from_utf8(package.get_part(&part).expect("main part").to_vec())
            .expect("UTF-8")
            .replacen(
                "<w:document ",
                r#"<w:document xmlns="http://schemas.microsoft.com/office/tasks/2019/documenttasks" "#,
                1,
            );
        package.set_part(&part, xml.into_bytes());
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).expect("package writes");
        Document::from_bytes(&out.into_inner()).expect("document opens")
    };
    let mut edited = google_root();
    edited.add_paragraph("Added");
    let mut edited = reopen(&mut edited);
    assert!(
        !main_xml(&mut edited).contains("documenttasks"),
        "the save keeps the unused declaration: this test no longer tests the comparison"
    );
    let mut doc = redline(google_root(), &edited);
    assert_eq!(revision_texts(&doc, RevisionKind::Insertion), ["Added"]);
    assert!(
        !kinds(&doc).contains(&RevisionKind::Deletion),
        "{:?}",
        kinds(&doc)
    );
    assert_eq!(texts(&rejected(&mut doc)), ["Page 1"]);
}

// ---- Equations ----

const MATH_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

/// A paragraph, then a display equation holding `x` (or the text given).
fn equation_body(text: &str, declarations: &str) -> String {
    format!(
        r#"<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:p><m:oMathPara xmlns:m="{MATH_NS}"{declarations}><m:oMath><m:r><w:rPr><w:i/></w:rPr><m:t>{text}</m:t></m:r></m:oMath></m:oMathPara></w:p>"#
    )
}

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

// After any edit, a save writes a kept equation again from its model; the
// comparison reads it as the same equation the original holds.
#[test]
fn an_unchanged_equation_is_no_change_once_the_edited_copy_was_saved() {
    let body = equation_body("x", "");
    let mut edited = doc_with_body(&body);
    edited.add_paragraph("Added");
    let edited = reopen(&mut edited);
    let mut doc = redline(doc_with_body(&body), &edited);
    assert!(
        kinds(&doc)
            .iter()
            .all(|kind| *kind == RevisionKind::Insertion),
        "only the added paragraph is a change: {:?}",
        kinds(&doc)
    );
    assert_eq!(revision_texts(&doc, RevisionKind::Insertion), ["Added"]);
    assert_eq!(texts(&accepted(&mut doc)), ["Before", "", "Added"]);
    assert_eq!(math_texts(&mut rejected(&mut doc)), ["x"]);
}

// Word, Google and LibreOffice declare their namespaces once, on the root; a
// copy whose equation declares them again holds the same equation.
#[test]
fn an_equation_that_declares_the_root_namespaces_again_is_no_change() {
    let again = concat!(
        r#" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#,
        r#" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#,
        r#" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#,
    );
    let mut edited = doc_with_body(&equation_body("x", again));
    edited.add_paragraph("Added");
    let mut doc = redline(doc_with_body(&equation_body("x", "")), &edited);
    assert_eq!(revision_texts(&doc, RevisionKind::Insertion), ["Added"]);
    assert!(
        !kinds(&doc).contains(&RevisionKind::Deletion),
        "the equation is not deleted and inserted again: {:?}",
        kinds(&doc)
    );
    assert_eq!(math_texts(&mut accepted(&mut doc)), ["x"]);
}

// An equation that changes is deleted and inserted with its paragraph, and
// the redline names no namespace of the comparison's own.
#[test]
fn a_changed_equation_resolves_to_either_version() {
    let mut doc = redline(
        doc_with_body(&equation_body("x", "")),
        &doc_with_body(&equation_body("y", "")),
    );
    assert!(kinds(&doc).contains(&RevisionKind::Deletion));
    assert!(kinds(&doc).contains(&RevisionKind::Insertion));
    let xml = main_xml(&mut doc);
    assert!(
        !xml.contains("rdocxcmp"),
        "a private namespace leaked: {xml}"
    );
    assert_eq!(math_texts(&mut accepted(&mut doc)), ["y"]);
    assert_eq!(math_texts(&mut rejected(&mut doc)), ["x"]);
}

// A word changed beside an inline equation is a word changed: the equation,
// written again by the save, stays where it is.
#[test]
fn a_word_changed_beside_an_inline_equation_is_revised_in_place() {
    let body = |word: &str| {
        format!(
            r#"<w:p><w:r><w:t xml:space="preserve">Let </w:t></w:r><m:oMath xmlns:m="{MATH_NS}"><m:r><w:rPr><w:i/></w:rPr><m:t>x</m:t></m:r></m:oMath><w:r><w:t xml:space="preserve"> be {word}.</w:t></w:r></w:p>"#
        )
    };
    for granularity in [
        ComparisonGranularity::Run,
        ComparisonGranularity::Word,
        ComparisonGranularity::Character,
    ] {
        let mut edited = doc_with_body(&body("small"));
        edited.add_paragraph("Added");
        let mut edited = reopen(&mut edited);
        assert!(edited.replace_all(&HashMap::from([("small", "large")])) == 1);
        let edited = reopen(&mut edited);
        let mut doc = doc_with_body(&body("small"));
        let options = ComparisonOptions {
            granularity,
            ..ComparisonOptions::default()
        };
        let diagnostics = doc
            .compare_with_options(&edited, AUTHOR, DATE, &options)
            .expect("comparison");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let mut doc = reopen(&mut doc);
        let deleted = revision_texts(&doc, RevisionKind::Deletion);
        assert!(
            deleted.iter().all(|text| !text.contains("Let")),
            "{granularity:?}: the paragraph is not deleted whole: {deleted:?}"
        );
        assert_eq!(math_texts(&mut accepted(&mut doc)), ["x"]);
        assert_eq!(texts(&accepted(&mut doc)), ["Let  be large.", "Added"]);
        assert_eq!(texts(&rejected(&mut doc)), ["Let  be small."]);
    }
}

// What an equation says still counts: a run property or a fraction type
// that changes inside it changes the equation.
#[test]
fn a_property_changed_inside_an_equation_is_a_change() {
    let fraction = |kind: &str, property: &str| {
        doc_with_body(&format!(
            r#"<w:p><m:oMathPara xmlns:m="{MATH_NS}"><m:oMath><m:f><m:fPr><m:type m:val="{kind}"/></m:fPr><m:num><m:r><w:rPr>{property}</w:rPr><m:t>1</m:t></m:r></m:num><m:den><m:r><m:t>2</m:t></m:r></m:den></m:f></m:oMath></m:oMathPara></w:p>"#
        ))
    };
    for (original, edited) in [
        (fraction("bar", "<w:i/>"), fraction("bar", "<w:b/>")),
        (fraction("bar", "<w:i/>"), fraction("lin", "<w:i/>")),
    ] {
        let mut edited = edited;
        let mut edited = reopen(&mut edited);
        let wanted = main_xml(&mut edited);
        let mut doc = redline(original, &edited);
        assert!(
            kinds(&doc).contains(&RevisionKind::Deletion),
            "{:?}",
            kinds(&doc)
        );
        assert!(
            kinds(&doc).contains(&RevisionKind::Insertion),
            "{:?}",
            kinds(&doc)
        );
        let accepted = main_xml(&mut accepted(&mut doc));
        for marker in [r#"m:val="lin""#, "<w:b/>"] {
            assert_eq!(
                accepted.contains(marker),
                wanted.contains(marker),
                "{marker}"
            );
        }
    }
}

// A raw child before an equation at the same run boundary (a heading's
// bookmark, as Word writes a table of contents target) keeps the equation
// paired with its raw child when a paragraph is inserted before it.
#[test]
fn a_bookmark_before_an_equation_does_not_unpair_it() {
    let body = format!(
        concat!(
            r#"<w:p><w:r><w:t>Intro</w:t></w:r></w:p>"#,
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="7" w:name="_Toc217000007"/>"#,
            r#"<m:oMath xmlns:m="{}"><m:r><w:rPr><w:i/></w:rPr><m:t>x</m:t></m:r></m:oMath>"#,
            r#"<w:r><w:t xml:space="preserve"> rises</w:t></w:r><w:bookmarkEnd w:id="7"/></w:p>"#,
        ),
        MATH_NS
    );
    let mut edited = doc_with_body(&body);
    edited.insert_paragraph(1, "Inserted");
    let edited = reopen(&mut edited);
    let mut doc = redline(doc_with_body(&body), &edited);
    assert_eq!(revision_texts(&doc, RevisionKind::Insertion), ["Inserted"]);
    assert!(
        !kinds(&doc).contains(&RevisionKind::Deletion),
        "{:?}",
        kinds(&doc)
    );
    let accepted = main_xml(&mut accepted(&mut doc));
    assert!(accepted.contains(r#"w:name="_Toc217000007""#), "{accepted}");
    assert_eq!(math_texts(&mut rejected(&mut doc)), ["x"]);
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

// Word and LibreOffice hold an equation and an endnote reference in one
// paragraph, Google spells every default out: no paragraph is deleted and
// inserted again.
#[test]
fn a_paper_with_equations_compares_with_an_edited_copy() {
    for name in [
        "word/paper.docx",
        "google/paper.docx",
        "libreoffice/paper.docx",
    ] {
        let original = texts(&corpus_doc(name));
        let equations = math_texts(&mut corpus_doc(name));
        assert!(equations.len() > 11, "{name}: {equations:?}");
        let mut edited = corpus_doc(name);
        edited.add_paragraph("A closing remark.");
        let edited = reopen(&mut edited);
        let mut doc = redline(corpus_doc(name), &edited);
        assert!(
            !kinds(&doc).contains(&RevisionKind::Deletion),
            "{name}: no paragraph is deleted and inserted again: {:?}",
            kinds(&doc)
        );
        let mut expected = original.clone();
        expected.push("A closing remark.".to_owned());
        let mut accepted = accepted(&mut doc);
        assert_eq!(texts(&accepted), expected, "{name}");
        assert_eq!(math_texts(&mut accepted), equations, "{name}");
        let mut rejected = rejected(&mut doc);
        assert_eq!(texts(&rejected), original, "{name}");
        assert_eq!(math_texts(&mut rejected), equations, "{name}");
    }
}

// ---- Bookmarks, tables of contents and notes ----

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
fn a_toc_reference_split_across_runs_keeps_its_bookmark_name() {
    // The edited link names `_Toc2` in one instruction run, `_To` + `c2` in
    // the other: renaming the bookmark would leave that half pointing at
    // nothing, so the edited name stays, and only the id follows.
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
    let mut doc = redline(
        document("_Toc1", false, "Old."),
        &document("_Toc2", true, "New."),
    );
    let copy = accepted(&mut doc);
    assert_eq!(bookmark_names(&copy), ["_Toc2"]);
    assert_eq!(
        link_targets(&copy),
        [("Intro1".to_owned(), Some("Intro".to_owned()))]
    );
    assert_eq!(bookmark_names(&rejected(&mut doc)), ["_Toc1"]);
    // The same pair with the reference in one run takes the original name.
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

#[test]
fn a_cross_referenced_heading_compares_after_a_heading_is_added_before_it() {
    // Word renumbers every bookmark id on save: the `_Ref` target that the
    // cross-reference names moves from id 2 to id 3 with the new heading.
    let heading = |text: &str, marks: &[(u32, &str)]| {
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
    };
    let body = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let original = || {
        doc_with_body(&format!(
            "{}{}{}{}",
            heading("Intro", &[(0, "_Toc1")]),
            body("Hello."),
            heading("Results", &[(1, "_Toc2"), (2, "_Ref9")]),
            body("Data."),
        ))
    };
    let edited = doc_with_body(&format!(
        "{}{}{}{}{}{}",
        heading("Intro", &[(0, "_Toc11")]),
        body("Hello."),
        heading("Methods", &[(1, "_Toc12")]),
        body("How."),
        heading("Results", &[(2, "_Toc13"), (3, "_Ref9")]),
        body("Data."),
    ));
    let mut doc = redline(original(), &edited);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    let names = bookmark_names(&copy);
    assert!(names.contains(&"_Ref9".to_owned()), "{names:?}");
    assert_eq!(names.len(), 4, "{names:?}");
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), texts(&original()));
    assert_eq!(bookmark_names(&copy), bookmark_names(&original()));
}

#[test]
fn a_link_to_another_file_keeps_the_bookmark_it_names() {
    // `\l "_Toc7"` after a file names that file's bookmark, not this one's.
    let document = |name: &str, text: &str| {
        doc_with_body(&format!(
            concat!(
                r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK "part1.docx" \l "_Toc7" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Part one</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
                r#"<w:p><w:fldSimple w:instr=" HYPERLINK &quot;part1.docx&quot; \l &quot;_Toc7&quot; "><w:r><w:t>Again</w:t></w:r></w:fldSimple></w:p>"#,
                r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="{name}"/><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
                r#"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"#,
            ),
            name = name,
            text = text,
        ))
    };
    let mut doc = redline(document("_Toc1", "Old."), &document("_Toc7", "New."));
    let copy = accepted(&mut doc);
    assert_eq!(bookmark_names(&copy), ["_Toc1"]);
    assert_eq!(
        field_instructions(&copy),
        [
            r#"HYPERLINK "part1.docx" \l "_Toc7""#,
            r#"HYPERLINK "part1.docx" \l "_Toc7""#
        ]
    );
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

/// A became a heading after the original's table of contents was built,
/// and its next heading was rewritten: the pairing would give the new
/// heading the old one's name, but the comparison pairs the old heading
/// with A.
fn late_heading_pair(a_id: u32, new_id: u32) -> (Document, Document) {
    let body = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let original = doc_with_body(&format!(
        "{}{}{}",
        heading_with("A", &[]),
        heading_with("Old", &[(0, "_Toc1")]),
        body("Body."),
    ));
    let edited = doc_with_body(&format!(
        "{}{}{}{}",
        body("New first."),
        heading_with("A", &[(a_id, "_Toc11")]),
        heading_with("New", &[(new_id, "_Toc12")]),
        body("Body."),
    ));
    (original, edited)
}

#[test]
fn a_heading_the_comparison_pairs_otherwise_keeps_its_own_toc_name() {
    let (original, edited) = late_heading_pair(5, 6);
    let mut doc = redline(original, &edited);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(bookmark_names(&copy), ["_Toc11", "_Toc12"]);
    assert_eq!(bookmark_names(&rejected(&mut doc)), ["_Toc1"]);
}

#[test]
fn ids_still_follow_the_original_when_toc_names_cannot() {
    // As above, with A under the id the original gives Old: the names stay,
    // and A takes a fresh id.
    let (original, edited) = late_heading_pair(0, 1);
    let mut doc = redline(original, &edited);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(bookmark_names(&copy), ["_Toc11", "_Toc12"]);
    assert_eq!(bookmark_names(&rejected(&mut doc)), ["_Toc1"]);
}

#[test]
fn a_kept_name_the_original_gives_another_heading_keeps_the_edited_id() {
    // The pairing falls back to ids only, and the edited New keeps `_Toc2`,
    // which the original gives Gone: New keeps its own id, so the deleted
    // Gone and the inserted New do not share one.
    let body = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let original = || {
        doc_with_body(&format!(
            "{}{}{}{}{}",
            heading_with("A", &[]),
            heading_with("Old", &[(0, "_Toc1")]),
            body("Body."),
            heading_with("Gone", &[(1, "_Toc2")]),
            body("End."),
        ))
    };
    let edited = doc_with_body(&format!(
        "{}{}{}{}{}",
        body("New first."),
        heading_with("A", &[(5, "_Toc11")]),
        heading_with("New", &[(6, "_Toc2")]),
        body("Body."),
        body("End."),
    ));
    let mut doc = redline(original(), &edited);
    assert_eq!(texts(&accepted(&mut doc)), texts(&edited));
    assert_eq!(texts(&rejected(&mut doc)), texts(&original()));
}

#[test]
fn a_toc_caption_in_a_table_takes_the_original_id() {
    // A table of figures names its captions `_Toc` too. Word renumbered the
    // ids after a new bookmark at the top: the caption in the table keeps
    // its name and takes back its original id, so the table is unchanged.
    let table = |id: u32| {
        format!(
            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:bookmarkStart w:id="{id}" w:name="_Toc2"/><w:r><w:t>Figure 1</w:t></w:r><w:bookmarkEnd w:id="{id}"/></w:p></w:tc></w:tr></w:tbl>"#
        )
    };
    let body = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let original = || {
        doc_with_body(&format!(
            "{}{}{}",
            heading_with("Intro", &[(0, "_Toc1")]),
            table(1),
            body("Body."),
        ))
    };
    let edited = doc_with_body(&format!(
        "{}{}{}{}",
        heading_with("Note", &[(0, "remark")]),
        heading_with("Intro", &[(1, "_Toc1")]),
        table(2),
        body("Body."),
    ));
    let mut doc = redline(original(), &edited);
    assert_eq!(revision_texts(&doc, RevisionKind::Insertion), ["Note"]);
    assert_eq!(
        revision_texts(&doc, RevisionKind::Deletion),
        Vec::<String>::new()
    );
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), texts(&edited));
    assert_eq!(texts(&rejected(&mut doc)), texts(&original()));
}

#[test]
fn a_toc_caption_in_a_table_takes_the_original_id_in_the_fallback() {
    // As above while the names fall back (the comparison pairs Old with A):
    // the caption still takes back the id Word renumbered.
    let caption = |id: u32| {
        format!(
            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:bookmarkStart w:id="{id}" w:name="_Toc5"/><w:r><w:t>Figure 1</w:t></w:r><w:bookmarkEnd w:id="{id}"/></w:p></w:tc></w:tr></w:tbl>"#
        )
    };
    let body = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let original = || {
        doc_with_body(&format!(
            "{}{}{}{}",
            heading_with("A", &[]),
            heading_with("Old", &[(0, "_Toc1")]),
            caption(1),
            body("Body."),
        ))
    };
    let edited = doc_with_body(&format!(
        "{}{}{}{}{}",
        body("New first."),
        heading_with("A", &[(5, "_Toc11")]),
        heading_with("New", &[(6, "_Toc12")]),
        caption(7),
        body("Body."),
    ));
    let mut doc = redline(original(), &edited);
    let changed = revision_texts(&doc, RevisionKind::Insertion)
        .into_iter()
        .chain(revision_texts(&doc, RevisionKind::Deletion))
        .collect::<Vec<_>>();
    assert!(
        !changed.iter().any(|text| text.contains("Figure")),
        "{changed:?}"
    );
    assert_eq!(texts(&accepted(&mut doc)), texts(&edited));
    assert_eq!(texts(&rejected(&mut doc)), texts(&original()));
}

#[test]
fn a_kept_heading_takes_back_its_renumbered_id_in_the_fallback() {
    // As in the late-heading pair, which forces the fallback, with a Kept
    // heading that did not change but whose id Word renumbered: it takes
    // back the original's, and is no change.
    let body = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let original = || {
        doc_with_body(&format!(
            "{}{}{}{}{}",
            heading_with("A", &[]),
            heading_with("Old", &[(0, "_Toc1")]),
            body("Body."),
            heading_with("Kept", &[(1, "_Toc3")]),
            body("End."),
        ))
    };
    let edited = doc_with_body(&format!(
        "{}{}{}{}{}{}",
        body("New first."),
        heading_with("A", &[(5, "_Toc11")]),
        heading_with("New", &[(6, "_Toc12")]),
        body("Body."),
        heading_with("Kept", &[(7, "_Toc3")]),
        body("End."),
    ));
    let mut doc = redline(original(), &edited);
    let changed = revision_texts(&doc, RevisionKind::Insertion)
        .into_iter()
        .chain(revision_texts(&doc, RevisionKind::Deletion))
        .collect::<Vec<_>>();
    assert!(!changed.iter().any(|text| text == "Kept"), "{changed:?}");
    assert_eq!(texts(&accepted(&mut doc)), texts(&edited));
    assert_eq!(texts(&rejected(&mut doc)), texts(&original()));
}

#[test]
fn a_claimed_toc_name_in_a_table_keeps_its_own_id_in_the_fallback() {
    // Intro is renamed `_Toc2` -> `_Toc1`, but a footnote names `_Toc2`, so
    // the names fall back; the caption holds `_Toc1`, which the original
    // gives Intro: it keeps its own id rather than take Intro's.
    let document = |intro: (u32, &str), caption: Option<u32>, note: bool| {
        let table = caption.map_or(String::new(), |id| {
            format!(
                r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:bookmarkStart w:id="{id}" w:name="_Toc1"/><w:r><w:t>Figure 1</w:t></w:r><w:bookmarkEnd w:id="{id}"/></w:p></w:tc></w:tr></w:tbl>"#
            )
        });
        let mut doc = doc_with_body(&format!(
            "{}{table}<w:p><w:r><w:t>Body.</w:t></w:r></w:p>",
            heading_with("Intro", &[intro]),
        ));
        if note {
            let at = doc.paragraphs().len() - 1;
            let location = doc.paragraph_story_location(at).unwrap().unwrap();
            doc.create_footnote(&location, "See _Toc2.").unwrap();
        }
        doc
    };
    let mut doc = redline(
        document((0, "_Toc1"), None, true),
        &document((1, "_Toc2"), Some(2), true),
    );
    let copy = accepted(&mut doc);
    assert_eq!(
        (texts(&copy), copy.tables().len()),
        (vec!["Intro".to_owned(), "Body.".to_owned()], 1)
    );
    let copy = rejected(&mut doc);
    assert_eq!(
        (texts(&copy), copy.tables().len()),
        (vec!["Intro".to_owned(), "Body.".to_owned()], 0)
    );
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
fn a_toc_bookmark_named_in_another_story_keeps_its_name() {
    // A footnote names the edited bookmark: renaming it in the body alone
    // would leave that name pointing at nothing, so it stays. The paragraph
    // holding the note is the same in both: edited, it would be replaced
    // whole with its note reference, which #4 handles.
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
    let mut doc = redline(
        document("_Toc1", "Body.", true),
        &document("_Toc7", "Body.", true),
    );
    assert_eq!(bookmark_names(&accepted(&mut doc)), ["_Toc7"]);
    assert_eq!(bookmark_names(&rejected(&mut doc)), ["_Toc1"]);
    // Without the note naming it, the edited bookmark takes the original name.
    let mut doc = redline(
        document("_Toc1", "Body.", false),
        &document("_Toc7", "Body.", false),
    );
    assert_eq!(bookmark_names(&accepted(&mut doc)), ["_Toc1"]);
}

#[test]
fn a_footnote_reference_moved_to_another_paragraph_is_compared() {
    let original = doc_with_notes(&[("Claim one.", Some("Source")), ("Claim two.", None)]);
    let edited = doc_with_notes(&[("Claim one.", None), ("Claim two.", Some("Source"))]);
    let mut doc = redline(original, &edited);
    // As Word shows it: the note is deleted with its old reference and
    // inserted again with the new one.
    assert_eq!(
        revision_texts(&doc, RevisionKind::Deletion),
        [" ", "Source"]
    );
    assert_eq!(
        revision_texts(&doc, RevisionKind::Insertion),
        [" ", "Source"]
    );
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Claim one.", "Claim two."]);
    assert_eq!(copy.footnotes().len(), 1);
    assert_eq!(noted_texts(&copy), ["Claim one.", "Claim two.[Source]"]);
    let copy = rejected(&mut doc);
    assert_eq!(copy.footnotes().len(), 1);
    assert_eq!(noted_texts(&copy), ["Claim one.[Source]", "Claim two."]);
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
fn swapped_footnote_references_are_compared() {
    // Both versions hold the same two notes, each referenced from the
    // other paragraph.
    let document = |swapped: bool| {
        let mut doc = Document::new();
        let first = doc.add_footnote("First");
        let second = doc.add_footnote("Second");
        let (a, b) = if swapped {
            (second, first)
        } else {
            (first, second)
        };
        doc.add_paragraph("A.").add_footnote_ref(a);
        doc.add_paragraph("B.").add_footnote_ref(b);
        doc
    };
    let mut doc = redline(document(false), &document(true));
    let copy = accepted(&mut doc);
    assert_eq!(noted_texts(&copy), ["A.[Second]", "B.[First]"]);
    assert_eq!(copy.footnotes().len(), 2);
    let copy = rejected(&mut doc);
    assert_eq!(noted_texts(&copy), ["A.[First]", "B.[Second]"]);
    assert_eq!(copy.footnotes().len(), 2);
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

#[test]
fn a_footnote_reference_moved_within_its_paragraph_is_compared() {
    let document = |at_end: bool| {
        let mut doc = Document::new();
        let id = doc.add_footnote("Source");
        let mut paragraph = doc.add_paragraph("Claim");
        if !at_end {
            paragraph.add_footnote_ref(id);
        }
        paragraph.add_run(" and more.");
        if at_end {
            paragraph.add_footnote_ref(id);
        }
        doc
    };
    let mut doc = redline(document(false), &document(true));
    assert_eq!(
        noted_texts(&accepted(&mut doc)),
        ["Claim and more.[Source]"]
    );
    assert_eq!(
        noted_texts(&rejected(&mut doc)),
        ["Claim[Source] and more."]
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
fn a_paragraph_moved_with_its_footnote_is_deleted_and_inserted_with_the_note() {
    // A move would leave its two ends naming one note, so the paragraph is
    // deleted with its note and inserted again with a new one.
    let original = doc_with_notes_in_order(&[("Claim.", Some("Source")), ("Other.", None)]);
    let edited = doc_with_notes_in_order(&[("Other.", None), ("Claim.", Some("Source"))]);
    let mut doc = redline(original, &edited);
    assert!(
        !kinds(&doc).contains(&RevisionKind::MoveFrom),
        "{:?}",
        kinds(&doc)
    );
    let copy = accepted(&mut doc);
    assert_eq!(noted_texts(&copy), ["Other.", "Claim.[Source]"]);
    assert_eq!(copy.footnotes().len(), 1);
    let copy = rejected(&mut doc);
    assert_eq!(noted_texts(&copy), ["Claim.[Source]", "Other."]);
    assert_eq!(copy.footnotes().len(), 1);
}

#[test]
fn swapped_footnote_references_numbered_by_position_are_compared() {
    // As Word saves them: each version numbers its notes in reading order.
    let original = doc_with_notes_in_order(&[("A.", Some("First")), ("B.", Some("Second"))]);
    let edited = doc_with_notes_in_order(&[("A.", Some("Second")), ("B.", Some("First"))]);
    let mut doc = redline(original, &edited);
    let copy = accepted(&mut doc);
    assert_eq!(noted_texts(&copy), ["A.[Second]", "B.[First]"]);
    assert_eq!(copy.footnotes().len(), 2);
    let copy = rejected(&mut doc);
    assert_eq!(noted_texts(&copy), ["A.[First]", "B.[Second]"]);
    assert_eq!(copy.footnotes().len(), 2);
}

#[test]
fn a_note_added_above_and_a_reference_moved_down_are_compared() {
    let original =
        doc_with_notes_in_order(&[("One.", Some("A")), ("Two.", None), ("Three.", Some("B"))]);
    let edited = doc_with_notes_in_order(&[
        ("Zero.", Some("N")),
        ("One.", None),
        ("Two.", Some("A")),
        ("Three.", Some("B")),
    ]);
    let mut doc = redline(original, &edited);
    assert_eq!(
        noted_texts(&accepted(&mut doc)),
        ["Zero.[N]", "One.", "Two.[A]", "Three.[B]"]
    );
    assert_eq!(
        noted_texts(&rejected(&mut doc)),
        ["One.[A]", "Two.", "Three.[B]"]
    );
}

#[test]
fn a_moved_note_under_a_new_id_is_compared() {
    // The edited version deleted the note and wrote a new one, under a new
    // id, at the next paragraph; the other note keeps its id.
    let original = doc_with_notes(&[("One.", Some("A")), ("Two.", None), ("Three.", Some("B"))]);
    let mut edited = doc_with_notes(&[("One.", Some("A")), ("Two.", None), ("Three.", Some("B"))]);
    let first = edited.footnotes()[0].0;
    edited.remove_footnote(first).unwrap();
    let id = edited.add_footnote("A, p. 4");
    edited.paragraph_mut(1).unwrap().add_footnote_ref(id);
    let edited = reopen(&mut edited);
    assert_eq!(noted_texts(&edited), ["One.", "Two.[A, p. 4]", "Three.[B]"]);
    let mut doc = redline(original, &edited);
    assert_eq!(
        noted_texts(&accepted(&mut doc)),
        ["One.", "Two.[A, p. 4]", "Three.[B]"]
    );
    assert_eq!(
        noted_texts(&rejected(&mut doc)),
        ["One.[A]", "Two.", "Three.[B]"]
    );
}

#[test]
fn a_note_neither_version_references_keeps_its_id() {
    let document = |source: &str| {
        let mut doc = Document::new();
        doc.add_footnote("Orphan");
        let id = doc.add_footnote(source);
        doc.add_paragraph("Claim.").add_footnote_ref(id);
        doc
    };
    let mut doc = redline(document("Src"), &document("Src, p. 4"));
    let copy = accepted(&mut doc);
    assert_eq!(noted_texts(&copy), ["Claim.[Src, p. 4]"]);
    let notes = copy
        .footnotes()
        .into_iter()
        .map(|(_, text)| text.trim().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(notes, ["Orphan", "Src, p. 4"]);
    assert_eq!(noted_texts(&rejected(&mut doc)), ["Claim.[Src]"]);
}

#[test]
fn a_note_the_edited_version_does_not_reference_keeps_the_id_a_pair_would_take() {
    // The edited version holds a note it does not reference under the id the
    // original gives the referenced note; that note keeps its id and the
    // referenced one is compared as moved to a new id.
    let original = {
        let mut doc = Document::new();
        let id = doc.add_footnote("A");
        doc.add_paragraph("Claim.").add_footnote_ref(id);
        doc
    };
    let edited = {
        let mut doc = Document::new();
        doc.add_footnote("B");
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
    let unreferenced = notes(&edited)[0].clone();
    assert_eq!(unreferenced.1, "B");
    let mut doc = redline(original, &edited);
    let copy = accepted(&mut doc);
    assert_eq!(noted_texts(&copy), ["Claim.[A]"]);
    let after = notes(&copy);
    assert_eq!(after.len(), 2);
    assert_eq!(after[0], unreferenced);
    assert_eq!(after[1].1, "A");
    let copy = rejected(&mut doc);
    assert_eq!(noted_texts(&copy), ["Claim.[A]"]);
    assert_eq!(
        notes(&copy)
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>(),
        ["A"]
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

#[test]
fn a_note_only_the_edited_version_references_is_revised_in_place() {
    // The original holds a draft note it does not reference yet; the edited
    // version rewrites it and references it.
    let document = |draft: &str, referenced: bool| {
        let mut doc = Document::new();
        let note = doc.add_footnote(draft);
        let source = doc.add_footnote("Src");
        doc.add_paragraph("One.").add_footnote_ref(source);
        let mut second = doc.add_paragraph("Two.");
        if referenced {
            second.add_footnote_ref(note);
        }
        doc
    };
    let notes = |doc: &Document| {
        doc.footnotes()
            .into_iter()
            .map(|(_, text)| text.trim().to_owned())
            .collect::<Vec<_>>()
    };
    let mut doc = redline(document("Draft", false), &document("Final", true));
    let copy = accepted(&mut doc);
    assert_eq!(noted_texts(&copy), ["One.[Src]", "Two.[Final]"]);
    assert_eq!(notes(&copy), ["Final", "Src"]);
    let copy = rejected(&mut doc);
    assert_eq!(noted_texts(&copy), ["One.[Src]", "Two."]);
    assert_eq!(notes(&copy), ["Draft", "Src"]);
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

#[test]
fn a_moved_endnote_reference_is_refused_and_changes_nothing() {
    // The comparison cannot delete and insert an endnote yet, and the
    // redline would name it from both places, which Word for Mac cannot
    // open.
    let mut doc = doc_with_endnotes(&[0]);
    let error = doc
        .compare(&doc_with_endnotes(&[1]), AUTHOR, DATE)
        .unwrap_err();
    assert!(
        error.to_string().contains("endnote") && error.to_string().contains("moved"),
        "{error}"
    );
    assert!(doc.revisions().is_empty());
    assert_eq!(endnote_paragraphs(&reopen(&mut doc)), [0]);
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

/// The indexes of the body paragraphs that hold an endnote reference.
fn endnote_paragraphs(doc: &Document) -> Vec<usize> {
    doc.paragraphs()
        .iter()
        .enumerate()
        .filter(|(_, paragraph)| {
            paragraph.runs().any(|run| {
                run.items()
                    .any(|item| matches!(item, RunItemRef::EndnoteReference(_)))
            })
        })
        .map(|(index, _)| index)
        .collect()
}

#[test]
fn an_edit_beside_an_endnote_still_compares() {
    let mut edited = doc_with_endnotes(&[0]);
    edited
        .paragraph_mut(1)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_text("Claim two, revised.");
    let mut doc = redline(doc_with_endnotes(&[0]), &edited);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Claim one.", "Claim two, revised."]);
    assert_eq!(endnote_paragraphs(&copy), [0]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), ["Claim one.", "Claim two."]);
    assert_eq!(endnote_paragraphs(&copy), [0]);
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

#[test]
fn an_edit_in_the_paragraph_of_an_endnote_still_compares() {
    let mut edited = doc_with_endnotes(&[0]);
    edited
        .paragraph_mut(0)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_text("Claim one, revised.");
    let mut doc = redline(doc_with_endnotes(&[0]), &edited);
    let copy = accepted(&mut doc);
    assert_eq!(texts(&copy), ["Claim one, revised.", "Claim two."]);
    assert_eq!(endnote_paragraphs(&copy), [0]);
    let copy = rejected(&mut doc);
    assert_eq!(texts(&copy), ["Claim one.", "Claim two."]);
    assert_eq!(endnote_paragraphs(&copy), [0]);
}

#[test]
fn a_paragraph_moved_with_its_endnote_is_refused() {
    // Both ends of the move would name the endnote.
    let document = |swapped: bool| {
        let mut doc = Document::new();
        let (first, second) = if swapped {
            ("Other.", "Claim.")
        } else {
            ("Claim.", "Other.")
        };
        doc.add_paragraph(first);
        doc.add_paragraph(second);
        let claim = usize::from(swapped);
        let location = doc.paragraph_story_location(claim).unwrap().unwrap();
        doc.create_endnote(&location, "Source").unwrap();
        doc
    };
    let mut doc = document(false);
    let error = doc.compare(&document(true), AUTHOR, DATE).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("comparison would give endnote ")
            && message.contains(
                " 2 references where each input has at most 1: its reference moved to another site, or what holds it was deleted and inserted again"
            ),
        "{message}"
    );
    assert!(doc.revisions().is_empty());
}

#[test]
fn an_endnote_the_original_holds_unreferenced_compares_while_endnotes_are_ignored() {
    // The original keeps the endnote but lost its reference; the edited
    // version references it again, and the ignored story supplies it.
    let mut original = doc_with_endnotes(&[0]);
    // set_text keeps a note mark, so the reference is cut out of the file.
    let original = {
        let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(
            original.to_bytes().unwrap(),
        ))
        .unwrap();
        let main = package.main_document_part().unwrap();
        let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
        let reference = xml.find("<w:endnoteReference").unwrap();
        let start = xml[..reference]
            .rfind("<w:r>")
            .max(xml[..reference].rfind("<w:r "))
            .unwrap();
        let end = reference + xml[reference..].find("</w:r>").unwrap() + "</w:r>".len();
        let xml = format!("{}{}", &xml[..start], &xml[end..]);
        package.set_part(&main, xml.into_bytes());
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        Document::from_bytes(&out.into_inner()).unwrap()
    };
    assert_eq!(endnote_paragraphs(&original), Vec::<usize>::new());
    let edited = doc_with_endnotes(&[0]);
    let id = (1..10)
        .find(|&id| edited.endnote_story(id).unwrap().is_some())
        .expect("the edited endnote");
    assert!(original.endnote_story(id).unwrap().is_some());
    let mut doc = original;
    let options = ComparisonOptions {
        ignored_stories: vec![ComparisonStoryKind::Endnote],
        ..Default::default()
    };
    doc.compare_with_options(&edited, AUTHOR, DATE, &options)
        .expect("the original supplies the endnote");
    let copy = accepted(&mut doc);
    assert_eq!(endnote_references(&copy), [(0, id)]);
    assert!(copy.endnote_story(id).unwrap().is_some());
    assert_eq!(endnote_paragraphs(&rejected(&mut doc)), Vec::<usize>::new());
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
        format!("ignored endnotes cannot supply the edited reference to endnote {added}")
    );
    assert!(doc.revisions().is_empty());
}

#[test]
fn an_endnote_added_while_endnotes_are_ignored_is_refused() {
    // The original's endnotes stay, so the new reference would name none.
    let mut doc = doc_with_endnotes(&[]);
    let options = ComparisonOptions {
        ignored_stories: vec![ComparisonStoryKind::Endnote],
        ..Default::default()
    };
    let error = doc
        .compare_with_options(&doc_with_endnotes(&[1]), AUTHOR, DATE, &options)
        .unwrap_err();
    assert!(error.to_string().contains("ignored endnotes"), "{error}");
    assert!(doc.revisions().is_empty());
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
fn a_merged_paragraph_takes_no_default_namespace_its_runs_do_not_use() {
    // The second paragraph declares a default namespace that no element of
    // it takes: its run moves into the first paragraph without it.
    let xml = accepted_main_part(&format!(
        r#"<w:p>{DELETED_MARK}<w:r><w:t xml:space="preserve">one </w:t></w:r></w:p><w:p xmlns="urn:example:tasks"><w:r><w:t>two</w:t></w:r></w:p>"#
    ));
    assert!(xml.contains("<w:t>two</w:t>"), "{xml}");
    assert!(!xml.contains(r#"xmlns="urn:example:tasks""#), "{xml}");
}

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
fn a_merged_paragraph_takes_no_undeclaration_its_runs_do_not_use() {
    // The second paragraph undeclares the default namespace, which no
    // element of it takes: its run moves into the first paragraph without it.
    let xml = accepted_main_part(&format!(
        r#"<w:p xmlns="urn:example:tasks">{DELETED_MARK}<w:r><w:t xml:space="preserve">one </w:t></w:r></w:p><w:p xmlns=""><w:r><w:t>two</w:t></w:r></w:p>"#
    ));
    assert!(xml.contains("<w:t>two</w:t>"), "{xml}");
    assert!(!xml.contains(r#"xmlns="""#), "{xml}");
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

/// `doc` saved with the content of story `part` (inside its root, or inside
/// note `note` of a notes part) replaced by `content`, every revision
/// accepted, and that part as the resolution wrote it.
fn accepted_story_part(mut doc: Document, part: &str, note: Option<i32>, content: &str) -> String {
    let bytes = doc.to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
    let (start, end) = match note.and_then(|id| xml.find(&format!(r#"w:id="{id}""#))) {
        Some(note) => {
            let start = note + xml[note..].find('>').unwrap() + 1;
            (start, start + xml[start..].find("</w:footnote>").unwrap())
        }
        _ => {
            let root = xml.find("?>").map_or(0, |end| end + 2);
            let start = root + xml[root..].find('>').unwrap() + 1;
            (start, xml.rfind("</w:").unwrap())
        }
    };
    let xml = format!("{}{content}{}", &xml[..start], &xml[end..]);
    package.set_part(part, xml.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    doc.accept_all().expect("accept all");
    let bytes = doc.to_bytes().unwrap();
    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap()
}

#[test]
fn merged_paragraphs_of_a_header_and_a_note_take_no_unused_default_namespace() {
    let content = format!(
        r#"<w:p>{DELETED_MARK}<w:r><w:t xml:space="preserve">one </w:t></w:r></w:p><w:p xmlns="urn:example:tasks"><w:r><w:t>two</w:t></w:r></w:p>"#
    );
    let mut with_header = doc_with(&["Body"]);
    with_header.set_header("Header");
    let header = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(
        with_header.to_bytes().unwrap(),
    ))
    .unwrap()
    .parts
    .keys()
    .find(|part| part.starts_with("/word/header"))
    .cloned()
    .expect("a header part");
    let mut with_note = Document::new();
    let id = with_note.add_footnote("Note");
    with_note.add_paragraph("Body ").add_footnote_ref(id);
    for (doc, part, note) in [
        (with_header, header.as_str(), None),
        (with_note, "/word/footnotes.xml", Some(id)),
    ] {
        let xml = accepted_story_part(doc, part, note, &content);
        assert!(xml.contains("<w:t>two</w:t>"), "{part}: {xml}");
        assert!(
            !xml.contains(r#"xmlns="urn:example:tasks""#),
            "{part}: {xml}"
        );
    }
}

#[test]
fn a_table_of_contents_removed_whole_is_compared_under_a_default_namespace() {
    // Word 2019 declares a default namespace on the document root; the
    // rejection check must still see the original's hyperlinks unchanged.
    let base = {
        let mut doc = Document::new();
        doc.add_paragraph("Intro").style("Heading1");
        let rel_id = doc.add_hyperlink_relationship("https://example.org/guide");
        doc.add_paragraph("See ")
            .add_hyperlink("the guide", &rel_id);
        let mut package =
            oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
                .unwrap();
        let main = package.main_document_part().unwrap();
        let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
        let xml = xml.replacen(
            "<w:document ",
            r#"<w:document xmlns="http://schemas.microsoft.com/office/tasks/2019/documenttasks" "#,
            1,
        );
        assert!(
            xml.contains("documenttasks"),
            "the root declares the default namespace"
        );
        package.set_part(&main, xml.into_bytes());
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        out.into_inner()
    };
    // At the start, and in the middle of the body filled by rebuild_toc.
    for (position, rebuilt) in [(0, false), (1, true)] {
        let with_toc = {
            let mut doc = Document::from_bytes(&base).unwrap();
            doc.insert_toc(position, 3).unwrap();
            if rebuilt {
                let report = doc.rebuild_toc().unwrap();
                assert_eq!(
                    (report.entry_count, report.diagnostics.len()),
                    (1, 0),
                    "{:?}",
                    report.diagnostics
                );
            }
            doc.to_bytes().unwrap()
        };
        let without = Document::from_bytes(&base).unwrap();
        for granularity in [
            ComparisonGranularity::Word,
            ComparisonGranularity::default(),
        ] {
            let mut doc = Document::from_bytes(&with_toc).unwrap();
            let options = ComparisonOptions {
                granularity,
                ..Default::default()
            };
            let diagnostics = doc
                .compare_with_options(&without, AUTHOR, DATE, &options)
                .expect("a table of contents removed whole compares");
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let mut doc = reopen(&mut doc);
            assert_eq!(texts(&accepted(&mut doc)), texts(&without));
            assert_eq!(
                texts(&rejected(&mut doc)),
                texts(&Document::from_bytes(&with_toc).unwrap())
            );
        }
    }
}
