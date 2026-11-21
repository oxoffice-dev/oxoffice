//! Fields (inserting, reading, updating, `update_fields_on_open`), bookmarks,
//! cross-references, captions with SEQ numbering and tables of contents.

use oxoffice_docx::{
    CaptionOptions, ContentLocation, CrossReferenceOptions, Document, Field, FieldDateTime,
    FieldEvaluationContext, FieldKind, FieldOutcome, HdrFtrType, ParagraphItemRef, RenderOptions,
    RevisionView, RunItemRef, RunPosition, RunRange, SequenceOptions, StoryId, StoryItemKind,
    StoryKind, StoryRangeKind, StoryRunPosition, StoryRunRange, TabAlignment, TabLeader,
    TableOfFiguresOptions, TocRebuildReport,
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
fn fields_update_in_libreoffice_files_that_keep_an_unused_even_page_header() {
    // LibreOffice writes even page headers and footers with even and odd
    // pages off: no section shows them, and their fields update without a
    // page of their own.
    for name in [
        "libreoffice/report.docx",
        "libreoffice/contract.docx",
        "libreoffice/letter.docx",
        "libreoffice/cv.docx",
        "libreoffice/form.docx",
        "libreoffice/minutes.docx",
        "libreoffice/manual.docx",
        "libreoffice/newsletter.docx",
        "libreoffice/long-report.docx",
    ] {
        let mut doc = corpus_doc(name);
        let before = texts(&doc);
        doc.update_fields(&no_context())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let doc = reopen(&mut doc);
        assert_eq!(texts(&doc).len(), before.len(), "{name}");
    }
}

#[test]
fn fields_in_an_even_page_header_no_section_shows_merge_and_keep_their_page_caches() {
    // An even page header with even and odd pages off, as LibreOffice
    // writes it: the merge fills its MERGEFIELD, and the SEQ and PAGE fields,
    // which need a page, keep their results.
    let mut doc = doc_with(&["Dear "]);
    doc.paragraph_mut(0)
        .unwrap()
        .add_run("")
        .add_field("MERGEFIELD Name", "«Name»")
        .unwrap();
    doc.set_header("Default");
    doc.set_raw_header_with_images(
        concat!(
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p>"#,
            r#"<w:fldSimple w:instr=" MERGEFIELD Name "><w:r><w:t>«Name»</w:t></w:r></w:fldSimple>"#,
            r#"<w:fldSimple w:instr=" SEQ Figure "><w:r><w:t>7</w:t></w:r></w:fldSimple>"#,
            r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>9</w:t></w:r></w:fldSimple>"#,
            r#"</w:p></w:hdr>"#,
        )
        .as_bytes()
        .to_vec(),
        &[],
        HdrFtrType::Even,
    );
    let record = [("Name".to_owned(), "Zoé".to_owned())]
        .into_iter()
        .collect();
    let mut outputs = doc.mail_merge(&[record]).expect("the merge passes");
    let merged = reopen(&mut outputs[0]);
    assert_eq!(texts(&merged), ["Dear Zoé"]);
    let even = merged
        .part_names()
        .unwrap()
        .into_iter()
        .map(|name| merged.part_xml(name.trim_start_matches('/')).unwrap())
        .find(|xml| xml.contains("SEQ Figure"))
        .expect("the even page header is kept");
    assert!(even.contains("Zoé"), "the merge fills it: {even}");
    assert!(
        even.contains("<w:t>7</w:t>") && even.contains("<w:t>9</w:t>"),
        "SEQ and PAGE keep their results: {even}"
    );
}

// LibreOffice writes `<w:rPr></w:rPr>` on every run of a field: the field
// reads its instruction and result as with `<w:rPr/>` or no properties, its
// result in the run of its end too, or with the whole field in one run.
#[test]
fn a_field_whose_runs_have_empty_properties_reads_its_result() {
    let spread = concat!(
        r#"<w:p><w:r>{p}<w:fldChar w:fldCharType="begin"></w:fldChar></w:r>"#,
        r#"<w:r>{p}<w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>"#,
        r#"<w:r>{p}<w:fldChar w:fldCharType="separate"></w:fldChar></w:r>"#,
        r#"<w:r>{p}<w:t>12</w:t><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    let single = concat!(
        r#"<w:p><w:r>{p}<w:fldChar w:fldCharType="begin"/><w:instrText xml:space="preserve"> PAGE </w:instrText>"#,
        r#"<w:fldChar w:fldCharType="separate"/><w:t>12</w:t><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    for body in [spread, single] {
        for properties in [
            "",
            "<w:rPr/>",
            "<w:rPr></w:rPr>",
            "<w:rPr><w:rFonts/></w:rPr>",
        ] {
            let mut doc = doc_with_body(&body.replace("{p}", properties));
            let expected = vec![(FieldKind::Complex, "PAGE".to_owned(), "12".to_owned())];
            assert_eq!(field_list(&doc), expected, "{properties} in {body}");
            doc.add_paragraph("Added");
            assert_eq!(
                field_list(&reopen(&mut doc)),
                expected,
                "{properties} in {body}"
            );
        }
    }
}

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

#[test]
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
    assert_eq!(doc.text(), "Page 1\n");
}

#[test]
fn a_field_added_to_a_table_cell_reads_in_the_cell_and_run_text_in_files_from_every_producer() {
    for name in [
        "word/report.docx",
        "google/report.docx",
        "libreoffice/report.docx",
    ] {
        let mut doc = corpus_doc(name);
        let before = doc.table(0).unwrap().cell(0, 0).unwrap().text();
        doc.table_mut(0)
            .unwrap()
            .cell(0, 0)
            .unwrap()
            .paragraph_mut(0)
            .unwrap()
            .add_run("")
            .add_field("PAGE", "7")
            .unwrap();
        let expected = format!("{before}7");
        {
            // The mutable cell and run read the same.
            let mut table = doc.table_mut(0).unwrap();
            let mut cell = table.cell(0, 0).unwrap();
            assert_eq!(cell.text(), expected, "{name}: the mutable cell");
            let mut paragraph = cell.paragraph_mut(0).unwrap();
            let last = paragraph.runs().count() - 1;
            assert_eq!(
                paragraph.run_mut(last).unwrap().text(),
                "7",
                "{name}: the mutable run"
            );
        }
        let check = |doc: &Document, when: &str| {
            let table = doc.table(0).unwrap();
            let cell = table.cell(0, 0).unwrap();
            assert_eq!(
                cell.text(),
                expected,
                "{name} {when}: the cell shows the result"
            );
            assert_eq!(
                cell.paragraph(0).unwrap().runs().last().unwrap().text(),
                "7",
                "{name} {when}: the run shows the result"
            );
        };
        check(&doc, "before a save");
        check(&reopen(&mut doc), "after a reopen");
    }
}

