//! Footnotes and endnotes: creating them and their references, reading,
//! editing, moving and removing them, note policies at document and section
//! level, separator entries, and notes added to Word- and Google-saved files.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use oxoffice_docx::paragraph::ParagraphRef;
use oxoffice_docx::{
    ContentFragment, ContentLocation, Document, NoteFamily, NoteNumberFormat, NotePlacement,
    NotePolicy, NoteRestart, NoteSpecialRecord, RunItemRef, SectionBreak, StoryKind,
    StoryRunPosition,
};
use oxoffice_docx_oxml::document::CT_NoteProperties;
use oxoffice_docx_oxml::text::CT_P;
use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, doc_with, reopen, texts};

use NoteFamily::{Endnote, Footnote};

/// The checked location of a body paragraph, by its index in `paragraphs()`.
fn paragraph_at(doc: &Document, index: usize) -> ContentLocation {
    doc.paragraph_story_location(index)
        .expect("paragraph location resolves")
        .expect("paragraph exists")
}

/// The boundary before run `run` of body paragraph `paragraph`.
fn boundary(doc: &Document, paragraph: usize, run: usize) -> StoryRunPosition {
    StoryRunPosition {
        location: paragraph_at(doc, paragraph),
        run_index: run,
    }
}

/// Create a note of either family at the end of a body paragraph.
fn create_note(doc: &mut Document, family: NoteFamily, paragraph: usize, text: &str) -> i32 {
    let location = paragraph_at(doc, paragraph);
    match family {
        Footnote => doc.create_footnote(&location, text),
        Endnote => doc.create_endnote(&location, text),
    }
    .expect("note is created")
}

