//! `Document::try_replace_all_checked` refuses a batch that leaves a pair's
//! text out of its replacement's reach, names where it is left, and publishes
//! it with `allow_unreached`, on the Word and Google Docs files of the corpus.

use std::io::Cursor;
use std::path::Path;

use oxoffice_docx::{
    Document, ReplaceOptions, ReplacementRefusal, StoryKind, UnreachedReplacement,
};
use oxoffice_opc::OpcPackage;

const PRODUCERS: [&str; 2] = ["word", "google"];

const CHECKED: ReplaceOptions = ReplaceOptions {
    allow_unreached: false,
};

/// ACME once in a plain paragraph, then cut by the edge of a content
/// control, of a tracked insertion and of a simple field.
const SPLIT: &str = r#"<w:p><w:r><w:t>ACME plain</w:t></w:r></w:p><w:p><w:r><w:t>AC</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t>ME control</w:t></w:r></w:sdtContent></w:sdt></w:p><w:p><w:r><w:t>AC</w:t></w:r><w:ins w:id="901" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME insertion</w:t></w:r></w:ins></w:p><w:p><w:r><w:t>AC</w:t></w:r><w:fldSimple w:instr=" AUTHOR "><w:r><w:t>ME field</w:t></w:r></w:fldSimple></w:p>"#;

fn corpus_bytes(producer: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(producer)
        .join("report.docx");
    std::fs::read(path).expect("corpus report is readable")
}

/// The corpus report of `producer` with `xml` inserted at the start of the
/// body (`at_start`) or before its last `w:sectPr`, or before the closing tag
/// of another part.
fn report_with(producer: &str, part_name: &str, xml: &str, at_start: bool) -> Document {
    let mut package = OpcPackage::from_reader(Cursor::new(corpus_bytes(producer))).unwrap();
    let part = String::from_utf8(package.get_part(part_name).unwrap().to_vec()).unwrap();
    let at = match (part_name, at_start) {
        ("/word/document.xml", true) => part.find("<w:body>").unwrap() + "<w:body>".len(),
        ("/word/document.xml", false) => part.rfind("<w:sectPr").unwrap(),
        _ => part.rfind("</w:").unwrap(),
    };
    package.set_part(
        part_name,
        format!("{}{xml}{}", &part[..at], &part[at..]).into_bytes(),
    );
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    Document::from_bytes(&bytes.into_inner()).unwrap()
}

fn body_with(producer: &str, xml: &str) -> Document {
    report_with(producer, "/word/document.xml", xml, false)
}

/// The text of the first body paragraph that starts with `prefix`.
fn paragraph_starting(doc: &Document, prefix: &str) -> String {
    doc.paragraphs()
        .iter()
        .map(|paragraph| paragraph.text())
        .find(|text| text.starts_with(prefix))
        .unwrap_or_else(|| panic!("no paragraph starting with {prefix:?}"))
}

fn unreached(
    result: oxoffice_docx::Result<Result<Vec<usize>, ReplacementRefusal>>,
) -> UnreachedReplacement {
    match result.unwrap() {
        Err(ReplacementRefusal::Unreached(unreached)) => unreached,
        other => panic!("expected an unreached refusal, got {other:?}"),
    }
}

#[test]
fn a_match_cut_by_a_control_a_revision_or_a_field_refuses_the_batch() {
    for producer in PRODUCERS {
        let mut doc = body_with(producer, SPLIT);
        let before = doc.to_bytes().unwrap();
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "Globex", None)], CHECKED));
        assert_eq!(refusal.index, 0, "{producer}");
        assert_eq!(refusal.placeholder, "ACME", "{producer}");
        assert_eq!(refusal.left, 3, "{producer}");
        let left = refusal
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            left,
            ["ACME control", "ACME insertion", "ACME field"],
            "{producer}"
        );
        assert_eq!(
            doc.to_bytes().unwrap(),
            before,
            "{producer}: a refused batch leaves the document as it was"
        );
        assert_eq!(doc.count_text("ACME").unwrap(), vec![(StoryKind::Body, 4)]);
    }
}

