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
    FragmentConflictPolicy, HdrFtrType, HeaderFooterKind, Length, MailMerge, MailMergeData,
    MailMergeDestination, MailMergeDocumentType, MailMergeFieldMapping, MailMergeFormatContext,
    MailMergeFormattedText, MailMergeImage, MailMergePackageDiagnosticKind,
    MailMergePackageOptions, MailMergePackageSource, MailMergeRecipient, MailMergeRecord,
    MailMergeSourcePolicy, MailMergeValue, PictureAnchor, StoryKind, TextBoxDirection,
    TextBoxOptions, TextCommentOptions, WordCreationProfile, WordPackageClass,
};
use oxoffice_docx_oxml::document::{BodyContent, CT_Body};
use oxoffice_docx_oxml::text::CT_P;
use oxoffice_opc::OpcPackage;
use serde_json::json;

use crate::support::{corpus_doc, corpus_root, doc_with, doc_with_body, reopen, texts};

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

/// The text of an endnote, its paragraphs joined by new lines.
fn endnote_text(doc: &Document, id: i32) -> String {
    let story = doc
        .endnote_story(id)
        .expect("endnote lookup")
        .expect("endnote exists");
    doc.story_items(&story)
        .expect("endnote story reads")
        .iter()
        .map(|item| item.text().expect("endnote item reads").unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
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

/// The text of each `w:t` of a part, in order.
fn part_texts(xml: &str) -> Vec<String> {
    xml.split("<w:t")
        .skip(1)
        .filter(|rest| rest.starts_with('>') || rest.starts_with(' '))
        .map(|rest| {
            let start = rest.find('>').unwrap() + 1;
            rest[start..start + rest[start..].find("</w:t>").unwrap()].to_owned()
        })
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
fn a_tag_takes_its_value_and_keeps_the_text_around_it() {
    let mut doc = doc_with(&["Hello {{ name }}, welcome.", "No tag here"]);
    assert_eq!(doc.render_template(&json!({ "name": "Ada" })).unwrap(), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Hello Ada, welcome.", "No tag here"]);
    assert!(!doc.part_xml("word/document.xml").unwrap().contains("{{"));
}

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
fn values_render_literally_whatever_characters_they_hold() {
    let values = [
        ("markup", "<b>Smith & \"Sons\"</b> 'Ltd'"),
        ("cjk", "東京都千代田区"),
        ("rtl", "مرحبا بالعالم"),
        ("emoji", "Merci 😀"),
        // A value that looks like a tag stays as written.
        ("tag", "{{ markup }}"),
    ];
    let paragraphs: Vec<String> = values
        .iter()
        .map(|(key, _)| format!("{{{{ {key} }}}}"))
        .collect();
    let paragraphs: Vec<&str> = paragraphs.iter().map(String::as_str).collect();
    let mut doc = doc_with(&paragraphs);
    let data: serde_json::Map<String, serde_json::Value> = values
        .iter()
        .map(|(key, value)| ((*key).to_owned(), json!(value)))
        .collect();
    assert_eq!(doc.render_template(&data.into()).unwrap(), values.len());
    let doc = reopen(&mut doc);
    let expected: Vec<&str> = values.iter().map(|(_, value)| *value).collect();
    assert_eq!(texts(&doc), expected);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains("&lt;b&gt;Smith &amp;"),
        "markup is escaped: {xml}"
    );
}

#[test]
fn a_document_without_tags_renders_nothing_and_stays_byte_identical() {
    let mut doc = doc_with(&["Plain text", "with { braces } and % signs"]);
    let before = doc.to_bytes().unwrap();
    assert_eq!(doc.render_template(&json!({ "unused": 1 })).unwrap(), 0);
    assert_eq!(doc.to_bytes().unwrap(), before);
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
fn tags_in_headers_footers_tables_and_notes_are_filled() {
    let mut doc = doc_with(&["Body {{ who }}"]);
    doc.set_header("Header {{ who }}");
    doc.set_footer("Footer {{ ref }}");
    {
        let mut table = doc.add_table(1, 2);
        table.cell(0, 0).unwrap().set_text("Cell {{ who }}");
        table.cell(0, 1).unwrap().set_text("{{ ref }}");
    }
    let location = doc.paragraph_story_location(0).unwrap().unwrap();
    let footnote = doc.create_footnote(&location, "Note {{ who }}").unwrap();
    let location = doc.paragraph_story_location(0).unwrap().unwrap();
    let endnote = doc.create_endnote(&location, "End {{ ref }}").unwrap();

    let count = doc
        .render_template(&json!({ "who": "Ada", "ref": "R-7" }))
        .unwrap();
    assert_eq!(count, 7);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Body Ada"]);
    assert_eq!(doc.header_text().as_deref(), Some("Header Ada"));
    assert_eq!(doc.footer_text().as_deref(), Some("Footer R-7"));
    let table = doc.table(0).unwrap();
    assert_eq!(table.cell(0, 0).unwrap().text(), "Cell Ada");
    assert_eq!(table.cell(0, 1).unwrap().text(), "R-7");
    // A new note starts with a space after its mark, as Word writes it.
    assert_eq!(doc.footnotes(), [(footnote, " Note Ada".to_owned())]);
    assert_eq!(endnote_text(&doc, endnote), " End R-7");
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
fn a_nested_loop_reads_the_outer_item_inside_the_inner_one() {
    let mut doc = doc_with(&[
        "{% for group in groups %}",
        "Group {{ group.name }}",
        "{% for item in group.items %}",
        "{{ group.name }}.{{ item }}",
        "{% endfor %}",
        "{% endfor %}",
    ]);
    let data = json!({ "groups": [
        { "name": "A", "items": [1, 2] },
        { "name": "B", "items": [3] }
    ] });
    assert_eq!(doc.render_template(&data).unwrap(), 8);
    assert_eq!(
        texts(&reopen(&mut doc)),
        ["Group A", "A.1", "A.2", "Group B", "B.3"]
    );
}

#[test]
fn an_if_block_keeps_its_paragraphs_only_for_a_truthy_value() {
    let cases = [
        (json!(false), false),
        (json!(null), false),
        (json!(0), false),
        (json!(""), false),
        (json!([]), false),
        (json!({}), false),
        (json!(true), true),
        (json!(2), true),
        (json!("no"), true),
        (json!([0]), true),
        (json!({ "k": 0 }), true),
    ];
    for (value, kept) in cases {
        let mut doc = doc_with(&["Start", "{% if flag %}", "Shown", "{% endif %}", "End"]);
        doc.render_template(&json!({ "flag": value })).unwrap();
        let expected: &[&str] = if kept {
            &["Start", "Shown", "End"]
        } else {
            &["Start", "End"]
        };
        assert_eq!(texts(&reopen(&mut doc)), expected, "flag = {value}");
    }
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
fn the_unversioned_grammar_reads_a_pipe_as_part_of_the_key() {
    let data = json!({ "name|upper": "from the key", "name": "Ada" });
    let mut plain = doc_with(&["{{name|upper}}"]);
    assert_eq!(plain.render_template(&data).unwrap(), 1);
    assert_eq!(texts(&reopen(&mut plain)), ["from the key"]);
    let mut versioned = doc_with(&["{{name|upper}}"]);
    assert_eq!(versioned.render_template_versioned(&data, 1).unwrap(), 1);
    assert_eq!(texts(&reopen(&mut versioned)), ["ADA"]);
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
fn a_google_docs_report_with_tables_renders_without_error() {
    let mut cell = doc_with_body(concat!(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>"#,
        r#"<w:tr><w:tc><w:tcPr/><w:p><w:r><w:t>{{ v }}</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#,
    ));
    let rendered = cell.render_template(&json!({ "v": "V" }));
    assert!(
        matches!(rendered, Ok(1)),
        "the Google Docs report renders: a cell with an empty tcPr gives {rendered:?}"
    );
    // The whole report then renders too.
    let mut doc = corpus_doc("google/report.docx");
    let before = texts(&doc);
    let rendered = doc.render_template(&json!({}));
    assert!(matches!(rendered, Ok(0)), "{rendered:?}");
    assert_eq!(texts(&doc), before);
}

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
fn a_libreoffice_thesis_with_twenty_two_header_and_footer_parts_renders() {
    let mut doc = corpus_doc("libreoffice/thesis.docx");
    let before = texts(&doc);
    assert_eq!(doc.render_template(&json!({})).unwrap(), 0);
    assert_eq!(texts(&doc), before);
}

#[test]
fn blocks_in_every_header_and_footer_of_the_libreoffice_letter_render() {
    let parts = [
        ("/word/header1.xml", "</w:hdr>"),
        ("/word/header2.xml", "</w:hdr>"),
        ("/word/header3.xml", "</w:hdr>"),
        ("/word/footer1.xml", "</w:ftr>"),
        ("/word/footer2.xml", "</w:ftr>"),
        ("/word/footer3.xml", "</w:ftr>"),
    ];
    let insertions = parts
        .iter()
        .enumerate()
        .map(|(index, (part, close))| {
            let shown = format!("Shown {index} {{{{ who }}}}");
            let block = raw_paragraphs(&["{% if flag %}", &shown, "{% endif %}"]);
            (*part, "<w:", *close, block)
        })
        .collect::<Vec<_>>();
    // The text each part holds besides the block, which must survive.
    let letter = corpus_doc("libreoffice/letter.docx");
    let own_texts = parts
        .iter()
        .map(|(part, _)| part_texts(&letter.part_xml(part).unwrap()))
        .collect::<Vec<_>>();
    for (flag, count) in [(true, 6), (false, 0)] {
        let mut doc = corpus_doc_with("libreoffice/letter.docx", &insertions);
        assert_eq!(
            doc.render_template(&json!({ "flag": flag, "who": "Ada" }))
                .unwrap(),
            count
        );
        let doc = reopen(&mut doc);
        for (index, (part, _)) in parts.iter().enumerate() {
            let mut expected = own_texts[index].clone();
            if flag {
                expected.push(format!("Shown {index} Ada"));
            }
            assert_eq!(part_texts(&doc.part_xml(part).unwrap()), expected, "{part}");
        }
    }
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
fn a_merge_gives_one_document_per_record_with_its_merge_fields_filled() {
    let template = doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Dear </w:t></w:r><w:fldSimple w:instr=" MERGEFIELD Name \* Upper "><w:r><w:t>«Name»</w:t></w:r></w:fldSimple><w:r><w:t>,</w:t></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD City \b "in " \f "!" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«City»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p><w:p>{}</w:p>"#,
        merge_field("Note")
    ));
    let mut outputs = template
        .mail_merge(&[
            record(&[("Name", "Zoë"), ("City", "Paris"), ("Note", "VIP")]),
            // An empty value and a field missing from the record both show
            // nothing, without the text before and after.
            record(&[("Name", "Bob"), ("City", "")]),
        ])
        .unwrap();
    assert_eq!(outputs.len(), 2);
    let first = reopen(&mut outputs[0]);
    assert_eq!(texts(&first), ["Dear ZOË,", "in Paris!", "VIP"]);
    let second = reopen(&mut outputs[1]);
    assert_eq!(texts(&second), ["Dear BOB,", "", ""]);
    // The fields stay fields, ready for another merge.
    let xml = first.part_xml("word/document.xml").unwrap();
    assert!(xml.contains(r#"MERGEFIELD Name \* Upper"#), "{xml}");
    // The template itself is untouched.
    assert_eq!(texts(&template), ["Dear «Name»,", "«City»", "«Note»"]);

    let error = refusal(template.mail_merge(&[]));
    assert!(error.contains("at least one record"), "{error}");
}

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

/// The text of the default header section `section` shows, if any.
fn header_shown(doc: &Document, section: usize) -> Option<String> {
    let shown = doc
        .section_story(section, HeaderFooterKind::Header, HdrFtrType::Default)
        .expect("section index in range")?;
    Some(
        doc.story_items(shown.story())
            .expect("header reads")
            .iter()
            .map(|item| item.text().expect("header text").unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[test]
fn every_section_of_a_sectioned_merge_keeps_the_template_page_size_and_header() {
    let mut template = doc_with_body(&format!(r#"<w:p>{}</w:p>"#, merge_field("Name")));
    template.set_page_size(Length::mm(148.0), Length::mm(210.0));
    template.set_header("Letterhead");
    let template = reopen(&mut template);
    let size = template.section(0).unwrap().page_size();
    assert_ne!(
        size,
        reopen(&mut Document::new()).section(0).unwrap().page_size(),
        "the template's page size is not the default"
    );
    let names = ["Ada", "Bob", "Cy"];
    let flat: Vec<_> = names.iter().map(|name| record(&[("Name", name)])).collect();
    let rich = MailMergeData {
        records: names
            .iter()
            .map(|name| MailMergeRecord {
                values: BTreeMap::from([("Name".to_owned(), text(name))]),
                regions: BTreeMap::new(),
            })
            .collect(),
        sources: BTreeMap::new(),
    };
    for (how, mut merged) in [
        ("flat", template.mail_merge_sections(&flat).unwrap()),
        (
            "rich",
            template.mail_merge_sections_rich(&rich, None).unwrap(),
        ),
    ] {
        let merged = reopen(&mut merged);
        assert_eq!(texts(&merged), ["Ada", "", "Bob", "", "Cy"], "{how}");
        assert_eq!(merged.section_count(), 3, "{how}");
        for index in 0..3 {
            assert_eq!(
                merged.section(index).unwrap().page_size(),
                size,
                "{how}: section {index} keeps the page size"
            );
            assert_eq!(
                header_shown(&merged, index).as_deref(),
                Some("Letterhead"),
                "{how}: section {index} shows the header"
            );
            let own = merged
                .section_story(index, HeaderFooterKind::Header, HdrFtrType::Default)
                .unwrap()
                .unwrap();
            assert!(
                !own.is_inherited(),
                "{how}: section {index} has its own header"
            );
        }
    }
}

/// A recipient of the corpus letters.
fn letter_recipient() -> BTreeMap<String, String> {
    record(&[
        ("Title", "Ms"),
        ("FirstName", "Zoé"),
        ("LastName", "Dupont"),
        ("JobTitle", "CFO"),
        ("Company", "Acme SARL"),
        ("Address1", "1 rue Haute"),
        ("PostalCode", "69001"),
        ("City", "Lyon"),
        ("Country", "France"),
    ])
}

/// The address block and the salutation of a corpus letter.
fn letter_lines(doc: &Document) -> Vec<String> {
    let texts = texts(doc);
    let mut lines = texts[2..8].to_vec();
    lines.push(
        texts
            .iter()
            .find(|text| text.starts_with("Dear "))
            .expect("the salutation")
            .clone(),
    );
    lines
}

const MERGED_LETTER: [&str; 7] = [
    "Ms Zoé Dupont",
    "CFO",
    "Acme SARL",
    "1 rue Haute",
    "69001 Lyon",
    "France",
    "Dear Ms Dupont,",
];

#[test]
fn the_python_docx_letter_merges_its_recipient_fields() {
    let template = corpus_doc("authored/letter.docx");
    assert_eq!(
        letter_lines(&template),
        [
            "«Title» «FirstName» «LastName»",
            "«JobTitle»",
            "«Company»",
            "«Address1»",
            "«PostalCode» «City»",
            "«Country»",
            "Dear «Title» «LastName»,"
        ]
    );
    let mut outputs = template.mail_merge(&[letter_recipient()]).unwrap();
    let merged = reopen(&mut outputs[0]);
    assert_eq!(letter_lines(&merged), MERGED_LETTER);
    // The rest of the letter stays as it was.
    assert_eq!(texts(&merged).len(), texts(&template).len());
    assert_eq!(texts(&merged)[9], texts(&template)[9]);
    assert_eq!(texts(&merged)[11..], texts(&template)[11..]);
    assert_eq!(merged.footer_text(), template.footer_text());
}

#[test]
fn the_libreoffice_letter_merges_its_recipient_fields() {
    let template = corpus_doc("libreoffice/letter.docx");
    let mut outputs = match template.mail_merge(&[letter_recipient()]) {
        Ok(outputs) => outputs,
        Err(error) if error.to_string().contains("source inventory identified") => {
            panic!("the LibreOffice letter merges: {error}")
        }
        Err(error) => panic!("another merge error: {error}"),
    };
    let merged = reopen(&mut outputs[0]);
    assert_eq!(letter_lines(&merged), MERGED_LETTER);
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

#[test]
fn a_region_a_record_does_not_hold_reads_the_named_source() {
    let template = doc_with_body(&format!(
        r#"<w:p>{}</w:p><w:p>{}</w:p><w:p>{}</w:p><w:p>{}</w:p>"#,
        merge_field("Client"),
        merge_field("TableStart:Products"),
        merge_field("Product"),
        merge_field("TableEnd:Products"),
    ));
    let product = |name: &str| MailMergeRecord {
        values: BTreeMap::from([("Product".to_owned(), text(name))]),
        regions: BTreeMap::new(),
    };
    let client = |name: &str, regions: BTreeMap<String, Vec<MailMergeRecord>>| MailMergeRecord {
        values: BTreeMap::from([("Client".to_owned(), text(name))]),
        regions,
    };
    let data = MailMergeData {
        records: vec![
            client("Acme", BTreeMap::new()),
            // A region of the record's own wins over the named source.
            client(
                "Bolt",
                BTreeMap::from([("Products".to_owned(), vec![product("Custom")])]),
            ),
        ],
        sources: BTreeMap::from([("Products".to_owned(), vec![product("Pen"), product("Ink")])]),
    };
    let mut outputs = template.mail_merge_rich(&data, None).unwrap();
    let merged: Vec<Vec<String>> = outputs
        .iter_mut()
        .map(|output| texts(&reopen(output)))
        .collect();
    assert_eq!(merged, [vec!["Acme", "Pen", "Ink"], vec!["Bolt", "Custom"]]);
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

// ---- Building blocks ----

#[test]
fn a_new_building_block_is_listed_after_a_save_with_its_metadata() {
    let mut doc = doc_with(&["Body"]);
    assert!(doc.building_blocks().unwrap().is_empty());
    doc.create_building_block(block("Closing", "Kind regards, Zoé"))
        .unwrap();
    let doc = reopen(&mut doc);
    let entries = doc.building_blocks().unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0].block;
    assert_eq!(entry.name, "Closing");
    assert_eq!(entry.kind, BuildingBlockKind::AutoText);
    assert_eq!(entry.category.as_deref(), Some("Letters"));
    assert_eq!(entry.gallery.as_deref(), Some("autoTxt"));
    assert_eq!(entry.description.as_deref(), Some("Closing text"));
    let lines: Vec<String> = entry.body.paragraphs().map(|p| p.text()).collect();
    assert_eq!(lines, ["Kind regards, Zoé"]);
    // The entry lives in the glossary, not in the body.
    assert_eq!(texts(&doc), ["Body"]);
    assert!(doc.part_xml("word/glossary/document.xml").is_ok());
}

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

#[test]
fn to_bytes_as_turns_a_document_into_a_template_and_back() {
    let doc = corpus_doc("authored/letter.docx");
    assert_eq!(doc.package_class().unwrap(), WordPackageClass::Document);
    let template = doc.to_bytes_as(WordPackageClass::Template).unwrap();
    assert_eq!(main_part_content_type(&template), MAIN_TEMPLATE_TYPE);
    let template = Document::from_bytes(&template).unwrap();
    assert_eq!(
        template.package_class().unwrap(),
        WordPackageClass::Template
    );
    assert_eq!(texts(&template), texts(&doc));
    let back = template.to_bytes_as(WordPackageClass::Document).unwrap();
    assert_eq!(main_part_content_type(&back), MAIN_DOCUMENT_TYPE);
    assert_eq!(texts(&Document::from_bytes(&back).unwrap()), texts(&doc));
}

// Fails today: a new document of the minimal profile has no main part until
// it is saved, so asking its class fails, where the Word-compatible profile
// answers at once. Issue: #203
#[test]
#[should_panic(expected = "a new minimal template knows its class")]
fn a_new_minimal_template_knows_its_class_before_a_save() {
    let profile = WordCreationProfile::Minimal(WordPackageClass::Template);
    let class = Document::new_with_profile(profile).package_class();
    let mut saved = Document::new_with_profile(profile);
    assert_eq!(
        reopen(&mut saved).package_class().unwrap(),
        WordPackageClass::Template,
        "after a save"
    );
    assert!(
        matches!(class, Ok(WordPackageClass::Template)),
        "a new minimal template knows its class: {class:?}"
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
