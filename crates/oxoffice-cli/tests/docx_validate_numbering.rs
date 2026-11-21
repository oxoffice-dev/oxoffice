//! `oxoffice docx validate` warns about a paragraph numbered by a numbering
//! instance the document does not define, and still passes the file.

use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;

use oxoffice_opc::OpcPackage;

#[test]
fn validate_warns_about_a_dangling_numbering_reference_and_passes_the_file() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files/word/contract.docx");
    let mut package =
        OpcPackage::from_reader(Cursor::new(std::fs::read(source).unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let at = xml.find("<w:numId w:val=\"").expect("a numbered paragraph");
    let end = at + xml[at..].find("/>").unwrap() + 2;
    let xml = format!("{}<w:numId w:val=\"4242\"/>{}", &xml[..at], &xml[end..]);
    package.set_part("/word/document.xml", xml.into_bytes());
    let file = std::env::temp_dir().join(format!(
        "oxoffice-cli-dangling-numbering-{}.docx",
        std::process::id()
    ));
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    std::fs::write(&file, out.into_inner()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "validate"])
        .arg(&file)
        .output()
        .expect("oxoffice runs");
    std::fs::remove_file(&file).ok();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "a warning does not fail: {stdout}");
    assert_eq!(
        stdout
            .matches("is numbered by numbering instance 4242, which the document does not define")
            .count(),
        1,
        "{stdout}"
    );
}