#[test]
fn allow_unreached_publishes_the_batch_and_leaves_the_cut_matches() {
    for producer in PRODUCERS {
        let mut doc = body_with(producer, SPLIT);
        let counts = doc
            .try_replace_all_checked(
                &[("ACME", "Globex", Some(1))],
                ReplaceOptions {
                    allow_unreached: true,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(counts, [1], "{producer}");
        assert_eq!(doc.count_text("ACME").unwrap(), vec![(StoryKind::Body, 3)]);
    }
}

#[test]
fn a_refusal_names_the_pair_the_part_the_style_and_the_text() {
    let split = r#"<w:p><w:pPr><w:pStyle w:val="HeaderNote"/></w:pPr><w:r><w:t>AC</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t>ME in the header</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = report_with(producer, "/word/header1.xml", split, false);
        let pairs = [
            ("Northwind", "Southwind", Some(1)),
            ("ACME", "Globex", None),
        ];
        let refusal = unreached(doc.try_replace_all_checked(&pairs, CHECKED));
        assert_eq!(refusal.index, 1, "{producer}");
        assert_eq!(refusal.paragraphs[0].kind, StoryKind::Header, "{producer}");
        assert_eq!(
            refusal.to_string(),
            "pair 1: 1 occurrence(s) of \"ACME\" left after its replacement, out of its \
             reach: /word/header1.xml [HeaderNote]: \"ACME in the header\"",
            "{producer}"
        );
    }
}

#[test]
fn a_count_mismatch_is_refused_before_the_reach_check() {
    for producer in PRODUCERS {
        let mut doc = body_with(producer, SPLIT);
        match doc
            .try_replace_all_checked(&[("ACME", "Globex", Some(4))], CHECKED)
            .unwrap()
        {
            Err(ReplacementRefusal::Count(mismatch)) => {
                assert_eq!((mismatch.expected, mismatch.found), (4, 1), "{producer}")
            }
            other => panic!("{producer}: expected a count refusal, got {other:?}"),
        }
    }
}

#[test]
fn a_replacement_that_holds_its_text_names_the_paragraphs_left_behind() {
    for producer in PRODUCERS {
        let mut doc = body_with(producer, SPLIT);
        let refusal =
            unreached(doc.try_replace_all_checked(&[("ACME", "ACME Corp", None)], CHECKED));
        assert_eq!(refusal.left, 3, "{producer}");
        let named = refusal
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            named,
            ["ACME control", "ACME insertion", "ACME field"],
            "{producer}: not the replaced one, which holds ACME Corp"
        );

        let mut plain = body_with(producer, r#"<w:p><w:r><w:t>ACME plain</w:t></w:r></w:p>"#);
        let counts = plain
            .try_replace_all_checked(&[("ACME", "ACME Corp", Some(1))], CHECKED)
            .unwrap()
            .unwrap();
        assert_eq!(counts, [1], "{producer}");
    }
}

#[test]
fn each_pair_is_checked_after_it_runs_not_after_the_batch() {
    for producer in PRODUCERS {
        let mut doc = body_with(producer, r#"<w:p><w:r><w:t>alpha beta</w:t></w:r></w:p>"#);
        let pairs = [("alpha", "gamma", Some(1)), ("beta", "alpha", Some(1))];
        let counts = doc
            .try_replace_all_checked(&pairs, CHECKED)
            .unwrap()
            .unwrap();
        assert_eq!(
            counts,
            [1, 1],
            "{producer}: a later pair may write an earlier text"
        );
        assert_eq!(
            doc.count_text("gamma alpha").unwrap(),
            vec![(StoryKind::Body, 1)]
        );
    }
}

#[test]
fn text_in_a_comment_is_not_searched_and_does_not_refuse() {
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        let counts = doc
            .try_replace_all_checked(&[("finance", "accounting", None)], CHECKED)
            .unwrap()
            .unwrap();
        assert_eq!(counts, [0], "{producer}");
        assert_eq!(
            doc.count_text("finance").unwrap(),
            vec![(StoryKind::Comment, 1)]
        );
    }
}

#[test]
fn table_of_contents_entries_are_replaced_with_their_heading() {
    // A table of contents as Word writes it: a block content control holding
    // the result of a TOC field, each entry a hyperlink with a PAGEREF field.
    let toc = r#"<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Table of Contents"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent><w:p><w:pPr><w:pStyle w:val="TM1"/></w:pPr><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:hyperlink w:anchor="_Toc1" w:history="1"><w:r><w:t>1. Highlights</w:t></w:r><w:r><w:tab/></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGEREF _Toc1 \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:hyperlink></w:p><w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:sdtContent></w:sdt>"#;
    for producer in PRODUCERS {
        let mut doc = report_with(producer, "/word/document.xml", toc, true);
        let counts = doc
            .try_replace_all_checked(&[("Highlights", "Key points", Some(2))], CHECKED)
            .unwrap()
            .unwrap();
        assert_eq!(counts, [2], "{producer}");
        let paragraphs = doc.story_paragraphs().unwrap();
        let entry = paragraphs
            .iter()
            .find(|paragraph| paragraph.style.as_deref() == Some("TM1"))
            .unwrap();
        assert_eq!(entry.text, "1. Key points\t1", "{producer}");
    }
}

#[test]
fn a_text_box_inside_a_note_is_not_searched_and_refuses() {
    let text_box = r#"<w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><mc:Choice Requires="wps"><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="914400"/><wp:docPr id="78" name="Text Box 78"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>ACME in a boxed note</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></mc:Choice></mc:AlternateContent></w:r>"#;
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        doc.add_footnote("A note");
        let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
        let notes =
            String::from_utf8(package.get_part("/word/footnotes.xml").unwrap().to_vec()).unwrap();
        let at = notes.rfind("</w:p>").unwrap();
        package.set_part(
            "/word/footnotes.xml",
            format!("{}{text_box}{}", &notes[..at], &notes[at..]).into_bytes(),
        );
        let mut bytes = Cursor::new(Vec::new());
        package.write_to(&mut bytes).unwrap();
        let mut doc = Document::from_bytes(&bytes.into_inner()).unwrap();
        assert_eq!(
            doc.count_text("ACME").unwrap(),
            vec![(StoryKind::Footnote, 1)],
            "{producer}"
        );
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "Globex", None)], CHECKED));
        assert_eq!(refusal.left, 1, "{producer}");
        assert_eq!(
            refusal.paragraphs[0].kind,
            StoryKind::Footnote,
            "{producer}"
        );
        assert_eq!(
            refusal.paragraphs[0].location.story().kind(),
            StoryKind::TextBox,
            "{producer}"
        );
    }
}