#[test]
fn simple_fields_from_a_file_read_in_cell_run_and_bookmark_text() {
    // `w:fldSimple` as Word writes it, in a cell and in a bookmark.
    let mut doc = doc_with_body(concat!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc><w:p>"#,
        r#"<w:r><w:t xml:space="preserve">Page </w:t></w:r>"#,
        r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple>"#,
        r#"</w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:p><w:bookmarkStart w:id="0" w:name="total"/>"#,
        r#"<w:r><w:t xml:space="preserve">Total: </w:t></w:r>"#,
        r#"<w:fldSimple w:instr=" =2+3 "><w:r><w:t>5</w:t></w:r></w:fldSimple>"#,
        r#"<w:bookmarkEnd w:id="0"/></w:p>"#,
    ));
    let check = |doc: &Document, when: &str| {
        let table = doc.table(0).unwrap();
        let cell = table.cell(0, 0).unwrap();
        assert_eq!(cell.text(), "Page 1", "{when}: the cell shows the result");
        let runs = cell
            .paragraph(0)
            .unwrap()
            .runs()
            .map(|run| run.text())
            .collect::<Vec<_>>();
        assert_eq!(
            runs,
            ["Page ", "1"],
            "{when}: the field run shows the result"
        );
        let bookmark = doc
            .bookmarks()
            .into_iter()
            .find(|bookmark| bookmark.name() == Some("total"))
            .expect("the bookmark is read");
        assert_eq!(
            bookmark.text(),
            "Total: 5",
            "{when}: the bookmark shows the result"
        );
        assert_eq!(texts(doc), ["Total: 5"]);
    };
    check(&doc, "as read");
    check(&reopen(&mut doc), "after a reopen");
}

/// The text of the first hyperlink of a body paragraph.
fn link_text(doc: &Document, paragraph: usize) -> String {
    doc.paragraph(paragraph)
        .unwrap()
        .items()
        .find_map(|item| match item {
            ParagraphItemRef::Hyperlink(link) => Some(link.text()),
            _ => None,
        })
        .expect("the paragraph holds a link")
}

#[test]
fn a_field_added_to_a_hyperlink_reads_in_the_link_text() {
    let mut doc = Document::new();
    doc.add_paragraph("")
        .add_internal_hyperlink("See page ", "total", None)
        .add_field("PAGEREF total", "2")
        .unwrap();
    doc.add_paragraph("")
        .add_internal_hyperlink("", "total", None)
        .add_field("PAGEREF total", "2")
        .unwrap();
    assert_eq!(link_text(&doc, 0), "See page 2", "beside the link text");
    assert_eq!(link_text(&doc, 1), "2", "alone in the link run");
    assert_eq!(texts(&doc), ["See page 2", "2"]);
}

#[test]
fn a_simple_field_inside_a_hyperlink_reads_after_a_reopen() {
    let mut doc = Document::new();
    doc.add_paragraph("")
        .add_internal_hyperlink("See page ", "total", None)
        .add_field("PAGEREF total", "2")
        .unwrap();
    doc.add_paragraph("")
        .add_internal_hyperlink("", "total", None)
        .add_field("PAGEREF total", "2")
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        ["See page 2", "2"],
        "the link keeps the field result after a reopen"
    );
    assert_eq!(link_text(&doc, 0), "See page 2");
    assert_eq!(link_text(&doc, 1), "2", "alone in the link run");
}

#[test]
fn a_simple_field_inside_a_hyperlink_in_a_file_reads_and_saves_as_written() {
    // As a file writes it: a w:fldSimple with its result, and an empty one.
    let body = r#"<w:p><w:hyperlink w:anchor="total"><w:r><w:t xml:space="preserve">See page </w:t></w:r><w:fldSimple w:instr=" PAGEREF total "><w:r><w:t>2</w:t></w:r></w:fldSimple><w:r><w:t xml:space="preserve"> of </w:t></w:r><w:fldSimple w:instr=" NUMPAGES "/></w:hyperlink></w:p>"#;
    let mut doc = doc_with_body(body);
    let reopened = reopen(&mut doc);
    for doc in [&doc, &reopened] {
        assert_eq!(texts(doc), ["See page 2 of "]);
        assert_eq!(link_text(doc, 0), "See page 2 of ");
        let mut fields = Vec::new();
        for run in doc.paragraph(0).unwrap().runs() {
            for item in run.items() {
                if let RunItemRef::Field(field) = item {
                    fields.push((field.kind(), field.instruction().trim().to_owned()));
                }
            }
        }
        assert_eq!(
            fields,
            [
                (FieldKind::Simple, "PAGEREF total".to_owned()),
                (FieldKind::Simple, "NUMPAGES".to_owned())
            ]
        );
    }
    // An edit in the paragraph writes the link again, its fields in place.
    doc.paragraph_mut(0).unwrap().add_run(" tail");
    let package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    let link = &xml[xml.find("<w:hyperlink").unwrap()..xml.find("</w:hyperlink>").unwrap()];
    let pageref = link
        .find("PAGEREF total")
        .expect("the PAGEREF stays in the link");
    let numpages = link
        .find("NUMPAGES")
        .expect("the NUMPAGES stays in the link");
    assert!(pageref < numpages, "{link}");
    assert_eq!(texts(&reopen(&mut doc)), ["See page 2 of  tail"]);

    // The story items read the field result, and a comparison of the file
    // with its own copy runs.
    let doc = doc_with_body(body);
    let items: Vec<_> = doc
        .story_items(&story(&doc, StoryKind::Body))
        .unwrap()
        .into_iter()
        .filter_map(|item| item.text().unwrap())
        .collect();
    // As for a simple field directly in a paragraph: the paragraph, then
    // the field with its result.
    assert_eq!(items, ["See page 2 of ", "2"]);
    let mut compared = doc_with_body(body);
    compared
        .compare(&doc_with_body(body), "Ana", "2026-01-01T00:00:00Z")
        .unwrap();
    assert_eq!(texts(&compared), ["See page 2 of "]);
    // A comparison with an edit in the link's text maps the link's runs,
    // its fields included.
    let mut compared = doc_with_body(body);
    compared
        .compare(
            &doc_with_body(&body.replace(" of </w:t>", " out of </w:t>")),
            "Ana",
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
    assert_eq!(texts(&compared), ["See page 2 out of "]);
}

#[test]
fn a_simple_field_inside_a_hyperlink_inside_an_insertion_reads_in_the_accepted_text() {
    let doc = doc_with_body(
        r#"<w:p><w:ins w:id="1" w:author="Ana" w:date="2026-01-01T00:00:00Z"><w:hyperlink w:anchor="total"><w:r><w:t xml:space="preserve">See page </w:t></w:r><w:fldSimple w:instr=" PAGE "><w:r><w:t>2</w:t></w:r></w:fldSimple></w:hyperlink></w:ins></w:p>"#,
    );
    assert_eq!(texts(&doc), ["See page 2"]);
}

#[test]
fn a_field_added_between_texts_of_a_run_reads_in_place_in_files_from_every_producer() {
    for name in [
        "word/report.docx",
        "google/report.docx",
        "libreoffice/report.docx",
    ] {
        let mut doc = corpus_doc(name);
        let title = doc.paragraph(0).unwrap().text();
        let mut paragraph = doc.paragraph_mut(0).unwrap();
        let mut run = paragraph.add_run(" (page ");
        run.add_field("PAGE", "3").unwrap();
        run.add_text(")");
        let expected = format!("{title} (page 3)");
        assert_eq!(doc.paragraph(0).unwrap().text(), expected, "{name}");
        assert!(
            doc.text().starts_with(&format!("{expected}\n")),
            "{name}: the document text shows the field result"
        );
        let doc = reopen(&mut doc);
        assert_eq!(
            doc.paragraph(0).unwrap().text(),
            expected,
            "{name}: the same text after a reopen"
        );
    }
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
fn updating_page_fields_keeps_a_table_of_contents_entry_saved_by_libreoffice() {
    // LibreOffice writes the whole entry as the result of its PAGEREF; Word
    // writes the page number alone.
    let entry = |result: &str| {
        format!(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">PAGEREF _Toc1 \h</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r>{result}</w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
        )
    };
    // Word's `\p` writes "on page N", which is not computed either.
    let relative = entry("<w:t>on page 7</w:t>").replace(r"PAGEREF _Toc1 \h", r"PAGEREF _Toc1 \p");
    let mut doc = doc_with_body(&format!(
        r#"{}{}{relative}<w:p><w:bookmarkStart w:id="1" w:name="_Toc1"/><w:r><w:t>Motivation</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>"#,
        entry("<w:t>1.1</w:t><w:tab/><w:t>Motivation</w:t><w:tab/><w:t>7</w:t>"),
        entry("<w:t>7</w:t>"),
    ));
    let report = doc.update_layout_backed_fields().unwrap();
    assert_eq!(report.page_reference_fields, 1, "{:?}", report.diagnostics);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|line| line.contains("retained cache: the result holds more than a page number")),
        "{:?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|line| line.contains(r"retained cache: unsupported switch \p")),
        "{:?}",
        report.diagnostics
    );
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        ["1.1\tMotivation\t7", "1", "on page 7", "Motivation"]
    );
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

