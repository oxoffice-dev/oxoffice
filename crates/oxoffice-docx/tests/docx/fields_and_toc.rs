//! Fields (inserting, reading, updating, `update_fields_on_open`), bookmarks,
//! cross-references, captions with SEQ numbering and tables of contents.

use oxoffice_docx::{
    CaptionOptions, ContentLocation, CrossReferenceOptions, Document, Field, FieldDateTime,
    FieldEvaluationContext, FieldKind, FieldOutcome, ParagraphItemRef, RenderOptions, RevisionView,
    RunItemRef, RunPosition, RunRange, SequenceOptions, StoryId, StoryItemKind, StoryKind,
    StoryRunPosition, TabAlignment, TabLeader, TableOfFiguresOptions, TocRebuildReport,
};

use crate::support::{doc_with, doc_with_body, reopen, texts};

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

#[test]
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

/// A complex field with one result run.
fn complex_field(instruction: &str, result: &str) -> String {
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{result}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

/// A simple field with one result run.
fn simple_field(instruction: &str, result: &str) -> String {
    format!(
        r#"<w:fldSimple w:instr=" {instruction} "><w:r><w:t>{result}</w:t></w:r></w:fldSimple>"#
    )
}

/// A paragraph that is the bookmark `name` around `content`.
fn bookmarked(id: u32, name: &str, content: &str) -> String {
    format!(
        r#"<w:p><w:bookmarkStart w:id="{id}" w:name="{name}"/>{content}<w:bookmarkEnd w:id="{id}"/></w:p>"#
    )
}

fn plain(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

/// The paragraph texts after each of three updates and after a reopen.
fn texts_over_updates(body: &str) -> Vec<Vec<String>> {
    let mut doc = doc_with_body(body);
    let mut rounds = Vec::new();
    for _ in 0..3 {
        doc.update_fields(&no_context()).unwrap();
        rounds.push(texts(&doc));
    }
    rounds.push(texts(&reopen(&mut doc)));
    rounds
}

#[test]
fn a_ref_reads_its_bookmark_as_a_reader_sees_it() {
    // A simple field's result, a drop-down that stores no result, and a REF
    // to another bookmark are part of the text a REF copies.
    let dropdown = word_dropdown(r#"<w:result w:val="1"/>"#);
    for (body, expected) in [
        (
            format!(
                "{}<w:p>{}</w:p>",
                bookmarked(
                    0,
                    "total",
                    &(plain("Total: ") + &simple_field("QUOTE 5", "5"))
                ),
                complex_field("REF total", "old")
            ),
            "Total: 5",
        ),
        (
            format!(
                "{}<w:p>{}</w:p>",
                bookmarked(0, "PaymentTerms", &dropdown),
                complex_field("REF PaymentTerms", "old")
            ),
            "30 days",
        ),
        (
            format!(
                "{}{}<w:p>{}</w:p>",
                bookmarked(0, "a", &(plain("a ") + &simple_field("REF c", "C"))),
                bookmarked(1, "c", &plain("C")),
                complex_field("REF a", "old")
            ),
            "a C",
        ),
    ] {
        let rounds = texts_over_updates(&body);
        for round in &rounds {
            assert_eq!(round.last().unwrap(), expected, "{body}: {rounds:?}");
        }
    }
}

#[test]
fn a_word_saved_dropdown_reads_its_selected_entry() {
    let doc = doc_with_body(&format!(
        "<w:p>{}{}</w:p>",
        plain("Terms: "),
        word_dropdown(r#"<w:result w:val="1"/>"#)
    ));
    assert_eq!(texts(&doc), ["Terms: 30 days"]);
}

#[test]
fn a_ref_evaluates_to_its_bookmark_as_a_reader_sees_it() {
    let doc = doc_with_body(&format!(
        "{}<w:p>{}</w:p>",
        bookmarked(
            0,
            "b",
            &(plain("X ") + &simple_field("QUOTE 5", "5") + &simple_field("REF b", "old"))
        ),
        complex_field("REF b", "old")
    ));
    let outcomes = doc
        .evaluate_fields(&no_context())
        .unwrap()
        .into_iter()
        .map(|evaluation| evaluation.outcome)
        .collect::<Vec<_>>();
    assert!(
        outcomes.contains(&FieldOutcome::Resolved("X 5".into())),
        "{outcomes:?}"
    );
}

#[test]
fn a_ref_to_a_footnote_bookmark_that_refers_to_itself_is_stable() {
    let mut doc = Document::new();
    let id = doc.add_footnote("NOTE");
    doc.add_paragraph("See").add_footnote_ref(id);
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    let notes = package
        .zip_entries()
        .unwrap()
        .into_iter()
        .map(|(name, _)| format!("/{name}"))
        .find(|name| name.ends_with("footnotes.xml"))
        .expect("a footnotes part");
    let xml = String::from_utf8(package.get_part(&notes).unwrap().to_vec()).unwrap();
    let edited = xml.replacen(
        "NOTE</w:t></w:r>",
        &format!(
            r#"NOTE</w:t></w:r><w:bookmarkStart w:id="90" w:name="fn"/>{}{}<w:bookmarkEnd w:id="90"/>"#,
            plain("N "),
            simple_field("REF fn", "old")
        ),
        1,
    );
    assert_ne!(edited, xml, "the note holds the bookmark");
    package.set_part(&notes, edited.into_bytes());
    let body = package.main_document_part().unwrap();
    let main = String::from_utf8(package.get_part(&body).unwrap().to_vec()).unwrap();
    let main = main.replacen(
        "<w:sectPr",
        &format!("<w:p>{}</w:p><w:sectPr", complex_field("REF fn", "old")),
        1,
    );
    package.set_part(&body, main.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    for _ in 0..3 {
        doc.update_fields(&no_context()).unwrap();
        assert_eq!(texts(&doc).last().unwrap(), "N ");
    }
}

#[test]
fn a_ref_draws_its_bookmark_as_a_reader_sees_it() {
    // Drawn without an update: the layout reads the bookmark itself.
    let dropdown = word_dropdown(r#"<w:result w:val="1"/>"#);
    for (body, drawn) in [
        (
            format!(
                "{}<w:p>{}</w:p>",
                bookmarked(
                    0,
                    "total",
                    &(plain("Total: ") + &simple_field("QUOTE 5", "5"))
                ),
                complex_field("REF total", "old")
            ),
            "Total: 5Total: 5",
        ),
        (
            format!(
                "{}<w:p>{}</w:p>",
                bookmarked(0, "PaymentTerms", &dropdown),
                complex_field("REF PaymentTerms", "old")
            ),
            "30 days30 days",
        ),
        // A REF to the bookmark itself, and two bookmarks that copy each
        // other, draw what update_fields writes.
        (
            bookmarked(0, "b", &(plain("X ") + &simple_field("REF b", "old"))),
            "X X ",
        ),
        (
            bookmarked(0, "a", &(plain("a ") + &simple_field("REF b", "x")))
                + &bookmarked(1, "b", &(plain("b ") + &simple_field("REF a", "y"))),
            "a b b a ",
        ),
    ] {
        assert_eq!(drawn_text(&doc_with_body(&body)), drawn, "{body}");
    }
}

#[test]
fn a_ref_that_depends_on_its_own_bookmark_gives_the_same_text_on_every_update() {
    // Word refuses a bookmark self-reference: the REF's result is left out of
    // the bookmark's value, so it cannot grow from one update to the next.
    let if_around = |reference: &str| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF 1 = 1 "</w:instrText></w:r>{reference}<w:r><w:instrText xml:space="preserve">" "no" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>old</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    for (body, expected) in [
        (
            bookmarked(0, "b", &(plain("X ") + &simple_field("REF b", "old"))),
            vec!["X X "],
        ),
        (
            bookmarked(0, "b", &(plain("X ") + &complex_field("REF b", "old"))),
            vec!["X X "],
        ),
        (
            bookmarked(
                0,
                "b",
                &(plain("X ") + &if_around(&complex_field("REF b", "in"))),
            ),
            vec!["X X "],
        ),
        (
            bookmarked(0, "a", &(plain("a ") + &simple_field("REF b", "x")))
                + &bookmarked(1, "b", &(plain("b ") + &simple_field("REF a", "y"))),
            vec!["a b ", "b a "],
        ),
    ] {
        let rounds = texts_over_updates(&body);
        for round in &rounds {
            assert_eq!(round, &expected, "{body}: {rounds:?}");
        }
    }
}

#[test]
fn a_tracked_deletion_of_its_only_citation_leaves_a_category_empty() {
    let ta = |category: u8| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:{{text}} xml:space="preserve"> TA \l "Case {category}" \s "C{category}" \c {category} </w:{{text}}></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    let deleted = format!(
        r#"<w:del w:id="9" w:author="A" w:date="2026-01-01T00:00:00Z">{}</w:del>"#,
        ta(2).replace("{text}", "delInstrText")
    );
    // A later citation marked with its short form alone, as Word's Mark All
    // writes it, has no category of its own.
    let short = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TA \s "C1" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
    let mut doc = doc_with_body(&format!(
        "<w:p>{}{deleted}{short}</w:p><w:p>{}</w:p>",
        ta(1).replace("{text}", "instrText"),
        plain("End")
    ));
    let end = ContentLocation::end(body_story(&doc));
    doc.insert_table_of_authorities(&end, &Default::default())
        .unwrap();
    let categories = field_list(&doc)
        .into_iter()
        .filter(|(_, instruction, _)| instruction.starts_with("TOA"))
        .map(|(_, instruction, _)| instruction)
        .collect::<Vec<_>>();
    assert_eq!(categories.len(), 1, "{categories:?}");
    assert!(categories[0].contains(r"\c 1"), "{categories:?}");

    let mut doc = doc_with_body(&format!("<w:p>{}{deleted}</w:p>", plain("Body")));
    let end = ContentLocation::end(body_story(&doc));
    let error = doc
        .insert_table_of_authorities(&end, &Default::default())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("no populated authority category"),
        "{error}"
    );
}

/// An IF field whose bookmark starts among its instruction runs, as
/// LibreOffice writes a form field's bookmark, with LibreOffice's empty runs.
const FIELD_WITH_INSTRUCTION_BOOKMARK: &str = r#"<w:p><w:r><w:t xml:space="preserve">Before </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF 1 = 1 "new" "no" </w:instrText></w:r><w:bookmarkStart w:id="0" w:name="Mark"/><w:r></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>old</w:t></w:r><w:r></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r></w:r><w:bookmarkEnd w:id="0"/><w:r><w:t xml:space="preserve"> after</w:t></w:r></w:p>"#;

/// The main part's body, from `<w:body>` to the section properties.
fn saved_body(doc: &mut Document) -> String {
    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(
        doc.to_bytes().expect("document saves"),
    ))
    .expect("package opens");
    let part = package.main_document_part().expect("main document part");
    let xml = String::from_utf8(package.get_part(&part).expect("main part").to_vec())
        .expect("main part is UTF-8");
    let start = xml.find("<w:body>").expect("body start");
    let end = xml.find("<w:sectPr").expect("section properties");
    xml[start..end].to_owned()
}

#[test]
fn a_complex_field_with_a_bookmark_before_its_result_is_read_and_updated() {
    for body in [
        FIELD_WITH_INSTRUCTION_BOOKMARK.to_owned(),
        // At the start of the result, as LibreOffice writes a drop-down's.
        FIELD_WITH_INSTRUCTION_BOOKMARK
            .replace(r#"<w:bookmarkStart w:id="0" w:name="Mark"/>"#, "")
            .replace(
                r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:bookmarkStart w:id="0" w:name="Mark"/>"#,
            ),
    ] {
        let mut doc = doc_with_body(&body);
        assert_eq!(
            field_list(&doc),
            [(FieldKind::Complex, r#"IF 1 = 1 "new" "no""#.into(), "old".into())]
        );
        let bookmark_text = |doc: &Document| {
            doc.bookmarks()
                .iter()
                .map(|bookmark| (bookmark.name().map(str::to_owned), bookmark.text().to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(bookmark_text(&doc), [(Some("Mark".into()), "old".into())]);
        assert_eq!(saved_body(&mut doc), format!("<w:body>{body}"), "saved unchanged");

        assert_eq!(
            doc.update_fields(&FieldEvaluationContext::default())
                .unwrap(),
            1
        );
        assert_eq!(texts(&doc), ["Before new after"]);
        assert_eq!(bookmark_text(&doc), [(Some("Mark".into()), "new".into())]);
        let saved = saved_body(&mut doc);
        assert_eq!(
            saved,
            format!("<w:body>{body}")
                .replace(r#"w:fldCharType="begin"/>"#, r#"w:fldCharType="begin" w:dirty="0"/>"#)
                .replace("<w:t>old</w:t>", "<w:t>new</w:t>"),
            "the bookmark keeps its place"
        );
        let reopened = reopen(&mut doc);
        assert_eq!(bookmark_text(&reopened), [(Some("Mark".into()), "new".into())]);
    }
}

#[test]
fn a_bookmark_after_a_picture_a_field_shows_keeps_its_place() {
    // The picture in the result is displayed: a bookmark after it does not
    // read at the field's start.
    let body = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> INCLUDEPICTURE "x.png" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:pict><v:rect xmlns:v="urn:schemas-microsoft-com:vml" style="width:10pt;height:10pt"/></w:pict></w:r><w:bookmarkStart w:id="0" w:name="After"/><w:r></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>tail</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#;
    let doc = doc_with_body(body);
    let starts = doc
        .bookmarks()
        .iter()
        .map(|bookmark| bookmark.range().expect("a whole bookmark").start.run_index)
        .collect::<Vec<_>>();
    assert_ne!(starts, [0], "the bookmark does not start before the field");
}

/// A complex field with this instruction, `{ ... }` marking each nested
/// MERGEFIELD by name, and this stored result.
fn merge_instruction_field(instruction: &str, result: &str) -> String {
    let mut runs = String::from(r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#);
    let mut rest = instruction;
    while let Some(open) = rest.find('{') {
        let close = rest[open..].find('}').expect("closed nested field") + open;
        runs.push_str(&format!(
            r#"<w:r><w:instrText xml:space="preserve">{}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD {} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«x»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
            &rest[..open],
            rest[open + 1..close].trim()
        ));
        rest = &rest[close + 1..];
    }
    runs.push_str(&format!(
        r#"<w:r><w:instrText xml:space="preserve">{rest}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{result}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    ));
    format!("<w:p>{runs}</w:p>")
}

#[test]
fn an_operand_joining_nested_fields_is_their_results_joined() {
    let context = merge_context(&[("First", "Ada"), ("Last", "Lovelace"), ("Qty", "4")]);
    for (instruction, expected) in [
        // Two nested fields with a space between them, quoted.
        (
            r#" IF "{First} {Last}" = "Ada Lovelace" "yes" "no" "#,
            "yes",
        ),
        // Text and a nested field side by side, without quotes.
        (" = {Qty}*2 ", "8"),
        // Two nested fields side by side, without quotes.
        (r#" IF {First}{Last} = "AdaLovelace" "yes" "no" "#, "yes"),
        // The field name written against a nested field, as Word's Ctrl+F9
        // leaves it.
        (" ={Qty}*2 ", "8"),
        (r#" IF{First} = "Ada" "yes" "no" "#, "yes"),
    ] {
        let mut doc = doc_with_body(&merge_instruction_field(instruction, "old"));
        doc.update_fields(&context).unwrap();
        assert_eq!(texts(&doc), [expected], "{instruction}");
        // Saved and opened again, the same operands evaluate the same way.
        let mut doc = reopen(&mut doc);
        doc.update_fields(&context).unwrap();
        assert_eq!(texts(&doc), [expected], "{instruction} after a reopen");
    }
}

#[test]
fn a_joined_operand_without_one_nested_result_keeps_the_stored_result() {
    let mut doc = doc_with_body(&merge_instruction_field(
        r#" IF "Mr {Name}" = "Mr Smith" "Known" "Unknown" "#,
        "stored",
    ));
    let outcome = doc
        .evaluate_fields(&no_context())
        .unwrap()
        .remove(0)
        .outcome;
    let FieldOutcome::KeepStored { diagnostic } = outcome else {
        panic!("a missing merge field keeps the stored result: {outcome:?}");
    };
    assert!(
        diagnostic.contains("nested field was not resolved"),
        "{diagnostic}"
    );
    doc.update_fields(&no_context()).unwrap();
    assert_eq!(texts(&doc), ["stored"]);
}

#[test]
fn a_new_field_with_a_joined_operand_is_written_and_read_back() {
    use oxoffice_docx_oxml::text::{FieldArgument, FieldForm, FieldInstruction};
    let instruction = FieldInstruction::new(
        "IF",
        vec![
            // Text with a quote and a backslash, then a nested field.
            FieldArgument::Joined(vec![
                FieldArgument::Text(r#"Mr "\ "#.into()),
                FieldArgument::Nested(Box::new(Field::new("MERGEFIELD Name", "«Name»"))),
            ]),
            FieldArgument::Text("=".into()),
            FieldArgument::Text(r#"Mr "\ Smith"#.into()),
            FieldArgument::Text("Known".into()),
            FieldArgument::Text("Unknown".into()),
        ],
        Vec::new(),
    )
    .unwrap();
    let field = Field::from_instruction(instruction, FieldForm::Complex, Vec::new()).unwrap();
    let mut doc = doc_with(&["x"]);
    doc.insert_field(&body_position(&doc, "x", 1), field)
        .unwrap();
    let mut doc = reopen(&mut doc);
    doc.update_fields(&merge_context(&[("Name", "Smith")]))
        .unwrap();
    assert_eq!(texts(&doc), ["xKnown"]);
}

#[test]
fn a_document_with_joined_operands_saves_unchanged() {
    let body = merge_instruction_field(
        r#" IF "Mr {Name}" = "Mr Smith" "Known" "Unknown" "#,
        "Unknown",
    );
    let mut doc = doc_with_body(&body);
    let saved = main_part_xml(&doc.to_bytes().unwrap());
    assert!(saved.contains(&body), "{saved}");
    // Opened again, the operand still joins its text and its nested field.
    let mut doc = reopen(&mut doc);
    doc.update_fields(&merge_context(&[("Name", "Smith")]))
        .unwrap();
    assert_eq!(texts(&doc), ["Known"]);
}

#[test]
fn a_new_joined_operand_needs_a_nested_field_and_no_adjacent_text() {
    use oxoffice_docx_oxml::text::{FieldArgument, FieldInstruction};
    let nested = || FieldArgument::Nested(Box::new(Field::new("MERGEFIELD Name", "x")));
    let text = |value: &str| FieldArgument::Text(value.into());
    for pieces in [
        Vec::new(),
        vec![nested()],
        vec![text("a"), text("b")],
        vec![text("a"), text("b"), nested()],
        vec![text(""), nested()],
    ] {
        let joined = FieldArgument::Joined(pieces);
        assert!(
            FieldInstruction::new("QUOTE", vec![joined.clone()], Vec::new()).is_err(),
            "{joined:?}"
        );
    }
    let joined = FieldArgument::Joined(vec![text("a"), nested()]);
    assert!(FieldInstruction::new("QUOTE", vec![joined], Vec::new()).is_ok());
}

// ---- Sequences and captions ----

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

// ---- Cross-references ----

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

/// A legacy drop-down as Word saves it: `w:ffData` with its entries and the
/// selected one, and no result run.
fn word_dropdown(result: &str) -> String {
    format!(
        r#"<w:r><w:rPr><w:sz w:val="28"/></w:rPr><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="PaymentTerms"/><w:enabled/><w:calcOnExit w:val="0"/><w:ddList>{result}<w:listEntry w:val="On receipt"/><w:listEntry w:val="30 days"/><w:listEntry w:val="60 days"/></w:ddList></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMDROPDOWN </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
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
fn an_index_entry_after_a_toc_ending_in_a_tracked_link_with_a_simple_page_field_is_indexed() {
    // The TOC's last entry is a tracked insertion of a link whose page number
    // is a simple field; its end follows in the same paragraph, then an XE in
    // a tracked insertion. The XE is outside the TOC: the index lists it.
    let toc = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p><w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:ins w:id="5" w:author="A" w:date="2026-01-01T00:00:00Z"><w:hyperlink w:anchor="_Toc1"><w:r><w:t>Intro</w:t></w:r><w:fldSimple w:instr=" PAGEREF _Toc1 \h "><w:r><w:t>3</w:t></w:r></w:fldSimple></w:hyperlink></w:ins><w:r><w:fldChar w:fldCharType="end"/></w:r><w:ins w:id="6" w:author="A" w:date="2026-01-01T00:00:00Z"><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> XE "Apple" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:ins></w:p>"#;
    let heading = r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="1" w:name="_Toc1"/><w:r><w:t>Intro</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>"#;
    let index = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> INDEX </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>old</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    let mut doc = doc_with_body(&format!("{toc}{heading}{index}"));
    let report = doc.rebuild_generated_tables().unwrap();
    assert_eq!(report.index_entries, 1, "{report:?}");
    assert!(
        texts(&doc).contains(&"Apple, 1".to_owned()),
        "{:?}",
        texts(&doc)
    );
}

/// The main document part of a saved package.
fn main_part_xml(bytes: &[u8]) -> String {
    let package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let part = package.main_document_part().unwrap();
    String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap()
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
fn a_bookmark_around_a_table_with_a_row_control_reads_its_rows() {
    let doc = doc_with_body(
        r#"<w:bookmarkStart w:id="1" w:name="Rows"/><w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>First</w:t></w:r></w:p></w:tc></w:tr><w:sdt><w:sdtPr><w:tag w:val="rows"/></w:sdtPr><w:sdtContent><w:tr><w:tc><w:p><w:r><w:t>Repeated</w:t></w:r></w:p></w:tc></w:tr></w:sdtContent></w:sdt></w:tbl><w:bookmarkEnd w:id="1"/><w:p><w:r><w:t>After</w:t></w:r></w:p>"#,
    );
    assert_eq!(bookmark_list(&doc), pairs(&[("Rows", "First\nRepeated")]));
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
/// pages as the layout. With every copy, it walks the record copies too.
#[test]
fn a_page_number_pass_on_a_thesis_is_stable_and_follows_the_layout() {
    let copies = crate::support::page_numbers_copies();
    crate::support::run_workflow("page-numbers", &copies, |path| {
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
            // The record is three pages long.
            assert!(
                distinct.len() > 5 || path.ends_with("record.docx"),
                "{path}: {pages:?}"
            );
        }

        // The PDF on the copies the walks render.
        if !crate::support::walk_renders(path) {
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

// A bookmark start written between blocks, as Google Docs writes a heading's
// `_Toc` bookmark, belongs to the block after it: inserting before that block
// goes before the start, and moving or removing the block takes the start.

/// A paragraph, a heading led by its Google bookmark start, a paragraph:
/// body items `Before`, the start, the heading, `After`.
const LED_HEADING: &str = r#"<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:bookmarkStart w:colFirst="0" w:colLast="0" w:name="_Toc217000001" w:id="4"/><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Introduction</w:t></w:r><w:bookmarkEnd w:id="4"/></w:p><w:p><w:r><w:t>After</w:t></w:r></w:p>"#;

/// The body index of the heading of [`LED_HEADING`].
const LED_HEADING_INDEX: usize = 2;

fn heading_keeps_its_bookmark(doc: &mut Document) {
    let reopened = reopen(doc);
    for doc in [&*doc, &reopened] {
        assert_eq!(
            bookmark_list(doc),
            pairs(&[("_Toc217000001", "Introduction")]),
            "the heading's bookmark wraps the heading alone"
        );
        assert!(
            doc.bookmarks()
                .iter()
                .all(|bookmark| bookmark.issue().is_none())
        );
    }
}

fn body_story(doc: &Document) -> StoryId {
    doc.stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap()
}

/// The location of the direct body paragraph that reads `text`.
fn paragraph_location(doc: &Document, text: &str) -> ContentLocation {
    doc.story_items(&body_story(doc))
        .unwrap()
        .into_iter()
        .find(|item| {
            item.kind() == StoryItemKind::Paragraph && item.text().unwrap().as_deref() == Some(text)
        })
        .unwrap()
        .location()
        .clone()
}

/// A one-paragraph fragment that reads `text`, cut from a new document.
fn paragraph_fragment(text: &str) -> oxoffice_docx::ContentFragment {
    let mut source = doc_with(&[text]);
    let location = paragraph_location(&source, text);
    source.remove_content_at(&location).unwrap()
}

#[test]
fn a_paragraph_inserted_before_a_google_heading_goes_before_its_bookmark() {
    let mut doc = doc_with_body(LED_HEADING);
    doc.insert_paragraph(LED_HEADING_INDEX, "New");
    assert_eq!(texts(&doc), ["Before", "New", "Introduction", "After"]);
    heading_keeps_its_bookmark(&mut doc);
    let mut doc = doc_with_body(LED_HEADING);
    doc.try_insert_paragraph(LED_HEADING_INDEX, "New").unwrap();
    heading_keeps_its_bookmark(&mut doc);
    // An insertion at the start itself keeps its exact place.
    let mut doc = doc_with_body(LED_HEADING);
    doc.insert_paragraph(LED_HEADING_INDEX - 1, "New");
    heading_keeps_its_bookmark(&mut doc);
}

#[test]
fn content_inserted_at_a_google_heading_goes_before_its_bookmark() {
    let mut doc = doc_with_body(LED_HEADING);
    let heading = paragraph_location(&doc, "Introduction");
    doc.insert_content(&heading, paragraph_fragment("New"))
        .unwrap();
    assert_eq!(texts(&doc), ["Before", "New", "Introduction", "After"]);
    heading_keeps_its_bookmark(&mut doc);

    let mut doc = doc_with_body(LED_HEADING);
    let heading = paragraph_location(&doc, "Introduction");
    let result = doc
        .insert_html_fragment(&heading, "<p>New</p>", &[])
        .unwrap();
    assert_eq!(result.direct_range, 1..2, "the range reports where it went");
    assert_eq!(texts(&doc), ["Before", "New", "Introduction", "After"]);
    heading_keeps_its_bookmark(&mut doc);

    let mut doc = doc_with_body(LED_HEADING);
    let heading = paragraph_location(&doc, "Introduction");
    let before = paragraph_location(&doc, "Before");
    doc.clone_content(&before, &heading).unwrap();
    assert_eq!(texts(&doc), ["Before", "Before", "Introduction", "After"]);
    heading_keeps_its_bookmark(&mut doc);

    let mut doc = doc_with_body(LED_HEADING);
    let heading = paragraph_location(&doc, "Introduction");
    let fragment = oxoffice_docx::DocumentFragment::from_range(
        &doc_with(&["Imported"]),
        &paragraph_location(&doc_with(&["Imported"]), "Imported"),
        &ContentLocation::end(body_story(&doc_with(&["Imported"]))),
        false,
    )
    .unwrap();
    doc.import_fragment(
        &heading,
        &fragment,
        oxoffice_docx::FragmentConflictPolicy::default(),
    )
    .unwrap();
    assert_eq!(texts(&doc), ["Before", "Imported", "Introduction", "After"]);
    heading_keeps_its_bookmark(&mut doc);
}

#[test]
fn a_removed_range_takes_only_the_leading_bookmarks_that_end_inside_it() {
    // `_Toc1` ends in the heading and goes with the range; `Span` ends in
    // `Last`, after the range, and stays.
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:bookmarkStart w:id="1" w:name="_Toc1"/><w:bookmarkStart w:id="5" w:name="Span"/><w:p><w:r><w:t>Introduction</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p><w:p><w:r><w:t>After</w:t></w:r></w:p><w:p><w:r><w:t>Last</w:t></w:r><w:bookmarkEnd w:id="5"/></w:p>"#,
    );
    assert_eq!(doc.remove_content_range(3, 5).unwrap(), 3);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Before", "Last"]);
    assert_eq!(bookmark_list(&doc), pairs(&[("Span", "Last")]));
}

#[test]
fn a_range_that_starts_at_a_google_heading_takes_its_bookmark_start() {
    let doc = doc_with_body(LED_HEADING);
    let fragment = oxoffice_docx::DocumentFragment::from_range(
        &doc,
        &paragraph_location(&doc, "Introduction"),
        &paragraph_location(&doc, "After"),
        false,
    )
    .unwrap();
    let mut target = doc_with(&["Target"]);
    target
        .import_fragment(
            &ContentLocation::end(body_story(&target)),
            &fragment,
            oxoffice_docx::FragmentConflictPolicy::default(),
        )
        .unwrap();
    let target = reopen(&mut target);
    assert_eq!(texts(&target), ["Target", "Introduction"]);
    assert_eq!(
        bookmark_list(&target),
        pairs(&[("_Toc217000001", "Introduction")])
    );
}

#[test]
fn a_range_with_a_leading_bookmark_that_ends_outside_it_is_refused_on_import() {
    // `_Toc1` ends in the range and `Span` after it: the range keeps its
    // start, so its copy holds `_Toc1`'s end without its start (taking the
    // starts would leave `Span` without its end), and the import refuses an
    // incomplete bookmark rather than writing one.
    let doc = doc_with_body(
        r#"<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:bookmarkStart w:id="1" w:name="_Toc1"/><w:bookmarkStart w:id="5" w:name="Span"/><w:p><w:r><w:t>Introduction</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p><w:p><w:r><w:t>After</w:t></w:r></w:p><w:p><w:r><w:t>Last</w:t></w:r><w:bookmarkEnd w:id="5"/></w:p>"#,
    );
    let fragment = oxoffice_docx::DocumentFragment::from_range(
        &doc,
        &paragraph_location(&doc, "Introduction"),
        &paragraph_location(&doc, "After"),
        false,
    )
    .unwrap();
    let mut target = doc_with(&["Target"]);
    let error = target
        .import_fragment(
            &ContentLocation::end(body_story(&target)),
            &fragment,
            oxoffice_docx::FragmentConflictPolicy::default(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("bookmark ownership"), "{error}");
    assert_eq!(texts(&target), ["Target"]);
}
