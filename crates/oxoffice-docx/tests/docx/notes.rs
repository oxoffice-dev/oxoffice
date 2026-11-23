//! Footnotes and endnotes: creating them and their references, reading,
//! editing, moving and removing them, note policies at document and section
//! level, separator entries, and notes added to Word- and Google-saved files.

use std::io::Cursor;

use oxoffice_docx::paragraph::ParagraphRef;
use oxoffice_docx::{
    ContentLocation, Document, NoteFamily, NoteNumberFormat, NotePlacement, NotePolicy,
    NoteRestart, NoteSpecialRecord, RunItemRef, SectionBreak, StoryItemKind, StoryKind,
    StoryRunPosition,
};
use oxoffice_docx_oxml::document::CT_NoteProperties;
use oxoffice_docx_oxml::text::CT_P;
use oxoffice_opc::OpcPackage;

use crate::support::{doc_with, reopen, texts};

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

/// The `w:style` element of `id` in a styles part, if any.
fn style_element<'a>(styles: &'a str, id: &str) -> Option<&'a str> {
    let start = styles.find(&format!(r#"w:styleId="{id}""#))?;
    let start = styles[..start].rfind("<w:style ")?;
    let end = start + styles[start..].find("</w:style>")? + "</w:style>".len();
    Some(&styles[start..end])
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

// ---- Reading ----

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
fn a_chicago_note_policy_writes_words_marks() {
    let mut doc = doc_with(&["Body"]);
    create_note(&mut doc, Footnote, 0, "Starred note");
    doc.set_note_policy(
        Footnote,
        NotePolicy {
            format: NoteNumberFormat::Chicago,
            start: 1,
            restart: NoteRestart::EachPage,
            placement: NotePlacement::PageBottom,
        },
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        policy_of(doc.note_properties(Footnote)),
        Some(("pageBottom", "chicago", 1, "eachPage"))
    );
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

// ---- Round trips ----

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
fn setting_a_story_paragraph_text_keeps_its_note_mark() {
    let mut doc = doc_with(&["Old"]);
    let id = create_note(&mut doc, Footnote, 0, "Kept");
    doc.set_story_text(&paragraph_at(&doc, 0), "New").unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.footnotes(), [(id, " Kept".to_owned())]);
    assert_eq!(references(&doc, 0), [(Footnote, id)]);
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
fn a_raw_carriage_return_in_note_text_reads_as_a_line_feed() {
    // A notes part written with Windows line ends holds a raw CR LF inside
    // w:t, which every XML reader reads as one line feed; a CR written as a
    // reference stays one.
    let mut doc = doc_with(&["Marked"]);
    let id = create_note(&mut doc, Footnote, 0, "MARK");
    let doc = with_part(&mut doc, "/word/footnotes.xml", |xml| {
        xml.replace("MARK", "one\r\ntwo\rthree&#13;four")
    });
    // The note text starts with the space written after the note mark.
    let expected = " one\ntwo\nthree\rfour";
    assert_eq!(note_text(&doc, Footnote, id).as_deref(), Some(expected));
    let story: Vec<_> = doc
        .story_paragraphs()
        .unwrap()
        .into_iter()
        .filter(|paragraph| paragraph.kind == StoryKind::Footnote)
        .map(|paragraph| paragraph.text)
        .collect();
    assert_eq!(story, [expected]);
}

#[test]
fn a_raw_carriage_return_in_deleted_note_text_reads_as_a_line_feed_in_the_revision() {
    // Text written as CDATA follows the same end-of-line handling as text.
    let mut doc = doc_with(&["Marked"]);
    create_note(&mut doc, Footnote, 0, "MARK");
    let doc = with_part(&mut doc, "/word/footnotes.xml", |xml| {
        xml.replace(
            "MARK</w:t></w:r>",
            concat!(
                "MARK</w:t></w:r><w:del w:id=\"90\" w:author=\"A\">",
                "<w:r><w:delText><![CDATA[x\r\ny]]></w:delText></w:r></w:del>"
            ),
        )
    });
    let texts: Vec<String> = doc
        .story_revisions()
        .unwrap()
        .iter()
        .map(|revision| revision.text().to_owned())
        .collect();
    assert_eq!(texts, ["x\ny"]);
}