/// An IF whose first operand quotes a nested merge field, split over runs
/// as Word writes it: `{ IF "{ MERGEFIELD Country }" = "<value>" ... }`.
fn quoted_if_body(value: &str, stored: &str) -> String {
    format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF "</w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Country </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«Country»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve">" = "{value}" "Bonjour" "Hello" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{stored}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
    )
}

#[test]
fn an_if_field_with_a_quoted_nested_merge_field_is_updated() {
    // Quoting the nested field is the form Word recommends for comparisons.
    let mut doc = doc_with_body(&quoted_if_body("France", "Hello"));
    doc.update_fields(&merge_context(&[("Country", "France")]))
        .unwrap();
    assert_eq!(
        texts(&doc),
        ["Bonjour"],
        "the IF field follows its quoted merge field"
    );
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Bonjour"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    let mut rest = xml.as_str();
    for piece in [
        r#"<w:instrText xml:space="preserve"> IF "</w:instrText>"#,
        r#"<w:instrText xml:space="preserve"> MERGEFIELD Country </w:instrText>"#,
        r#"<w:instrText xml:space="preserve">" = "France" "Bonjour" "Hello" </w:instrText>"#,
    ] {
        let at = rest
            .find(piece)
            .unwrap_or_else(|| panic!("the instruction keeps {piece} in order: {xml}"));
        rest = &rest[at + piece.len()..];
    }
}

#[test]
fn an_if_field_picks_a_quoted_nested_merge_field_as_its_result() {
    // The mail-merge form: the result to show is itself a quoted merge field.
    let doc = doc_with_body(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF "</w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Country </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«Country»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve">" = "France" "</w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD City </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«City»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve">" "Abroad" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Abroad</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    for (country, shown) in [("France", "Lyon"), ("Spain", "Abroad")] {
        let outcome = doc
            .evaluate_fields(&merge_context(&[("Country", country), ("City", "Lyon")]))
            .unwrap()
            .remove(0)
            .outcome;
        assert_eq!(outcome, FieldOutcome::Resolved(shown.into()), "{country}");
    }
}

// Fails today: a quoted operand made of text and a nested field has no model, so the IF gets too many operands and keeps its stored result. Issue: #50
#[test]
#[should_panic(expected = "the IF field compares the text joined to its nested field")]
fn an_if_field_whose_quoted_operand_joins_text_and_a_nested_field_is_updated() {
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF "Mr </w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Name </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«Name»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve">" = "Mr Smith" "Known" "Unknown" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Unknown</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    doc.update_fields(&merge_context(&[("Name", "Smith")]))
        .unwrap();
    assert_eq!(
        texts(&doc),
        ["Known"],
        "the IF field compares the text joined to its nested field"
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

#[test]
fn a_table_of_figures_numbered_by_chapter_keeps_its_prefix_out_of_the_page_reference() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t>Intro</w:t></w:r></w:p>"#,
        r#"<w:p><w:fldSimple w:instr=" SEQ Chapter "><w:r><w:t>1</w:t></w:r></w:fldSimple><w:r><w:t xml:space="preserve"> Chapter</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>chart</w:t></w:r></w:p>"#,
    ));
    let location = paragraph_at(&doc, StoryKind::Body, "chart");
    doc.insert_caption(
        &location,
        &CaptionOptions {
            text: "Sales".into(),
            ..Default::default()
        },
    )
    .unwrap();
    doc.update_fields(&no_context()).unwrap();
    let location = paragraph_at(&doc, StoryKind::Body, "Intro");
    doc.insert_table_of_figures(&location, &TableOfFiguresOptions::default())
        .unwrap();
    // Number the pages by chapter: `\s Chapter`.
    let bytes = doc.to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let xml = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let numbered = xml.replacen(r" TOC \c Figure ", r" TOC \s Chapter \c Figure ", 1);
    assert_ne!(numbered, xml, "the table of figures takes \\s");
    package.set_part("/word/document.xml", numbered.into_bytes());
    let mut out = Vec::new();
    package
        .write_to(&mut std::io::Cursor::new(&mut out))
        .unwrap();
    let mut doc = Document::from_bytes(&out).unwrap();

    doc.rebuild_generated_tables().unwrap();
    let entry = |doc: &Document| {
        texts(doc)
            .into_iter()
            .find(|text| text.contains("Sales\t"))
            .expect("the entry")
    };
    assert!(entry(&doc).ends_with("\t1-1"), "{}", entry(&doc));
    // The page update recomputes the page and keeps the prefix before it.
    let report = doc.update_layout_backed_fields().unwrap();
    assert_eq!(report.page_reference_fields, 1, "{:?}", report.diagnostics);
    let doc = reopen(&mut doc);
    assert!(entry(&doc).ends_with("\t1-1"), "{}", entry(&doc));
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
    // Word for Mac in French names the heading styles Titre1, Titre2. The
    // table insert_toc wrote already holds the six headings and their
    // bookmarks, so the rebuild finds them all and adds none.
    let mut doc = corpus_doc("word/report.docx");
    doc.insert_toc(0, 3).unwrap();
    let report = doc.rebuild_toc().unwrap();
    assert_eq!((report.entry_count, report.bookmark_count), (6, 0));
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc)[1..7],
        [
            "1. Highlights\t1",
            "2. Financial overview\t1",
            "3. Operations\t2",
            "3.1 Support\t2",
            "3.2 Next steps\t2",
            "Appendix A. Regional detail\t3"
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

#[test]
fn inserting_a_toc_in_a_word_file_saved_in_french_lists_its_headings() {
    let mut doc = corpus_doc("word/report.docx");
    doc.insert_toc(0, 3).unwrap();
    assert_eq!(
        texts(&doc)[1],
        "1. Highlights\t",
        "insert_toc lists the headings of a Word file saved in French"
    );
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc)[1..7],
        [
            "1. Highlights\t",
            "2. Financial overview\t",
            "3. Operations\t",
            "3.1 Support\t",
            "3.2 Next steps\t",
            "Appendix A. Regional detail\t"
        ]
    );
    // insert_toc indents each level by a quarter inch.
    let indents: Vec<_> = doc.paragraphs()[1..7]
        .iter()
        .map(|paragraph| paragraph.indent_left().map(|indent| indent.to_twips()))
        .collect();
    assert_eq!(
        indents,
        [None, None, None, Some(360), Some(360), None],
        "levels"
    );
    // Each entry links to the bookmark around its own heading.
    let expected: Vec<(String, String)> = bookmark_list(&doc)
        .into_iter()
        .filter(|(name, _)| name.starts_with("_Toc"))
        .map(|(name, heading)| (heading, name))
        .collect();
    assert_eq!(expected.len(), 6, "one bookmark per heading");
    assert_eq!(link_anchors(&doc), expected);
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

