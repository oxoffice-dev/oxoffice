//! Fields (inserting, reading, updating, `update_fields_on_open`), bookmarks,
//! cross-references, captions with SEQ numbering and tables of contents.

use oxoffice_docx::{
    CaptionOptions, ContentLocation, CrossReferenceOptions, Document, Field, FieldDateTime,
    FieldEvaluationContext, FieldKind, FieldOutcome, RunItemRef, RunPosition, RunRange,
    SequenceOptions, StoryId, StoryItemKind, StoryKind, StoryRangeKind, StoryRunPosition,
    StoryRunRange, TableOfFiguresOptions, TocRebuildReport,
};

use crate::support::{corpus_doc, doc_with, doc_with_body, reopen, texts};

/// Every field held by a body run, as (kind, instruction, cached result).
fn field_list(doc: &Document) -> Vec<(FieldKind, String, String)> {
    let mut fields = Vec::new();
    for paragraph in doc.paragraphs() {
        for run in paragraph.runs() {
            for item in run.items() {
                if let RunItemRef::Field(field) = item {
                    fields.push((
                        field.kind(),
                        field.instruction().to_owned(),
                        field.cached_result().to_owned(),
                    ));
                }
            }
        }
    }
    fields
}

fn story(doc: &Document, kind: StoryKind) -> StoryId {
    doc.stories()
        .expect("stories are listed")
        .into_iter()
        .find(|story| story.kind() == kind)
        .unwrap_or_else(|| panic!("no {kind:?} story"))
}

/// The location of the first paragraph of a story whose text is `text`.
fn paragraph_at(doc: &Document, kind: StoryKind, text: &str) -> ContentLocation {
    doc.story_items(&story(doc, kind))
        .expect("story items are listed")
        .into_iter()
        .find(|item| {
            item.kind() == StoryItemKind::Paragraph
                && item.text().expect("item text").as_deref() == Some(text)
        })
        .unwrap_or_else(|| panic!("no {kind:?} paragraph {text:?}"))
        .location()
        .clone()
}

/// A run boundary in the body paragraph whose text is `text`.
fn body_position(doc: &Document, text: &str, run_index: usize) -> StoryRunPosition {
    StoryRunPosition {
        location: paragraph_at(doc, StoryKind::Body, text),
        run_index,
    }
}

/// Runs `start_run..end_run` from body child `start` to body child `end`.
fn runs(start: usize, start_run: usize, end: usize, end_run: usize) -> RunRange {
    RunRange {
        start: RunPosition {
            body_index: start,
            run_index: start_run,
        },
        end: RunPosition {
            body_index: end,
            run_index: end_run,
        },
    }
}

/// The main-story bookmarks as (name, text).
fn bookmark_list(doc: &Document) -> Vec<(String, String)> {
    doc.bookmarks()
        .iter()
        .map(|bookmark| {
            (
                bookmark.name().unwrap_or_default().to_owned(),
                bookmark.text().to_owned(),
            )
        })
        .collect()
}

/// The body hyperlinks as (text, internal anchor).
fn link_anchors(doc: &Document) -> Vec<(String, String)> {
    doc.links()
        .into_iter()
        .map(|link| (link.text, link.anchor.unwrap_or_default()))
        .collect()
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

fn merge_context(values: &[(&str, &str)]) -> FieldEvaluationContext {
    FieldEvaluationContext {
        merge_fields: pairs(values).into_iter().collect(),
        ..Default::default()
    }
}

fn no_context() -> FieldEvaluationContext {
    FieldEvaluationContext::default()
}

// ---- Fields ----

#[test]
fn a_simple_field_keeps_its_instruction_and_result_after_a_save() {
    let mut doc = Document::new();
    doc.add_paragraph("Page ")
        .add_run("")
        .add_field("PAGE", "1")
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        field_list(&doc),
        [(FieldKind::Simple, "PAGE".to_owned(), "1".to_owned())]
    );
    assert_eq!(texts(&doc), ["Page 1"]);
}

// Fails today: a field added with Run::add_field is left out of the paragraph text until a save and reopen. Issue: #31
#[test]
#[should_panic(expected = "the field result is part of the paragraph text")]
fn a_field_result_is_part_of_the_paragraph_text_before_a_save() {
    let mut doc = Document::new();
    doc.add_paragraph("Page ")
        .add_run("")
        .add_field("PAGE", "1")
        .unwrap();
    assert_eq!(
        texts(&doc),
        ["Page 1"],
        "the field result is part of the paragraph text"
    );
}

