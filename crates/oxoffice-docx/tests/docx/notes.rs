//! Footnotes and endnotes: creating them and their references, reading,
//! editing, moving and removing them, note policies at document and section
//! level, separator entries, and notes added to Word- and Google-saved files.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use oxoffice_docx::paragraph::ParagraphRef;
use oxoffice_docx::{
    ContentFragment, ContentLocation, Document, NoteFamily, NoteNumberFormat, NotePlacement,
    NotePolicy, NoteRestart, NoteSpecialRecord, RunItemRef, SectionBreak, StoryItemKind, StoryKind,
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

#[test]
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

/// The `w:style` element of `id` in a styles part, if any.
fn style_element<'a>(styles: &'a str, id: &str) -> Option<&'a str> {
    let start = styles.find(&format!(r#"w:styleId="{id}""#))?;
    let start = styles[..start].rfind("<w:style ")?;
    let end = start + styles[start..].find("</w:style>")? + "</w:style>".len();
    Some(&styles[start..end])
}

#[test]
fn a_low_level_reference_brings_back_a_note_mark_style_the_file_lacks() {
    // `add_footnote` adds the style and names it on the note's own mark; a
    // file can still lack both, as a localized Word file names its own.
    let mut doc = doc_with(&["Low level"]);
    let id = doc.add_footnote("Typed note");
    let mut doc = without_note_mark_style(&mut doc);
    let mut saved = reopen(&mut doc);
    assert!(saved.style("FootnoteReference").is_none());
    saved.paragraph_mut(0).unwrap().add_footnote_ref(id);
    let doc = reopen(&mut saved);
    let styles = doc.part_xml("word/styles.xml").unwrap();
    let style = style_element(&styles, "FootnoteReference").expect("the note mark style");
    assert!(
        style.contains(r#"w:vertAlign w:val="superscript""#),
        "{style}"
    );
    assert!(style.contains(r#"w:val="footnote reference""#), "{style}");
}

/// `doc` saved without the note mark style, in its styles part and on the
/// marks of its notes.
fn without_note_mark_style(doc: &mut Document) -> Document {
    let mut doc = with_part(doc, "/word/footnotes.xml", |xml| {
        xml.replace(r#"<w:rStyle w:val="FootnoteReference"/>"#, "")
    });
    let doc = with_part(&mut doc, "/word/styles.xml", |xml| {
        let style = style_element(xml, "FootnoteReference").expect("the note mark style");
        xml.replacen(style, "", 1)
    });
    assert!(
        !doc.part_xml("word/footnotes.xml")
            .unwrap()
            .contains("FootnoteReference")
    );
    doc
}

#[test]
fn a_hyperlink_and_a_note_mark_style_both_come_back_in_one_save() {
    let mut doc = doc_with(&["Low level"]);
    let id = doc.add_footnote("Typed note");
    let mut doc = without_note_mark_style(&mut doc);
    assert!(doc.style("Hyperlink").is_none());
    let rel_id = doc.add_hyperlink_relationship("https://example.com/");
    let mut paragraph = doc.paragraph_mut(0).unwrap();
    paragraph.add_hyperlink("a link", &rel_id);
    paragraph.add_footnote_ref(id);
    let doc = reopen(&mut doc);
    assert!(doc.style("Hyperlink").is_some());
    assert!(doc.style("FootnoteReference").is_some());
}

#[test]
fn only_the_missing_style_a_run_names_comes_back() {
    // Both styles are missing and only a hyperlink names one.
    let mut doc = doc_with(&["Low level"]);
    doc.add_footnote("Typed note");
    let mut doc = without_note_mark_style(&mut doc);
    let rel_id = doc.add_hyperlink_relationship("https://example.com/");
    doc.paragraph_mut(0)
        .unwrap()
        .add_hyperlink("a link", &rel_id);
    let doc = reopen(&mut doc);
    assert!(doc.style("Hyperlink").is_some());
    assert!(doc.style("FootnoteReference").is_none());
}

#[test]
fn a_dangling_low_level_reference_adds_no_note_mark_style_without_notes() {
    let mut doc = doc_with(&["Low level"]);
    doc.paragraph_mut(0).unwrap().add_footnote_ref(7);
    let doc = reopen(&mut doc);
    assert!(doc.style("FootnoteReference").is_none());
}

#[test]
fn an_edited_word_file_without_notes_gains_no_note_mark_style() {
    let mut doc = corpus_doc("word/report.docx");
    assert!(doc.style("FootnoteReference").is_none());
    doc.add_paragraph("Added.");
    let doc = reopen(&mut doc);
    assert!(doc.style("FootnoteReference").is_none());
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
        let run = &xml[reference_run(xml, dagger)];
        // The expanded form of a rendered page break sits before the mark.
        let mark = run
            .find("<w:t ")
            .or_else(|| run.find("<w:t>"))
            .expect("the mark");
        let run = format!(
            "{}<w:lastRenderedPageBreak></w:lastRenderedPageBreak>{}",
            &run[..mark],
            &run[mark..]
        );
        let deleted = run
            .replace("<w:t>", "<w:delText>")
            .replace("<w:t ", "<w:delText ")
            .replace("</w:t>", "</w:delText>");
        let span = reference_run(xml, dagger);
        format!(
            r#"{}<w:del w:id="90" w:author="Reviewer" w:date="2026-01-01T00:00:00Z">{deleted}</w:del>{}"#,
            &xml[..span.start],
            &xml[span.end..]
        )
    });
    assert!(
        doc.part_xml("word/document.xml")
            .unwrap()
            .contains("<w:delText")
    );
    let before = doc.part_xml("word/document.xml").unwrap();
    assert!(
        before.contains("<w:lastRenderedPageBreak></w:lastRenderedPageBreak>"),
        "{before}"
    );
    doc.remove_footnote(dagger).unwrap();
    let main = doc.part_xml("word/document.xml").unwrap();
    assert!(!main.contains('\u{2020}'), "{main}");
    assert!(main.contains("lastRenderedPageBreak"), "{main}");
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
    // `"on"`, whitespace, a rendered page break, an XML comment and a
    // processing instruction, both in one run.
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
                "<w:lastRenderedPageBreak/><!-- kept --><?kept?>",
                "<w:t>\u{2021}</w:t></w:r>{}"
            ),
            &xml[..from],
            first,
            second,
            &xml[to..]
        )
    });
    // The reader passes over the same children as the removal.
    assert_eq!(marked_text(&doc, 0), format!("Marked[f{first}][f{second}]"));
    doc.remove_footnote(first).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(marked_text(&doc, 0), format!("Marked[f{second}]"));
    let main = doc.part_xml("word/document.xml").unwrap();
    assert!(!main.contains("w:sym"), "{main}");
    assert!(main.contains('\u{2021}'), "{main}");
    assert_eq!(doc.footnotes().len(), 1);
    doc.remove_footnote(second).unwrap();
    let doc = reopen(&mut doc);
    let main = doc.part_xml("word/document.xml").unwrap();
    assert!(!main.contains('\u{2021}'), "{main}");
    // What sat between the reference and its mark stays.
    for kept in ["<w:lastRenderedPageBreak/>", "<!-- kept -->", "<?kept?>"] {
        assert!(main.contains(kept), "{kept} in {main}");
    }
    assert!(doc.footnotes().is_empty());
    assert_eq!(texts(&doc), ["Marked"]);
}