#[test]
fn a_replacement_that_forms_its_text_again_with_its_neighbours_is_not_refused() {
    for producer in PRODUCERS {
        let mut doc = body_with(producer, r#"<w:p><w:r><w:t>aZZZb</w:t></w:r></w:p>"#);
        let counts = doc
            .try_replace_all_checked(&[("ZZ", "Z", Some(1))], CHECKED)
            .unwrap()
            .unwrap();
        assert_eq!(counts, [1], "{producer}");
        assert_eq!(doc.count_text("aZZb").unwrap(), vec![(StoryKind::Body, 1)]);
    }
}

#[test]
fn a_match_cut_by_a_hyperlink_is_replaced() {
    let link = r#"<w:p><w:r><w:t>AC</w:t></w:r><w:hyperlink w:anchor="top"><w:r><w:t>ME link</w:t></w:r></w:hyperlink></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, link);
        let counts = doc
            .try_replace_all_checked(&[("ACME", "Globex", Some(1))], CHECKED)
            .unwrap()
            .unwrap();
        assert_eq!(counts, [1], "{producer}");
        assert_eq!(doc.count_text("ACME").unwrap(), vec![], "{producer}");
    }
}

#[test]
fn a_table_of_contents_inserted_by_oxoffice_is_replaced_with_its_headings() {
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        doc.insert_toc(1, 3).unwrap();
        doc.rebuild_toc().unwrap();
        assert_eq!(
            doc.count_text("Highlights").unwrap(),
            vec![(StoryKind::Body, 2)],
            "{producer}: the heading and its entry"
        );
        let counts = doc
            .try_replace_all_checked(&[("Highlights", "Key points", Some(2))], CHECKED)
            .unwrap()
            .unwrap();
        assert_eq!(counts, [2], "{producer}");
    }
}