#[test]
fn a_word_complex_field_split_over_runs_reads_as_one_field() {
    // Word splits the instruction and the result over runs and marks the
    // field dirty when it wants it updated.
    let mut doc = doc_with_body(
        r#"<w:p><w:r w:rsidR="00A1"><w:t xml:space="preserve">Page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin" w:dirty="true"/></w:r><w:r><w:instrText xml:space="preserve"> PA</w:instrText></w:r><w:r w:rsidR="00A2"><w:instrText xml:space="preserve">GE \* MERGEFORMAT </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r w:rsidR="00A3"><w:t>1</w:t></w:r><w:r w:rsidR="00A4"><w:t>2</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> of the report</w:t></w:r></w:p>"#,
    );
    let check = |doc: &Document| {
        assert_eq!(texts(doc), ["Page 12 of the report"]);
        let mut seen = Vec::new();
        for run in doc.paragraph(0).unwrap().runs() {
            for item in run.items() {
                if let RunItemRef::Field(field) = item {
                    seen.push((
                        field.kind(),
                        field.name().to_owned(),
                        field.instruction().to_owned(),
                        field.cached_result().to_owned(),
                        field.dirty(),
                    ));
                }
            }
        }
        assert_eq!(
            seen,
            [(
                FieldKind::Complex,
                "PAGE".to_owned(),
                r"PAGE \* MERGEFORMAT".to_owned(),
                "12".to_owned(),
                Some(true)
            )]
        );
    };
    check(&doc);
    check(&reopen(&mut doc));
}

#[test]
fn a_field_needs_a_name_and_xml_text() {
    let mut doc = doc_with(&["Body"]);
    let mut paragraph = doc.paragraph_mut(0).unwrap();
    let error = paragraph
        .add_run("")
        .add_field("  ", "x")
        .unwrap_err()
        .to_string();
    assert!(error.contains("field name"), "{error}");
    let error = paragraph
        .add_run("")
        .add_field("PAGE\u{1}", "x")
        .unwrap_err()
        .to_string();
    assert!(error.contains("U+0001"), "{error}");
    assert!(field_list(&doc).is_empty());
    assert_eq!(texts(&reopen(&mut doc)), ["Body"]);
}

#[test]
fn updating_fields_fills_date_file_name_and_merge_fields_from_the_context() {
    let mut doc = Document::new();
    for (label, instruction, stored) in [
        ("Date: ", r#"DATE \@ "yyyy-MM-dd""#, "2000-01-01"),
        ("File: ", "FILENAME", "old.docx"),
        ("Dear ", "MERGEFIELD Name", "«Name»"),
        ("City: ", "MERGEFIELD City", "«City»"),
    ] {
        doc.add_paragraph(label)
            .add_run("")
            .add_field(instruction, stored)
            .unwrap();
    }
    let context = FieldEvaluationContext {
        now: Some(FieldDateTime {
            year: 2026,
            month: 9,
            day: 15,
            hour: 10,
            minute: 30,
            second: 0,
        }),
        file_name: Some("report.docx".into()),
        ..merge_context(&[("Name", "Zoë Ångström")])
    };

    // Evaluation only reports: the stored results stay until an update.
    let outcomes: Vec<FieldOutcome> = doc
        .evaluate_fields(&context)
        .unwrap()
        .into_iter()
        .map(|field| field.outcome)
        .collect();
    assert_eq!(outcomes[0], FieldOutcome::Resolved("2026-09-15".into()));
    assert_eq!(outcomes[1], FieldOutcome::Resolved("report.docx".into()));
    assert_eq!(outcomes[2], FieldOutcome::Resolved("Zoë Ångström".into()));
    // A value missing from the context keeps what the document shows.
    assert!(
        matches!(&outcomes[3], FieldOutcome::KeepStored { diagnostic } if diagnostic.contains("City was not supplied")),
        "{:?}",
        outcomes[3]
    );

    let bytes = doc.to_bytes_with_field_updates(&context).unwrap();
    let doc = Document::from_bytes(&bytes).unwrap();
    assert_eq!(
        texts(&doc),
        [
            "Date: 2026-09-15",
            "File: report.docx",
            "Dear Zoë Ångström",
            "City: «City»"
        ]
    );
    assert_eq!(field_list(&doc)[2].1, "MERGEFIELD Name");
}

#[test]
fn a_locked_field_keeps_its_result_on_update() {
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin" w:fldLock="true"/></w:r><w:r><w:instrText>DATE \@ "yyyy"</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1999</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>
        <w:p><w:fldSimple w:instr=" DATE \@ &quot;yyyy&quot; " w:fldLock="true"><w:r><w:t>1998</w:t></w:r></w:fldSimple></w:p>"#,
    );
    let context = FieldEvaluationContext {
        now: Some(FieldDateTime {
            year: 2026,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            second: 5,
        }),
        ..Default::default()
    };
    doc.update_fields(&context).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["1999", "1998"]);
    let mut locked = Vec::new();
    for paragraph in doc.paragraphs() {
        for run in paragraph.runs() {
            for item in run.items() {
                if let RunItemRef::Field(field) = item {
                    locked.push(field.locked());
                }
            }
        }
    }
    assert_eq!(locked, [Some(true), Some(true)]);
}