#[test]
fn a_legacy_checkbox_set_through_the_facade_keeps_its_opening_run_formatting() {
    let mut doc = doc_with_body(&format!(
        "<w:p>{}</w:p>",
        word_checkbox("<w:sizeAuto/>", false)
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
        assert_eq!(
            checked.1, 10.0,
            "the opening run's size, the stored result has none"
        );
    }
}

#[test]
fn a_legacy_checkbox_takes_its_opening_run_style() {
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:rPr><w:rStyle w:val="FormBox"/></w:rPr><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Check1"/><w:enabled/><w:calcOnExit w:val="0"/><w:checkBox><w:sizeAuto/><w:default w:val="0"/></w:checkBox></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMCHECKBOX </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    doc.add_style(oxoffice_docx::StyleBuilder::character("FormBox", "Form box").size(14.0))
        .unwrap();
    let runs = drawn_runs(&doc);
    let unchecked = runs
        .iter()
        .find(|(text, _, _)| text == "\u{2610}")
        .expect("empty box");
    assert_eq!(unchecked.1, 14.0, "the character style of the opening run");
}

#[test]
fn a_legacy_checkbox_inside_another_fields_result_draws_its_box_at_its_size() {
    // An IF field whose stored result holds checkboxes as Word saves them,
    // with no result run of their own.
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF 1 = 1 "yes" "no" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}{}{}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        word_checkbox("<w:sizeAuto/>", true),
        word_checkbox(r#"<w:size w:val="24"/>"#, false),
        // A box that stores a result takes that result's formatting first.
        r#"<w:r><w:rPr><w:sz w:val="20"/></w:rPr><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Check3"/><w:enabled/><w:calcOnExit w:val="0"/><w:checkBox><w:sizeAuto/><w:default w:val="0"/></w:checkBox></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMCHECKBOX </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:rPr><w:sz w:val="40"/></w:rPr><w:t>X</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    ));
    let runs = drawn_runs(&doc);
    let drawn: Vec<(&str, f64)> = runs
        .iter()
        .map(|(text, size, _)| (text.as_str(), *size))
        .collect();
    assert_eq!(
        drawn,
        [("\u{2612}", 10.0), ("\u{2610}", 12.0), ("\u{2610}", 20.0)],
        "each nested box draws from its state, at the opening run's size or its own"
    );
}

#[test]
fn a_styleref_to_a_heading_holding_a_legacy_checkbox_reads_its_text_without_the_box() {
    // The heading draws its box on the page; navigation text, which STYLEREF
    // reads, keeps the checkbox's stored text, empty as Word saves it.
    let doc = doc_with_body(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{}<w:r><w:t xml:space="preserve"> Approve</w:t></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> STYLEREF "Heading 1" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>old</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        word_checkbox("<w:sizeAuto/>", false)
    ));
    let texts: String = drawn_runs(&doc)
        .into_iter()
        .map(|(text, _, _)| text)
        .collect();
    assert_eq!(texts, "\u{2610} Approve Approve");
}

#[test]
fn a_legacy_checkbox_in_a_tracked_deletion_draws_its_box_in_the_tracked_view() {
    // The checkbox stores an X, which Word does not draw: it draws the box of
    // the state. Its field code and result in plain runs, then as Word saves
    // a deletion, in w:delInstrText and w:delText.
    let plain = r#"<w:del w:id="9" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Check1"/><w:enabled/><w:calcOnExit w:val="0"/><w:checkBox><w:sizeAuto/><w:default w:val="0"/><w:checked/></w:checkBox></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMCHECKBOX </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>X</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:del>"#;
    let word = plain
        .replace("w:instrText", "w:delInstrText")
        .replace("<w:t>X</w:t>", "<w:delText>X</w:delText>");
    for deleted in [plain, word.as_str()] {
        let doc = doc_with_body(&format!(
            r#"<w:p>{deleted}<w:r><w:t xml:space="preserve"> kept</w:t></w:r></w:p>"#
        ));
        let texts = drawn_text_in(&doc, RevisionView::Tracked);
        assert!(
            texts.contains('\u{2612}'),
            "the deleted box draws: {texts:?} for {deleted}"
        );
        assert!(!texts.contains('X'), "not its stored X: {texts:?}");
        let accepted: String = drawn_runs(&doc)
            .into_iter()
            .map(|(text, _, _)| text)
            .collect();
        assert_eq!(accepted, " kept", "the accepted view draws no box");
    }
}