#[test]
fn the_paragraphs_of_a_block_custom_xml_element_refuse() {
    let block = r#"<w:customXml w:element="section"><w:p><w:r><w:t>ACME in a custom element</w:t></w:r></w:p></w:customXml>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, block);
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "Globex", None)], CHECKED));
        assert_eq!(refusal.left, 1, "{producer}");
        assert_eq!(refusal.paragraphs[0].text, "ACME in a custom element");
    }
}

#[test]
fn a_document_the_check_cannot_read_is_refused_unless_unreached_text_is_allowed() {
    for producer in PRODUCERS {
        let mut package = OpcPackage::from_reader(Cursor::new(corpus_bytes(producer))).unwrap();
        package.remove_part("/word/header1.xml").unwrap();
        let mut bytes = Cursor::new(Vec::new());
        package.write_to(&mut bytes).unwrap();
        let bytes = bytes.into_inner();

        let mut doc = Document::from_bytes(&bytes).unwrap();
        match doc
            .try_replace_all_checked(&[("Summary", "Overview", Some(1))], CHECKED)
            .unwrap()
        {
            Err(ReplacementRefusal::Unchecked(unchecked)) => {
                assert_eq!(
                    (unchecked.index, unchecked.placeholder.as_str()),
                    (0, "Summary")
                );
                assert!(
                    unchecked.reason.contains("header"),
                    "{producer}: {unchecked}"
                );
            }
            other => panic!("{producer}: expected an unchecked refusal, got {other:?}"),
        }

        let mut doc = Document::from_bytes(&bytes).unwrap();
        let counts = doc
            .try_replace_all_checked(
                &[("Summary", "Overview", Some(1))],
                ReplaceOptions {
                    allow_unreached: true,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(counts, [1], "{producer}: replaced as before");
    }
}

#[test]
fn a_nested_field_result_in_a_field_code_does_not_hide_a_match_left_behind() {
    // The nested DOCPROPERTY result sits in the code of the IF field, which no
    // reader sees: the paragraph cut by the insertion is still named.
    let hidden = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF </w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> DOCPROPERTY Company </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>ACME</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve"> = "" "none" "some" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>some</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p><w:p><w:r><w:t>AC</w:t></w:r><w:ins w:id="905" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME cut</w:t></w:r></w:ins></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, hidden);
        assert_eq!(doc.count_text("ACME").unwrap(), vec![(StoryKind::Body, 1)]);
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "Globex", None)], CHECKED));
        assert_eq!(refusal.left, 1, "{producer}");
        let named = refusal
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(named, ["ACME cut"], "{producer}");
    }
}

#[test]
fn a_refusal_names_the_paragraphs_left_behind_not_those_formed_again() {
    let cut = r#"<w:p><w:r><w:t>aZZZb</w:t></w:r></w:p><w:p><w:r><w:t>pZ</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t>Zq</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, cut);
        let refusal = unreached(doc.try_replace_all_checked(&[("ZZ", "Z", None)], CHECKED));
        assert_eq!(refusal.left, 1, "{producer}");
        let named = refusal
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(named, ["pZZq"], "{producer}");
    }
}