/// The note references of a paragraph, in reading order.
fn paragraph_references(paragraph: &ParagraphRef<'_>) -> Vec<(NoteFamily, i32)> {
    paragraph
        .runs()
        .flat_map(|run| {
            run.items()
                .filter_map(|item| match item {
                    RunItemRef::FootnoteReference(id) => Some((Footnote, id)),
                    RunItemRef::EndnoteReference(id) => Some((Endnote, id)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The note references of body paragraph `index`, in reading order.
fn references(doc: &Document, index: usize) -> Vec<(NoteFamily, i32)> {
    paragraph_references(&doc.paragraph(index).expect("paragraph exists"))
}

/// The text of body paragraph `index` with each note reference shown where it
/// sits, `[f2]` for footnote 2 and `[e2]` for endnote 2.
fn marked_text(doc: &Document, index: usize) -> String {
    let paragraph = doc.paragraph(index).expect("paragraph exists");
    let mut out = String::new();
    for run in paragraph.runs() {
        for item in run.items() {
            match item {
                RunItemRef::Text(text) => out.push_str(text),
                RunItemRef::FootnoteReference(id) => out.push_str(&format!("[f{id}]")),
                RunItemRef::EndnoteReference(id) => out.push_str(&format!("[e{id}]")),
                _ => {}
            }
        }
    }
    out
}

/// The character style of each run of body paragraph `index` that holds a
/// note reference.
fn reference_styles(doc: &Document, index: usize) -> Vec<Option<String>> {
    let paragraph = doc.paragraph(index).expect("paragraph exists");
    paragraph
        .runs()
        .filter(|run| {
            run.items().any(|item| {
                matches!(
                    item,
                    RunItemRef::FootnoteReference(_) | RunItemRef::EndnoteReference(_)
                )
            })
        })
        .map(|run| run.style_id().map(str::to_owned))
        .collect()
}

/// The text of a note of either family, its paragraphs joined by new lines,
/// or `None` when the document has no such note.
fn note_text(doc: &Document, family: NoteFamily, id: i32) -> Option<String> {
    let story = match family {
        Footnote => doc.footnote_story(id),
        Endnote => doc.endnote_story(id),
    }
    .expect("note lookup succeeds")?;
    let items = doc.story_items(&story).expect("note story reads");
    let lines: Vec<String> = items
        .iter()
        .map(|item| item.text().expect("note item reads").unwrap_or_default())
        .collect();
    Some(lines.join("\n"))
}

/// The location of the first paragraph of a note, for editing it.
fn note_paragraph(doc: &Document, family: NoteFamily, id: i32) -> ContentLocation {
    let story = match family {
        Footnote => doc.footnote_story(id),
        Endnote => doc.endnote_story(id),
    }
    .expect("note lookup succeeds")
    .expect("note exists");
    doc.story_items(&story).expect("note story reads")[0]
        .location()
        .clone()
}

/// The texts of the endnotes in file order, as `footnotes()` lists footnotes.
fn endnote_texts(doc: &Document) -> Vec<String> {
    doc.stories()
        .expect("stories are listed")
        .into_iter()
        .filter(|story| story.kind() == StoryKind::Endnote)
        .map(|story| {
            let items = doc.story_items(&story).expect("endnote story reads");
            items
                .iter()
                .map(|item| item.text().expect("endnote item reads").unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

/// The four policy values as Word spells them, or `None` when one is unset:
/// placement, number format, first number, restart.
fn policy_of(properties: Option<&CT_NoteProperties>) -> Option<(&str, &str, u32, &str)> {
    let properties = properties?;
    Some((
        properties.pos.as_deref()?,
        properties.num_fmt.as_deref()?,
        properties.num_start?,
        properties.num_restart.as_deref()?,
    ))
}

/// How many separator and continuation separator entries a notes part holds.
fn separator_counts(doc: &Document, part: &str) -> (usize, usize) {
    let xml = doc.part_xml(part).expect("notes part is present");
    (
        xml.matches("w:type=\"separator\"").count(),
        xml.matches("w:type=\"continuationSeparator\"").count(),
    )
}

const FOOTNOTE_POLICY: NotePolicy = NotePolicy {
    format: NoteNumberFormat::LowerRoman,
    start: 4,
    restart: NoteRestart::EachPage,
    placement: NotePlacement::BeneathText,
};

const ENDNOTE_POLICY: NotePolicy = NotePolicy {
    format: NoteNumberFormat::UpperRoman,
    start: 3,
    restart: NoteRestart::EachSection,
    placement: NotePlacement::SectionEnd,
};

// ---- Creating ----

#[test]
fn a_footnote_at_the_end_of_a_paragraph_survives_a_save() {
    let mut doc = doc_with(&["Revenue grew by 12 %."]);
    let id = create_note(&mut doc, Footnote, 0, "Source: annual report.");
    let doc = reopen(&mut doc);
    // Word writes a space between the automatic mark and the note text.
    assert_eq!(
        doc.footnotes(),
        [(id, " Source: annual report.".to_owned())]
    );
    // The automatic mark is not text: the paragraph reads as before.
    assert_eq!(texts(&doc), ["Revenue grew by 12 %."]);
    assert_eq!(
        marked_text(&doc, 0),
        format!("Revenue grew by 12 %.[f{id}]")
    );
    assert_eq!(
        reference_styles(&doc, 0),
        [Some("FootnoteReference".to_owned())]
    );
}

#[test]
fn an_endnote_at_the_end_of_a_paragraph_survives_a_save() {
    let mut doc = doc_with(&["Churn fell to 2.1 %."]);
    let id = create_note(&mut doc, Endnote, 0, "Monthly average.");
    let doc = reopen(&mut doc);
    assert_eq!(
        note_text(&doc, Endnote, id).as_deref(),
        Some(" Monthly average.")
    );
    assert!(doc.footnotes().is_empty(), "an endnote is not a footnote");
    assert_eq!(marked_text(&doc, 0), format!("Churn fell to 2.1 %.[e{id}]"));
    assert_eq!(
        reference_styles(&doc, 0),
        [Some("EndnoteReference".to_owned())]
    );
}

#[test]
fn a_footnote_can_be_placed_at_any_run_boundary() {
    let mut doc = Document::new();
    for _ in 0..3 {
        doc.add_paragraph("alpha").add_run("beta");
    }
    let start = doc
        .create_footnote_at(&boundary(&doc, 0, 0), "at the start")
        .unwrap();
    let middle = doc
        .create_footnote_at(&boundary(&doc, 1, 1), "between the runs")
        .unwrap();
    let end = doc
        .create_footnote_at(&boundary(&doc, 2, 2), "at the end")
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(marked_text(&doc, 0), format!("[f{start}]alphabeta"));
    assert_eq!(marked_text(&doc, 1), format!("alpha[f{middle}]beta"));
    assert_eq!(marked_text(&doc, 2), format!("alphabeta[f{end}]"));
    assert_eq!(texts(&doc), ["alphabeta"; 3]);
}

#[test]
fn a_footnote_in_a_table_cell_survives_a_save_and_its_removal_clears_the_cell() {
    let mut doc = doc_with(&["Before the table"]);
    {
        let mut table = doc.add_table(1, 2);
        table.cell(0, 0).unwrap().set_text("left");
        table.cell(0, 1).unwrap().set_text("right");
    }
    // Each cell is a story of its own; the second one is cell (0, 1).
    let cell = doc
        .stories()
        .unwrap()
        .into_iter()
        .filter(|story| story.kind() == StoryKind::TableCell)
        .nth(1)
        .expect("the table has two cell stories");
    let location = doc.story_items(&cell).unwrap()[0].location().clone();
    let id = doc
        .create_footnote_at(
            &StoryRunPosition {
                location,
                run_index: 1,
            },
            "Cell note",
        )
        .unwrap();
    let cell_references = |doc: &Document| {
        paragraph_references(
            &doc.table(0)
                .unwrap()
                .cell(0, 1)
                .unwrap()
                .paragraph(0)
                .unwrap(),
        )
    };
    let mut doc = reopen(&mut doc);
    assert_eq!(cell_references(&doc), [(Footnote, id)]);
    assert_eq!(doc.footnotes(), [(id, " Cell note".to_owned())]);

    doc.remove_footnote(id).unwrap();
    let doc = reopen(&mut doc);
    assert!(cell_references(&doc).is_empty());
    assert!(doc.footnotes().is_empty());
    assert_eq!(doc.table(0).unwrap().cell(0, 1).unwrap().text(), "right");
}

#[test]
fn a_reference_where_word_cannot_hold_one_is_refused_and_changes_nothing() {
    let mut doc = doc_with(&["Body"]);
    doc.set_header("Running title");
    doc.add_table(1, 1).cell(0, 0).unwrap().set_text("cell");
    let stories = doc.stories().unwrap();
    let story_of = |kind: StoryKind| {
        let story = stories.iter().find(|story| story.kind() == kind).unwrap();
        doc.story_items(story).unwrap()[0].location().clone()
    };
    let header = story_of(StoryKind::Header);
    let cell = story_of(StoryKind::TableCell);
    let before = doc.to_bytes().unwrap();

    let in_header = StoryRunPosition {
        location: header,
        run_index: 0,
    };
    assert!(doc.create_footnote_at(&in_header, "refused").is_err());
    assert!(doc.create_endnote_at(&in_header, "refused").is_err());
    // The whole-paragraph form takes a direct body paragraph only.
    assert!(doc.create_footnote(&cell, "refused").is_err());
    // A run boundary past the last run.
    assert!(
        doc.create_endnote_at(&boundary(&doc, 0, 5), "refused")
            .is_err()
    );

    assert_eq!(doc.to_bytes().unwrap(), before);
    assert!(doc.footnotes().is_empty());
}

#[test]
fn a_first_note_brings_the_separators_word_expects_and_they_are_not_listed() {
    let mut doc = doc_with(&["one", "two", "three"]);
    let mut ids = Vec::new();
    for index in 0..3 {
        ids.push(create_note(
            &mut doc,
            Footnote,
            index,
            &format!("note {index}"),
        ));
    }
    let doc = reopen(&mut doc);
    assert_eq!(separator_counts(&doc, "word/footnotes.xml"), (1, 1));
    let listed: Vec<i32> = doc.footnotes().iter().map(|(id, _)| *id).collect();
    assert_eq!(listed, ids);
    assert!(
        ids.iter().all(|id| *id > 0),
        "separator ids are -1 and 0: {ids:?}"
    );
    let mut distinct = ids.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), ids.len(), "each note has its own id");
}

#[test]
fn footnotes_and_endnotes_count_their_ids_separately() {
    let mut doc = doc_with(&["Body"]);
    let footnote = create_note(&mut doc, Footnote, 0, "Footnote");
    let endnote = create_note(&mut doc, Endnote, 0, "Endnote");
    assert_eq!(footnote, endnote, "each family numbers its own notes");

    doc.remove_endnote(endnote).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(footnote, " Footnote".to_owned())]);
    assert_eq!(note_text(&doc, Endnote, endnote), None);
    assert_eq!(references(&doc, 0), [(Footnote, footnote)]);
}

#[test]
fn a_custom_mark_shows_in_the_body_and_opens_the_note() {
    let mut doc = doc_with(&["Marked"]);
    let dagger = doc
        .create_footnote_with_mark(&paragraph_at(&doc, 0), "Dagger note", "\u{2020}")
        .unwrap();
    let star = doc
        .create_endnote_with_mark(&paragraph_at(&doc, 0), "Star note", "*")
        .unwrap();
    let before = doc.to_bytes().unwrap();
    assert!(
        doc.create_footnote_with_mark(&paragraph_at(&doc, 0), "refused", "")
            .is_err(),
        "an empty mark is refused"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    let doc = reopen(&mut doc);
    // Unlike the automatic number, a custom mark is text in both places.
    assert_eq!(texts(&doc), ["Marked\u{2020}*"]);
    assert_eq!(references(&doc, 0), [(Footnote, dagger), (Endnote, star)]);
    assert_eq!(
        doc.footnotes(),
        [(dagger, "\u{2020} Dagger note".to_owned())]
    );
    assert_eq!(
        note_text(&doc, Endnote, star).as_deref(),
        Some("* Star note")
    );
}

#[test]
fn a_low_level_footnote_and_its_reference_pair_up() {
    let mut doc = doc_with(&["Low level"]);
    let id = doc.add_footnote("Typed note");
    doc.paragraph_mut(0).unwrap().add_footnote_ref(id);
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(id, " Typed note".to_owned())]);
    let referenced: Vec<i32> = doc
        .paragraph(0)
        .unwrap()
        .runs()
        .filter_map(|run| run.footnote_id())
        .collect();
    assert_eq!(referenced, [id]);
}

// Fails today: Paragraph::add_footnote_ref writes a reference run without the
// FootnoteReference style, so Word shows the number full size. Issue: #30
#[test]
#[should_panic(expected = "the reference is styled as a note mark")]
fn a_low_level_reference_is_styled_as_a_note_mark() {
    let mut doc = doc_with(&["Low level"]);
    let id = doc.add_footnote("Typed note");
    doc.paragraph_mut(0).unwrap().add_footnote_ref(id);
    let doc = reopen(&mut doc);
    assert!(doc.style("FootnoteReference").is_some());
    assert_eq!(
        reference_styles(&doc, 0),
        [Some("FootnoteReference".to_owned())],
        "the reference is styled as a note mark"
    );
}

// ---- Reading ----

#[test]
fn note_text_keeps_empty_unicode_markup_and_long_text() {
    let long = "continued footnote text ".repeat(400);
    let samples = [
        "",
        "مرحبا بالعالم",
        "שלום עולם",
        "脚注のテキスト，注释",
        "emoji 😀 and e\u{301}",
        "x < y & \"z\" > 'w'",
        long.as_str(),
    ];
    let mut doc = doc_with(&["paragraph"; 7]);
    let mut expected = Vec::new();
    for (index, text) in samples.iter().enumerate() {
        let id = create_note(&mut doc, Footnote, index, text);
        expected.push((id, format!(" {text}")));
    }
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), expected);
}

#[test]
fn a_note_can_hold_several_paragraphs() {
    let mut doc = doc_with(&["Body"]);
    let id = create_note(&mut doc, Footnote, 0, "First paragraph.");
    let story = doc.footnote_story(id).unwrap().unwrap();
    let mut paragraph = CT_P::new();
    paragraph.add_run("Second paragraph.");
    doc.insert_content(
        &ContentLocation::end(story),
        ContentFragment::paragraph(paragraph).unwrap(),
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.footnotes(),
        [(id, " First paragraph.\nSecond paragraph.".to_owned())]
    );
    let story = doc.footnote_story(id).unwrap().unwrap();
    assert_eq!(doc.story_items(&story).unwrap().len(), 2);
}

#[test]
fn notes_are_stories_and_follow_the_body_in_the_text_report() {
    let mut doc = doc_with(&["Body text"]);
    create_note(&mut doc, Footnote, 0, "Foot text");
    create_note(&mut doc, Endnote, 0, "End text");
    let doc = reopen(&mut doc);
    let kinds: Vec<StoryKind> = doc.stories().unwrap().iter().map(|s| s.kind()).collect();
    assert_eq!(
        kinds,
        [StoryKind::Body, StoryKind::Footnote, StoryKind::Endnote]
    );
    let others: Vec<(StoryKind, Vec<String>)> = doc
        .other_story_texts()
        .unwrap()
        .into_iter()
        .map(|story| {
            let lines = story.items.into_iter().map(|item| item.text).collect();
            (story.story.kind(), lines)
        })
        .collect();
    assert_eq!(
        others,
        [
            (StoryKind::Footnote, vec![" Foot text".to_owned()]),
            (StoryKind::Endnote, vec![" End text".to_owned()]),
        ]
    );
    let report = doc.text_report().text;
    let body = report.find("Body text").expect("body in the report");
    let foot = report.find("Foot text").expect("footnote in the report");
    let end = report.find("End text").expect("endnote in the report");
    assert!(body < foot && foot < end, "{report}");
}

#[test]
fn looking_up_a_note_that_does_not_exist_finds_nothing() {
    let mut doc = doc_with(&["Body"]);
    // No notes part yet.
    assert_eq!(doc.footnote_story(2).unwrap(), None);
    assert_eq!(doc.endnote_story(2).unwrap(), None);

    let id = create_note(&mut doc, Footnote, 0, "Only note");
    assert!(doc.footnote_story(id).unwrap().is_some());
    assert_eq!(doc.footnote_story(id + 1).unwrap(), None);
    assert_eq!(doc.endnote_story(id).unwrap(), None);
    // The separator entries are not notes.
    assert_eq!(doc.footnote_story(-1).unwrap(), None);
    assert_eq!(doc.footnote_story(0).unwrap(), None);
}

#[test]
fn an_authored_separator_is_not_counted_as_a_note() {
    let mut doc = doc_with(&["Body"]);
    let id = create_note(&mut doc, Footnote, 0, "User note");
    let mut rule = CT_P::new();
    rule.add_run("~~~~");
    let separator = doc
        .set_note_special_record(Footnote, NoteSpecialRecord::Separator, vec![rule])
        .unwrap();
    let mut notice = CT_P::new();
    notice.add_run("Continued on the next page");
    let continuation = doc
        .set_note_special_record(
            Footnote,
            NoteSpecialRecord::ContinuationNotice,
            vec![notice],
        )
        .unwrap();
    let doc = reopen(&mut doc);

    assert_eq!(doc.footnotes(), [(id, " User note".to_owned())]);
    assert_eq!(doc.footnote_story(separator).unwrap(), None);
    assert_eq!(doc.footnote_story(continuation).unwrap(), None);
    let selected = &doc.note_properties(Footnote).unwrap().special_references;
    assert!(
        selected.contains(&separator) && selected.contains(&continuation),
        "{selected:?}"
    );
    let xml = doc.part_xml("word/footnotes.xml").unwrap();
    assert!(xml.contains("~~~~") && xml.contains("Continued on the next page"));
}

// ---- Editing ----

#[test]
fn editing_note_text_keeps_its_mark_and_survives_a_save() {
    for (family, part, mark) in [
        (Footnote, "word/footnotes.xml", "<w:footnoteRef/>"),
        (Endnote, "word/endnotes.xml", "<w:endnoteRef/>"),
    ] {
        let mut doc = doc_with(&["one", "two"]);
        let first = create_note(&mut doc, family, 0, "first note");
        let second = create_note(&mut doc, family, 1, "second note");
        let paragraph = note_paragraph(&doc, family, first);
        doc.set_story_text(&paragraph, "édité, 改訂").unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(
            note_text(&doc, family, first).as_deref(),
            Some("édité, 改訂")
        );
        assert_eq!(
            note_text(&doc, family, second).as_deref(),
            Some(" second note")
        );
        // The automatic number in the note is not text, and the edit keeps it.
        let xml = doc.part_xml(part).unwrap();
        assert_eq!(xml.matches(mark).count(), 2, "{family:?}: {xml}");
    }
}

#[test]
fn replacing_text_reaches_footnotes_and_endnotes() {
    let mut doc = doc_with(&["Draft body"]);
    let footnote = create_note(&mut doc, Footnote, 0, "Draft footnote");
    let endnote = create_note(&mut doc, Endnote, 0, "Draft endnote");
    assert_eq!(doc.replace_text("Draft", "Final"), 3);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Final body"]);
    assert_eq!(doc.footnotes(), [(footnote, " Final footnote".to_owned())]);
    assert_eq!(
        note_text(&doc, Endnote, endnote).as_deref(),
        Some(" Final endnote")
    );
}

#[test]
fn replacing_text_around_a_reference_keeps_the_reference() {
    let mut doc = Document::new();
    doc.add_paragraph("alpha").add_run("beta");
    let id = doc
        .create_footnote_at(&boundary(&doc, 0, 1), "between")
        .unwrap();
    assert_eq!(texts(&doc), ["alphabeta"]);
    assert_eq!(doc.replace_text("alphabeta", "gamma"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["gamma"]);
    assert_eq!(references(&doc, 0), [(Footnote, id)]);
    assert_eq!(doc.footnotes(), [(id, " between".to_owned())]);
}

// ---- Moving ----

#[test]
fn moving_a_note_reorders_the_notes_and_leaves_the_references_in_place() {
    let mut doc = doc_with(&["one", "two"]);
    let first = create_note(&mut doc, Footnote, 0, "first");
    let first_end = create_note(&mut doc, Endnote, 0, "first end");
    let second = create_note(&mut doc, Footnote, 1, "second");
    let second_end = create_note(&mut doc, Endnote, 1, "second end");
    doc.move_footnote_before(second, first).unwrap();
    doc.move_endnote_before(second_end, first_end).unwrap();
    doc.move_footnote_before(first, first).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.footnotes(),
        [(second, " second".to_owned()), (first, " first".to_owned())]
    );
    assert_eq!(endnote_texts(&doc), [" second end", " first end"]);
    assert_eq!(
        references(&doc, 0),
        [(Footnote, first), (Endnote, first_end)]
    );
    assert_eq!(
        references(&doc, 1),
        [(Footnote, second), (Endnote, second_end)]
    );
}

#[test]
fn a_moved_paragraph_takes_its_note_reference_along() {
    let mut doc = doc_with(&["Moves", "Stays"]);
    let id = create_note(&mut doc, Footnote, 0, "Travelling note");
    let fragment = doc.remove_content_at(&paragraph_at(&doc, 0)).unwrap();
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap();
    doc.insert_content(&ContentLocation::end(body), fragment)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Stays", "Moves"]);
    assert_eq!(references(&doc, 1), [(Footnote, id)]);
    assert_eq!(doc.footnotes(), [(id, " Travelling note".to_owned())]);
}

#[test]
fn moving_or_removing_a_missing_note_is_refused_and_changes_nothing() {
    let mut doc = doc_with(&["Body"]);
    let id = create_note(&mut doc, Footnote, 0, "Only note");
    let before = doc.to_bytes().unwrap();
    assert!(doc.remove_footnote(id + 1).is_err());
    assert!(doc.remove_endnote(id).is_err());
    assert!(doc.move_footnote_before(id, id + 1).is_err());
    assert!(doc.move_endnote_before(id, id).is_err());
    // A separator entry is not a note either.
    assert!(doc.remove_footnote(-1).is_err());
    assert!(doc.move_footnote_before(id, 0).is_err());
    assert_eq!(doc.to_bytes().unwrap(), before);
}

// ---- Removing ----

#[test]
fn removing_a_footnote_removes_its_reference_and_keeps_the_others() {
    let mut doc = doc_with(&["one", "two"]);
    let first = create_note(&mut doc, Footnote, 0, "first");
    let second = create_note(&mut doc, Footnote, 1, "second");
    doc.remove_footnote(first).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(second, " second".to_owned())]);
    assert!(references(&doc, 0).is_empty());
    assert_eq!(references(&doc, 1), [(Footnote, second)]);
    assert_eq!(texts(&doc), ["one", "two"]);
}

#[test]
fn removing_a_note_referenced_twice_clears_both_references() {
    let mut doc = doc_with(&["one", "two"]);
    let id = create_note(&mut doc, Footnote, 0, "Shared");
    doc.paragraph_mut(1).unwrap().add_footnote_ref(id);
    doc.remove_footnote(id).unwrap();
    let doc = reopen(&mut doc);
    assert!(doc.footnotes().is_empty());
    assert!(references(&doc, 0).is_empty());
    assert!(references(&doc, 1).is_empty());
}

#[test]
fn removing_the_last_footnote_keeps_the_separators_for_the_next_one() {
    let mut doc = doc_with(&["Body"]);
    let id = create_note(&mut doc, Footnote, 0, "Gone");
    doc.remove_footnote(id).unwrap();
    let mut doc = reopen(&mut doc);
    assert!(doc.footnotes().is_empty());
    assert!(references(&doc, 0).is_empty());
    assert_eq!(separator_counts(&doc, "word/footnotes.xml"), (1, 1));

    let again = create_note(&mut doc, Footnote, 0, "Again");
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(again, " Again".to_owned())]);
    assert_eq!(separator_counts(&doc, "word/footnotes.xml"), (1, 1));
}