#[test]
fn a_custom_mark_after_on_is_the_mark_of_its_reference() {
    // Word reads `customMarkFollows` as an on/off value: "on" sets it as "1"
    // and "true" do, and the text after the reference is its mark.
    for value in ["1", "true", "on"] {
        let mut doc = doc_with(&["Marked"]);
        let id = doc
            .create_footnote_with_mark(&paragraph_at(&doc, 0), "Note", "*")
            .unwrap();
        let mut doc = with_main_part(&mut doc, |xml| {
            let run = reference_run(xml, id);
            format!(
                concat!(
                    "{}<w:r>",
                    r#"<w:footnoteReference w:customMarkFollows="{}" w:id="{}"/>"#,
                    "<w:t>\u{2021}</w:t></w:r>{}"
                ),
                &xml[..run.start],
                value,
                id,
                &xml[run.end..]
            )
        });
        assert_eq!(marked_text(&doc, 0), format!("Marked[f{id}]"), "{value}");
        doc.replace_text("Marked", "Kept");
        let doc = reopen(&mut doc);
        assert_eq!(marked_text(&doc, 0), format!("Kept[f{id}]"), "{value}");
        assert_eq!(texts(&doc), ["Kept\u{2021}"], "{value}");
        let main = doc.part_xml("word/document.xml").unwrap();
        assert_eq!(main.matches("customMarkFollows").count(), 1, "{main}");
    }
}

#[test]
fn a_child_between_a_reference_and_its_text_leaves_that_text_in_the_body() {
    // The note removal passes over whitespace, XML comments, processing
    // instructions and a rendered page break only, and takes the first text
    // as the mark; anything else ends the mark, so the text after it is body
    // text for the reader too.
    for between in [
        r#"<x:kept xmlns:x="urn:example:kept"/>"#,
        "<w:t></w:t>",
        "stray",
    ] {
        let mut doc = doc_with(&["Marked"]);
        let id = doc
            .create_footnote_with_mark(&paragraph_at(&doc, 0), "Note", "*")
            .unwrap();
        let mut doc = with_main_part(&mut doc, |xml| {
            let run = reference_run(xml, id);
            format!(
                concat!(
                    "{}<w:r>",
                    r#"<w:footnoteReference w:customMarkFollows="1" w:id="{}"/>"#,
                    "{}<w:t>\u{2021}</w:t></w:r>{}"
                ),
                &xml[..run.start],
                id,
                between,
                &xml[run.end..]
            )
        });
        assert_eq!(
            marked_text(&doc, 0),
            format!("Marked[f{id}]\u{2021}"),
            "{between}"
        );
        doc.remove_footnote(id).unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(marked_text(&doc, 0), "Marked\u{2021}", "{between}");
    }
}

#[test]
fn a_long_form_page_break_between_a_reference_and_its_mark_is_passed_over() {
    let mut doc = doc_with(&["Marked"]);
    let id = doc
        .create_footnote_with_mark(&paragraph_at(&doc, 0), "Note", "*")
        .unwrap();
    let mut doc = with_main_part(&mut doc, |xml| {
        let run = reference_run(xml, id);
        format!(
            concat!(
                "{}<w:r>",
                r#"<w:footnoteReference w:customMarkFollows="1" w:id="{}"/>"#,
                "<w:lastRenderedPageBreak></w:lastRenderedPageBreak>",
                "<w:t>\u{2021}</w:t></w:r>{}"
            ),
            &xml[..run.start],
            id,
            &xml[run.end..]
        )
    });
    assert_eq!(marked_text(&doc, 0), format!("Marked[f{id}]"));
    doc.remove_footnote(id).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(marked_text(&doc, 0), "Marked");
}