/// A legacy drop-down as Word saves it: `w:ffData` with its entries and the
/// selected one, and no result run.
fn word_dropdown(result: &str) -> String {
    format!(
        r#"<w:r><w:rPr><w:sz w:val="28"/></w:rPr><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="PaymentTerms"/><w:enabled/><w:calcOnExit w:val="0"/><w:ddList>{result}<w:listEntry w:val="On receipt"/><w:listEntry w:val="30 days"/><w:listEntry w:val="60 days"/></w:ddList></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMDROPDOWN </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

#[test]
fn a_word_saved_legacy_dropdown_draws_its_selected_entry_in_its_opening_runs_format() {
    let doc = doc_with_body(&format!(
        "<w:p>{}</w:p><w:p>{}</w:p>",
        word_dropdown(r#"<w:result w:val="1"/>"#),
        word_dropdown(""),
    ));
    let runs = drawn_runs(&doc);
    let texts: String = runs.iter().map(|(text, _, _)| text.as_str()).collect();
    assert_eq!(
        texts, "30 daysOn receipt",
        "the selected entry, else the first"
    );
    assert!(
        runs.iter().all(|(_, size, _)| *size == 14.0),
        "the opening run's size: {runs:?}"
    );
}

fn drawn_text(doc: &Document) -> String {
    drawn_runs(doc)
        .into_iter()
        .map(|(text, _, _)| text)
        .collect()
}

#[test]
fn a_word_saved_legacy_dropdown_draws_its_entry_nested_locked_or_by_default() {
    let nested = doc_with_body(&format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF 1 = 1 "yes" "no" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        word_dropdown(r#"<w:result w:val="1"/>"#)
    ));
    assert_eq!(
        drawn_text(&nested),
        "30 days",
        "nested in another field's result"
    );
    let by_default = doc_with_body(&format!(
        "<w:p>{}</w:p>",
        word_dropdown(r#"<w:default w:val="2"/>"#)
    ));
    assert_eq!(drawn_text(&by_default), "60 days", "its default entry");
    let locked = doc_with_body(&format!(
        "<w:p>{}</w:p>",
        word_dropdown("").replace(
            r#"w:fldCharType="begin">"#,
            r#"w:fldCharType="begin" w:fldLock="1">"#
        )
    ));
    assert_eq!(drawn_text(&locked), "On receipt", "locked, it still draws");
}

#[test]
fn a_legacy_dropdown_in_a_tracked_deletion_draws_its_entry_in_the_tracked_view() {
    let doc = doc_with_body(&format!(
        r#"<w:p><w:del w:id="9" w:author="a" w:date="2026-01-01T00:00:00Z">{}</w:del><w:r><w:t xml:space="preserve"> kept</w:t></w:r></w:p>"#,
        word_dropdown(r#"<w:result w:val="1"/>"#)
    ));
    let options = RenderOptions::default().with_revision_view(RevisionView::Tracked);
    let result = doc.layout_with_options(options).unwrap();
    let mut texts = String::new();
    for page in &result.layout.pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            if let oxoffice_layout::PositionedElement::Text(run) = element {
                texts.push_str(&run.text);
            }
        });
    }
    assert!(
        texts.contains("30 days"),
        "the deleted entry draws: {texts:?}"
    );
    assert_eq!(drawn_text(&doc), " kept", "the accepted view does not");
}

#[test]
fn a_styleref_to_a_heading_holding_a_legacy_dropdown_reads_its_entry() {
    let doc = doc_with_body(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{}<w:r><w:t xml:space="preserve"> terms</w:t></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> STYLEREF "Heading 1" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>old</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        word_dropdown(r#"<w:result w:val="1"/>"#)
    ));
    assert_eq!(drawn_text(&doc), "30 days terms30 days terms");
}

#[test]
fn a_locked_legacy_checkbox_draws_its_box() {
    let checkbox = word_checkbox("<w:sizeAuto/>", true).replacen(
        r#"<w:fldChar w:fldCharType="begin">"#,
        r#"<w:fldChar w:fldCharType="begin" w:fldLock="1">"#,
        1,
    );
    let doc = doc_with_body(&format!("<w:p>{checkbox}</w:p>"));
    let runs = drawn_runs(&doc);
    assert!(
        runs.iter().any(|(text, _, _)| text == "\u{2612}"),
        "a locked checkbox still draws its box"
    );
}

/// A PAGE field deleted in Word, as Word saves the deletion: its code in
/// `w:delInstrText`, its result in `w:delText`.
const WORD_DELETED_PAGE: &str = r#"<w:del w:id="7" w:author="Ada" w:date="2026-01-01T00:00:00Z"><w:r w:rsidDel="00A1B2C3"><w:fldChar w:fldCharType="begin"/></w:r><w:r w:rsidDel="00A1B2C3"><w:delInstrText xml:space="preserve"> PAGE </w:delInstrText></w:r><w:r w:rsidDel="00A1B2C3"><w:fldChar w:fldCharType="separate"/></w:r><w:r w:rsidDel="00A1B2C3"><w:delText>3</w:delText></w:r><w:r w:rsidDel="00A1B2C3"><w:fldChar w:fldCharType="end"/></w:r></w:del>"#;

/// The text drawn on the pages in this revision view.
fn drawn_text_in(doc: &Document, view: RevisionView) -> String {
    let options = RenderOptions::default().with_revision_view(view);
    let result = doc.layout_with_options(options).unwrap();
    let mut text = String::new();
    for page in &result.layout.pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            if let oxoffice_layout::PositionedElement::Text(run) = element {
                text.push_str(&run.text);
            }
        });
    }
    text
}

/// A heading as Google Docs writes it: its `_Toc` bookmark starts between
/// paragraphs, a direct child of the body, and ends inside the heading.
const GOOGLE_HEADING: &str = r#"<w:bookmarkStart w:colFirst="0" w:colLast="0" w:name="_Toc217000001" w:id="4"/><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Introduction</w:t></w:r><w:bookmarkEnd w:id="4"/></w:p>"#;

/// A TOC field as Google Docs writes it, with one stale entry.
const GOOGLE_TOC: &str = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \h \u \z \t "Heading 1,1,Heading 2,2," </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p><w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:hyperlink w:anchor="_Toc217000001"><w:r><w:t>Old entry</w:t></w:r><w:r><w:tab/><w:t>9</w:t></w:r></w:hyperlink></w:p><w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;

#[test]
fn a_heading_bookmark_that_starts_between_paragraphs_is_reused_by_the_toc_rebuild() {
    let mut doc = doc_with_body(&format!("{GOOGLE_TOC}{GOOGLE_HEADING}"));
    let report = doc.rebuild_toc().unwrap();
    assert_eq!((report.entry_count, report.bookmark_count), (1, 0));
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc)[1], "Introduction\t1");
    assert_eq!(
        link_anchors(&doc),
        pairs(&[("Introduction", "_Toc217000001")]),
        "the entry links to the heading's own bookmark"
    );
}

#[test]
fn a_bookmark_that_starts_or_ends_between_paragraphs_is_listed_with_its_text() {
    // Starts between paragraphs and ends inside the heading; then starts
    // inside a paragraph and ends between two; then wraps two paragraphs.
    let doc = doc_with_body(&format!(
        r#"{GOOGLE_HEADING}<w:p><w:bookmarkStart w:id="5" w:name="Tail"/><w:r><w:t>Body</w:t></w:r></w:p><w:bookmarkEnd w:id="5"/><w:bookmarkStart w:id="6" w:name="Both"/><w:p><w:r><w:t>One</w:t></w:r></w:p><w:p><w:r><w:t>Two</w:t></w:r></w:p><w:bookmarkEnd w:id="6"/>"#
    ));
    assert_eq!(
        bookmark_list(&doc),
        pairs(&[
            ("_Toc217000001", "Introduction"),
            ("Tail", "Body"),
            ("Both", "One\nTwo")
        ])
    );
    assert!(
        doc.bookmarks()
            .iter()
            .all(|bookmark| bookmark.issue().is_none()),
        "{:?}",
        doc.bookmarks()
    );
}

#[test]
fn ref_and_pageref_read_a_bookmark_that_starts_between_paragraphs() {
    let mut doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">See </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> REF _Toc217000001 \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>old</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> on page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGEREF _Toc217000001 \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>9</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p><w:p><w:r><w:br w:type="page"/></w:r></w:p>{GOOGLE_HEADING}"#
    ));
    assert_eq!(
        drawn_text_in(&doc, RevisionView::Accepted),
        "See Introduction on page 2Introduction",
        "the layout reads the heading's text and page"
    );
    // REF updates with the fields, PAGEREF with the layout.
    doc.update_fields(&no_context()).unwrap();
    let report = doc.update_layout_backed_fields().unwrap();
    assert_eq!(report.page_reference_fields, 1, "{:?}", report.diagnostics);
    assert_eq!(texts(&reopen(&mut doc))[0], "See Introduction on page 2");
}

