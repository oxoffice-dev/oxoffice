//! A part holding a character XML 1.0 cannot carry is refused, read or
//! written, with where the character is and what to remove.

use std::io::{Cursor, Write};

use oxoffice_opc::OpcPackage;

const PART: &str = "<?xml version=\"1.0\"?>\r\n<root>a\u{1}b</root>";
const REFUSAL: &str = "/data/part.xml holds U+0001 at line 2, column 8, a character XML 1.0 cannot carry, so no XML reader accepts that part: remove the character from the part, or from the text written into it";

#[test]
fn a_part_to_be_written_with_a_forbidden_character_is_refused() {
    let mut package = OpcPackage::new();
    package
        .content_types
        .add_override("/data/part.xml", "application/xml");
    package.set_part("/data/part.xml", PART.as_bytes().to_vec());
    let error = package.write_to(Cursor::new(Vec::new())).unwrap_err();
    assert_eq!(error.to_string(), REFUSAL);
}

#[test]
fn a_part_read_with_a_forbidden_character_is_refused() {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::DEFAULT;
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(
        br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/data/part.xml" ContentType="application/xml"/></Types>"#,
    )
    .unwrap();
    zip.start_file("data/part.xml", options).unwrap();
    zip.write_all(PART.as_bytes()).unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let error = OpcPackage::from_reader(Cursor::new(bytes)).err().unwrap();
    assert_eq!(error.to_string(), REFUSAL);
}