#[test]
fn page_fields_take_their_numbers_from_the_layout_except_in_a_footer() {
    let mut doc = doc_with(&["Page one"]);
    doc.add_paragraph("Page ")
        .page_break_before(true)
        .add_run("")
        .add_field("PAGE", "7")
        .unwrap();
    let mut paragraph = doc.paragraph_mut(1).unwrap();
    paragraph.add_run(" of ");
    paragraph.add_run("").add_field("NUMPAGES", "9").unwrap();
    doc.set_footer("Footer page ");
    let footer = paragraph_at(&doc, StoryKind::Footer, "Footer page ");
    doc.insert_field(
        &StoryRunPosition {
            location: footer,
            run_index: 1,
        },
        Field::new("PAGE", "?"),
    )
    .unwrap();

    let report = doc.update_layout_backed_fields().unwrap();
    assert_eq!((report.page_fields, report.num_pages_fields), (1, 1));
    // Word keeps the stored result of a header or footer PAGE field and
    // draws the live number, so the footer field keeps its result.
    assert!(
        report
            .diagnostics
            .iter()
            .any(|line| line.contains("Word preserves dynamic header and footer caches")),
        "{:?}",
        report.diagnostics
    );
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Page one", "Page 2 of 2"]);
    paragraph_at(&doc, StoryKind::Footer, "Footer page ?");
}

#[test]
fn update_fields_on_open_is_set_cleared_and_removed() {
    let mut doc = Document::new();
    assert_eq!(doc.update_fields_on_open(), None);
    for value in [Some(true), Some(false), None] {
        doc.set_update_fields_on_open(value).unwrap();
        doc = reopen(&mut doc);
        assert_eq!(doc.update_fields_on_open(), value);
    }
}

#[test]
fn update_fields_on_open_keeps_the_other_settings_of_a_word_file() {
    let mut doc = corpus_doc("word/report.docx");
    assert_eq!(doc.update_fields_on_open(), None);
    doc.set_update_fields_on_open(Some(true)).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.update_fields_on_open(), Some(true));
    let settings = doc.part_xml("word/settings.xml").unwrap();
    for kept in [
        r#"<w:zoom w:percent="146"/>"#,
        r#"<w:decimalSymbol w:val=","/>"#,
        "<w:rsids>",
        "<m:mathPr>",
    ] {
        assert!(settings.contains(kept), "lost {kept}: {settings}");
    }
}

#[test]
fn an_if_field_picks_its_result_from_a_nested_merge_field() {
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t xml:space="preserve">Dear </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF </w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Gender </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«Gender»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve"> = "F" "Madam" "Sir" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Sir</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    for (gender, greeting) in [("F", "Madam"), ("M", "Sir")] {
        let outcome = doc
            .evaluate_fields(&merge_context(&[("Gender", gender)]))
            .unwrap()
            .remove(0)
            .outcome;
        assert_eq!(outcome, FieldOutcome::Resolved(greeting.into()));
    }
    doc.update_fields(&merge_context(&[("Gender", "F")]))
        .unwrap();
    assert_eq!(texts(&reopen(&mut doc)), ["Dear Madam"]);
}

// Fails today: a nested field inside a quoted IF operand breaks the IF ("requires 5 positional operands"), so the stored result stays. Issue: #31
#[test]
#[should_panic(expected = "the IF field follows its quoted merge field")]
fn an_if_field_with_a_quoted_nested_merge_field_is_updated() {
    // Quoting the nested field is the form Word recommends for comparisons.
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF "</w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Country </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«Country»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve">" = "France" "Bonjour" "Hello" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Hello</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    doc.update_fields(&merge_context(&[("Country", "France")]))
        .unwrap();
    assert_eq!(
        texts(&doc),
        ["Bonjour"],
        "the IF field follows its quoted merge field"
    );
}

// ---- Sequences and captions ----

#[test]
fn sequence_fields_count_in_document_order_with_restart_and_format() {
    let mut doc = doc_with(&["First ", "Second ", "Restarted ", "Roman "]);
    for (text, options) in [
        ("First ", SequenceOptions::default()),
        ("Second ", SequenceOptions::default()),
        (
            "Restarted ",
            SequenceOptions {
                restart: Some(10),
                ..Default::default()
            },
        ),
        (
            "Roman ",
            SequenceOptions {
                format: Some("ROMAN".into()),
                ..Default::default()
            },
        ),
    ] {
        let position = body_position(&doc, text, 1);
        doc.insert_sequence(&position, "Table", &options).unwrap();
    }
    // A new sequence field has no result until fields are updated.
    assert_eq!(texts(&doc)[0], "First ");
    doc.update_fields(&no_context()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        ["First 1", "Second 2", "Restarted 10", "Roman XI"]
    );
    assert_eq!(field_list(&doc)[2].1, r"SEQ Table \r 10");
}

#[test]
fn a_sequence_needs_a_word_identifier_and_consistent_switches() {
    let mut doc = doc_with(&["Table "]);
    let position = body_position(&doc, "Table ", 1);
    for (identifier, options, message) in [
        (
            "1Table",
            SequenceOptions::default(),
            "invalid Word reference",
        ),
        (
            "My Table",
            SequenceOptions::default(),
            "invalid Word reference",
        ),
        (
            "Table",
            SequenceOptions {
                repeat: true,
                restart: Some(1),
                ..Default::default()
            },
            "mutually exclusive",
        ),
        (
            "Table",
            SequenceOptions {
                restart_heading: Some(10),
                ..Default::default()
            },
            "heading level",
        ),
    ] {
        let error = doc
            .insert_sequence(&position, identifier, &options)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{identifier}: {error}");
    }
    assert!(field_list(&doc).is_empty());
}