#[test]
fn the_google_docs_thesis_rebuilds_its_tocs_on_its_own_heading_bookmarks() {
    // Google Docs writes every `_Toc` start between paragraphs, and saves
    // the Word copy's table of figures as a second heading TOC: two TOCs of
    // the 29 headings. The 23 headings with a `_Toc` bookmark keep it, the
    // six without one (Abstract, Contents, References...) get one.
    let mut google = corpus_doc("google/thesis.docx");
    let report = google.rebuild_toc().unwrap();
    assert_eq!(
        (report.entry_count, report.bookmark_count),
        (58, 6),
        "{report:?}"
    );
    assert!(report.diagnostics.is_empty(), "{report:?}");
}

/// The main document part of a saved package.
fn main_part_xml(bytes: &[u8]) -> String {
    let package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let part = package.main_document_part().unwrap();
    String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap()
}

#[test]
fn a_deleted_field_draws_its_stored_result_in_the_tracked_view_only() {
    // Word's shape, and the same deletion with its code in w:instrText.
    for deleted in [
        WORD_DELETED_PAGE.to_owned(),
        WORD_DELETED_PAGE.replace("w:delInstrText", "w:instrText"),
    ] {
        let doc = doc_with_body(&format!(
            r#"<w:p><w:r><w:t xml:space="preserve">Page </w:t></w:r>{deleted}<w:r><w:t xml:space="preserve"> kept</w:t></w:r></w:p>"#
        ));
        assert_eq!(
            drawn_text_in(&doc, RevisionView::Tracked),
            "Page 3 kept",
            "the stored result, not the field code nor a page number: {deleted}"
        );
        assert_eq!(drawn_text_in(&doc, RevisionView::Accepted), "Page  kept");
    }
}

#[test]
fn a_field_deleted_in_word_keeps_its_deleted_code_through_saves_and_updates() {
    let mut doc = doc_with_body(&format!(
        r#"<w:p>{WORD_DELETED_PAGE}<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Name </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«Name»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
    ));
    let context = merge_context(&[("Name", "Ada")]);
    // The deleted field is not a live field: only the merge field evaluates.
    assert_eq!(doc.evaluate_fields(&context).unwrap().len(), 1);
    let updated = doc.to_bytes_with_field_updates(&context).unwrap();
    for bytes in [doc.to_bytes().unwrap(), updated.clone()] {
        let xml = main_part_xml(&bytes);
        assert!(xml.contains(WORD_DELETED_PAGE), "{xml}");
    }
    assert_eq!(texts(&Document::from_bytes(&updated).unwrap()), ["Ada"]);
}

#[test]
fn a_field_deleted_in_word_is_the_same_story_item_as_its_code_in_instruction_text() {
    let items = |deleted: &str| {
        let doc = doc_with_body(&format!("<w:p>{deleted}</w:p>"));
        doc.story_items(&story(&doc, StoryKind::Body))
            .unwrap()
            .into_iter()
            .map(|item| (item.kind(), item.text().unwrap()))
            .collect::<Vec<_>>()
    };
    let word = items(WORD_DELETED_PAGE);
    assert!(
        word.iter().any(|(kind, _)| *kind == StoryItemKind::Field),
        "{word:?}"
    );
    assert_eq!(
        word,
        items(&WORD_DELETED_PAGE.replace("w:delInstrText", "w:instrText"))
    );
}

#[test]
fn a_caption_retyped_with_tracked_changes_keeps_its_tables_rebuildable() {
    // Word saves a retyped caption as the old one deleted, its SEQ code in
    // w:delInstrText, and the new one inserted.
    let field = |code: &str, text: &str, result: &str| {
        format!(
            r#"<w:r><w:{text}>Figure </w:{text}></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:{code} xml:space="preserve"> SEQ Figure \* ARABIC </w:{code}></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:{text}>{result}</w:{text}></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    let mut doc = doc_with_body(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Caption"/></w:pPr><w:del w:id="1" w:author="Ana" w:date="2026-01-01T00:00:00Z">{}</w:del><w:ins w:id="2" w:author="Ana" w:date="2026-01-01T00:00:00Z">{}</w:ins><w:r><w:t xml:space="preserve">: Sales</w:t></w:r></w:p>"#,
        field("delInstrText", "delText", "1"),
        field("instrText", "t", "1"),
    ));
    doc.rebuild_generated_tables().unwrap();
}