#[test]
fn removing_a_custom_marked_note_removes_its_mark_from_the_body() {
    let mut doc = doc_with(&["Marked"]);
    let dagger = doc
        .create_footnote_with_mark(&paragraph_at(&doc, 0), "Dagger note", "\u{2020}")
        .unwrap();
    let star = doc
        .create_endnote_with_mark(&paragraph_at(&doc, 0), "Star note", "*")
        .unwrap();
    doc.remove_footnote(dagger).unwrap();
    doc.remove_endnote(star).unwrap();
    let doc = reopen(&mut doc);
    assert!(doc.footnotes().is_empty());
    assert!(references(&doc, 0).is_empty());
    assert_eq!(
        texts(&doc),
        ["Marked"],
        "the custom marks left the body with their notes"
    );
}

/// `doc` saved, its main part rewritten by `edit`, and opened again.
fn with_main_part(doc: &mut Document, edit: impl FnOnce(&str) -> String) -> Document {
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let main = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
    package.set_part(&main, edit(&xml).into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

/// The span of the run of `xml` that holds the footnote reference to `id`.
fn reference_run(xml: &str, id: i32) -> std::ops::Range<usize> {
    let reference = xml
        .match_indices("<w:footnoteReference ")
        .map(|(at, _)| at)
        .find(|&at| xml[at..at + xml[at..].find('>').unwrap()].contains(&format!(r#"w:id="{id}""#)))
        .expect("reference to the note");
    let start = xml[..reference]
        .rfind("<w:r>")
        .max(xml[..reference].rfind("<w:r "))
        .expect("run of the reference");
    let end = reference + xml[reference..].find("</w:r>").unwrap() + "</w:r>".len();
    start..end
}

#[test]
fn removing_a_note_keeps_the_text_after_its_reference() {
    // Only a custom mark follows its reference into removal: text after a
    // numbered note's reference, in the same run, stays.
    let mut doc = doc_with(&["Claim"]);
    let id = create_note(&mut doc, Footnote, 0, "Source");
    let mut doc = with_main_part(&mut doc, |xml| {
        let close = reference_run(xml, id).end - "</w:r>".len();
        format!(
            r#"{}<w:t xml:space="preserve"> and more.</w:t>{}"#,
            &xml[..close],
            &xml[close..]
        )
    });
    assert_eq!(texts(&doc), ["Claim and more."]);
    doc.remove_footnote(id).unwrap();
    let doc = reopen(&mut doc);
    assert!(doc.footnotes().is_empty());
    assert_eq!(texts(&doc), ["Claim and more."]);
}

#[test]
fn removing_a_custom_marked_note_in_a_deletion_removes_its_deleted_mark() {
    // A tracked deletion holds the mark as deleted text; rejecting the
    // deletion after the note is gone must not bring the mark back.
    let mut doc = doc_with(&["Marked"]);
    let dagger = doc
        .create_footnote_with_mark(&paragraph_at(&doc, 0), "Dagger note", "\u{2020}")
        .unwrap();
    let mut doc = with_main_part(&mut doc, |xml| {
        let run = reference_run(xml, dagger);
        let deleted = xml[run.clone()]
            .replace("<w:t>", "<w:delText>")
            .replace("<w:t ", "<w:delText ")
            .replace("</w:t>", "</w:delText>");
        format!(
            r#"{}<w:del w:id="90" w:author="Reviewer" w:date="2026-01-01T00:00:00Z">{deleted}</w:del>{}"#,
            &xml[..run.start],
            &xml[run.end..]
        )
    });
    assert!(
        doc.part_xml("word/document.xml")
            .unwrap()
            .contains("<w:delText")
    );
    doc.remove_footnote(dagger).unwrap();
    let mut doc = reopen(&mut doc);
    assert!(doc.footnotes().is_empty());
    let main = doc.part_xml("word/document.xml").unwrap();
    assert!(!main.contains('\u{2020}'), "{main}");
    doc.reject_all().unwrap();
    assert_eq!(texts(&reopen(&mut doc)), ["Marked"]);
}

#[test]
fn removing_custom_marked_notes_of_one_run_removes_each_mark() {
    // A symbol mark after `customMarkFollows="true"`, then a text mark after
    // `"on"` and some whitespace, both in one run.
    let mut doc = doc_with(&["Marked"]);
    let first = doc
        .create_footnote_with_mark(&paragraph_at(&doc, 0), "First note", "*")
        .unwrap();
    let second = doc
        .create_footnote_with_mark(&paragraph_at(&doc, 0), "Second note", "+")
        .unwrap();
    let mut doc = with_main_part(&mut doc, |xml| {
        let (from, to) = (
            reference_run(xml, first).start,
            reference_run(xml, second).end,
        );
        format!(
            concat!(
                "{}<w:r>",
                r#"<w:footnoteReference w:customMarkFollows="true" w:id="{}"/>"#,
                r#"<w:sym w:font="Symbol" w:char="F0A7"/>"#,
                r#"<w:footnoteReference w:customMarkFollows="on" w:id="{}"/> "#,
                "<w:t>\u{2021}</w:t></w:r>{}"
            ),
            &xml[..from],
            first,
            second,
            &xml[to..]
        )
    });
    doc.remove_footnote(first).unwrap();
    let mut doc = reopen(&mut doc);
    let main = doc.part_xml("word/document.xml").unwrap();
    assert!(!main.contains("w:sym"), "{main}");
    assert!(main.contains('\u{2021}'), "{main}");
    assert_eq!(doc.footnotes().len(), 1);
    doc.remove_footnote(second).unwrap();
    let doc = reopen(&mut doc);
    let main = doc.part_xml("word/document.xml").unwrap();
    assert!(!main.contains('\u{2021}'), "{main}");
    assert!(doc.footnotes().is_empty());
    assert_eq!(texts(&doc), ["Marked"]);
}

// Fails today: removing a paragraph keeps the footnotes it referenced, which
// `footnotes()` then lists although nothing references them. Issue: #30
#[test]
#[should_panic(expected = "the note went with its paragraph")]
fn removing_a_paragraph_removes_the_notes_it_references() {
    let mut doc = doc_with(&["Kept", "Removed"]);
    let kept = create_note(&mut doc, Footnote, 0, "Kept note");
    create_note(&mut doc, Footnote, 1, "Removed note");
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept"]);
    assert_eq!(
        doc.footnotes(),
        [(kept, " Kept note".to_owned())],
        "the note went with its paragraph"
    );
}

// ---- Note policies ----

#[test]
fn a_document_note_policy_survives_a_save() {
    let mut doc = doc_with(&["Body"]);
    let id = create_note(&mut doc, Footnote, 0, "Kept note");
    doc.set_note_policy(Footnote, FOOTNOTE_POLICY).unwrap();
    doc.set_note_policy(
        Endnote,
        NotePolicy {
            format: NoteNumberFormat::UpperLetter,
            start: 1,
            restart: NoteRestart::Continuous,
            placement: NotePlacement::DocumentEnd,
        },
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        policy_of(doc.note_properties(Footnote)),
        Some(("beneathText", "lowerRoman", 4, "eachPage"))
    );
    assert_eq!(
        policy_of(doc.note_properties(Endnote)),
        Some(("docEnd", "upperLetter", 1, "continuous"))
    );
    assert_eq!(doc.footnotes(), [(id, " Kept note".to_owned())]);
}

#[test]
fn an_invalid_note_policy_is_refused_and_changes_nothing() {
    let mut doc = doc_with(&["Body"]);
    create_note(&mut doc, Footnote, 0, "Note");
    let before = doc.to_bytes().unwrap();
    let footnotes_at_section_end = NotePolicy {
        placement: NotePlacement::SectionEnd,
        ..FOOTNOTE_POLICY
    };
    let endnotes_at_page_bottom = NotePolicy {
        placement: NotePlacement::PageBottom,
        ..ENDNOTE_POLICY
    };
    let starting_at_zero = NotePolicy {
        start: 0,
        ..FOOTNOTE_POLICY
    };
    assert!(
        doc.set_note_policy(Footnote, footnotes_at_section_end)
            .is_err()
    );
    assert!(
        doc.set_note_policy(Endnote, endnotes_at_page_bottom)
            .is_err()
    );
    assert!(doc.set_note_policy(Footnote, starting_at_zero).is_err());
    let mut section = doc.section_mut(0).unwrap();
    assert!(
        section
            .set_note_policy(Footnote, footnotes_at_section_end)
            .is_err()
    );
    assert!(
        section
            .set_note_policy(Endnote, endnotes_at_page_bottom)
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert!(doc.note_properties(Footnote).is_none());
    assert!(doc.section(0).unwrap().footnote_properties().is_none());
}

#[test]
fn a_section_note_policy_stays_in_its_section_and_can_be_removed() {
    let mut doc = doc_with(&["First section"]);
    doc.add_paragraph("End of the first section")
        .section_break(SectionBreak::NextPage);
    doc.add_paragraph("Second section");
    assert_eq!(doc.section_count(), 2);
    doc.section_mut(0)
        .unwrap()
        .set_note_policy(Endnote, ENDNOTE_POLICY)
        .unwrap();
    let mut doc = reopen(&mut doc);
    let expected = Some(("sectEnd", "upperRoman", 3, "eachSect"));
    assert_eq!(
        policy_of(doc.section(0).unwrap().endnote_properties()),
        expected
    );
    assert!(doc.section(1).unwrap().endnote_properties().is_none());
    assert!(doc.note_properties(Endnote).is_none(), "no document policy");

    let removed = doc.section_mut(0).unwrap().remove_note_policy(Endnote);
    assert_eq!(policy_of(removed.as_ref()), expected);
    let doc = reopen(&mut doc);
    let left = doc.section(0).unwrap().endnote_properties();
    assert!(
        left.is_none_or(|p| p.pos.is_none()
            && p.num_fmt.is_none()
            && p.num_start.is_none()
            && p.num_restart.is_none()),
        "every field of the policy goes: {left:?}"
    );
}

// ---- Word- and Google-saved files ----

#[test]
fn the_word_report_lists_no_notes_and_keeps_its_note_parts_byte_for_byte() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files/word/report.docx");
    let original = fs::read(path).unwrap();
    let package = OpcPackage::from_reader(Cursor::new(&original)).unwrap();
    let doc = corpus_doc("word/report.docx");
    // Word saves a separator and a continuation separator for each family.
    assert!(doc.footnotes().is_empty());
    assert!(endnote_texts(&doc).is_empty());
    assert_eq!(
        doc.note_properties(Footnote).unwrap().special_references,
        [-1, 0]
    );
    for part in ["word/footnotes.xml", "word/endnotes.xml"] {
        let saved = doc.part_xml(part).unwrap();
        let source = package.get_part(&format!("/{part}")).unwrap();
        assert!(saved.as_bytes() == source, "{part} changed on a no-op save");
    }
}

#[test]
fn a_footnote_between_word_runs_survives_a_save() {
    let mut doc = corpus_doc("word/report.docx");
    let before = texts(&doc);
    // Paragraph 13 is split into runs around "6 h 40" and "3 h 15".
    let id = doc
        .create_footnote_at(&boundary(&doc, 13, 2), "Median over the quarter.")
        .unwrap();
    let doc = reopen(&mut doc);
    assert!(id > 0, "Word's separators keep -1 and 0: {id}");
    assert_eq!(
        doc.footnotes(),
        [(id, " Median over the quarter.".to_owned())]
    );
    assert_eq!(texts(&doc), before);
    let marked = marked_text(&doc, 13);
    assert!(
        marked.starts_with(&format!(
            "Median first response time went from 6 h 40[f{id}] to "
        )),
        "{marked}"
    );
    assert_eq!(separator_counts(&doc, "word/footnotes.xml"), (1, 1));
    assert_eq!(
        doc.note_properties(Footnote).unwrap().special_references,
        [-1, 0]
    );
}

#[test]
fn an_endnote_added_to_the_word_report_reuses_its_endnotes_part() {
    let mut doc = corpus_doc("word/report.docx");
    let id = create_note(&mut doc, Endnote, 3, "Unaudited figure.");
    let doc = reopen(&mut doc);
    assert_eq!(
        note_text(&doc, Endnote, id).as_deref(),
        Some(" Unaudited figure.")
    );
    assert_eq!(references(&doc, 3), [(Endnote, id)]);
    let note_parts: Vec<String> = doc
        .part_names()
        .unwrap()
        .into_iter()
        .filter(|name| name.contains("endnotes"))
        .collect();
    assert_eq!(note_parts, ["/word/endnotes.xml"]);
    assert_eq!(separator_counts(&doc, "word/endnotes.xml"), (1, 1));
    assert!(doc.footnotes().is_empty());
}

#[test]
fn notes_added_to_the_google_report_create_their_parts() {
    let mut doc = corpus_doc("google/report.docx");
    assert!(
        !doc.part_names()
            .unwrap()
            .iter()
            .any(|name| name.contains("notes")),
        "Google saves no notes part when there is no note"
    );
    let footnote = create_note(&mut doc, Footnote, 1, "Google footnote");
    let endnote = create_note(&mut doc, Endnote, 3, "Google endnote");
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(footnote, " Google footnote".to_owned())]);
    assert_eq!(
        note_text(&doc, Endnote, endnote).as_deref(),
        Some(" Google endnote")
    );
    assert_eq!(references(&doc, 1), [(Footnote, footnote)]);
    assert_eq!(references(&doc, 3), [(Endnote, endnote)]);
    assert!(doc.style("FootnoteReference").is_some());
    assert!(doc.style("EndnoteReference").is_some());
    let relationships = doc.part_xml("word/_rels/document.xml.rels").unwrap();
    assert!(relationships.contains("relationships/footnotes"));
    assert!(relationships.contains("relationships/endnotes"));
    let content_types = doc.part_xml("[Content_Types].xml").unwrap();
    assert!(content_types.contains("footnotes+xml"));
    assert!(content_types.contains("endnotes+xml"));
}

#[test]
fn a_policy_on_the_word_report_is_removed_without_its_separator_selection() {
    let mut doc = corpus_doc("word/report.docx");
    assert_eq!(
        doc.remove_note_policy(Footnote).unwrap(),
        None,
        "Word's footnotePr selects separators only"
    );
    doc.set_note_policy(Footnote, FOOTNOTE_POLICY).unwrap();
    let mut doc = reopen(&mut doc);
    let properties = doc.note_properties(Footnote).unwrap();
    assert_eq!(
        policy_of(Some(properties)),
        Some(("beneathText", "lowerRoman", 4, "eachPage"))
    );
    assert_eq!(properties.special_references, [-1, 0]);

    let removed = doc.remove_note_policy(Footnote).unwrap();
    assert!(policy_of(removed.as_ref()).is_some());
    let doc = reopen(&mut doc);
    let properties = doc.note_properties(Footnote).unwrap();
    // Every field of the policy goes, not only one of them.
    assert_eq!(
        (
            properties.pos.as_deref(),
            properties.num_fmt.as_deref(),
            properties.num_start,
            properties.num_restart.as_deref()
        ),
        (None, None, None, None)
    );
    assert_eq!(properties.special_references, [-1, 0]);
}

#[test]
fn a_section_policy_on_the_word_report_leaves_its_page_layout() {
    let mut doc = corpus_doc("word/report.docx");
    let layout = |doc: &Document| {
        doc.sections()
            .map(|section| (section.page_size(), section.orientation()))
            .collect::<Vec<_>>()
    };
    let before = layout(&doc);
    // The second section is the landscape one.
    doc.section_mut(1)
        .unwrap()
        .set_note_policy(Endnote, ENDNOTE_POLICY)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(layout(&doc), before);
    assert_eq!(
        policy_of(doc.section(1).unwrap().endnote_properties()),
        Some(("sectEnd", "upperRoman", 3, "eachSect"))
    );
    assert!(doc.section(0).unwrap().endnote_properties().is_none());
}

// ---- Round trips ----

#[test]
fn a_document_with_notes_saves_the_same_bytes_every_time() {
    let mut doc = doc_with(&["one", "two"]);
    create_note(&mut doc, Footnote, 0, "Plain footnote");
    doc.create_footnote_with_mark(&paragraph_at(&doc, 1), "Marked footnote", "*")
        .unwrap();
    create_note(&mut doc, Endnote, 1, "Plain endnote");
    doc.set_note_policy(Footnote, FOOTNOTE_POLICY).unwrap();
    let mut once = reopen(&mut doc);
    let first = once.to_bytes().unwrap();
    let mut twice = Document::from_bytes(&first).unwrap();
    assert_eq!(twice.to_bytes().unwrap(), first);
    assert_eq!(twice.footnotes(), once.footnotes());
}