#[test]
fn captions_are_numbered_once_fields_are_updated() {
    let mut doc = doc_with(&["Intro", "chart", "map"]);
    // A caption goes before the item it is given.
    for (before, text) in [("chart", "Sales"), ("map", "Sites")] {
        let location = paragraph_at(&doc, StoryKind::Body, before);
        doc.insert_caption(
            &location,
            &CaptionOptions {
                text: text.into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    assert_eq!(texts(&doc)[1], "Figure : Sales");
    doc.update_fields(&no_context()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        [
            "Intro",
            "Figure 1: Sales",
            "chart",
            "Figure 2: Sites",
            "map"
        ]
    );
    // Each caption gets three bookmarks, made unique when the base is taken.
    assert_eq!(
        bookmark_list(&doc),
        pairs(&[
            ("Caption", "Figure 1: Sales"),
            ("Caption_Label", "Figure 1"),
            ("Caption_Number", "1"),
            ("Caption_1", "Figure 2: Sites"),
            ("Caption_Label_1", "Figure 2"),
            ("Caption_Number_1", "2"),
        ])
    );
}

#[test]
fn a_cross_reference_to_a_caption_follows_renumbering() {
    let mut doc = doc_with(&["See ", "chart", "map"]);
    let location = paragraph_at(&doc, StoryKind::Body, "map");
    let map = doc
        .insert_caption(
            &location,
            &CaptionOptions {
                text: "Sites".into(),
                bookmark: "Sites".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let position = body_position(&doc, "See ", 1);
    doc.insert_cross_reference(
        &position,
        &map.label_and_number,
        &CrossReferenceOptions {
            hyperlink: true,
            ..Default::default()
        },
    )
    .unwrap();
    doc.update_fields(&no_context()).unwrap();
    assert_eq!(texts(&doc)[0], "See Figure 1");

    let location = paragraph_at(&doc, StoryKind::Body, "chart");
    doc.insert_caption(
        &location,
        &CaptionOptions {
            text: "Sales".into(),
            bookmark: "Sales".into(),
            ..Default::default()
        },
    )
    .unwrap();
    doc.update_fields(&no_context()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        [
            "See Figure 2",
            "Figure 1: Sales",
            "chart",
            "Figure 2: Sites",
            "map"
        ]
    );
    assert_eq!(field_list(&doc)[0].1, r"REF Sites_Label \h");
}

#[test]
fn a_table_of_figures_lists_the_captions() {
    let mut doc = doc_with(&["Intro", "chart", "map"]);
    for (before, text) in [("chart", "Sales"), ("map", "Sites")] {
        let location = paragraph_at(&doc, StoryKind::Body, before);
        doc.insert_caption(
            &location,
            &CaptionOptions {
                text: text.into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    doc.update_fields(&no_context()).unwrap();
    let location = paragraph_at(&doc, StoryKind::Body, "Intro");
    doc.insert_table_of_figures(&location, &TableOfFiguresOptions::default())
        .unwrap();
    let report = doc.rebuild_generated_tables().unwrap();
    assert_eq!(report.figure_entries, 2);
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc)[1..3],
        ["Figure 1: Sales\t1", "Figure 2: Sites\t1"]
    );
}

// ---- Bookmarks ----

#[test]
fn a_bookmark_over_runs_is_listed_with_its_text_after_a_save() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("The ");
    paragraph.add_run("日本語 target");
    paragraph.add_run(" phrase");
    let id = doc.add_bookmark("target", runs(0, 1, 0, 2)).unwrap();
    let doc = reopen(&mut doc);
    let bookmarks = doc.bookmarks();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].id(), Some(id));
    assert_eq!(bookmarks[0].name(), Some("target"));
    assert_eq!(bookmarks[0].text(), "日本語 target");
    assert_eq!(bookmarks[0].range(), Some(runs(0, 1, 0, 2)));
    assert_eq!(bookmarks[0].issue(), None);
    assert_eq!(texts(&doc), ["The 日本語 target phrase"]);
}

#[test]
fn a_bookmark_can_span_paragraphs() {
    let mut doc = doc_with(&["Alpha beta", "Gamma", "Delta"]);
    doc.add_bookmark("span", runs(0, 0, 1, 1)).unwrap();
    let doc = reopen(&mut doc);
    let bookmark = &doc.bookmarks()[0];
    assert_eq!(bookmark.text(), "Alpha beta\nGamma");
    assert_eq!(bookmark.range(), Some(runs(0, 0, 1, 1)));
    assert_eq!(texts(&doc), ["Alpha beta", "Gamma", "Delta"]);
}

#[test]
fn bookmark_names_follow_word_rules() {
    let mut doc = doc_with(&["Alpha", "Beta"]);
    let longest = "x".repeat(40);
    doc.add_bookmark(&longest, runs(0, 0, 0, 1)).unwrap();
    doc.add_bookmark("chapter_2", runs(0, 0, 0, 1)).unwrap();
    for (name, message) in [
        ("", "must not be empty"),
        ("2chapter", "not a valid Word bookmark name"),
        ("has space", "not a valid Word bookmark name"),
        ("résumé", "not a valid Word bookmark name"),
        (&"x".repeat(41), "not a valid Word bookmark name"),
        // Names starting with an underscore are Word's own (`_Toc`, `_Ref`).
        ("_Toc9", "reserved for producer use"),
        ("chapter_2", "already exists"),
    ] {
        let error = doc
            .add_bookmark(name, runs(1, 0, 1, 1))
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{name:?}: {error}");
    }
    let names: Vec<_> = bookmark_list(&reopen(&mut doc))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, [longest.as_str(), "chapter_2"]);
}

#[test]
fn removing_a_bookmark_keeps_the_text_and_references_keep_their_last_result() {
    let mut doc = doc_with(&["Target", "See "]);
    let id = doc.add_bookmark("target", runs(0, 0, 0, 1)).unwrap();
    let position = body_position(&doc, "See ", 1);
    doc.insert_cross_reference(&position, "target", &CrossReferenceOptions::default())
        .unwrap();
    doc.update_fields(&no_context()).unwrap();
    assert_eq!(texts(&doc), ["Target", "See Target"]);

    assert!(doc.remove_story_bookmark(id).unwrap());
    assert!(!doc.remove_story_bookmark(id).unwrap());
    let outcome = doc
        .evaluate_fields(&no_context())
        .unwrap()
        .remove(0)
        .outcome;
    assert!(
        matches!(&outcome, FieldOutcome::KeepStored { diagnostic } if diagnostic.contains("not found")),
        "{outcome:?}"
    );
    doc.update_fields(&no_context()).unwrap();
    let doc = reopen(&mut doc);
    assert!(doc.bookmarks().is_empty());
    assert_eq!(texts(&doc), ["Target", "See Target"]);
}

#[test]
fn a_bookmark_in_a_header_is_a_story_range() {
    let mut doc = doc_with(&["Body"]);
    doc.set_header("Header text");
    let header = paragraph_at(&doc, StoryKind::Header, "Header text");
    let id = doc
        .add_story_bookmark(
            "in_header",
            StoryRunRange {
                start: StoryRunPosition {
                    location: header.clone(),
                    run_index: 0,
                },
                end: StoryRunPosition {
                    location: header,
                    run_index: 1,
                },
            },
        )
        .unwrap();
    let mut doc = reopen(&mut doc);
    // `bookmarks` reads the main story only.
    assert!(doc.bookmarks().is_empty());
    let ranges = doc.story_ranges().unwrap();
    assert_eq!(ranges.len(), 1);
    assert_eq!(
        ranges[0].kind(),
        &StoryRangeKind::Bookmark {
            id,
            name: "in_header".into()
        }
    );
    assert_eq!(
        ranges[0].range().start.location.story().kind(),
        StoryKind::Header
    );
    assert!(doc.remove_story_bookmark(id).unwrap());
    let doc = reopen(&mut doc);
    assert!(doc.story_ranges().unwrap().is_empty());
    paragraph_at(&doc, StoryKind::Header, "Header text");
}

#[test]
fn a_bookmark_covers_the_runs_word_split_a_paragraph_into() {
    let mut doc = corpus_doc("word/report.docx");
    let index = doc.find_content_index("Summary.").unwrap();
    let paragraph = doc.paragraph(1).unwrap();
    let run_texts: Vec<_> = paragraph.runs().map(|run| run.text()).collect();
    assert_eq!(run_texts.len(), 2, "Word wrote a bold run then a plain run");
    let whole = paragraph.text();
    doc.add_bookmark("summary", runs(index, 0, index, 2))
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(bookmark_list(&doc), [("summary".to_owned(), whole)]);
    let after: Vec<_> = doc
        .paragraph(1)
        .unwrap()
        .runs()
        .map(|run| run.text())
        .collect();
    assert_eq!(after, run_texts);
}

// ---- Cross-references ----

#[test]
fn a_cross_reference_to_a_heading_follows_the_new_heading_text() {
    let mut doc = Document::new();
    doc.add_paragraph("Introduction").style("Heading1");
    doc.add_paragraph("See ");
    doc.add_bookmark("intro", runs(0, 0, 0, 1)).unwrap();
    let position = body_position(&doc, "See ", 1);
    doc.insert_cross_reference(
        &position,
        "intro",
        &CrossReferenceOptions {
            hyperlink: true,
            ..Default::default()
        },
    )
    .unwrap();
    doc.update_fields(&no_context()).unwrap();
    assert_eq!(texts(&doc), ["Introduction", "See Introduction"]);

    doc.paragraph_mut(0)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_text("Overview");
    doc.update_fields(&no_context()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Overview", "See Overview"]);
    assert_eq!(
        field_list(&doc),
        [(
            FieldKind::Complex,
            r"REF intro \h".to_owned(),
            "Overview".to_owned()
        )]
    );
}

#[test]
fn a_cross_reference_with_position_says_above_or_below() {
    let mut doc = doc_with(&["Before ", "Target", "After "]);
    doc.add_bookmark("target", runs(1, 0, 1, 1)).unwrap();
    for text in ["Before ", "After "] {
        let position = body_position(&doc, text, 1);
        doc.insert_cross_reference(
            &position,
            "target",
            &CrossReferenceOptions {
                position: true,
                ..Default::default()
            },
        )
        .unwrap();
    }
    doc.update_fields(&no_context()).unwrap();
    assert_eq!(
        texts(&reopen(&mut doc)),
        ["Before below", "Target", "After above"]
    );
}

#[test]
fn a_page_reference_takes_the_page_of_the_bookmark() {
    let mut doc = doc_with(&["See page "]);
    doc.add_paragraph("Target").page_break_before(true);
    doc.add_bookmark("target", runs(1, 0, 1, 1)).unwrap();
    doc.paragraph_mut(0)
        .unwrap()
        .add_run("")
        .add_field(r"PAGEREF target \h", "?")
        .unwrap();
    let report = doc.update_layout_backed_fields().unwrap();
    assert_eq!(report.page_reference_fields, 1);
    assert_eq!(texts(&reopen(&mut doc)), ["See page 2", "Target"]);
}

// ---- Tables of contents ----

#[test]
fn inserting_a_toc_lists_headings_up_to_the_level_with_links_to_toc_bookmarks() {
    let mut doc = Document::new();
    doc.add_paragraph("Intro");
    doc.add_paragraph("Chapter one").style("Heading1");
    doc.add_paragraph("Section one point one").style("Heading2");
    doc.add_paragraph("Deep detail").style("Heading3");
    doc.insert_toc(0, 2).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc)[..4],
        [
            "Table of Contents",
            "Chapter one\t",
            "Section one point one\t",
            "Intro"
        ]
    );
    // A new entry shows its level by its indent; `rebuild_toc` gives the
    // entries the TOC 1 to TOC 9 styles instead.
    let indents: Vec<_> = doc.paragraphs()[1..3]
        .iter()
        .map(|paragraph| paragraph.indent_left().map(|indent| indent.to_twips()))
        .collect();
    assert_eq!(indents, [None, Some(360)]);
    assert_eq!(
        link_anchors(&doc),
        pairs(&[("Chapter one", "_Toc1"), ("Section one point one", "_Toc2")])
    );
    assert_eq!(
        bookmark_list(&doc),
        pairs(&[("_Toc1", "Chapter one"), ("_Toc2", "Section one point one")])
    );
}

#[test]
fn rebuilding_a_toc_adds_page_numbers() {
    let mut doc = Document::new();
    doc.add_paragraph("First").style("Heading1");
    doc.add_paragraph("Second")
        .style("Heading1")
        .page_break_before(true);
    doc.add_paragraph("Résumé 日本語 مقدمة").style("Heading2");
    doc.insert_toc(0, 3).unwrap();
    assert_eq!(texts(&doc)[1], "First\t");
    let report = doc.rebuild_toc().unwrap();
    assert_eq!((report.entry_count, report.bookmark_count), (3, 0));
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc)[1..4],
        ["First\t1", "Second\t2", "Résumé 日本語 مقدمة\t2"]
    );
    assert_eq!(field_list(&doc)[1].1, r"PAGEREF _Toc2 \h");
}