#[test]
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

#[test]
fn removing_a_paragraph_removes_its_endnotes() {
    let mut doc = doc_with(&["Kept", "Removed"]);
    create_note(&mut doc, Endnote, 1, "Removed endnote");
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept"]);
    assert!(endnote_texts(&doc).is_empty(), "{:?}", endnote_texts(&doc));
}

#[test]
fn removing_a_range_removes_the_notes_of_every_paragraph_in_it() {
    let mut doc = doc_with(&["One", "Two", "Three", "Four"]);
    let kept = create_note(&mut doc, Footnote, 0, "One note");
    create_note(&mut doc, Footnote, 1, "Two note");
    create_note(&mut doc, Footnote, 2, "Three note");
    assert_eq!(doc.remove_content_range(1, 3).unwrap(), 2);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["One", "Four"]);
    assert_eq!(doc.footnotes(), [(kept, " One note".to_owned())]);
}

#[test]
fn a_note_another_paragraph_still_references_stays() {
    let mut doc = doc_with(&["Kept", "Removed"]);
    let id = create_note(&mut doc, Footnote, 1, "Shared note");
    doc.paragraph_mut(0).unwrap().add_footnote_ref(id);
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept"]);
    assert_eq!(doc.footnotes(), [(id, " Shared note".to_owned())]);
    assert_eq!(references(&doc, 0), [(Footnote, id)]);
}

/// `doc` saved, its part `name` rewritten by `edit`, and opened again.
fn with_part(doc: &mut Document, name: &str, edit: impl FnOnce(&str) -> String) -> Document {
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part(name).unwrap().to_vec()).unwrap();
    package.set_part(name, edit(&xml).into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

/// `xml` with the run holding the footnote reference to `id` wrapped in
/// `open` and `close`.
fn wrapped_reference(xml: &str, id: i32, open: &str, close: &str) -> String {
    let reference = xml
        .find(&format!(r#"<w:footnoteReference w:id="{id}""#))
        .expect("reference to the note");
    let start = xml[..reference]
        .rfind("<w:r>")
        .max(xml[..reference].rfind("<w:r "))
        .expect("run of the reference");
    let end = reference + xml[reference..].find("</w:r>").unwrap() + "</w:r>".len();
    format!(
        "{}{open}{}{close}{}",
        &xml[..start],
        &xml[start..end],
        &xml[end..]
    )
}

const TRACKED: &str = r#"w:author="Reviewer" w:date="2026-01-01T00:00:00Z""#;

#[test]
fn a_note_a_tracked_deletion_still_references_stays() {
    // Paragraph 0 holds a deleted reference to the note; rejecting that
    // deletion needs the note, so removing paragraph 1 keeps it.
    let mut doc = doc_with(&["Kept", "Removed"]);
    let id = create_note(&mut doc, Footnote, 1, "Shared note");
    let mut doc = with_part(&mut doc, "/word/document.xml", |xml| {
        let first = xml.find("</w:p>").expect("first paragraph");
        format!(
            r#"{}<w:del w:id="91" {TRACKED}><w:r><w:footnoteReference w:id="{id}"/></w:r></w:del>{}"#,
            &xml[..first],
            &xml[first..]
        )
    });
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept"]);
    assert_eq!(doc.footnotes().len(), 1);
    let main = doc.part_xml("word/document.xml").unwrap();
    assert!(
        main.contains(&format!(r#"w:footnoteReference w:id="{id}""#)),
        "{main}"
    );
}

#[test]
fn removing_a_paragraph_removes_the_note_of_a_tracked_insertion() {
    // Word wraps a reference inserted with track changes on in `w:ins`.
    let mut doc = doc_with(&["Kept", "Removed"]);
    let id = create_note(&mut doc, Footnote, 1, "Inserted note");
    let mut doc = with_part(&mut doc, "/word/document.xml", |xml| {
        wrapped_reference(
            xml,
            id,
            &format!(r#"<w:ins w:id="92" {TRACKED}>"#),
            "</w:ins>",
        )
    });
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept"]);
    assert!(doc.footnotes().is_empty(), "{:?}", doc.footnotes());
}

/// A table of `rows` x `columns` cells holding text, with a footnote in each
/// cell `notes` names (row, column, note text); returns the note ids.
fn table_with_cell_notes(
    doc: &mut Document,
    rows: usize,
    columns: usize,
    notes: &[(usize, usize, &str)],
) -> Vec<i32> {
    {
        let mut table = doc.add_table(rows, columns);
        for row in 0..rows {
            for column in 0..columns {
                table
                    .cell(row, column)
                    .unwrap()
                    .set_text(&format!("cell {row}{column}"));
            }
        }
    }
    notes
        .iter()
        .map(|(row, column, text)| {
            let cell = doc
                .stories()
                .unwrap()
                .into_iter()
                .filter(|story| story.kind() == StoryKind::TableCell)
                .nth(row * columns + column)
                .expect("the cell story");
            let location = doc.story_items(&cell).unwrap()[0].location().clone();
            doc.create_footnote_at(
                &StoryRunPosition {
                    location,
                    run_index: 1,
                },
                text,
            )
            .unwrap()
        })
        .collect()
}

#[test]
fn removing_a_table_row_removes_the_notes_of_its_cells() {
    let mut doc = doc_with(&["Before"]);
    let ids = table_with_cell_notes(&mut doc, 2, 2, &[(0, 0, "Kept"), (1, 1, "Removed")]);
    let mut doc = reopen(&mut doc);
    assert!(doc.remove_table_row(0, 1).unwrap());
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(ids[0], " Kept".to_owned())]);
}

#[test]
fn removing_a_row_of_a_nested_table_removes_the_notes_of_its_cells() {
    // A table inside a body cell is edited through its story location.
    let mut doc = doc_with(&["Before"]);
    {
        let mut outer = doc.add_table(1, 1);
        let mut cell = outer.cell(0, 0).unwrap();
        cell.set_text("outer");
        let mut nested = cell.add_table(2, 1);
        nested.cell(0, 0).unwrap().set_text("kept row");
        nested.cell(1, 0).unwrap().set_text("removed row");
    }
    let cells = |doc: &Document| {
        doc.stories()
            .unwrap()
            .into_iter()
            .filter(|story| story.kind() == StoryKind::TableCell)
            .collect::<Vec<_>>()
    };
    let note_in = |doc: &mut Document, text: &str, note: &str| {
        let cell = cells(doc)
            .into_iter()
            .find(|cell| {
                doc.story_items(cell).unwrap().iter().any(|item| {
                    item.kind() == StoryItemKind::Paragraph
                        && item.text().unwrap().as_deref() == Some(text)
                })
            })
            .expect("the cell");
        let location = doc.story_items(&cell).unwrap()[0].location().clone();
        doc.create_footnote_at(
            &StoryRunPosition {
                location,
                run_index: 1,
            },
            note,
        )
        .unwrap()
    };
    let kept = note_in(&mut doc, "kept row", "Kept");
    let removed = note_in(&mut doc, "removed row", "Removed");
    comment_on_note(&mut doc, removed);
    let outer_cell = cells(&doc)[0].clone();
    let nested = doc
        .story_items(&outer_cell)
        .unwrap()
        .into_iter()
        .find(|item| item.kind() == StoryItemKind::Table)
        .expect("the nested table")
        .location()
        .clone();
    assert!(doc.remove_story_table_row(&nested, 1).unwrap());
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(kept, " Kept".to_owned())]);
    assert!(doc.comments().is_empty(), "the thread went with its note");
}

#[test]
fn deleting_a_table_column_removes_the_notes_of_its_cells() {
    let mut doc = doc_with(&["Before"]);
    let ids = table_with_cell_notes(&mut doc, 1, 2, &[(0, 0, "Kept"), (0, 1, "Removed")]);
    let mut doc = reopen(&mut doc);
    doc.delete_table_column(0, 1).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(ids[0], " Kept".to_owned())]);
}

