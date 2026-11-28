//! Filling a document from data and working with templates: `{{ tag }}`
//! templates with loops and conditions, merge fields filled record by record,
//! rich merge regions, the mail-merge settings and recipient package, building
//! blocks, `.dotx` templates saved as documents, and the refusal to save a
//! VBA project under a macro-free extension.

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use oxoffice_docx::{
    BuildingBlock, BuildingBlockKind, ChartData, ChartKind, CommentDate, Document,
    DrawingHorizontalRelativeFrom, DrawingVerticalRelativeFrom, DrawingWrap,
    FragmentConflictPolicy, HdrFtrType, Length, MailMerge, MailMergeData, MailMergeDestination,
    MailMergeDocumentType, MailMergeFieldMapping, MailMergeFormatContext, MailMergeFormattedText,
    MailMergeImage, MailMergePackageDiagnosticKind, MailMergePackageOptions,
    MailMergePackageSource, MailMergeRecipient, MailMergeRecord, MailMergeSourcePolicy,
    MailMergeValue, PictureAnchor, StoryKind, TextBoxDirection, TextBoxOptions, TextCommentOptions,
    WordCreationProfile, WordPackageClass,
};
use oxoffice_docx_oxml::document::{BodyContent, CT_Body};
use oxoffice_docx_oxml::text::CT_P;
use oxoffice_opc::OpcPackage;
use serde_json::json;

use crate::support::{
    corpus_doc, corpus_root, doc_with, doc_with_body, reopen, text_box_run, texts, with_body,
};

const MAIN_DOCUMENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const MAIN_TEMPLATE_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml";