#[test]
fn rebuilding_a_toc_follows_renamed_added_and_removed_headings() {
    let mut doc = Document::new();
    for heading in ["Scope", "Method", "Results"] {
        doc.add_paragraph(heading).style("Heading1");
    }
    doc.insert_toc(0, 1).unwrap();

    let scope = texts(&doc)
        .iter()
        .rposition(|text| text == "Scope")
        .unwrap();
    doc.paragraph_mut(scope)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_text("Purpose");
    let method = *doc.find_content_indices("Method").last().unwrap();
    assert!(doc.remove_content(method));
    doc.add_paragraph("Outlook").style("Heading1");

    let report = doc.rebuild_toc().unwrap();
    assert_eq!(report.entry_count, 3);
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc)[1..4],
        ["Purpose\t1", "Results\t1", "Outlook\t1"]
    );
    let anchors: Vec<_> = link_anchors(&doc)
        .into_iter()
        .map(|(_, anchor)| anchor)
        .collect();
    let names: Vec<_> = bookmark_list(&doc)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(anchors, names, "every entry links to its heading bookmark");
}

#[test]
fn inserting_a_second_toc_keeps_bookmark_names_unique() {
    let mut doc = Document::new();
    doc.add_paragraph("A").style("Heading1");
    doc.add_paragraph("B").style("Heading1");
    doc.insert_toc(0, 1).unwrap();
    doc.insert_toc(0, 1).unwrap();
    let doc = reopen(&mut doc);
    let mut names: Vec<_> = bookmark_list(&doc)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names.sort();
    assert_eq!(names, ["_Toc1", "_Toc2", "_Toc3", "_Toc4"]);
    assert!(
        doc.bookmarks()
            .iter()
            .all(|bookmark| bookmark.issue().is_none())
    );
    for (_, anchor) in link_anchors(&doc) {
        assert!(names.contains(&anchor), "{anchor} has no bookmark");
    }
}