#[test]
fn a_standing_field_whose_result_holds_deleted_text_draws_only_its_display() {
    // A field outside any deletion with a stray w:delText in its result, as
    // tools that reject changes naively leave it.
    let doc = doc_with_body(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD Name </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>new</w:t></w:r><w:r><w:delText>old</w:delText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    assert_eq!(drawn_text_in(&doc, RevisionView::Accepted), "new");
}

#[test]
fn a_standing_field_whose_code_was_deleted_is_no_story_item() {
    // Its markers stand, its whole code was deleted with tracking on.
    let doc = doc_with_body(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:del w:id="1" w:author="Ana" w:date="2026-01-01T00:00:00Z"><w:r><w:delInstrText xml:space="preserve"> PAGE </w:delInstrText></w:r></w:del><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>3</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    let kinds: Vec<_> = doc
        .story_items(&story(&doc, StoryKind::Body))
        .unwrap()
        .into_iter()
        .map(|item| item.kind())
        .collect();
    assert!(!kinds.contains(&StoryItemKind::Field), "{kinds:?}");
}

#[test]
fn a_heading_holding_a_field_deleted_in_word_keeps_its_result_in_tracked_navigation_text() {
    // STYLEREF reads the heading's navigation text, the deleted result
    // included in the tracked view, as the page draws it.
    let deleted =
        WORD_DELETED_PAGE.replace("<w:delText>3</w:delText>", "<w:delText>Q3</w:delText>");
    let doc = doc_with_body(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t xml:space="preserve">Results </w:t></w:r>{deleted}</w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> STYLEREF "Heading 1" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>old</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
    ));
    assert_eq!(
        drawn_text_in(&doc, RevisionView::Tracked),
        "Results Q3Results Q3"
    );
}

#[test]
fn bookmarks_between_cells_and_collapsed_between_blocks_read_without_issue() {
    // Google Docs collapses an empty bookmark before a cell paragraph; Word
    // puts its form field bookmarks between cells of a row.
    for name in ["google/form.docx", "word/form.docx"] {
        let doc = corpus_doc(name);
        let issues: Vec<_> = doc
            .bookmarks()
            .iter()
            .filter_map(|bookmark| bookmark.issue().map(str::to_owned))
            .collect();
        assert!(issues.is_empty(), "{name}: {issues:?}");
    }
    let doc = doc_with_body(
        r#"<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:bookmarkStart w:id="1" w:name="Empty"/><w:bookmarkEnd w:id="1"/><w:p><w:r><w:t>After</w:t></w:r></w:p>"#,
    );
    let empty = &doc.bookmarks()[0];
    assert_eq!((empty.issue(), empty.text()), (None, ""));
    assert_eq!(
        empty.range().unwrap().start.body_index,
        1,
        "at the next paragraph"
    );
}

#[test]
fn a_bookmark_around_a_table_with_a_row_control_reads_its_rows() {
    let doc = doc_with_body(
        r#"<w:bookmarkStart w:id="1" w:name="Rows"/><w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>First</w:t></w:r></w:p></w:tc></w:tr><w:sdt><w:sdtPr><w:tag w:val="rows"/></w:sdtPr><w:sdtContent><w:tr><w:tc><w:p><w:r><w:t>Repeated</w:t></w:r></w:p></w:tc></w:tr></w:sdtContent></w:sdt></w:tbl><w:bookmarkEnd w:id="1"/><w:p><w:r><w:t>After</w:t></w:r></w:p>"#,
    );
    assert_eq!(bookmark_list(&doc), pairs(&[("Rows", "First\nRepeated")]));
}

#[test]
fn a_heading_bookmark_that_stops_before_the_heading_ends_is_not_reused() {
    // A run of properties only after the end leaves the bookmark whole; a
    // run of text does not, so the rebuild gives the heading its own.
    let mut doc = doc_with_body(&format!(
        r#"{GOOGLE_TOC}<w:bookmarkStart w:id="4" w:name="_Toc217000001"/><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="4"/><w:r><w:t xml:space="preserve"> and more</w:t></w:r></w:p>"#
    ));
    let report = doc.rebuild_toc().unwrap();
    assert_eq!((report.entry_count, report.bookmark_count), (1, 1));
    let mut doc = doc_with_body(&format!(
        r#"{GOOGLE_TOC}<w:bookmarkStart w:id="4" w:name="_Toc217000001"/><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="4"/><w:r><w:rPr><w:b/></w:rPr></w:r></w:p>"#
    ));
    let report = doc.rebuild_toc().unwrap();
    assert_eq!((report.entry_count, report.bookmark_count), (1, 0));
}

#[test]
fn a_toc_leaves_out_a_heading_deleted_with_tracked_changes_as_word_writes_it() {
    // Word records a whole deleted heading as its runs in w:del and its
    // paragraph mark in w:pPr/w:rPr/w:del; a heading whose mark alone is
    // deleted joins the next paragraph, whose entry it opens.
    let toc = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p><w:p><w:r><w:t>stale</w:t></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    let deleted_mark =
        r#"<w:rPr><w:del w:id="9" w:author="Ana" w:date="2026-01-01T00:00:00Z"/></w:rPr>"#;
    let mut doc = doc_with_body(&format!(
        r#"{toc}<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Kept</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="Heading1"/>{deleted_mark}</w:pPr><w:del w:id="8" w:author="Ana" w:date="2026-01-01T00:00:00Z"><w:r><w:delText>Removed</w:delText></w:r></w:del></w:p><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Next</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="Heading1"/>{deleted_mark}</w:pPr><w:r><w:t xml:space="preserve">Joined </w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="Heading1"/>{deleted_mark}</w:pPr><w:r><w:t xml:space="preserve">twice </w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>tail</w:t></w:r></w:p>"#
    ));
    let revisions = doc.revisions().len();
    let report = doc.rebuild_toc().unwrap();
    assert_eq!(report.entry_count, 3, "{report:?}");
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc)[1..4],
        ["Kept\t1", "Next\t1", "Joined twice tail\t1"]
    );
    assert_eq!(
        doc.paragraph(3).unwrap().style_id(),
        Some("TOC2"),
        "the joined entry takes the level of the paragraph it joins"
    );
    assert_eq!(doc.revisions().len(), revisions, "the revisions stay");
}