/// A paragraph whose IF field code holds a nested DOCPROPERTY result "ACME",
/// which no reader sees.
const HIDDEN: &str = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF </w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> DOCPROPERTY Company </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>ACME</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve"> = "" "none" "some" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>some</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;

#[test]
fn a_changed_paragraph_that_keeps_a_cut_match_refuses() {
    let mixed = format!(
        "{HIDDEN}{}",
        r#"<w:p><w:r><w:t xml:space="preserve">ACME and AC</w:t></w:r><w:ins w:id="906" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME</w:t></w:r></w:ins></w:p>"#
    );
    for producer in PRODUCERS {
        let mut doc = body_with(producer, &mixed);
        let counts = body_with(producer, &mixed)
            .try_replace_all_checked(
                &[("ACME", "Globex", None)],
                ReplaceOptions {
                    allow_unreached: true,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(counts, [1], "{producer}: the first visible one only");
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "Globex", None)], CHECKED));
        assert_eq!(refusal.left, 1, "{producer}");
        let named = refusal
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(named, ["Globex and ACME"], "{producer}");
    }
}

#[test]
fn a_value_equal_to_its_placeholder_is_not_refused() {
    for producer in PRODUCERS {
        let mut doc = body_with(producer, r#"<w:p><w:r><w:t>ACME plain</w:t></w:r></w:p>"#);
        let counts = doc
            .try_replace_all_checked(&[("ACME", "ACME", Some(1))], CHECKED)
            .unwrap()
            .unwrap();
        assert_eq!(counts, [1], "{producer}");
    }
}

#[test]
fn a_refusal_names_every_paragraph_left_behind() {
    let cuts = r#"<w:p><w:r><w:t xml:space="preserve">ACME and AC</w:t></w:r><w:ins w:id="907" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME</w:t></w:r></w:ins></w:p><w:p><w:r><w:t>AC</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t>ME control</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, cuts);
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "Globex", None)], CHECKED));
        assert_eq!(refusal.left, 2, "{producer}");
        let named = refusal
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(named, ["Globex and ACME", "ACME control"], "{producer}");
    }
}

#[test]
fn a_text_split_by_a_line_break_is_no_match_and_a_cut_one_beside_it_refuses() {
    // Replacement and the story view both read the break as "\n" (#88).
    let paragraph = r#"<w:p><w:r><w:t xml:space="preserve">Jean </w:t></w:r><w:r><w:br/><w:t xml:space="preserve">Dupont, Jean Du</w:t></w:r><w:ins w:id="908" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>pont</w:t></w:r></w:ins></w:p>"#;
    for producer in PRODUCERS {
        let counts = body_with(producer, paragraph)
            .try_replace_all_checked(
                &[("Jean Dupont", "J. Dupont", None)],
                ReplaceOptions {
                    allow_unreached: true,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(counts, [0], "{producer}: neither is replaced");
        let mut doc = body_with(producer, paragraph);
        assert_eq!(
            doc.count_text("Jean Dupont").unwrap(),
            vec![(StoryKind::Body, 1)],
            "{producer}: the cut one only"
        );
        let refusal =
            unreached(doc.try_replace_all_checked(&[("Jean Dupont", "J. Dupont", None)], CHECKED));
        assert_eq!(refusal.left, 1, "{producer}");
    }
}
#[test]
fn a_text_split_by_a_break_in_one_paragraph_does_not_offset_a_cut_one_in_another() {
    let paragraphs = r#"<w:p><w:r><w:t>AC</w:t></w:r><w:r><w:br/><w:t>ME across</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">ACME and AC</w:t></w:r><w:ins w:id="909" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME</w:t></w:r></w:ins></w:p>"#;
    for producer in PRODUCERS {
        let counts = body_with(producer, paragraphs)
            .try_replace_all_checked(
                &[("ACME", "Globex", None)],
                ReplaceOptions {
                    allow_unreached: true,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(counts, [1], "{producer}: the first visible one");
        let mut doc = body_with(producer, paragraphs);
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "Globex", None)], CHECKED));
        assert_eq!(refusal.left, 1, "{producer}");
        assert_eq!(refusal.paragraphs[0].text, "Globex and ACME", "{producer}");
    }
}
#[test]
fn a_value_as_long_as_its_placeholder_is_checked_too() {
    for producer in PRODUCERS {
        let mut doc = body_with(producer, SPLIT);
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "ACNE", None)], CHECKED));
        assert_eq!(refusal.left, 3, "{producer}");
    }
}