#[test]
fn rebuilding_a_word_toc_reuses_its_bookmarks_and_follows_its_switches() {
    // A TOC as Word writes it: \o "1-3" takes Heading 1 to 3, \u takes
    // paragraphs with an outline level, and entries link to `_Toc` bookmarks.
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:hyperlink w:anchor="_Toc211111111"><w:r><w:t>Old entry</w:t></w:r><w:r><w:tab/></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGEREF _Toc211111111 \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:hyperlink></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="_Toc211111111"/><w:r><w:t>Overview</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>
        <w:p><w:pPr><w:outlineLvl w:val="1"/></w:pPr><w:r><w:t>Outline item</w:t></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="Heading4"/></w:pPr><w:r><w:t>Too deep</w:t></w:r></w:p>"#,
    );
    let report = doc.rebuild_toc().unwrap();
    assert_eq!((report.entry_count, report.bookmark_count), (2, 1));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc)[1..3], ["Overview\t1", "Outline item\t1"]);
    assert!(!texts(&doc).iter().any(|text| text.contains("Old entry")));
    assert_eq!(
        link_anchors(&doc),
        pairs(&[("Overview", "_Toc211111111"), ("Outline item", "_Toc1")])
    );
}

#[test]
fn rebuilding_changes_nothing_without_a_toc_it_supports() {
    let mut doc = Document::new();
    doc.add_paragraph("Heading").style("Heading1");
    assert_eq!(doc.rebuild_toc().unwrap(), TocRebuildReport::default());

    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>TOC \x</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p>
        <w:p><w:r><w:t>kept entry</w:t></w:r></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Heading</w:t></w:r></w:p>"#,
    );
    let before = doc.part_xml("word/document.xml").unwrap();
    let report = doc.rebuild_toc().unwrap();
    assert_eq!(report.entry_count, 0);
    assert_eq!(
        report.diagnostics,
        ["field TOC uses unsupported switch \\x, stored display retained"]
    );
    assert_eq!(doc.part_xml("word/document.xml").unwrap(), before);
}

