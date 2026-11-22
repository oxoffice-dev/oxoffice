//! `Document::try_replace_all_checked` refuses a batch that leaves a pair's
//! text out of its replacement's reach, names where it is left, and publishes
//! it with `allow_unreached`, on the Word and Google Docs files of the corpus.

use std::io::Cursor;
use std::path::Path;

use oxoffice_docx::{Document, ReplaceOptions, ReplacementRefusal, UnreachedReplacement};
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