/// One flat merge record from `(field, value)` pairs.
fn record(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// A simple merge field as Word writes it, `«Name»` shown.
fn merge_field(name: &str) -> String {
    format!(
        r#"<w:fldSimple w:instr=" MERGEFIELD {name} "><w:r><w:t>«{name}»</w:t></w:r></w:fldSimple>"#
    )
}

/// A complex merge field as Word writes it, over five runs.
fn complex_merge_field(name: &str) -> String {
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD {name} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«{name}»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

/// The message of a merge that must fail; its documents have no `Debug`.
fn refusal<T>(result: oxoffice_docx::Result<T>) -> String {
    match result {
        Ok(_) => panic!("the merge was expected to fail"),
        Err(error) => error.to_string(),
    }
}

/// A rich merge text value.
fn text(value: &str) -> MailMergeValue {
    MailMergeValue::Text(value.to_owned())
}

/// The content type the saved package declares for its main part.
fn main_part_content_type(bytes: &[u8]) -> String {
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let main = package.main_document_part().expect("main document part");
    package
        .content_types
        .override_for(&main)
        .expect("main part has an override")
        .to_owned()
}

/// An AutoText building block of one paragraph.
fn block(name: &str, text: &str) -> BuildingBlock {
    let mut paragraph = CT_P::new();
    paragraph.add_run(text);
    let mut body = CT_Body::new();
    body.sect_pr = None;
    body.content
        .push(BodyContent::Paragraph(Box::new(paragraph)));
    BuildingBlock {
        name: name.to_owned(),
        kind: BuildingBlockKind::AutoText,
        category: Some("Letters".to_owned()),
        description: Some(format!("{name} text")),
        guid: None,
        gallery: Some("autoTxt".to_owned()),
        behaviors: Vec::new(),
        body,
    }
}

/// A folder of its own under the system temporary folder, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oxoffice-docx-templates-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch folder");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// Raw paragraphs holding these texts, one each.
fn raw_paragraphs(texts: &[&str]) -> String {
    texts
        .iter()
        .map(|text| format!(r#"<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#))
        .collect()
}

/// The corpus file `name` with raw XML written into its parts: each
/// `(part, after, before, xml)` puts `xml` in front of the first `before`
/// that follows `after` in that part.
fn corpus_doc_with(name: &str, insertions: &[(&str, &str, &str, String)]) -> Document {
    let bytes = fs::read(corpus_root().join("files").join(name)).expect("corpus file reads");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("corpus package opens");
    for (part, after, before, xml) in insertions {
        let mut source = String::from_utf8(package.get_part(part).expect("part").to_vec())
            .expect("part is UTF-8");
        let from = source.find(after).expect("anchor in the part");
        let at = from + source[from..].find(before).expect("end in the part");
        source.insert_str(at, xml);
        package.set_part(part, source.into_bytes());
    }
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("edited corpus file opens")
}

// ---- Scalar tags ----

#[test]
fn dotted_paths_read_nested_objects_and_scalars_render_as_text() {
    let mut doc = doc_with(&[
        "City: {{ client.address.city }}",
        "{{ count }} / {{ ratio }} / {{ paid }} / [{{ note }}]",
    ]);
    let data = json!({
        "client": { "address": { "city": "Lyon" } },
        "count": 3,
        "ratio": 2.5,
        "paid": true,
        "note": null
    });
    assert_eq!(doc.render_template(&data).unwrap(), 5);
    assert_eq!(
        texts(&reopen(&mut doc)),
        ["City: Lyon", "3 / 2.5 / true / []"]
    );
}

#[test]
fn a_tag_word_split_into_runs_renders_in_the_format_of_its_first_run() {
    // Word splits a typed tag into runs with their own rsids and marks the
    // unknown word with spelling markers.
    let mut doc = doc_with_body(
        r#"<w:p w:rsidR="00A1"><w:r w:rsidR="00A2"><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">Dear {{ na</w:t></w:r><w:proofErr w:type="spellStart"/><w:r w:rsidR="00A3"><w:t>me</w:t></w:r><w:proofErr w:type="spellEnd"/><w:r w:rsidR="00A4"><w:t xml:space="preserve"> }}, hello</w:t></w:r></w:p>"#,
    );
    assert_eq!(doc.render_template(&json!({ "name": "Ada" })).unwrap(), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Dear Ada, hello"]);
    let paragraph = doc.paragraph(0).unwrap();
    let first = paragraph.run(0).unwrap();
    assert_eq!(first.text(), "Dear Ada");
    assert!(
        first.is_bold(),
        "the value takes the bold of the tag's first run"
    );
    let rest: String = paragraph.runs().skip(1).map(|run| run.text()).collect();
    assert_eq!(rest, ", hello");
    assert!(paragraph.runs().skip(1).all(|run| !run.is_bold()));
}

#[test]
fn data_the_template_cannot_use_fails_and_changes_nothing() {
    let mut doc = doc_with(&["To {{ client.name }}"]);
    let before = doc.to_bytes().unwrap();
    for (data, message) in [
        (json!({ "client": {} }), "missing path `client.name`"),
        (json!({}), "missing path `client.name`"),
        (
            json!({ "client": { "name": { "first": "Ada" } } }),
            "does not resolve to a scalar value",
        ),
        (
            json!({ "client": { "name": ["Ada"] } }),
            "does not resolve to a scalar value",
        ),
        (json!({ "client": "Ada" }), "crosses a non-object value"),
        // A character XML cannot carry has no place in the saved file.
        (json!({ "client": { "name": "A\u{1}B" } }), "U+0001"),
    ] {
        let error = doc.render_template(&data).unwrap_err().to_string();
        assert!(error.contains(message), "{data}: {error}");
        assert_eq!(doc.to_bytes().unwrap(), before, "{data}");
    }
    assert_eq!(texts(&reopen(&mut doc)), ["To {{ client.name }}"]);
}

#[test]
fn a_malformed_tag_fails_and_changes_nothing() {
    for (source, message) in [
        ("Hello {{ name", "unclosed scalar tag"),
        ("Hello {{ }}", "empty scalar path"),
        ("Hello {{ a..b }}", "invalid dotted scalar path"),
        ("Hello name }}", "unmatched delimiter"),
    ] {
        let mut doc = doc_with(&[source]);
        let before = doc.to_bytes().unwrap();
        let error = doc
            .render_template(&json!({ "name": "Ada" }))
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{source}: {error}");
        assert_eq!(doc.to_bytes().unwrap(), before, "{source}");
    }
}

#[test]
fn tags_in_a_text_box_and_in_chart_labels_are_filled() {
    let mut doc = doc_with(&["Body"]);
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap();
    let anchor = PictureAnchor {
        horizontal_relative_from: DrawingHorizontalRelativeFrom::Column,
        horizontal_offset: Length::emu(0),
        horizontal_alignment: None,
        vertical_relative_from: DrawingVerticalRelativeFrom::Paragraph,
        vertical_offset: Length::emu(0),
        vertical_alignment: None,
        wrap: DrawingWrap::None,
        distance_top: Length::emu(0),
        distance_bottom: Length::emu(0),
        distance_left: Length::emu(0),
        distance_right: Length::emu(0),
        relative_height: 1,
        behind_text: false,
    };
    let options = TextBoxOptions {
        width: Length::inches(2.0),
        height: Length::inches(1.0),
        anchor,
        rotation_degrees: 0.0,
        text_direction: TextBoxDirection::Horizontal,
        fill_color: None,
    };
    doc.add_text_box_to_story(&body, "Boxed {{ who }}", options)
        .unwrap();
    let data = ChartData {
        categories: vec!["{{ first }}".into(), "Q2".into()],
        series: vec![("{{ region }}".into(), vec![1.0, 2.0])],
        ..ChartData::default()
    };
    doc.add_chart(
        ChartKind::Bar,
        Length::inches(4.0),
        Length::inches(3.0),
        &data,
    )
    .unwrap();

    doc.render_template(&json!({ "who": "Ada", "first": "Q1", "region": "North" }))
        .unwrap();
    let mut doc = reopen(&mut doc);
    // Word's text box and the fallback copy older readers show both change.
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(!xml.contains("{{"), "{xml}");
    assert_eq!(xml.matches(">Boxed Ada</w:t>").count(), 2, "{xml}");
    let charts = doc.charts().unwrap();
    assert_eq!(charts.len(), 1);
    assert_eq!(charts[0].1.categories, ["Q1", "Q2"]);
    assert_eq!(charts[0].1.series[0].0, "North");
    let saved = doc.to_bytes().unwrap();
    let package = OpcPackage::from_reader(Cursor::new(saved)).unwrap();
    let entries = package.zip_entries().unwrap();
    for (name, bytes) in &entries {
        if name
            .trim_start_matches('/')
            .starts_with("word/charts/chart")
        {
            let chart = String::from_utf8_lossy(bytes);
            assert!(!chart.contains("{{"), "{name}: {chart}");
        }
    }
}

#[test]
fn tags_in_block_and_inline_content_controls_are_filled() {
    let mut doc = doc_with_body(concat!(
        r#"<w:sdt><w:sdtPr><w:id w:val="11"/><w:tag w:val="block"/></w:sdtPr><w:sdtContent>"#,
        r#"<w:p><w:r><w:t>Block {{ who }}</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
        r#"<w:p><w:r><w:t xml:space="preserve">Ref </w:t></w:r><w:sdt><w:sdtPr><w:id w:val="12"/><w:tag w:val="inline"/></w:sdtPr>"#,
        r#"<w:sdtContent><w:r><w:t>{{ ref }}</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    ));
    let count = doc
        .render_template(&json!({ "who": "Ada", "ref": "R-7" }))
        .unwrap();
    assert_eq!(count, 2);
    let doc = reopen(&mut doc);
    let controls: Vec<(Option<i32>, Option<String>, String)> = doc
        .content_controls()
        .iter()
        .map(|control| {
            (
                control.id(),
                control.tag().map(str::to_owned),
                control.text(),
            )
        })
        .collect();
    assert_eq!(
        controls,
        [
            (Some(11), Some("block".to_owned()), "Block Ada".to_owned()),
            (Some(12), Some("inline".to_owned()), "R-7".to_owned()),
        ],
        "each control keeps its identity and shows its value"
    );
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(!xml.contains("{{"), "{xml}");
}

#[test]
fn a_tag_in_a_comment_is_left_as_written() {
    // The template fills the stories that print (body, headers, footers,
    // notes, text boxes); review comments are skipped, tag and all.
    let mut doc = doc_with(&["Commented {{ who }}"]);
    doc.add_comment_on_text(
        "Commented",
        "Ann",
        "Ask {{ who }}",
        TextCommentOptions {
            date: CommentDate::Undated,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(doc.render_template(&json!({ "who": "Ada" })).unwrap(), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Commented Ada"]);
    let comments: Vec<String> = doc
        .comments()
        .iter()
        .map(|comment| comment.text())
        .collect();
    assert_eq!(comments, ["Ask {{ who }}"]);
    assert_eq!(
        doc.comment_anchor_text(doc.comments()[0].id())
            .unwrap()
            .as_deref(),
        Some("Commented")
    );
}

// ---- Loops and conditions ----

#[test]
fn a_for_block_repeats_its_paragraphs_for_each_item_and_drops_its_markers() {
    let mut doc = doc_with(&[
        "Order {{ number }}",
        "{% for line in lines %}",
        "{{ line.name }} for order {{ number }}",
        "Quantity {{ line.qty }}",
        "{% endfor %}",
        "End of order",
    ]);
    let data = json!({
        "number": 42,
        "lines": [{ "name": "Pen", "qty": 2 }, { "name": "Ink", "qty": 10 }]
    });
    assert_eq!(doc.render_template(&data).unwrap(), 7);
    assert_eq!(
        texts(&reopen(&mut doc)),
        [
            "Order 42",
            "Pen for order 42",
            "Quantity 2",
            "Ink for order 42",
            "Quantity 10",
            "End of order"
        ]
    );

    // An empty list leaves neither the markers nor the repeated paragraphs.
    let mut doc = doc_with(&[
        "{% for line in lines %}",
        "{{ line }}",
        "{% endfor %}",
        "After",
    ]);
    assert_eq!(doc.render_template(&json!({ "lines": [] })).unwrap(), 0);
    assert_eq!(texts(&reopen(&mut doc)), ["After"]);
}

#[test]
fn a_for_block_over_table_rows_repeats_the_rows_between_its_markers() {
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(4, 2);
        table.cell(0, 0).unwrap().set_text("Item");
        table.cell(0, 1).unwrap().set_text("Qty");
        table
            .cell(1, 0)
            .unwrap()
            .set_text("{% for line in lines %}");
        table.cell(2, 0).unwrap().set_text("{{ line.name }}");
        table.cell(2, 1).unwrap().set_text("{{ line.qty }}");
        table.cell(3, 0).unwrap().set_text("{% endfor %}");
    }
    let data = json!({ "lines": [
        { "name": "Pen", "qty": 2 },
        { "name": "Ink", "qty": 10 },
        { "name": "Pad", "qty": 1 }
    ] });
    assert_eq!(doc.render_template(&data).unwrap(), 6);
    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    let rows: Vec<[String; 2]> = (0..table.row_count())
        .map(|row| {
            [
                table.cell(row, 0).unwrap().text(),
                table.cell(row, 1).unwrap().text(),
            ]
        })
        .collect();
    assert_eq!(
        rows,
        [["Item", "Qty"], ["Pen", "2"], ["Ink", "10"], ["Pad", "1"]]
    );
}

#[test]
fn a_control_marker_must_stand_alone_be_closed_and_loop_over_a_list() {
    let cases: [(&[&str], serde_json::Value, &str); 5] = [
        (
            &["{% for x in xs %} and text", "{{ x }}", "{% endfor %}"],
            json!({ "xs": [1] }),
            "a control marker must occupy its own paragraph",
        ),
        (
            &["{% for x in xs %}", "{{ x }}"],
            json!({ "xs": [1] }),
            "missing endfor marker",
        ),
        (
            &["{% if x %}", "{{ x }}", "{% endfor %}"],
            json!({ "x": 1 }),
            "expected endif before endfor",
        ),
        (
            &["{% for x in xs %}", "{{ x }}", "{% endfor %}"],
            json!({ "xs": "not a list" }),
            "loop path must resolve to an array",
        ),
        (
            &["{% if flag %}", "Shown", "{% endif %}"],
            json!({}),
            "missing path `flag`",
        ),
    ];
    for (paragraphs, data, message) in cases {
        let mut doc = doc_with(paragraphs);
        let before = doc.to_bytes().unwrap();
        let error = doc.render_template(&data).unwrap_err().to_string();
        assert!(error.contains(message), "{paragraphs:?}: {error}");
        assert_eq!(doc.to_bytes().unwrap(), before, "{paragraphs:?}");
    }
}

#[test]
fn a_loop_copies_its_note_and_picture_beside_header_and_footer_tags() {
    let mut doc = doc_with(&["{% for x in xs %}", "Item {{ x }}"]);
    let location = doc.paragraph_story_location(1).unwrap().unwrap();
    doc.create_footnote(&location, "Note {{ who }}").unwrap();
    doc.add_picture(PNG, "dot.png", Length::cm(1.0), Length::cm(1.0));
    doc.add_paragraph("{% endfor %}");
    doc.add_paragraph("After {{ who }}");
    doc.set_header("Header {{ who }}");
    doc.set_footer("Footer {{ who }}");

    doc.render_template(&json!({ "xs": ["a", "b", "c"], "who": "Ada" }))
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        ["Item a", "", "Item b", "", "Item c", "", "After Ada"]
    );
    assert_eq!(doc.header_text().as_deref(), Some("Header Ada"));
    assert_eq!(doc.footer_text().as_deref(), Some("Footer Ada"));
    // Each copy has its own note and its own drawing id.
    let xml = doc.part_xml("word/document.xml").unwrap();
    let ids_after = |marker: &str| {
        xml.split(marker)
            .skip(1)
            .map(|rest| rest[..rest.find('"').unwrap()].to_owned())
            .collect::<Vec<_>>()
    };
    let references = ids_after(r#"<w:footnoteReference w:id=""#);
    let drawings = ids_after(r#"<wp:docPr id=""#);
    let notes = doc.footnotes();
    assert_eq!(references.len(), 3);
    for reference in &references {
        let id: i32 = reference.parse().unwrap();
        assert!(notes.contains(&(id, " Note Ada".to_owned())), "{reference}");
    }
    assert_eq!(doc.pictures().unwrap().len(), 3);
    assert_eq!(drawings.len(), 3);
    for (index, id) in drawings.iter().enumerate() {
        assert!(!drawings[..index].contains(id), "{drawings:?}");
    }
    for (index, id) in references.iter().enumerate() {
        assert!(!references[..index].contains(id), "{references:?}");
    }
}

const PAGE_FIELD: &str = r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple>"#;
const NUMPAGES_FIELD: &str = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> NUMPAGES </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>2</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#;

/// The instructions of the fields a part holds, in order.
fn field_instructions(xml: &str) -> Vec<String> {
    let mut found = Vec::new();
    for piece in xml.split('<').skip(1) {
        if let Some(rest) = piece.strip_prefix(r#"w:fldSimple w:instr=""#) {
            found.push(rest[..rest.find('"').unwrap()].trim().to_owned());
        } else if let Some(rest) = piece.strip_prefix(r#"w:instrText xml:space="preserve">"#) {
            found.push(rest.trim().to_owned());
        }
    }
    found
}

#[test]
fn a_tag_beside_a_field_is_filled_and_the_field_kept() {
    let mut doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Delivery {{{{ delivery }}}} - page </w:t></w:r>{PAGE_FIELD}<w:r><w:t xml:space="preserve"> of </w:t></w:r>{NUMPAGES_FIELD}</w:p>"#
    ));
    doc.set_raw_footer_with_images(
        format!(
            r#"<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:r><w:t xml:space="preserve">Ref {{{{ ref }}}} - page </w:t></w:r>{PAGE_FIELD}<w:r><w:t xml:space="preserve"> of </w:t></w:r>{NUMPAGES_FIELD}</w:p></w:ftr>"#
        )
        .into_bytes(),
        &[],
        HdrFtrType::Default,
    );

    let data = json!({ "delivery": "2026-01-01", "ref": "R-7" });
    assert_eq!(doc.render_template(&data).unwrap(), 2);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Delivery 2026-01-01 - page 1 of 2"]);
    assert_eq!(doc.footer_text().as_deref(), Some("Ref R-7 - page 1 of 2"));
    let body = doc.part_xml("word/document.xml").unwrap();
    assert_eq!(field_instructions(&body), ["PAGE", "NUMPAGES"]);
    let footer = doc
        .part_names()
        .unwrap()
        .into_iter()
        .find(|name| name.contains("word/footer"))
        .unwrap();
    let footer = doc.part_xml(&footer).unwrap();
    assert_eq!(field_instructions(&footer), ["PAGE", "NUMPAGES"]);
}

#[test]
fn a_loop_repeats_a_paragraph_holding_a_field_with_its_field() {
    for (items, filled) in [
        (json!([]), 0),
        (json!(["a"]), 1),
        (json!(["a", "b", "c"]), 3),
    ] {
        let mut doc = doc_with_body(&format!(
            r#"<w:p><w:r><w:t>{{% for x in xs %}}</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">Line {{{{ x }}}} - page </w:t></w:r>{PAGE_FIELD}</w:p><w:p><w:r><w:t xml:space="preserve">Total </w:t></w:r>{NUMPAGES_FIELD}</w:p><w:p><w:r><w:t>{{% endfor %}}</w:t></w:r></w:p>"#
        ));
        assert_eq!(
            doc.render_template(&json!({ "xs": items })).unwrap(),
            filled
        );
        let doc = reopen(&mut doc);
        let expected = items
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|x| {
                [
                    format!("Line {} - page 1", x.as_str().unwrap()),
                    "Total 2".to_owned(),
                ]
            })
            .collect::<Vec<_>>();
        assert_eq!(texts(&doc), expected);
        let body = doc.part_xml("word/document.xml").unwrap();
        assert_eq!(field_instructions(&body).len(), 2 * filled);
    }
}

// ---- Grammar version 1 ----

#[test]
fn grammar_one_adds_filters_quoted_keys_indexes_and_escapes() {
    let mut doc = doc_with(&[
        r#"{{ person["full.name"]|trim|upper }} / {{ tags|join(", ") }} / {{ nickname|default("guest") }}"#,
        "{{ list[1] }} / {{ city|lower }} / {{{{ literal }}}}",
    ]);
    let data = json!({
        "person": { "full.name": "  Ada Lovelace " },
        "tags": ["a", 1, false],
        "nickname": null,
        "list": ["first", "second"],
        "city": "ÉCOLE"
    });
    assert_eq!(doc.render_template_versioned(&data, 1).unwrap(), 5);
    assert_eq!(
        texts(&reopen(&mut doc)),
        [
            "ADA LOVELACE / a, 1, false / guest",
            "second / école / {{ literal }}"
        ]
    );
}

#[test]
fn grammar_one_refuses_other_versions_unknown_filters_and_wrong_paths() {
    let mut doc = doc_with(&["{{ name }}"]);
    let before = doc.to_bytes().unwrap();
    for version in [0, 2] {
        let error = doc
            .render_template_versioned(&json!({ "name": "Ada" }), version)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!("unsupported template grammar version {version}")),
            "{error}"
        );
    }
    assert_eq!(doc.to_bytes().unwrap(), before);

    for (source, data, message) in [
        (
            "{{ name|shout }}",
            json!({ "name": "Ada" }),
            "unknown template filter",
        ),
        (
            // A default does not hide a path that crosses a number.
            r#"{{ total.net|default("n/a") }}"#,
            json!({ "total": 3 }),
            "crosses a non-object value",
        ),
        (
            "{{ list[5] }}",
            json!({ "list": [1] }),
            "missing template path",
        ),
    ] {
        let mut doc = doc_with(&[source]);
        let before = doc.to_bytes().unwrap();
        let error = doc
            .render_template_versioned(&data, 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{source}: {error}");
        assert_eq!(doc.to_bytes().unwrap(), before, "{source}");
    }
}

#[test]
fn a_formatter_rewrites_one_tag_and_gives_it_a_run_format() {
    let mut doc = doc_with(&["{{ name }} owes {{ amount }}", "Thanks {{ name }}"]);
    let mut seen = Vec::new();
    let count = doc
        .render_template_with_formatter(
            &json!({ "name": "Ada", "amount": 12 }),
            &mut |context, value| {
                seen.push((
                    context.path.to_owned(),
                    context.occurrence,
                    context.location.index_path().to_vec(),
                    value.text.clone(),
                ));
                if context.path == "amount" {
                    value.text = format!("EUR {}.00", value.text);
                    let mut properties = value.run_properties.clone().unwrap_or_default();
                    properties.bold = Some(true);
                    value.run_properties = Some(properties);
                }
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(count, 3);
    assert_eq!(
        seen,
        [
            ("name".to_owned(), 0, vec![0], "Ada".to_owned()),
            ("amount".to_owned(), 1, vec![0], "12".to_owned()),
            ("name".to_owned(), 2, vec![1], "Ada".to_owned()),
        ]
    );
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Ada owes EUR 12.00", "Thanks Ada"]);
    let runs: Vec<(String, bool)> = doc
        .paragraph(0)
        .unwrap()
        .runs()
        .map(|run| (run.text(), run.is_bold()))
        .collect();
    assert_eq!(
        runs,
        [
            ("Ada owes ".to_owned(), false),
            ("EUR 12.00".to_owned(), true)
        ]
    );
}

// ---- Templates saved by other editors ----

#[test]
fn a_libreoffice_letter_renders_without_error() {
    let mut doc = corpus_doc("libreoffice/letter.docx");
    let before = texts(&doc);
    let rendered = doc.render_template(&json!({}));
    assert!(
        matches!(rendered, Ok(0)),
        "the LibreOffice letter renders: {rendered:?}"
    );
    assert_eq!(texts(&doc), before);
}

#[test]
fn two_notes_of_one_part_each_take_their_block() {
    let before = corpus_doc("libreoffice/contract.docx").footnotes();
    let insertions = [
        (
            "/word/footnotes.xml",
            r#"<w:footnote w:id="2">"#,
            "</w:footnote>",
            raw_paragraphs(&["{% if flag %}", "Shown {{ who }}", "{% endif %}"]),
        ),
        (
            "/word/footnotes.xml",
            r#"<w:footnote w:id="3">"#,
            "</w:footnote>",
            raw_paragraphs(&["{% for x in xs %}", "Item {{ x }}", "{% endfor %}"]),
        ),
    ];
    for (data, count, added) in [
        (
            json!({ "flag": true, "who": "Ada", "xs": ["a", "b"] }),
            3,
            ["\nShown Ada", "\nItem a\nItem b"],
        ),
        (
            json!({ "flag": false, "who": "Ada", "xs": [] }),
            0,
            ["", ""],
        ),
    ] {
        let mut doc = corpus_doc_with("libreoffice/contract.docx", &insertions);
        assert_eq!(doc.render_template(&data).unwrap(), count);
        let expected = before
            .iter()
            .zip(added)
            .map(|((id, text), added)| (*id, format!("{text}{added}")))
            .collect::<Vec<_>>();
        assert_eq!(reopen(&mut doc).footnotes(), expected, "{data}");
    }
}

// ---- Merge fields ----

#[test]
fn sectioned_merges_put_each_record_in_its_own_next_page_section() {
    let template = doc_with_body(&format!(r#"<w:p>{}</w:p>"#, merge_field("Name")));
    let mut merged = template
        .mail_merge_sections(&[
            record(&[("Name", "Ada")]),
            record(&[("Name", "Bob")]),
            record(&[("Name", "Cy")]),
        ])
        .unwrap();
    let merged = reopen(&mut merged);
    assert_eq!(merged.section_count(), 3);
    // The empty paragraphs carry the section breaks.
    assert_eq!(texts(&merged), ["Ada", "", "Bob", "", "Cy"]);
    let xml = merged.part_xml("word/document.xml").unwrap();
    assert_eq!(
        xml.matches(r#"<w:type w:val="nextPage"/>"#).count(),
        2,
        "{xml}"
    );
    let error = refusal(template.mail_merge_sections(&[]));
    assert!(error.contains("at least one record"), "{error}");

    let customer = |name: &str| MailMergeRecord {
        values: BTreeMap::from([("Name".to_owned(), text(name))]),
        regions: BTreeMap::new(),
    };
    let data = MailMergeData {
        records: vec![customer("Acme"), customer("Bolt")],
        sources: BTreeMap::new(),
    };
    let mut merged = template.mail_merge_sections_rich(&data, None).unwrap();
    let merged = reopen(&mut merged);
    assert_eq!(merged.section_count(), 2);
    assert_eq!(texts(&merged), ["Acme", "", "Bolt"]);
}

#[test]
fn a_quoted_header_merge_field_name_keeps_its_spaces_when_sections_are_checked() {
    // Word quotes a field name with a space, and writes the quotes as
    // entities: `MERGEFIELD &quot;Account Id&quot;`.
    let mut template = doc_with_body(&format!(r#"<w:p>{}</w:p>"#, merge_field("Name")));
    template.set_raw_header_with_images(
        concat!(
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p>"#,
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
            r#"<w:r><w:instrText xml:space="preserve"> MERGEFIELD &quot;Account Id&quot; </w:instrText></w:r>"#,
            r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«Account Id»</w:t></w:r>"#,
            r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:hdr>"#,
        )
        .as_bytes()
        .to_vec(),
        &[],
        HdrFtrType::Default,
    );
    let records = [
        record(&[("Name", "Ada"), ("Account Id", "A-1")]),
        record(&[("Name", "Bob"), ("Account Id", "B-2")]),
    ];
    let error = refusal(template.mail_merge_sections(&records));
    assert!(error.contains("cannot vary fields in headers"), "{error}");
}

#[test]
fn a_header_merge_field_is_filled_per_record_but_cannot_vary_between_sections() {
    let mut template = doc_with_body(&format!(r#"<w:p>{}</w:p>"#, merge_field("Name")));
    // A complex field: the header text reader leaves out simple field
    // results (issue #60).
    template.set_raw_header_with_images(
        format!(
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:r><w:t xml:space="preserve">Account </w:t></w:r>{}</w:p></w:hdr>"#,
            complex_merge_field("Account")
        )
        .into_bytes(),
        &[],
        HdrFtrType::Default,
    );
    let records = [
        record(&[("Name", "Ada"), ("Account", "A-1")]),
        record(&[("Name", "Bob"), ("Account", "B-2")]),
    ];
    let mut outputs = template.mail_merge(&records).unwrap();
    let headers: Vec<Option<String>> = outputs
        .iter_mut()
        .map(|output| reopen(output).header_text())
        .collect();
    assert_eq!(
        headers,
        [
            Some("Account A-1".to_owned()),
            Some("Account B-2".to_owned())
        ]
    );

    // One document shares its header between sections, so the header field
    // must be the same in every record.
    let error = refusal(template.mail_merge_sections(&records));
    assert!(error.contains("cannot vary fields in headers"), "{error}");
    let same = [
        record(&[("Name", "Ada"), ("Account", "Shared")]),
        record(&[("Name", "Bob"), ("Account", "Shared")]),
    ];
    let mut merged = template.mail_merge_sections(&same).unwrap();
    let merged = reopen(&mut merged);
    assert_eq!(merged.header_text().as_deref(), Some("Account Shared"));
}

// ---- Rich merge ----

#[test]
fn a_rich_merge_repeats_a_region_for_each_child_record() {
    let template = doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Invoice for </w:t></w:r>{client}</w:p><w:p>{start}</w:p><w:p>{item}<w:r><w:t xml:space="preserve"> for </w:t></w:r>{client}</w:p><w:p>{end}</w:p><w:p><w:r><w:t>Thanks</w:t></w:r></w:p>"#,
        client = merge_field("Client"),
        start = merge_field("TableStart:Lines"),
        item = merge_field("Item"),
        end = merge_field("TableEnd:Lines"),
    ));
    let line = |item: &str| MailMergeRecord {
        values: BTreeMap::from([("Item".to_owned(), text(item))]),
        regions: BTreeMap::new(),
    };
    let customer = |client: &str, lines: Vec<MailMergeRecord>| MailMergeRecord {
        values: BTreeMap::from([("Client".to_owned(), text(client))]),
        regions: BTreeMap::from([("Lines".to_owned(), lines)]),
    };
    let data = MailMergeData {
        records: vec![
            customer("Acme", vec![line("Pen"), line("Ink")]),
            customer("Bolt", Vec::new()),
        ],
        sources: BTreeMap::new(),
    };
    let mut seen = Vec::new();
    let mut formatter = |context: &MailMergeFormatContext<'_>,
                         value: &mut MailMergeFormattedText| {
        seen.push((
            context.source_name.to_owned(),
            context.field_name.to_owned(),
            context.record_number,
        ));
        if context.field_name == "Item" {
            value.text = value.text.to_uppercase();
        }
        Ok(())
    };
    let mut outputs = template
        .mail_merge_rich(&data, Some(&mut formatter))
        .unwrap();
    assert_eq!(outputs.len(), 2);
    assert_eq!(
        texts(&reopen(&mut outputs[0])),
        ["Invoice for Acme", "PEN for Acme", "INK for Acme", "Thanks"]
    );
    // An empty region leaves neither its markers nor its paragraphs.
    assert_eq!(
        texts(&reopen(&mut outputs[1])),
        ["Invoice for Bolt", "Thanks"]
    );
    let first_record: Vec<(&str, &str, u32)> = seen[..5]
        .iter()
        .map(|(source, field, number)| (source.as_str(), field.as_str(), *number))
        .collect();
    assert_eq!(
        first_record,
        [
            ("records", "Client", 1),
            ("Lines", "Item", 1),
            ("Lines", "Client", 1),
            ("Lines", "Item", 2),
            ("Lines", "Client", 2),
        ]
    );

    let missing = MailMergeData {
        records: vec![MailMergeRecord::default()],
        sources: BTreeMap::new(),
    };
    let error = refusal(template.mail_merge_rich(&missing, None));
    assert!(error.contains("source Lines is missing"), "{error}");
    let error = refusal(template.mail_merge_rich(&MailMergeData::default(), None));
    assert!(error.contains("at least one record"), "{error}");
}

#[test]
fn a_rich_merge_puts_a_whole_document_in_place_of_a_field() {
    let template = doc_with_body(&format!(
        r#"<w:p><w:r><w:t>Terms:</w:t></w:r></w:p><w:p>{}</w:p><w:p><w:r><w:t>Signed</w:t></w:r></w:p>"#,
        merge_field("Terms")
    ));
    let terms = doc_with(&["Payment within 30 days.", "Prices in euros."])
        .to_bytes()
        .unwrap();
    let data = MailMergeData {
        records: vec![MailMergeRecord {
            values: BTreeMap::from([("Terms".to_owned(), MailMergeValue::Fragment(terms))]),
            regions: BTreeMap::new(),
        }],
        sources: BTreeMap::new(),
    };
    let mut outputs = template.mail_merge_rich(&data, None).unwrap();
    assert_eq!(
        texts(&reopen(&mut outputs[0])),
        [
            "Terms:",
            "Payment within 30 days.",
            "Prices in euros.",
            "Signed"
        ]
    );
}

/// One table row of these cells, each a paragraph of raw runs.
fn row(cells: &[String]) -> String {
    let cells: String = cells
        .iter()
        .map(|cell| format!("<w:tc><w:p>{cell}</w:p></w:tc>"))
        .collect();
    format!("<w:tr>{cells}</w:tr>")
}

/// A run of plain text.
fn run(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

/// A one-cell row spanning the two columns, holding only this merge field.
fn marker_row(name: &str) -> String {
    format!(
        r#"<w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p>{}</w:p></w:tc></w:tr>"#,
        merge_field(name)
    )
}

/// A two-column table of these rows, then an empty paragraph.
fn table(rows: &[String]) -> String {
    format!(
        r#"<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid><w:gridCol w:w="4000"/><w:gridCol w:w="2000"/></w:tblGrid>{}</w:tbl><w:p/>"#,
        rows.concat()
    )
}

/// The text of every cell of body table 0, row by row.
fn table_texts(doc: &Document) -> Vec<Vec<String>> {
    let table = doc.table(0).expect("the table");
    (0..table.row_count())
        .map(|index| {
            let row = table.row(index).unwrap();
            (0..row.cell_count())
                .map(|cell| row.cell(cell).unwrap().text())
                .collect()
        })
        .collect()
}

fn cells(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|text| (*text).to_owned()).collect()
}

#[test]
fn a_rich_merge_repeats_the_table_rows_between_its_marker_rows() {
    // The invoice layout: a heading row, one row per line, a total row.
    let template = doc_with_body(&table(&[
        row(&[run("Item"), run("Qty")]),
        marker_row("TableStart:Lines"),
        row(&[merge_field("Item"), merge_field("Qty")]),
        marker_row("TableEnd:Lines"),
        row(&[run("Total"), merge_field("Total")]),
    ]));
    let line = |item: &str, qty: &str| MailMergeRecord {
        values: BTreeMap::from([
            ("Item".to_owned(), text(item)),
            ("Qty".to_owned(), text(qty)),
        ]),
        regions: BTreeMap::new(),
    };
    let invoice = |total: &str, lines: Vec<MailMergeRecord>| MailMergeRecord {
        values: BTreeMap::from([("Total".to_owned(), text(total))]),
        regions: BTreeMap::from([("Lines".to_owned(), lines)]),
    };
    let data = MailMergeData {
        records: vec![
            invoice("7", vec![line("Pen", "2"), line("Ink", "5")]),
            invoice("0", Vec::new()),
        ],
        sources: BTreeMap::new(),
    };
    let mut outputs = template.mail_merge_rich(&data, None).unwrap();
    assert_eq!(
        table_texts(&reopen(&mut outputs[0])),
        [
            cells(&["Item", "Qty"]),
            cells(&["Pen", "2"]),
            cells(&["Ink", "5"]),
            cells(&["Total", "7"]),
        ]
    );
    // An empty region leaves neither its marker rows nor its rows.
    assert_eq!(
        table_texts(&reopen(&mut outputs[1])),
        [cells(&["Item", "Qty"]), cells(&["Total", "0"])]
    );
}

#[test]
fn region_markers_sharing_a_row_with_other_cells_are_refused() {
    // The markers must own their whole row: in the first and last cells of
    // the repeated row they are refused, and the template stays as it was.
    let mut template = doc_with_body(&table(&[row(&[
        format!("{}{}", merge_field("TableStart:Lines"), merge_field("Item")),
        format!("{}{}", merge_field("Qty"), merge_field("TableEnd:Lines")),
    ])]));
    let before = template.to_bytes().unwrap();
    let data = MailMergeData {
        records: vec![MailMergeRecord {
            values: BTreeMap::new(),
            regions: BTreeMap::from([("Lines".to_owned(), Vec::new())]),
        }],
        sources: BTreeMap::new(),
    };
    let error = refusal(template.mail_merge_rich(&data, None));
    assert!(
        error.contains("region field TableStart:Lines must own its whole block"),
        "{error}"
    );
    assert_eq!(template.to_bytes().unwrap(), before);
}

#[test]
fn an_image_value_puts_its_picture_in_place_of_the_field() {
    let template = doc_with_body(&format!(
        r#"<w:p>{}{}</w:p><w:p>{}</w:p>"#,
        run("Logo: "),
        merge_field("Logo"),
        merge_field("Name")
    ));
    let logo = |width: Length| MailMergeImage {
        data: PNG.to_vec(),
        filename: "logo.png".to_owned(),
        width,
        height: Length::inches(0.5),
    };
    let data = |width: Length| MailMergeData {
        records: vec![MailMergeRecord {
            values: BTreeMap::from([
                ("Logo".to_owned(), MailMergeValue::Image(logo(width))),
                ("Name".to_owned(), text("Acme")),
            ]),
            regions: BTreeMap::new(),
        }],
        sources: BTreeMap::new(),
    };
    let mut outputs = template
        .mail_merge_rich(&data(Length::inches(1.0)), None)
        .unwrap();
    let merged = reopen(&mut outputs[0]);
    assert_eq!(texts(&merged), ["Logo: ", "Acme"]);
    let images: Vec<_> = merged
        .images()
        .into_iter()
        .map(|image| {
            (
                merged.image_data(&image.embed_id),
                image.width_emu,
                image.height_emu,
            )
        })
        .collect();
    assert_eq!(images, [(Some(PNG.to_vec()), 914_400, 457_200)]);
    let xml = merged.part_xml("word/document.xml").unwrap();
    assert!(!xml.contains("MERGEFIELD Logo"), "{xml}");

    let error = refusal(template.mail_merge_rich(&data(Length::emu(0)), None));
    assert!(
        error.contains("image Logo has non-positive dimensions"),
        "{error}"
    );
}

// ---- Mail-merge settings and recipient data ----

#[test]
fn mail_merge_settings_are_written_in_word_order_read_back_and_removed() {
    let mut doc = Document::new();
    assert_eq!(doc.mail_merge_settings(), None);
    assert_eq!(doc.remove_mail_merge_settings().unwrap(), None);
    let settings = MailMerge {
        main_document_type: Some(MailMergeDocumentType::FormLetters),
        data_type: Some("textFile".to_owned()),
        connect_string: Some("people.csv".to_owned()),
        query: Some("SELECT * FROM people.csv".to_owned()),
        destination: Some(MailMergeDestination::NewDocument),
        view_merged_data: Some(true),
        active_record: Some(2),
        ..MailMerge::default()
    };
    doc.set_mail_merge_settings(settings.clone()).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.mail_merge_settings(), Some(&settings));
    let xml = doc.part_xml("word/settings.xml").unwrap();
    assert!(
        xml.contains(
            r#"<w:mailMerge><w:mainDocumentType w:val="formLetters"/><w:dataType w:val="textFile"/><w:connectString w:val="people.csv"/><w:query w:val="SELECT * FROM people.csv"/><w:destination w:val="newDocument"/><w:viewMergedData/><w:activeRecord w:val="2"/></w:mailMerge>"#
        ),
        "{xml}"
    );

    assert_eq!(doc.remove_mail_merge_settings().unwrap(), Some(settings));
    let doc = reopen(&mut doc);
    assert_eq!(doc.mail_merge_settings(), None);
    assert!(
        !doc.part_xml("word/settings.xml")
            .unwrap()
            .contains("mailMerge")
    );
}

fn package_settings() -> MailMerge {
    MailMerge {
        main_document_type: Some(MailMergeDocumentType::FormLetters),
        data_type: Some("textFile".to_owned()),
        ..MailMerge::default()
    }
}

#[test]
fn an_embedded_recipient_table_survives_a_save_and_feeds_a_merge() {
    let mut doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Dear </w:t></w:r>{}</w:p>"#,
        merge_field("Name")
    ));
    let columns = vec!["Name".to_owned(), "City".to_owned()];
    let rows = vec![
        vec!["Zoë".to_owned(), "Lyon".to_owned()],
        vec!["<Bob & Co>".to_owned(), String::new()],
    ];
    doc.set_mail_merge_package(MailMergePackageOptions {
        settings: package_settings(),
        source: MailMergePackageSource::Embedded {
            columns: columns.clone(),
            records: rows.clone(),
        },
        mappings: Vec::new(),
        recipients: Vec::new(),
    })
    .unwrap();
    let mut doc = reopen(&mut doc);

    let package = doc.mail_merge_package().unwrap().expect("the package");
    assert_eq!(package.settings, package_settings());
    assert_eq!(
        package.source,
        MailMergePackageSource::Embedded {
            columns,
            records: rows
        }
    );
    let prepared = doc
        .prepare_mail_merge_data(MailMergeSourcePolicy::Offline, None)
        .unwrap();
    assert!(
        prepared.diagnostics.is_empty(),
        "{:?}",
        prepared.diagnostics
    );
    assert_eq!(
        prepared.records,
        [
            record(&[("Name", "Zoë"), ("City", "Lyon")]),
            record(&[("Name", "<Bob & Co>"), ("City", "")])
        ]
    );
    let mut letters = doc.mail_merge(&prepared.records).unwrap();
    let letters: Vec<Vec<String>> = letters
        .iter_mut()
        .map(|letter| texts(&reopen(letter)))
        .collect();
    assert_eq!(letters, [["Dear Zoë"], ["Dear <Bob & Co>"]]);

    // Removing the package removes its settings too, once.
    assert!(doc.remove_mail_merge_package().unwrap());
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.mail_merge_package().unwrap(), None);
    assert_eq!(doc.mail_merge_settings(), None);
    assert!(!doc.remove_mail_merge_package().unwrap());
    let error = doc
        .prepare_mail_merge_data(MailMergeSourcePolicy::Offline, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("package is absent"), "{error}");
}

#[test]
fn an_external_recipient_source_is_blocked_offline_and_takes_caller_rows() {
    let uri = "https://data.example/people.csv";
    let mut doc = Document::new();
    doc.set_mail_merge_package(MailMergePackageOptions {
        settings: package_settings(),
        source: MailMergePackageSource::External {
            uri: uri.to_owned(),
        },
        mappings: Vec::new(),
        recipients: Vec::new(),
    })
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.mail_merge_package()
            .unwrap()
            .expect("the package")
            .source,
        MailMergePackageSource::External {
            uri: uri.to_owned()
        }
    );

    let blocked = doc
        .prepare_mail_merge_data(MailMergeSourcePolicy::Offline, None)
        .unwrap();
    assert!(blocked.records.is_empty());
    assert_eq!(blocked.diagnostics.len(), 1);
    assert_eq!(
        blocked.diagnostics[0].kind,
        MailMergePackageDiagnosticKind::ExternalSourceBlocked
    );
    // The diagnostic does not leak the source address.
    assert!(!blocked.diagnostics[0].message.contains("data.example"));

    let unavailable = doc
        .prepare_mail_merge_data(MailMergeSourcePolicy::CallerSupplied, None)
        .unwrap();
    assert_eq!(
        unavailable.diagnostics[0].kind,
        MailMergePackageDiagnosticKind::ExternalSourceUnavailable
    );
    let rows = [record(&[("Name", "Ada")])];
    let supplied = doc
        .prepare_mail_merge_data(MailMergeSourcePolicy::CallerSupplied, Some(&rows))
        .unwrap();
    assert_eq!(supplied.records, rows);
    assert!(supplied.diagnostics.is_empty());
    let error = doc
        .prepare_mail_merge_data(MailMergeSourcePolicy::Offline, Some(&rows))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("offline policy rejects caller rows"),
        "{error}"
    );

    let mut other = Document::new();
    let before = other.to_bytes().unwrap();
    let error = other
        .set_mail_merge_package(MailMergePackageOptions {
            settings: package_settings(),
            source: MailMergePackageSource::External {
                uri: "ftp://data.example/people.csv".to_owned(),
            },
            mappings: Vec::new(),
            recipients: Vec::new(),
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("not a file or https"), "{error}");
    assert_eq!(other.to_bytes().unwrap(), before);
}

#[test]
fn a_package_with_field_mappings_and_recipients_survives_a_save() {
    let mapping = |column: u32, name: &str| MailMergeFieldMapping {
        column,
        source_name: name.to_owned(),
        mapped_name: name.to_owned(),
        language: None,
        dynamic_address: None,
    };
    let recipient = |tag: &[u8], active: bool| MailMergeRecipient {
        column: 0,
        unique_tag: tag.to_vec(),
        active,
    };
    let options = |recipients: Vec<MailMergeRecipient>| MailMergePackageOptions {
        settings: package_settings(),
        source: MailMergePackageSource::Embedded {
            columns: vec!["FirstName".to_owned(), "City".to_owned()],
            records: vec![
                vec!["Zoë".to_owned(), "Lyon".to_owned()],
                vec!["Bob".to_owned(), "Nice".to_owned()],
            ],
        },
        mappings: vec![mapping(0, "FirstName"), mapping(1, "City")],
        recipients,
    };

    let mut doc = doc_with_body(&format!("<w:p>{}</w:p>", merge_field("FirstName")));
    let with_recipients = options(vec![
        recipient(b"Zo\xc3\xab", true),
        recipient(b"Bob", false),
    ]);
    doc.set_mail_merge_package(with_recipients.clone()).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.mail_merge_package().unwrap(), Some(with_recipients));
    let recipients = doc.part_xml("word/recipientData.xml").unwrap();
    assert_eq!(
        recipients.matches("<w:recipientData>").count(),
        2,
        "{recipients}"
    );
    // Recipient rows can leave a recipient out, which the merge does not
    // evaluate: such a package is not applied.
    let prepared = doc
        .prepare_mail_merge_data(MailMergeSourcePolicy::Offline, None)
        .unwrap();
    assert!(prepared.records.is_empty());
    assert_eq!(
        prepared
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.kind)
            .collect::<Vec<_>>(),
        [MailMergePackageDiagnosticKind::UnsupportedSource]
    );

    // Without recipients, the faithful mappings feed the merge.
    let without = options(Vec::new());
    doc.set_mail_merge_package(without.clone()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.mail_merge_package().unwrap(), Some(without));
    let prepared = doc
        .prepare_mail_merge_data(MailMergeSourcePolicy::Offline, None)
        .unwrap();
    assert!(
        prepared.diagnostics.is_empty(),
        "{:?}",
        prepared.diagnostics
    );
    assert_eq!(
        prepared.records,
        [
            record(&[("FirstName", "Zoë"), ("City", "Lyon")]),
            record(&[("FirstName", "Bob"), ("City", "Nice")])
        ]
    );
}

/// The custom XML part that holds the merge store, and its bytes.
fn merge_store(doc: &Document) -> (String, String) {
    doc.part_names()
        .unwrap()
        .into_iter()
        .filter(|name| name.contains("customXml/item") && !name.contains("Props"))
        .map(|name| {
            let xml = doc.part_xml(&name).unwrap();
            (name, xml)
        })
        .find(|(_, xml)| xml.contains("first"))
        .expect("the store of the first package")
}

#[test]
fn a_merge_store_bound_from_a_text_box_is_neither_overwritten_nor_removed() {
    let one = |value: &str| MailMergePackageOptions {
        settings: package_settings(),
        source: MailMergePackageSource::Embedded {
            columns: vec!["name".to_owned()],
            records: vec![vec![value.to_owned()]],
        },
        mappings: Vec::new(),
        recipients: Vec::new(),
    };
    let mut doc = Document::new();
    doc.set_mail_merge_package(one("first")).unwrap();
    let store = doc
        .document_variable("rdocx.mailMerge.storeItemId")
        .expect("the store id")
        .to_owned();
    // A control in a text box binds the store: the text box is kept as raw
    // XML in its run, out of the typed controls' reach.
    let control = format!(
        r#"<w:sdt><w:sdtPr><w:tag w:val="bound"/><w:dataBinding w:xpath="/*[1]" w:storeItemID="{store}"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>bound</w:t></w:r></w:p></w:sdtContent></w:sdt>"#
    );
    let bytes = with_body(
        doc.to_bytes().unwrap(),
        &format!("<w:p>{}</w:p>", text_box_run(1, &control)),
    );
    let mut doc = Document::from_bytes(&bytes).unwrap();
    let (part, first) = merge_store(&doc);

    // The second package takes a store of its own.
    doc.set_mail_merge_package(one("second")).unwrap();
    assert_eq!(doc.part_xml(&part).unwrap(), first);
    let package = doc.mail_merge_package().unwrap().expect("the package");
    assert_eq!(
        package.source,
        MailMergePackageSource::Embedded {
            columns: vec!["name".to_owned()],
            records: vec![vec!["second".to_owned()]],
        }
    );
    // Removing the package keeps the store the text box binds.
    assert!(doc.remove_mail_merge_package().unwrap());
    let doc = reopen(&mut doc);
    assert_eq!(doc.part_xml(&part).unwrap(), first);
    assert_eq!(doc.mail_merge_package().unwrap(), None);
}

// ---- Building blocks ----

#[test]
fn inserting_a_building_block_copies_its_text_into_the_body() {
    let mut doc = doc_with(&["Dear client,", "Yours sincerely,"]);
    doc.create_building_block(block("Closing", "Kind regards"))
        .unwrap();
    let mut doc = reopen(&mut doc);
    let entry = doc.building_blocks().unwrap().remove(0);
    let destination = doc.paragraph_story_location(1).unwrap().unwrap();
    doc.insert_building_block(&destination, &entry, FragmentConflictPolicy::rename_all())
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        ["Dear client,", "Kind regards", "Yours sincerely,"]
    );
    // The entry stays available for the next insertion.
    assert_eq!(doc.building_blocks().unwrap().len(), 1);
}

#[test]
fn a_building_block_is_updated_and_removed_only_through_a_current_entry() {
    let mut doc = Document::new();
    let closing = doc
        .create_building_block(block("Closing", "Kind regards"))
        .unwrap();
    doc.create_building_block(block("Signature", "Zoé Dupont"))
        .unwrap();

    let mut renamed = closing.block.clone();
    renamed.name = "Formal closing".to_owned();
    renamed.description = Some("For customers".to_owned());
    let updated = doc.update_building_block(&closing, renamed).unwrap();

    // The entry read before the update no longer matches the document:
    // neither a removal nor an update goes through it.
    let before = doc.to_bytes().unwrap();
    let error = doc.remove_building_block(&closing).unwrap_err().to_string();
    assert!(error.contains("stale building block snapshot"), "{error}");
    let mut overwrite = closing.block.clone();
    overwrite.name = "Overwritten".to_owned();
    let error = doc
        .update_building_block(&closing, overwrite)
        .unwrap_err()
        .to_string();
    assert!(error.contains("stale building block snapshot"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
    // A second entry under the same name is refused.
    let mut duplicate = updated.block.clone();
    duplicate.name = "Signature".to_owned();
    let error = doc
        .update_building_block(&updated, duplicate)
        .unwrap_err()
        .to_string();
    assert!(error.contains("duplicate building block name"), "{error}");

    let mut doc = reopen(&mut doc);
    let entries = doc.building_blocks().unwrap();
    let names: Vec<(&str, Option<&str>)> = entries
        .iter()
        .map(|entry| {
            (
                entry.block.name.as_str(),
                entry.block.description.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            ("Formal closing", Some("For customers")),
            ("Signature", Some("Signature text"))
        ]
    );
    let removed = doc.remove_building_block(&entries[0]).unwrap();
    assert_eq!(removed.name, "Formal closing");
    let doc = reopen(&mut doc);
    let names: Vec<String> = doc
        .building_blocks()
        .unwrap()
        .into_iter()
        .map(|entry| entry.block.name)
        .collect();
    assert_eq!(names, ["Signature"]);
}

// ---- .dotx templates ----

#[test]
fn a_dotx_template_filled_and_saved_as_docx_declares_a_document() {
    let scratch = Scratch::new("dotx");
    let mut template = Document::new_with_profile(WordCreationProfile::WordCompatible(
        WordPackageClass::Template,
    ));
    template.add_paragraph("Dear {{ name }},");
    assert_eq!(
        template.package_class().unwrap(),
        WordPackageClass::Template
    );
    let dotx = scratch.0.join("letter.dotx");
    template.save(&dotx).unwrap();
    assert_eq!(
        main_part_content_type(&fs::read(&dotx).unwrap()),
        MAIN_TEMPLATE_TYPE
    );

    // The extension alone makes a new document a template.
    let mut plain = Document::new();
    plain.add_paragraph("Plain");
    let plain_dotx = scratch.0.join("plain.dotx");
    plain.save(&plain_dotx).unwrap();
    assert_eq!(
        main_part_content_type(&fs::read(&plain_dotx).unwrap()),
        MAIN_TEMPLATE_TYPE
    );

    let mut filled = Document::open(&dotx).unwrap();
    assert_eq!(filled.package_class().unwrap(), WordPackageClass::Template);
    filled.render_template(&json!({ "name": "Ada" })).unwrap();
    let docx = scratch.0.join("letter.docx");
    filled.save(&docx).unwrap();
    assert_eq!(
        main_part_content_type(&fs::read(&docx).unwrap()),
        MAIN_DOCUMENT_TYPE
    );
    let reopened = Document::open(&docx).unwrap();
    assert_eq!(
        reopened.package_class().unwrap(),
        WordPackageClass::Document
    );
    assert_eq!(texts(&reopened), ["Dear Ada,"]);
    // The template file stays a template with its tag.
    let source = Document::open(&dotx).unwrap();
    assert_eq!(texts(&source), ["Dear {{ name }},"]);
    assert_eq!(source.package_class().unwrap(), WordPackageClass::Template);

    // Another extension keeps the class the document was opened with.
    let other = scratch.0.join("letter.bin");
    Document::open(&dotx).unwrap().save(&other).unwrap();
    assert_eq!(
        main_part_content_type(&fs::read(&other).unwrap()),
        MAIN_TEMPLATE_TYPE
    );
}

// ---- Macro-enabled documents ----

const MACRO_DOCUMENT_TYPE: &str = "application/vnd.ms-word.document.macroEnabled.main+xml";
const VBA_PROJECT_RELATIONSHIP: &str =
    "http://schemas.microsoft.com/office/2006/relationships/vbaProject";
const VBA_PART: &str = "/word/vbaProject.bin";

/// A macro-enabled document whose main part carries a VBA project, as
/// Word saves a `.docm`.
fn macro_enabled_document() -> Document {
    let bytes = doc_with(&["Macro text"]).to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let main = package.main_document_part().unwrap();
    package.set_part(VBA_PART, b"opaque VBA project".to_vec());
    package
        .content_types
        .add_override(VBA_PART, "application/vnd.ms-office.vbaProject");
    package
        .content_types
        .add_override(&main, MACRO_DOCUMENT_TYPE);
    package
        .get_or_create_part_rels(&main)
        .add(VBA_PROJECT_RELATIONSHIP, "vbaProject.bin");
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).expect("the macro-enabled document opens")
}

fn carries_vba(bytes: &[u8]) -> bool {
    OpcPackage::from_reader(Cursor::new(bytes))
        .unwrap()
        .get_part(VBA_PART)
        .is_some()
}

#[test]
fn a_document_with_a_vba_project_is_refused_under_a_macro_free_extension() {
    let scratch = Scratch::new("vba");
    let mut doc = macro_enabled_document();
    assert_eq!(
        doc.package_class().unwrap(),
        WordPackageClass::MacroEnabledDocument
    );
    for extension in ["docx", "dotx"] {
        let path = scratch.0.join(format!("macro.{extension}"));
        // A file already there stays as it was.
        fs::write(&path, b"previous").unwrap();
        let error = doc.save(&path).unwrap_err().to_string();
        assert!(
            error.contains("carries a VBA project"),
            "{extension}: {error}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"previous", "{extension}");
        let error = doc.to_bytes_for_path(&path).unwrap_err().to_string();
        assert!(
            error.contains("save it as .docm or .dotm"),
            "{extension}: {error}"
        );
    }

    for (extension, class) in [
        ("docm", WordPackageClass::MacroEnabledDocument),
        ("dotm", WordPackageClass::MacroEnabledTemplate),
    ] {
        let path = scratch.0.join(format!("macro.{extension}"));
        doc.save(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(carries_vba(&bytes), "{extension}: the project stays");
        let saved = Document::from_bytes(&bytes).unwrap();
        assert_eq!(saved.package_class().unwrap(), class, "{extension}");
        assert_eq!(texts(&saved), ["Macro text"], "{extension}");
    }

    // The explicit conversion is the documented way through, VBA kept.
    let converted = doc.to_bytes_as(WordPackageClass::Document).unwrap();
    assert_eq!(main_part_content_type(&converted), MAIN_DOCUMENT_TYPE);
    assert!(carries_vba(&converted));
}

#[test]
fn a_loop_over_a_paragraph_with_an_endnote_leaves_only_the_endnotes_of_its_copies() {
    let mut doc = doc_with(&["{% for x in xs %}", "Item {{ x }}"]);
    let at = doc.paragraph_story_location(1).unwrap().unwrap();
    let original = doc.create_endnote(&at, "End {{ who }}").unwrap();
    doc.add_paragraph("{% endfor %}");
    let mut doc = reopen(&mut doc);
    doc.render_template(&json!({ "xs": ["a", "b"], "who": "Ada" }))
        .unwrap();
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/endnotes.xml").unwrap();
    assert!(!xml.contains(&format!(r#"w:id="{original}""#)), "{xml}");
    assert_eq!(
        xml.matches("<w:endnote ").count() - 2,
        2,
        "two endnotes and the separators"
    );
}

#[test]
fn a_raw_carriage_return_in_an_embedded_recipient_cell_reads_as_a_line_feed() {
    // A custom XML part written with Windows line ends holds a raw CR LF in
    // a cell, which every XML reader reads as one line feed; a CR written as
    // a reference stays one.
    let mut doc = doc_with_body(&format!("<w:p>{}</w:p>", merge_field("Name")));
    doc.set_mail_merge_package(MailMergePackageOptions {
        settings: package_settings(),
        source: MailMergePackageSource::Embedded {
            columns: vec!["Name".to_owned()],
            records: vec![vec!["CELLMARK".to_owned()]],
        },
        mappings: Vec::new(),
        recipients: Vec::new(),
    })
    .unwrap();
    let bytes = doc.to_bytes().unwrap();
    let store = crate::support::package_parts(&bytes)
        .into_iter()
        .find(|(_, data)| data.windows(8).any(|window| window == b"CELLMARK"))
        .map(|(name, _)| format!("/{name}"))
        .expect("the part holding the recipient table");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let xml = String::from_utf8(package.get_part(&store).unwrap().to_vec()).unwrap();
    package.set_part(
        &store,
        xml.replace("CELLMARK", "one\r\ntwo\rthree&#13;four")
            .into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let doc = Document::from_bytes(&out.into_inner()).unwrap();
    let package = doc.mail_merge_package().unwrap().expect("the package");
    assert_eq!(
        package.source,
        MailMergePackageSource::Embedded {
            columns: vec!["Name".to_owned()],
            records: vec![vec!["one\ntwo\nthree\rfour".to_owned()]],
        }
    );
}

#[test]
fn a_building_block_name_category_and_description_with_tabs_and_line_ends_read_back_as_set() {
    let created = "a\tb\nc\rd";
    let mut new = block(created, "Kind regards");
    new.category = Some(created.to_owned());
    new.description = Some(created.to_owned());
    let mut doc = doc_with(&["Body"]);
    doc.create_building_block(new).unwrap();
    let mut doc = reopen(&mut doc);
    let entry = doc.building_blocks().unwrap().remove(0);
    let read = &entry.block;
    assert_eq!(
        (
            &*read.name,
            read.category.as_deref(),
            read.description.as_deref()
        ),
        (created, Some(created), Some(created))
    );

    // An update rewrites the values in the existing entry.
    let updated = "e\r\nf\tg";
    let mut changed = entry.block.clone();
    changed.name = updated.to_owned();
    changed.category = Some(updated.to_owned());
    changed.description = Some(updated.to_owned());
    doc.update_building_block(&entry, changed).unwrap();
    let doc = reopen(&mut doc);
    let read = &doc.building_blocks().unwrap()[0].block;
    assert_eq!(
        (
            &*read.name,
            read.category.as_deref(),
            read.description.as_deref()
        ),
        (updated, Some(updated), Some(updated))
    );
}