#[test]
fn rebuilding_the_toc_of_a_word_file_saved_in_french_finds_its_headings() {
    // Word for Mac in French names the heading styles Titre1, Titre2.
    let mut doc = corpus_doc("word/report.docx");
    doc.insert_toc(0, 3).unwrap();
    let report = doc.rebuild_toc().unwrap();
    assert_eq!((report.entry_count, report.bookmark_count), (6, 6));
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc)[1..7],
        [
            "1. Highlights\t1",
            "2. Financial overview\t1",
            "3. Operations\t2",
            "3.1 Support\t2",
            "3.2 Next steps\t2",
            "Appendix A. Regional detail\t4"
        ]
    );
    let styles: Vec<_> = doc.paragraphs()[1..7]
        .iter()
        .map(|paragraph| paragraph.style_id().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(styles, ["TOC1", "TOC1", "TOC1", "TOC2", "TOC2", "TOC1"]);
    assert_eq!(
        bookmark_list(&doc)[3],
        ("_Toc4".to_owned(), "3.1 Support".to_owned())
    );
}

// Fails today: insert_toc finds headings by the English style ids Heading1 to Heading9, so a Word file saved in French gets an empty table. Issue: #27
#[test]
#[should_panic(expected = "insert_toc lists the headings of a Word file saved in French")]
fn inserting_a_toc_in_a_word_file_saved_in_french_lists_its_headings() {
    let mut doc = corpus_doc("word/report.docx");
    doc.insert_toc(0, 3).unwrap();
    assert_eq!(
        texts(&doc)[1],
        "1. Highlights\t",
        "insert_toc lists the headings of a Word file saved in French"
    );
}

#[test]
fn a_toc_in_a_google_docs_file_links_each_heading_beside_google_bookmarks() {
    let mut doc = corpus_doc("google/report.docx");
    // Google Docs marks each heading with an empty bookmark of its own.
    let google = bookmark_list(&doc);
    assert_eq!(google.len(), 7);
    assert!(
        google
            .iter()
            .all(|(name, text)| name.starts_with('_') && text.is_empty())
    );
    doc.insert_toc(0, 2).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        link_anchors(&doc),
        pairs(&[
            ("1. Highlights", "_Toc1"),
            ("2. Financial overview", "_Toc2"),
            ("3. Operations", "_Toc3"),
            ("3.1 Support", "_Toc4"),
            ("3.2 Next steps", "_Toc5"),
            ("Appendix A. Regional detail", "_Toc6"),
        ])
    );
    let after = bookmark_list(&doc);
    assert_eq!(after.len(), 13);
    for kept in &google {
        assert!(after.contains(kept), "lost {kept:?}");
    }
}

/// Every glyph run a laid-out document draws, as (text, size, font family).
fn drawn_runs(doc: &Document) -> Vec<(String, f64, String)> {
    let result = doc.layout().expect("the document lays out");
    let mut runs = Vec::new();
    for page in &result.layout.pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            if let oxoffice_layout::PositionedElement::Text(run) = element {
                let family = result
                    .layout
                    .fonts
                    .iter()
                    .find(|font| font.id == run.font_id)
                    .map(|font| font.family.clone())
                    .unwrap_or_default();
                runs.push((run.text.clone(), run.font_size, family));
            }
        });
    }
    runs
}