#[test]
fn a_text_split_by_a_break_does_not_refuse_a_pair_that_reached_everything() {
    let paragraph = r#"<w:p><w:r><w:t xml:space="preserve">Call AC</w:t></w:r><w:r><w:br/><w:t xml:space="preserve">ME today, ACME</w:t></w:r></w:p>"#;
    for producer in PRODUCERS {
        for value in ["Globex", "ACME Inc.", "ACME"] {
            let mut doc = body_with(producer, paragraph);
            let counts = doc
                .try_replace_all_checked(&[("ACME", value, None)], CHECKED)
                .unwrap()
                .unwrap_or_else(|refusal| panic!("{producer} {value}: {refusal}"));
            assert_eq!(counts, [1], "{producer} {value}: the one after the break");
        }
    }
}
#[test]
fn many_adjacent_cut_matches_deleted_are_counted_by_length() {
    let cut = r#"<w:r><w:t>AC</w:t></w:r><w:ins w:id="910" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME</w:t></w:r></w:ins>"#;
    let paragraph = format!("<w:p>{}</w:p>", cut.repeat(400));
    for producer in PRODUCERS {
        let mut doc = body_with(producer, &paragraph);
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "", None)], CHECKED));
        assert_eq!(refusal.left, 400, "{producer}");
    }
}

#[test]
fn past_the_reading_cap_a_text_split_by_breaks_does_not_count() {
    // 300 ACME cut by an insertion, then 300 split by a line break, which
    // neither replacement nor the story view reads as ACME.
    let cut = r#"<w:r><w:t>AC</w:t></w:r><w:ins w:id="911" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME</w:t></w:r></w:ins>"#;
    let across = r#"<w:r><w:t>AC</w:t></w:r><w:r><w:br/><w:t>ME</w:t></w:r>"#;
    let paragraph = format!("<w:p>{}{}</w:p>", cut.repeat(300), across.repeat(300));
    for producer in PRODUCERS {
        let mut doc = body_with(producer, &paragraph);
        let refusal = unreached(doc.try_replace_all_checked(&[("ACME", "", None)], CHECKED));
        assert_eq!(refusal.left, 300, "{producer}");
    }
}
#[test]
fn a_cut_occurrence_beside_a_replaced_one_refuses_whatever_breaks_the_value_holds() {
    let paragraph = r#"<w:p><w:r><w:t xml:space="preserve">Call ACME, then AC</w:t></w:r><w:ins w:id="912" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME</w:t></w:r></w:ins></w:p>"#;
    for value in ["ACME\nInc.", "ACME\n", "ACME\t", "\nACME", "X"] {
        for producer in PRODUCERS {
            let mut doc = body_with(producer, paragraph);
            let refusal = unreached(doc.try_replace_all_checked(&[("ACME", value, None)], CHECKED));
            assert_eq!(refusal.left, 1, "{producer}, {value:?}");
        }
    }
}
#[test]
fn a_break_of_the_paragraph_beside_a_cut_occurrence_is_not_taken_for_the_value_s() {
    // The cut ACME is followed (or preceded) by a break of its own, on the
    // side where the value holds its break.
    let ins = r#"<w:ins w:id="913" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME</w:t></w:r></w:ins>"#;
    let after = format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Call ACME, then AC</w:t></w:r>{ins}<w:r><w:br/><w:t>Inc.</w:t></w:r></w:p>"#
    );
    let before = format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Call ACME, then</w:t></w:r><w:r><w:br/><w:t>AC</w:t></w:r>{ins}</w:p>"#
    );
    for (paragraph, value) in [(&after, "ACME\n"), (&before, "\nACME")] {
        for producer in PRODUCERS {
            let mut doc = body_with(producer, paragraph);
            let refusal = unreached(doc.try_replace_all_checked(&[("ACME", value, None)], CHECKED));
            assert_eq!(refusal.left, 1, "{producer}, {value:?}");
        }
    }
}