/// What a tool does after each edit pass on a long document: rebuild the
/// table of contents, refresh the page fields from the layout, save. The TOC
/// lists the headings to its depth, in the file's own TOC styles, with the
/// pages of the layout; the header and footer parts keep the page caches
/// Word left there; a second pass changes no byte; and the PDF has as many
/// pages as the layout.
#[test]
fn a_page_number_pass_on_a_thesis_is_stable_and_follows_the_layout() {
    crate::support::run_workflow("page-numbers", &crate::support::thesis_copies(), |path| {
        let source = crate::support::workflow_source(path);
        let pass = |bytes: &[u8]| {
            let mut doc = Document::from_bytes(bytes).unwrap();
            let toc = doc
                .rebuild_toc()
                .unwrap_or_else(|e| panic!("{path}: rebuild: {e}"));
            let fields = doc.update_layout_backed_fields().unwrap();
            (toc, fields, doc.to_bytes().unwrap())
        };
        let (toc, fields, once) = pass(&source);
        let (_, _, twice) = pass(&once);
        assert!(once == twice, "{path}: a second pass changes no byte");

        // Only the body's caches move; header and footer parts stay as saved.
        // A file without TOC styles (the small thesis, Google's copy) gets them.
        let before = crate::support::package_parts(&source);
        let after = crate::support::package_parts(&once);
        let changed = after
            .iter()
            .filter(|(name, bytes)| before.get(*name) != Some(*bytes))
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        let has_toc_styles = String::from_utf8_lossy(&before["word/styles.xml"])
            .contains(r#"w:styleId="TOC1""#)
            || String::from_utf8_lossy(&before["word/styles.xml"]).contains(r#"w:styleId="TM1""#);
        let expected: &[&str] = if !has_toc_styles {
            &["word/document.xml", "word/styles.xml"]
        } else {
            &["word/document.xml"]
        };
        assert_eq!(changed, expected, "{path}");
        assert!(fields.page_reference_fields > 0, "{path}: {fields:?}");

        // Two fields: the contents (`\o "1-3" \u`) list the headings of levels
        // 1 to 3, the list of figures and tables (`\t "Caption,1"`) the
        // captions; each entry in the TOC style of its level as the file
        // names it (Word in French calls them TM1, TM2...). Google rewrites
        // the list of figures as a second contents of the heading styles, so
        // its copy lists the headings twice.
        let doc = Document::from_bytes(&once).unwrap();
        let mut expected = doc
            .headings()
            .into_iter()
            .filter(|(level, _)| *level <= 3)
            .collect::<Vec<_>>();
        let first_list = expected.len();
        if path.starts_with("google/") {
            expected.extend(expected.clone());
        } else {
            expected.extend(
                (0..doc.paragraph_count())
                    .map(|index| doc.paragraph(index).unwrap())
                    .filter(|paragraph| matches!(paragraph.style_id(), Some("Caption" | "Lgende")))
                    .map(|paragraph| (1, paragraph.text())),
            );
        }
        let toc_style = |level: u32| {
            if path.starts_with("word/") {
                format!("TM{level}")
            } else {
                format!("TOC{level}")
            }
        };
        let entries = (0..doc.paragraph_count())
            .map(|index| doc.paragraph(index).unwrap())
            .filter(|paragraph| {
                paragraph
                    .style_id()
                    .is_some_and(|style| style.starts_with("TOC") || style.starts_with("TM"))
            })
            .map(|paragraph| {
                let text = paragraph.text();
                let parts = text.split('\t').collect::<Vec<_>>();
                let title = parts[parts.len().saturating_sub(2)].to_owned();
                // The page number sits on a right tab with a dotted leader.
                let dotted = (0..)
                    .map_while(|index| paragraph.tab_stop(index))
                    .any(|tab| {
                        tab.checked_values().ok()
                            == Some((TabAlignment::Right, Some(TabLeader::Dot)))
                    });
                assert!(dotted, "{path}: {title} has a dotted right tab");
                (
                    paragraph.style_id().unwrap().to_owned(),
                    title,
                    parts.last().unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), toc.entry_count, "{path}");
        assert_eq!(
            entries
                .iter()
                .map(|(_, title, _)| title.as_str())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>(),
            "{path}"
        );
        for ((style, title, page), (level, _)) in entries.iter().zip(&expected) {
            assert_eq!(style, &toc_style(*level), "{path}: {title}");
            assert!(
                page.chars()
                    .all(|c| c.is_ascii_digit() || "ivxlc".contains(c))
                    && !page.is_empty(),
                "{path}: {title} has a page, {page:?}"
            );
        }
        // The pages follow the layout: on the small thesis each chapter starts
        // a page; on a corpus copy the arabic pages of the contents, then of
        // the list of figures, go back only where a section restarts at 1,
        // and spread over the document.
        let pages = entries
            .iter()
            .map(|(_, _, page)| page.as_str())
            .collect::<Vec<_>>();
        if path == crate::support::SMALL_THESIS {
            assert_eq!(
                pages,
                ["1", "2", "2", "2", "3", "3", "3", "4", "4"],
                "{path}"
            );
        } else {
            let headings = first_list;
            for list in [&pages[..headings], &pages[headings..]] {
                let arabic = list
                    .iter()
                    .filter_map(|page| page.parse::<u32>().ok())
                    .collect::<Vec<_>>();
                // A section that restarts its numbering goes back to 1
                // (Google numbers the front matter in arabic too).
                assert!(
                    arabic
                        .windows(2)
                        .all(|pair| pair[0] <= pair[1] || pair[1] == 1),
                    "{path}: {pages:?}"
                );
            }
            let mut distinct = pages[..headings].to_vec();
            distinct.dedup();
            assert!(distinct.len() > 5, "{path}: {pages:?}");
        }

        // The PDF on the small thesis and one corpus copy: every corpus
        // file's render checks the same on the file as saved, and a thesis
        // PDF costs a layout of its own.
        if path != crate::support::SMALL_THESIS && !path.starts_with("word/") {
            return;
        }
        let pdf = oxoffice_render_test::pdf::Pdf::read(&doc.to_pdf().unwrap());
        assert_eq!(
            pdf.page_count(),
            doc.layout_deterministic().unwrap().layout.pages.len(),
            "{path}: the PDF has the layout's pages"
        );
    });
}

/// The same pass on a record co-edited in Word and Google Docs, whose
/// contents sit in a content control and take outline levels (`\u`) next to
/// a table with a vertically merged cell: the contents list the headings of
/// levels 1 and 2 (its own title included) with their numbers and pages, a
/// paragraph of outline level 3 stays out, and a second pass changes no byte.
#[test]
fn a_page_number_pass_on_a_co_edited_record_is_stable() {
    // The record's headings of levels 1 and 2 as the contents list them,
    // with their numbers; the contents title is a heading without a number.
    const ENTRIES: [&str; 10] = [
        "Contents",
        "1.\tIdentification",
        "2.\tScope",
        "2.1.\tIn scope",
        "2.2.\tOut of scope",
        "3.\tImpact assessment",
        "3.1.\tRisks",
        "4.\tTasks",
        "5.\tDecision",
        "6.\tHistory",
    ];
    crate::support::run_workflow("page-numbers", &crate::support::record_copies(), |path| {
        let source = crate::support::workflow_source(path);
        let pass = |bytes: &[u8]| {
            let mut doc = Document::from_bytes(bytes).unwrap();
            let toc = doc
                .rebuild_toc()
                .unwrap_or_else(|e| panic!("{path}: rebuild: {e}"));
            doc.update_layout_backed_fields().unwrap();
            (toc, doc.to_bytes().unwrap())
        };
        let (toc, once) = pass(&source);

        // The contents of the first pass, read back from the saved package.
        let doc = Document::from_bytes(&once).unwrap();
        // Google Docs moves the contents title out of the control and starts
        // the field in an empty paragraph: the entries start at the title's.
        let first = (0..doc.paragraph_count())
            .find(|&index| {
                doc.paragraph(index)
                    .unwrap()
                    .text()
                    .starts_with("Contents\t")
            })
            .unwrap_or_else(|| panic!("{path}: the contents title's entry"));
        assert_eq!(toc.entry_count, ENTRIES.len(), "{path}");
        let entries = (first..first + toc.entry_count)
            .map(|index| doc.paragraph(index).unwrap().text())
            .collect::<Vec<_>>();
        // An entry ends with its page, on a tab.
        let (titles, pages): (Vec<_>, Vec<_>) = entries
            .iter()
            .map(|entry| {
                let (title, page) = entry
                    .rsplit_once('\t')
                    .unwrap_or_else(|| panic!("{path}: {entry:?} has a page"));
                (
                    title,
                    page.parse::<u32>()
                        .unwrap_or_else(|_| panic!("{path}: {entry:?}")),
                )
            })
            .unzip();
        assert_eq!(titles, ENTRIES, "{path}: {entries:?}");
        assert!(
            pages.windows(2).all(|pair| pair[0] <= pair[1]) && pages.last() > pages.first(),
            "{path}: {entries:?}"
        );

        let (_, twice) = pass(&once);
        assert!(once == twice, "{path}: a second pass changes no byte");
    });
}

/// A Word TOC field over levels 1 to 3 that also takes outline levels (`\u`),
/// with one stale entry, followed by `body`.
fn word_toc_before(body: &str) -> Document {
    doc_with_body(&format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:r><w:t>Stale entry</w:t></w:r></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>{body}"#
    ))
}

// A TOC that takes outline levels was once refused when the document held a
// table with a vertically merged cell.
#[test]
fn a_toc_taking_outline_levels_is_rebuilt_beside_a_vertically_merged_table() {
    let cell = |merge: &str, text: &str| {
        format!(
            r#"<w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/>{merge}</w:tcPr><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>"#
        )
    };
    let table = format!(
        r#"<w:tbl><w:tblPr><w:tblW w:w="4000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr></w:tbl>"#,
        cell(r#"<w:vMerge w:val="restart"/>"#, "Merged"),
        cell("", "First row"),
        cell("<w:vMerge/>", ""),
        cell("", "Second row"),
    );
    let mut doc = word_toc_before(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Scope</w:t></w:r></w:p>{table}
        <w:p><w:pPr><w:outlineLvl w:val="1"/></w:pPr><w:r><w:t>Outline item</w:t></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Results</w:t></w:r></w:p>"#
    ));
    let report = doc
        .rebuild_toc()
        .expect("a merged cell does not block the rebuild");
    assert_eq!(report.entry_count, 3);
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc)[1..4],
        ["Scope\t1", "Outline item\t1", "Results\t1"]
    );
    assert!(!texts(&doc).iter().any(|text| text.contains("Stale entry")));
    assert_eq!(doc.tables().len(), 1, "the table is kept");
}
