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

fn unreached(
    result: oxoffice_docx::Result<Result<Vec<usize>, ReplacementRefusal>>,
) -> UnreachedReplacement {
    match result.unwrap() {
        Err(ReplacementRefusal::Unreached(unreached)) => unreached,
        other => panic!("expected an unreached refusal, got {other:?}"),
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

// ---- A replacement scoped to one paragraph ----

/// The bytes of `part` in this package.
fn package_part(bytes: &[u8], part: &str) -> String {
    let parts = crate::support::package_parts(bytes);
    String::from_utf8(parts[part.trim_start_matches('/')].clone()).expect("the part is UTF-8")
}

/// The paragraph of `xml` that holds `text`: from its start tag to its end.
fn paragraph_holding<'a>(xml: &'a str, text: &str) -> &'a str {
    let at = xml
        .find(text)
        .unwrap_or_else(|| panic!("{text} is in the part"));
    let start = xml[..at]
        .rmatch_indices("<w:p")
        .find(|(index, _)| matches!(xml.as_bytes()[index + 4], b' ' | b'>'))
        .map(|(index, _)| index)
        .expect("a paragraph holds the text");
    let end = xml[at..].find("</w:p>").expect("the paragraph ends") + at + "</w:p>".len();
    &xml[start..end]
}

#[test]
fn a_paragraph_edited_through_its_location_is_written_back_without_declarations_of_its_own() {
    // An edit reads the paragraph with the bindings in scope declared on it;
    // the part takes the paragraph back with the start tag it had and no
    // namespace declared in it that it did not declare: a word replaced in
    // it, and a run added to it (as the bindings add one to a paragraph of
    // another story). Comment marks, Google's attributes on the paragraph,
    // a header.
    for (copy, kind, word) in [
        ("authored/contract.docx", StoryKind::Body, "Each"),
        ("google/contract.docx", StoryKind::Body, "Each"),
        ("libreoffice/record.docx", StoryKind::Header, "Change"),
    ] {
        let source = std::fs::read(crate::support::corpus_root().join("files").join(copy))
            .expect("the corpus copy reads");
        let doc = Document::from_bytes(&source).expect("the copy opens");
        let paragraph = doc
            .story_paragraphs()
            .expect("story paragraphs")
            .into_iter()
            .find(|paragraph| {
                paragraph.location.story().kind() == kind
                    && paragraph
                        .text
                        .split(|c: char| !c.is_alphabetic())
                        .any(|w| w == word)
                    && paragraph.text.matches(word).count() == 1
            })
            .unwrap_or_else(|| panic!("{copy}: a {kind:?} paragraph holds {word}"));
        let part = paragraph.part_name().to_owned();
        let before = package_part(&source, &part);
        assert_eq!(
            before.matches(word).count(),
            1,
            "{copy}: {word} once in {part}"
        );
        let before = paragraph_holding(&before, word);
        let mut replaced = Document::from_bytes(&source).unwrap();
        assert_eq!(
            replaced
                .try_replace_text_at(&paragraph.location, word, "Zephyr", Some(1))
                .unwrap(),
            Ok(1),
            "{copy}"
        );
        let mut added = Document::from_bytes(&source).unwrap();
        added
            .edit_story_paragraph(&paragraph.location, |edited| {
                edited.add_run(" Zephyr.");
                Ok(())
            })
            .unwrap_or_else(|error| panic!("{copy}: {error}"));
        for (edit, mut doc) in [("replaced", replaced), ("added", added)] {
            let after = package_part(&doc.to_bytes().unwrap(), &part);
            let after = paragraph_holding(&after, "Zephyr");
            let start_tag = |paragraph: &str| paragraph[..=paragraph.find('>').unwrap()].to_owned();
            assert_eq!(start_tag(after), start_tag(before), "{copy} {part} {edit}");
            assert_eq!(
                after.matches("xmlns").count(),
                before.matches("xmlns").count(),
                "{copy} {part} {edit}: {after}"
            );
        }
    }
}

#[test]
fn a_replacement_in_one_paragraph_names_the_text_it_cannot_reach() {
    // The letter's country is a merge field's result: the paragraph shows
    // it, the replacement does not reach it, and says so.
    let doc = crate::support::corpus_doc("word/letter.docx");
    let paragraph = doc
        .story_paragraphs()
        .unwrap()
        .into_iter()
        .find(|paragraph| paragraph.text.contains("Country"))
        .expect("a paragraph shows the merge field");
    let mut doc = crate::support::corpus_doc("word/letter.docx");
    let mismatch = doc
        .try_replace_text_at(&paragraph.location, "Country", "Nation", Some(1))
        .unwrap()
        .unwrap_err();
    assert_eq!((mismatch.found, mismatch.unreached), (0, 1));
    assert!(
        mismatch.to_string().contains("1 it cannot reach"),
        "{mismatch}"
    );
    // A cell's text is no part of the first body paragraph, though both
    // stories are numbered 0 of their kind.
    let mut doc = crate::support::doc_with_body(concat!(
        "<w:p><w:r><w:t>Hello there.</w:t></w:r></w:p>",
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>Target cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        "<w:p/>"
    ));
    let location = doc.story_paragraphs().unwrap()[0].location.clone();
    let mismatch = doc
        .try_replace_text_at(&location, "Target", "Goal", Some(1))
        .unwrap()
        .unwrap_err();
    assert_eq!((mismatch.found, mismatch.unreached), (0, 0));
}

#[test]
fn every_route_that_writes_an_item_back_declares_no_namespace_in_it() {
    // A run added and a word replaced in a text box, written twice (its
    // choice and its fallback), and a field inserted at a run: no edited
    // paragraph declares what the part binds.
    let body = format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Before </w:t></w:r>{}</w:p><w:p><w:r><w:t>Plain words.</w:t></w:r></w:p>"#,
        crate::support::text_box_run(7, "<w:p><w:r><w:t>Inside the box</w:t></w:r></w:p>")
    );
    let at = |doc: &Document, text: &str| {
        doc.story_paragraphs()
            .unwrap()
            .into_iter()
            .find(|paragraph| paragraph.text == text)
            .unwrap()
            .location
    };
    type Edit = fn(&mut Document, oxoffice_docx::ContentLocation, oxoffice_docx::ContentLocation);
    let routes: [(&str, Edit); 3] = [
        ("run added in a text box", |doc, boxed, _| {
            doc.edit_story_paragraph(&boxed, |paragraph| {
                paragraph.add_run(" Zephyr.");
                Ok(())
            })
            .unwrap();
        }),
        ("word replaced in a text box", |doc, boxed, _| {
            assert_eq!(
                doc.try_replace_text_at(&boxed, "Inside", "Within", Some(1))
                    .unwrap(),
                Ok(1)
            );
        }),
        ("field inserted", |doc, _, plain| {
            let position = oxoffice_docx::StoryRunPosition {
                location: plain,
                run_index: 1,
            };
            doc.insert_field(&position, oxoffice_docx::Field::new("PAGE", "1"))
                .unwrap();
        }),
    ];
    let declaring = routes
        .into_iter()
        .filter(|(_, edit)| {
            let mut doc = crate::support::doc_with_body(&body);
            let (boxed, plain) = (at(&doc, "Inside the box"), at(&doc, "Plain words."));
            edit(&mut doc, boxed, plain);
            package_part(&doc.to_bytes().unwrap(), "/word/document.xml").contains("<w:p xmlns")
        })
        .map(|(route, _)| route)
        .collect::<Vec<_>>();
    assert!(declaring.is_empty(), "{declaring:?}");
}