/// A comment on the text of note `id`.
fn comment_on_note(doc: &mut Document, id: i32) {
    let paragraph = note_paragraph(doc, Footnote, id);
    doc.add_story_comment(
        oxoffice_docx::StoryRunRange {
            start: StoryRunPosition {
                location: paragraph.clone(),
                run_index: 1,
            },
            end: StoryRunPosition {
                location: paragraph,
                run_index: 2,
            },
        },
        "Reviewer",
        None,
        "On the note",
    )
    .unwrap();
}

/// `xml` with a control around the reference to note `id`: around its
/// paragraph (a block control) or around its run (an inline control).
fn control_around_reference(xml: &str, id: i32, inline: bool) -> String {
    if inline {
        return wrapped_reference(xml, id, "<w:sdt><w:sdtContent>", "</w:sdtContent></w:sdt>");
    }
    let run = reference_run(xml, id);
    let start = xml[..run.start]
        .rfind("<w:p>")
        .max(xml[..run.start].rfind("<w:p "))
        .unwrap();
    let end = run.end + xml[run.end..].find("</w:p>").unwrap() + "</w:p>".len();
    format!(
        "{}<w:sdt><w:sdtContent>{}</w:sdtContent></w:sdt>{}",
        &xml[..start],
        &xml[start..end],
        &xml[end..]
    )
}

/// The first content control of the body.
fn body_control(doc: &Document) -> ContentLocation {
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap();
    doc.story_items(&body)
        .unwrap()
        .into_iter()
        .find(|item| item.kind() == StoryItemKind::ContentControl)
        .expect("the control")
        .location()
        .clone()
}

#[test]
fn removing_a_content_control_removes_the_comments_inside_its_notes() {
    for inline in [false, true] {
        let mut doc = doc_with(&["Kept", "Removed"]);
        let removed = create_note(&mut doc, Footnote, 1, "Removed note");
        comment_on_note(&mut doc, removed);
        assert_eq!(doc.comments().len(), 1);
        let mut doc = with_main_part(&mut doc, |xml| {
            control_around_reference(xml, removed, inline)
        });
        doc.remove_content_control(&body_control(&doc)).unwrap();
        let doc = reopen(&mut doc);
        assert!(doc.footnotes().is_empty(), "inline: {inline}");
        assert!(doc.comments().is_empty(), "inline: {inline}");
    }
}