/// A legacy checkbox as Word saves it: `w:ffData` and no result run.
fn word_checkbox(size: &str, checked: bool) -> String {
    format!(
        r#"<w:r><w:rPr><w:sz w:val="20"/></w:rPr><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Check1"/><w:enabled/><w:calcOnExit w:val="0"/><w:checkBox>{size}<w:default w:val="0"/>{checked}</w:checkBox></w:ffData></w:fldChar></w:r><w:r><w:rPr><w:sz w:val="20"/></w:rPr><w:instrText xml:space="preserve"> FORMCHECKBOX </w:instrText></w:r><w:r><w:rPr><w:sz w:val="20"/></w:rPr><w:fldChar w:fldCharType="end"/></w:r>"#,
        checked = if checked { r#"<w:checked/>"# } else { "" },
    )
}

#[test]
fn a_word_saved_legacy_checkbox_draws_its_box_at_its_size() {
    let doc = doc_with_body(&format!(
        "<w:p>{}<w:r><w:t xml:space=\"preserve\"> I agree</w:t></w:r></w:p><w:p>{}</w:p>",
        word_checkbox(r#"<w:size w:val="24"/>"#, true),
        word_checkbox("<w:sizeAuto/>", false),
    ));
    let runs = drawn_runs(&doc);
    let checked = runs
        .iter()
        .find(|(text, _, _)| text == "\u{2612}")
        .expect("checked box drawn");
    assert_eq!(checked.1, 12.0, "w:size is in half-points");
    let unchecked = runs
        .iter()
        .find(|(text, _, _)| text == "\u{2610}")
        .expect("empty box drawn");
    assert_eq!(unchecked.1, 10.0, "w:sizeAuto takes the run's size");
    assert_eq!(
        unchecked.2, "Noto Sans Symbols 2",
        "a font that has the box"
    );
}

#[test]
fn a_legacy_checkbox_with_a_stored_result_draws_its_box_from_its_state_once() {
    let doc = doc_with_body(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Check1"/><w:enabled/><w:calcOnExit w:val="0"/><w:checkBox><w:sizeAuto/><w:default w:val="0"/><w:checked/></w:checkBox></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMCHECKBOX </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>X</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    let texts: Vec<String> = drawn_runs(&doc)
        .into_iter()
        .map(|(text, _, _)| text)
        .collect();
    assert_eq!(
        texts.concat(),
        "\u{2612}",
        "the box of its state, and not the stored X"
    );
}

#[test]
fn a_legacy_checkbox_set_through_the_facade_keeps_its_size() {
    let mut doc = doc_with_body(&format!(
        "<w:p>{}</w:p>",
        word_checkbox(r#"<w:size w:val="24"/>"#, false)
    ));
    let field = doc.legacy_form_fields().unwrap().remove(0);
    doc.set_legacy_form_field_value(
        &field.source_part,
        field.ordinal,
        oxoffice_docx::LegacyFormFieldValue::Checked(true),
    )
    .unwrap();
    let reopened = reopen(&mut doc);
    for doc in [&doc, &reopened] {
        let runs = drawn_runs(doc);
        let checked = runs
            .iter()
            .find(|(text, _, _)| text == "\u{2612}")
            .expect("checked box");
        assert_eq!(checked.1, 12.0, "w:size still sizes the box");
    }
}

#[test]
fn a_legacy_checkbox_reads_its_opening_run_under_any_prefix() {
    let doc = doc_with_body(
        r#"<ns0:p xmlns:ns0="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><ns0:r><ns0:rPr><ns0:sz ns0:val="20"/></ns0:rPr><ns0:fldChar ns0:fldCharType="begin"><ns0:ffData><ns0:name ns0:val="Check1"/><ns0:enabled/><ns0:calcOnExit ns0:val="0"/><ns0:checkBox><ns0:sizeAuto/><ns0:default ns0:val="0"/></ns0:checkBox></ns0:ffData></ns0:fldChar></ns0:r><ns0:r><ns0:instrText xml:space="preserve"> FORMCHECKBOX </ns0:instrText></ns0:r><ns0:r><ns0:fldChar ns0:fldCharType="end"/></ns0:r></ns0:p>"#,
    );
    let runs = drawn_runs(&doc);
    let unchecked = runs
        .iter()
        .find(|(text, _, _)| text == "\u{2610}")
        .expect("empty box");
    assert_eq!(unchecked.1, 10.0, "the opening run's size under ns0");
}

#[test]
fn a_legacy_checkbox_in_a_table_cell_draws_its_box() {
    let doc = doc_with_body(&format!(
        r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="5000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="5000" w:type="dxa"/></w:tcPr><w:p>{}</w:p></w:tc></w:tr></w:tbl><w:p/>"#,
        word_checkbox(r#"<w:size w:val="24"/>"#, true)
    ));
    let runs = drawn_runs(&doc);
    let checked = runs
        .iter()
        .find(|(text, _, _)| text == "\u{2612}")
        .expect("checked box in a cell");
    assert_eq!(checked.1, 12.0);
}
