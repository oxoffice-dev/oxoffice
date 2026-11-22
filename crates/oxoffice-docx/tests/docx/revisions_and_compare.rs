//! Tracked changes: the track revisions setting, reading revisions (kind,
//! author, date, story, text), accepting and rejecting them, and comparing two
//! versions of a document into a redline that resolves back to either one.

use std::collections::HashMap;

use oxoffice_docx::{
    Alignment, BodyItemRef, ComparisonGranularity, ComparisonOptions, ComparisonStoryKind,
    Document, RenderOptions, RevisionKind, RevisionStyle, RevisionView, RunItemRef, StoryKind,
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

// ---- Comparing editors' files ----
// A comparison of the corpus report costs seconds in a debug build, so these
// tests compare once and read the redline back through its resolved copy.

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
    // name uses, and an oxoffice save of the edited copy leaves it out: the
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
fn a_simple_field_inside_a_hyperlink_keeps_the_whole_paragraph_refusal() {
    let linked = r#"<w:p><w:hyperlink w:anchor="_Top"><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:hyperlink></w:p>"#;
    let after = plain("After.");
    let refused = |original: &str, edited: &str| {
        let mut doc = doc_with_body(original);
        let error = doc
            .compare(&doc_with_body(edited), AUTHOR, DATE)
            .expect_err("refused");
        assert!(
            error
                .to_string()
                .contains("cannot track a whole paragraph's simple field"),
            "{error}"
        );
    };
    // Inserted and deleted whole.
    refused(&after, &format!("{linked}{after}"));
    refused(&format!("{linked}{after}"), &after);
    // Paired, its hyperlink pointing elsewhere: written as a deleted and an
    // inserted paragraph, refused the same way.
    let relinked = linked.replace("_Top", "_Bottom");
    refused(&format!("{linked}{after}"), &format!("{relinked}{after}"));
}

#[test]
fn a_simple_field_nested_in_another_keeps_a_refusal_when_its_paragraph_goes_whole() {
    // The conversion writes one complex field; a simple field in its result
    // has no qualified place there, and the comparison says so.
    let nested = r#"<w:p><w:fldSimple w:instr=" IF 1 = 1 "><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:fldSimple></w:p>"#;
    let after = plain("After.");
    for (original, edited) in [
        (after.clone(), format!("{nested}{after}")),
        (format!("{nested}{after}"), after.clone()),
    ] {
        let mut doc = doc_with_body(&original);
        let error = doc
            .compare(&doc_with_body(&edited), AUTHOR, DATE)
            .expect_err("refused");
        assert!(error.to_string().contains("fldSimple"), "{error}");
        assert!(doc.revisions().is_empty());
    }
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