#[test]
fn removing_a_content_control_removes_the_notes_inside_it() {
    // A block control around a paragraph, and an inline control around the
    // reference run: the two ways a control is removed.
    for inline in [false, true] {
        let mut doc = doc_with(&["Kept", "Removed"]);
        let kept = create_note(&mut doc, Footnote, 0, "Kept note");
        let removed = create_note(&mut doc, Footnote, 1, "Removed note");
        let mut doc = with_main_part(&mut doc, |xml| {
            control_around_reference(xml, removed, inline)
        });
        doc.remove_content_control(&body_control(&doc)).unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(
            doc.footnotes(),
            [(kept, " Kept note".to_owned())],
            "inline: {inline}"
        );
    }
}

#[test]
fn removing_a_table_removes_the_notes_of_its_cells() {
    let mut doc = doc_with(&["In a cell", "After"]);
    create_note(&mut doc, Footnote, 0, "Cell note");
    let mut doc = with_part(&mut doc, "/word/document.xml", |xml| {
        let body = xml.find("<w:body>").unwrap() + "<w:body>".len();
        let start = body
            + [xml[body..].find("<w:p>"), xml[body..].find("<w:p ")]
                .into_iter()
                .flatten()
                .min()
                .unwrap();
        let end = start + xml[start..].find("</w:p>").unwrap() + "</w:p>".len();
        format!(
            concat!(
                "{}<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid>",
                "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"2000\" w:type=\"dxa\"/></w:tcPr>",
                "{}</w:tc></w:tr></w:tbl>{}"
            ),
            &xml[..start],
            &xml[start..end],
            &xml[end..]
        )
    });
    assert_eq!(doc.tables().len(), 1);
    assert!(doc.remove_content(0));
    let doc = reopen(&mut doc);
    assert!(doc.tables().is_empty());
    assert!(doc.footnotes().is_empty(), "{:?}", doc.footnotes());
}

#[test]
fn an_endnote_another_paragraph_still_references_stays() {
    let mut doc = doc_with(&["Kept", "Removed"]);
    create_note(&mut doc, Endnote, 1, "Shared endnote");
    let mut doc = with_part(&mut doc, "/word/document.xml", |xml| {
        let reference = xml.find("<w:endnoteReference ").unwrap();
        let tag = &xml[reference..reference + xml[reference..].find("/>").unwrap() + 2];
        let first = xml.find("</w:p>").expect("first paragraph");
        format!("{}<w:r>{tag}</w:r>{}", &xml[..first], &xml[first..])
    });
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept"]);
    assert_eq!(endnote_texts(&doc).len(), 1);
}

#[test]
fn a_note_a_header_references_stays() {
    let mut doc = doc_with(&["Removed"]);
    let id = create_note(&mut doc, Footnote, 0, "Header note");
    doc.set_header("Head");
    let mut doc = with_part(&mut doc, "/word/header1.xml", |xml| {
        let end = xml.rfind("</w:p>").expect("header paragraph");
        format!(
            r#"{}<w:r><w:footnoteReference w:id="{id}"/></w:r>{}"#,
            &xml[..end],
            &xml[end..]
        )
    });
    assert!(doc.remove_content(0));
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes().len(), 1);
}

#[test]
fn removing_a_paragraph_removes_the_note_of_an_inline_content_control() {
    let mut doc = doc_with(&["Kept", "Removed"]);
    let id = create_note(&mut doc, Footnote, 1, "Control note");
    let mut doc = with_part(&mut doc, "/word/document.xml", |xml| {
        wrapped_reference(
            xml,
            id,
            "<w:sdt><w:sdtPr/><w:sdtContent>",
            "</w:sdtContent></w:sdt>",
        )
    });
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept"]);
    assert!(doc.footnotes().is_empty(), "{:?}", doc.footnotes());
}

#[test]
fn removing_a_paragraph_of_a_libreoffice_file_removes_its_note() {
    let mut doc = corpus_doc("libreoffice/thesis.docx");
    let before = doc.footnotes();
    let (paragraph, id) = (0..doc.paragraph_count())
        .find_map(|index| {
            references(&doc, index)
                .into_iter()
                .find(|(family, _)| *family == Footnote)
                .map(|(_, id)| (index, id))
        })
        .expect("a paragraph with a footnote");
    let content = doc.content_index_of_paragraph(paragraph).unwrap();
    assert!(doc.remove_content(content));
    let doc = reopen(&mut doc);
    let after = doc.footnotes();
    assert_eq!(after.len(), before.len() - 1);
    assert!(after.iter().all(|(note, _)| *note != id));
}