#[test]
fn a_placeholder_holding_a_break_is_found_across_one_and_a_cut_one_refuses() {
    let reached = r#"<w:p><w:r><w:t xml:space="preserve">Call AC</w:t></w:r><w:r><w:br/><w:t xml:space="preserve">ME today</w:t></w:r></w:p>"#;
    let cut = r#"<w:p><w:r><w:t xml:space="preserve">Call AC</w:t></w:r><w:r><w:br/><w:t xml:space="preserve">ME, then AC</w:t></w:r><w:r><w:br/></w:r><w:ins w:id="914" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME</w:t></w:r></w:ins></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, reached);
        let counts = doc
            .try_replace_all_checked(&[("AC\nME", "ACME", None)], CHECKED)
            .unwrap()
            .unwrap_or_else(|refusal| panic!("{producer}: {refusal}"));
        assert_eq!(counts, [1], "{producer}");
        assert_eq!(
            paragraph_starting(&doc, "Call"),
            "Call ACME today",
            "{producer}"
        );
        let mut doc = body_with(producer, cut);
        let refusal = unreached(doc.try_replace_all_checked(&[("AC\nME", "ACME", None)], CHECKED));
        assert_eq!(refusal.left, 1, "{producer}");
    }
}
#[test]
fn a_value_holding_a_windows_line_ending_is_checked_as_the_break_it_writes() {
    let cut = r#"<w:p><w:r><w:t xml:space="preserve">ACME and AC</w:t></w:r><w:ins w:id="915" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>ME</w:t></w:r></w:ins></w:p>"#;
    let reached = r#"<w:p><w:r><w:t xml:space="preserve">Call ACME today, ACME</w:t></w:r></w:p>"#;
    for value in ["ACME\r\nInc.", "ACME\rInc."] {
        for producer in PRODUCERS {
            let mut doc = body_with(producer, cut);
            let refusal = unreached(doc.try_replace_all_checked(&[("ACME", value, None)], CHECKED));
            assert_eq!(refusal.left, 1, "{producer}, {value:?}");
            let mut doc = body_with(producer, reached);
            let counts = doc
                .try_replace_all_checked(&[("ACME", value, None)], CHECKED)
                .unwrap()
                .unwrap_or_else(|refusal| panic!("{producer} {value:?}: {refusal}"));
            assert_eq!(counts, [2], "{producer} {value:?}");
            assert_eq!(
                paragraph_starting(&doc, "Call"),
                "Call ACME\nInc. today, ACME\nInc.",
                "{producer} {value:?}"
            );
        }
    }
}

#[test]
fn a_value_holding_a_break_or_a_tab_publishes_and_reads_back_as_written() {
    let paragraph =
        r#"<w:p><w:r><w:t xml:space="preserve">Call ACME today, ACME</w:t></w:r></w:p>"#;
    for value in ["ACME\nInc.", "\nACME", "ACME\t"] {
        for producer in PRODUCERS {
            let mut doc = body_with(producer, paragraph);
            let counts = doc
                .try_replace_all_checked(&[("ACME", value, None)], CHECKED)
                .unwrap()
                .unwrap_or_else(|refusal| panic!("{producer} {value:?}: {refusal}"));
            assert_eq!(counts, [2], "{producer} {value:?}");
            assert_eq!(
                paragraph_starting(&doc, "Call"),
                "Call ACME today, ACME".replace("ACME", value),
                "{producer} {value:?}"
            );
        }
    }
}