#[test]
fn a_note_whose_id_its_part_holds_twice_stays() {
    // The removal still succeeds; only that ambiguous note is left alone.
    let mut doc = doc_with(&["Kept", "Removed"]);
    let id = create_note(&mut doc, Footnote, 1, "Twice");
    let mut doc = with_part(&mut doc, "/word/footnotes.xml", |xml| {
        let start = xml
            .find(&format!(r#"<w:footnote w:id="{id}""#))
            .expect("the note");
        let end = start + xml[start..].find("</w:footnote>").unwrap() + "</w:footnote>".len();
        format!("{}{}{}", &xml[..end], &xml[start..end], &xml[end..])
    });
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept"]);
    let notes = doc.part_xml("word/footnotes.xml").unwrap();
    assert_eq!(
        notes.matches(&format!(r#"w:id="{id}""#)).count(),
        2,
        "{notes}"
    );
}

/// A one-pixel PNG.
const PIXEL: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// A document whose paragraphs `Kept` and `Removed` each hold a footnote
/// showing a picture, saved and opened again so the pictures come from the
/// file; the ids of the kept and the removed note.
fn notes_with_pictures() -> (Document, i32, i32) {
    let mut doc = doc_with(&["Kept", "Removed"]);
    let kept = create_note(&mut doc, Footnote, 0, "Kept picture");
    let removed = create_note(&mut doc, Footnote, 1, "Removed picture");
    for id in [kept, removed] {
        let story = doc.footnote_story(id).unwrap().expect("the note story");
        doc.add_picture_to_story(
            &story,
            PIXEL,
            "pixel.png",
            oxoffice_docx::Length::emu(9525),
            oxoffice_docx::Length::emu(9525),
        )
        .unwrap();
    }
    (reopen(&mut doc), kept, removed)
}

fn media_parts(doc: &Document) -> Vec<String> {
    doc.part_names()
        .unwrap()
        .into_iter()
        .filter(|name| name.contains("media/"))
        .collect()
}

#[test]
fn removing_a_paragraph_removes_the_picture_of_its_note_only() {
    let (mut doc, kept, _) = notes_with_pictures();
    assert_eq!(media_parts(&doc).len(), 2);
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes().len(), 1);
    assert_eq!(doc.footnotes()[0].0, kept);
    assert_eq!(media_parts(&doc).len(), 1, "{:?}", media_parts(&doc));
}

#[test]
fn a_reference_to_a_note_the_part_lacks_does_not_block_the_removal() {
    // The removed paragraph names a missing note and a real one: the real
    // one still goes, the missing one is passed over.
    let mut doc = doc_with(&["Kept", "Removed"]);
    let kept = create_note(&mut doc, Footnote, 0, "Kept note");
    create_note(&mut doc, Footnote, 1, "Removed note");
    doc.paragraph_mut(1).unwrap().add_footnote_ref(kept + 40);
    assert!(doc.remove_content(1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept"]);
    assert_eq!(doc.footnotes(), [(kept, " Kept note".to_owned())]);
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

#[test]
fn setting_a_paragraph_text_keeps_its_note_marks() {
    let mut doc = doc_with(&["Old text"]);
    let footnote = create_note(&mut doc, Footnote, 0, "Foot");
    let endnote = create_note(&mut doc, Endnote, 0, "End");
    let dagger = doc
        .create_footnote_with_mark(&paragraph_at(&doc, 0), "Dagger note", "\u{2020}")
        .unwrap();
    let mut doc = reopen(&mut doc);
    doc.paragraph_mut(0).unwrap().set_text("New").unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["New\u{2020}"]);
    assert_eq!(
        references(&doc, 0),
        [(Footnote, footnote), (Endnote, endnote), (Footnote, dagger)]
    );
    assert_eq!(doc.footnotes().len(), 2);
    assert_eq!(note_text(&doc, Endnote, endnote).as_deref(), Some(" End"));
    // Each mark keeps its run properties: the note mark style.
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert_eq!(
        xml.matches(r#"<w:rStyle w:val="FootnoteReference"/>"#)
            .count(),
        2
    );
    assert_eq!(
        xml.matches(r#"<w:rStyle w:val="EndnoteReference"/>"#)
            .count(),
        1
    );
}

#[test]
fn setting_a_cell_text_keeps_its_note_mark() {
    let mut doc = doc_with(&["Before"]);
    let ids = table_with_cell_notes(&mut doc, 1, 1, &[(0, 0, "Cell note")]);
    let mut doc = reopen(&mut doc);
    doc.table_mut(0)
        .unwrap()
        .cell(0, 0)
        .unwrap()
        .try_set_text("New")
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(ids[0], " Cell note".to_owned())]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(xml.contains(&format!(r#"<w:footnoteReference w:id="{}"/>"#, ids[0])));
}

#[test]
fn setting_the_text_of_a_paragraph_holding_a_deleted_note_mark_is_refused() {
    let mut doc = doc_with(&["Old"]);
    let id = create_note(&mut doc, Footnote, 0, "Deleted mark");
    let mut doc = with_main_part(&mut doc, |xml| {
        wrapped_reference(
            xml,
            id,
            &format!(r#"<w:del w:id="900" {TRACKED}>"#),
            "</w:del>",
        )
    });
    let before = doc.to_bytes().unwrap();
    let error = doc.paragraph_mut(0).unwrap().set_text("New").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("tracked deletion of the mark of footnote"),
        "{error}"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn replacing_story_content_removes_the_notes_it_no_longer_references() {
    let mut doc = doc_with(&["Kept", "Replaced"]);
    let kept = create_note(&mut doc, Footnote, 0, "Kept note");
    create_note(&mut doc, Footnote, 1, "Replaced note");
    let mut paragraph = CT_P::new();
    paragraph.add_run("New");
    doc.replace_story_content(
        &paragraph_at(&doc, 1),
        ContentFragment::paragraph(paragraph).unwrap(),
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept", "New"]);
    assert_eq!(doc.footnotes(), [(kept, " Kept note".to_owned())]);
}

#[test]
fn setting_a_story_paragraph_text_keeps_its_note_mark() {
    let mut doc = doc_with(&["Old"]);
    let id = create_note(&mut doc, Footnote, 0, "Kept");
    doc.set_story_text(&paragraph_at(&doc, 0), "New").unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(id, " Kept".to_owned())]);
    assert_eq!(references(&doc, 0), [(Footnote, id)]);
}

#[test]
fn setting_a_run_text_keeps_its_note_mark() {
    let mut doc = doc_with(&["Old"]);
    let numbered = create_note(&mut doc, Footnote, 0, "Numbered");
    let dagger = doc
        .create_footnote_with_mark(&paragraph_at(&doc, 0), "Dagger note", "\u{2020}")
        .unwrap();
    let mut doc = reopen(&mut doc);
    {
        let mut paragraph = doc.paragraph_mut(0).unwrap();
        // The find-and-replace loop: every run is written back.
        let count = paragraph.runs().count();
        for index in 0..count {
            let mut run = paragraph.run_mut(index).unwrap();
            let text = run.text().replace("Old", "New");
            run.set_text(&text);
        }
    }
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["New\u{2020}"]);
    assert_eq!(
        references(&doc, 0),
        [(Footnote, numbered), (Footnote, dagger)]
    );
    assert_eq!(doc.footnotes().len(), 2);
}

#[test]
fn setting_the_text_of_a_note_paragraph_keeps_the_note_number() {
    let mut doc = doc_with(&["Body"]);
    let id = create_note(&mut doc, Footnote, 0, "Old note");
    let mut doc = reopen(&mut doc);
    doc.edit_story_paragraph(&note_paragraph(&doc, Footnote, id), |paragraph| {
        paragraph.set_text("New note")
    })
    .unwrap();
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/footnotes.xml").unwrap();
    assert!(xml.contains("<w:footnoteRef/>"), "{xml}");
    // The space Word writes after the number was part of the old text.
    assert_eq!(doc.footnotes(), [(id, "New note".to_owned())]);
}

#[test]
fn a_kept_note_mark_keeps_its_own_run_properties() {
    let mut doc = doc_with(&["Old"]);
    let id = create_note(&mut doc, Footnote, 0, "Note");
    let mut doc = with_main_part(&mut doc, |xml| {
        let reference = xml
            .find(&format!(r#"<w:footnoteReference w:id="{id}""#))
            .unwrap();
        let style = xml[..reference]
            .rfind(r#"<w:rStyle w:val="FootnoteReference"/>"#)
            .unwrap();
        let end = style + r#"<w:rStyle w:val="FootnoteReference"/>"#.len();
        format!("{}<w:color w:val=\"FF0000\"/>{}", &xml[..end], &xml[end..])
    });
    doc.paragraph_mut(0).unwrap().set_text("New").unwrap();
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    let reference = xml.find("<w:footnoteReference").unwrap();
    let run = xml[..reference]
        .rfind("<w:r>")
        .max(xml[..reference].rfind("<w:r "))
        .unwrap();
    assert!(
        xml[run..reference].contains(r#"<w:color w:val="FF0000"/>"#),
        "{xml}"
    );
}

#[test]
fn a_bookmark_around_a_note_mark_still_holds_it_after_set_text() {
    let mut doc = doc_with(&["Old"]);
    let id = create_note(&mut doc, Footnote, 0, "Note");
    let mut doc = with_main_part(&mut doc, |xml| {
        wrapped_reference(
            xml,
            id,
            r#"<w:bookmarkStart w:id="77" w:name="_Ref1"/>"#,
            r#"<w:bookmarkEnd w:id="77"/>"#,
        )
    });
    doc.paragraph_mut(0).unwrap().set_text("New").unwrap();
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    let start = xml.find(r#"w:name="_Ref1""#).unwrap();
    let mark = xml.find("<w:footnoteReference").unwrap();
    let end = xml.find(r#"<w:bookmarkEnd w:id="77"/>"#).unwrap();
    assert!(start < mark && mark < end, "{xml}");
}

#[test]
fn replacing_a_paragraph_xml_removes_the_notes_it_no_longer_references() {
    let mut doc = doc_with(&["Kept", "Replaced"]);
    let kept = create_note(&mut doc, Footnote, 0, "Kept note");
    create_note(&mut doc, Footnote, 1, "Replaced note");
    doc.replace_paragraph_xml(
        &paragraph_at(&doc, 1),
        r#"<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:r><w:t>New</w:t></w:r></w:p>"#,
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Kept", "New"]);
    assert_eq!(doc.footnotes(), [(kept, " Kept note".to_owned())]);
}

#[test]
fn writing_a_run_text_back_keeps_a_custom_mark_where_it_was() {
    let mut doc = doc_with(&["Body"]);
    let id = doc
        .create_footnote_with_mark(&paragraph_at(&doc, 0), "Star note", "*")
        .unwrap();
    // The mark between two texts of its own run, as a foreign file may hold it.
    let mut doc = with_main_part(&mut doc, |xml| {
        let reference = xml
            .find(&format!(r#"w:id="{id}""#))
            .and_then(|at| xml[..at].rfind("<w:footnoteReference"))
            .unwrap();
        let end = reference + xml[reference..].find("</w:r>").unwrap();
        format!(
            "{}<w:t>x</w:t>{}<w:t>y</w:t>{}",
            &xml[..reference],
            &xml[reference..end],
            &xml[end..]
        )
    });
    let before = texts(&doc);
    assert_eq!(before, ["Bodyx*y"]);
    {
        let mut paragraph = doc.paragraph_mut(0).unwrap();
        let count = paragraph.runs().count();
        for index in 0..count {
            let mut run = paragraph.run_mut(index).unwrap();
            let text = run.text();
            run.set_text(&text);
        }
    }
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), before);
    assert_eq!(references(&doc, 0), [(Footnote, id)]);
}

#[test]
fn setting_the_text_of_a_note_paragraph_with_a_deleted_number_is_refused() {
    let mut doc = doc_with(&["Body"]);
    let id = create_note(&mut doc, Footnote, 0, "Note");
    let mut doc = with_part(&mut doc, "/word/footnotes.xml", |xml| {
        let number = xml.find("<w:footnoteRef/>").unwrap();
        let start = xml[..number]
            .rfind("<w:r>")
            .max(xml[..number].rfind("<w:r "))
            .unwrap();
        let end = number + xml[number..].find("</w:r>").unwrap() + "</w:r>".len();
        format!(
            r#"{}<w:del w:id="901" {TRACKED}>{}</w:del>{}"#,
            &xml[..start],
            &xml[start..end],
            &xml[end..]
        )
    });
    let error = doc
        .edit_story_paragraph(&note_paragraph(&doc, Footnote, id), |paragraph| {
            paragraph.set_text("New")
        })
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("tracked deletion of its note number"),
        "{error}"
    );
}

#[test]
fn each_kept_mark_of_a_note_referenced_twice_keeps_its_own_run_properties() {
    let mut doc = doc_with(&["Old"]);
    let id = create_note(&mut doc, Footnote, 0, "Note");
    let mut doc = with_main_part(&mut doc, |xml| {
        let run = reference_run(xml, id);
        let red = xml[run.clone()].replacen(
            r#"<w:rStyle w:val="FootnoteReference"/>"#,
            r#"<w:rStyle w:val="FootnoteReference"/><w:color w:val="FF0000"/>"#,
            1,
        );
        format!("{}{}{}", &xml[..run.end], red, &xml[run.end..])
    });
    doc.paragraph_mut(0).unwrap().set_text("New").unwrap();
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    let second = xml.rfind("<w:footnoteReference").unwrap();
    let first = xml[..second].rfind("<w:footnoteReference").unwrap();
    let run_of = |at: usize| {
        xml[..at]
            .rfind("<w:r>")
            .max(xml[..at].rfind("<w:r "))
            .unwrap()
    };
    assert!(!xml[run_of(first)..first].contains("FF0000"), "{xml}");
    assert!(xml[run_of(second)..second].contains("FF0000"), "{xml}");
}

/// The `w:style` elements of a styles part whose name is `name`.
fn styles_named(styles: &str, name: &str) -> Vec<String> {
    styles
        .split("<w:style ")
        .skip(1)
        .filter(|style| style.contains(&format!(r#"<w:name w:val="{name}"/>"#)))
        .map(|style| style.split("</w:style>").next().unwrap().to_owned())
        .collect()
}

#[test]
fn a_note_mark_uses_the_localized_style_of_its_built_in_name() {
    // A French Word file names the note mark styles otherwise.
    let mut doc = doc_with(&["Body"]);
    for (id, name) in [
        ("Appelnotedebasdep", "footnote reference"),
        ("Appeldenotedefin", "endnote reference"),
    ] {
        doc.add_style(
            oxoffice_docx::StyleBuilder::character(id, name).run_properties(
                oxoffice_docx_oxml::properties::CT_RPr {
                    vert_align: Some("superscript".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
    }
    doc.create_footnote_at(&boundary(&doc, 0, 1), "a footnote")
        .unwrap();
    doc.add_footnote("another");
    doc.create_endnote_at(&boundary(&doc, 0, 0), "an endnote")
        .unwrap();
    let doc = reopen(&mut doc);
    let styles = doc.part_xml("word/styles.xml").unwrap();
    for name in ["footnote reference", "endnote reference"] {
        assert_eq!(styles_named(&styles, name).len(), 1, "{name}: {styles}");
    }
    let body = doc.part_xml("word/document.xml").unwrap();
    assert!(
        body.contains(r#"<w:rStyle w:val="Appelnotedebasdep"/>"#)
            && body.contains(r#"<w:rStyle w:val="Appeldenotedefin"/>"#)
            && !body.contains("FootnoteReference")
            && !body.contains("EndnoteReference"),
        "{body}"
    );
    let notes = doc.part_xml("word/footnotes.xml").unwrap();
    assert!(!notes.contains("FootnoteReference"), "{notes}");
}

#[test]
fn a_created_note_mark_style_is_defined_as_word_defines_it() {
    // A French Word file, whose default paragraph font is `Policepardfaut`.
    let mut doc = corpus_doc("word/report.docx");
    doc.create_endnote_at(&boundary(&doc, 0, 1), "an endnote")
        .unwrap();
    let doc = reopen(&mut doc);
    let styles = doc.part_xml("word/styles.xml").unwrap();
    let style = &styles_named(&styles, "endnote reference")[0];
    for expected in [
        r#"<w:basedOn w:val="Policepardfaut"/>"#,
        r#"<w:uiPriority w:val="99"/>"#,
        "<w:semiHidden/>",
        "<w:unhideWhenUsed/>",
        r#"<w:vertAlign w:val="superscript"/>"#,
    ] {
        assert!(style.contains(expected), "{expected}: {style}");
    }
}
