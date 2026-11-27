//! A file that is not a workbook is told by its content, never its
//! extension, and refused with a plain message naming the way out.

use std::io::{Cursor, Write};

use oxoffice_opc::OpcPackage;
use oxoffice_opc::content_types::{PRESENTATION, WORD_DOCUMENT};
use oxoffice_xlsx::Workbook;
use oxoffice_xlsx_oxml::namespaces::content_types::BINARY_WORKBOOK;

use crate::support::zip_of;

/// A compound file holding one stream named `stream`, as Office writes a
/// legacy file or a password-protected package.
fn compound_file(stream: &str) -> Vec<u8> {
    let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).expect("compound file");
    file.create_stream(format!("/{stream}"))
        .expect("stream")
        .write_all(&[0; 4096])
        .expect("stream bytes");
    file.flush().expect("flushed");
    file.into_inner().into_inner()
}

/// A package whose main part has `content_type`.
fn package(main: &str, content_type: &str) -> Vec<u8> {
    let mut package = OpcPackage::with_main_part(main, content_type);
    package.set_part(main, b"<x/>".to_vec());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).expect("package");
    bytes.into_inner()
}

#[test]
fn what_is_not_a_workbook_is_refused_by_its_content() {
    let cases: [(&str, Vec<u8>, &str); 10] = [
        (
            "password-protected",
            compound_file("EncryptionInfo"),
            "the workbook is password-protected: open it with a password",
        ),
        (
            "xls",
            compound_file("Workbook"),
            "a legacy Excel 97-2003 workbook (.xls): save it as .xlsx in Excel or LibreOffice first",
        ),
        (
            "Excel 5 xls",
            compound_file("Book"),
            "a legacy Excel 97-2003 workbook (.xls): save it as .xlsx in Excel or LibreOffice first",
        ),
        (
            "doc",
            compound_file("WordDocument"),
            "a legacy Word 97-2003 document (.doc), not a workbook",
        ),
        (
            "ppt",
            compound_file("PowerPoint Document"),
            "a legacy PowerPoint 97-2003 presentation (.ppt), not a workbook",
        ),
        (
            "another compound file",
            compound_file("Contents"),
            "a compound file that holds no workbook",
        ),
        (
            "xlsb",
            package("/xl/workbook.bin", BINARY_WORKBOOK),
            "a binary workbook (.xlsb): save it as .xlsx first",
        ),
        (
            "ods",
            zip_of(&[
                (
                    "mimetype",
                    b"application/vnd.oasis.opendocument.spreadsheet",
                ),
                ("content.xml", b"<office:document-content/>"),
            ]),
            "an OpenDocument spreadsheet (.ods): save it as .xlsx first",
        ),
        (
            "docx",
            package("/word/document.xml", WORD_DOCUMENT),
            "a Word document, not a workbook: open it with oxoffice docx",
        ),
        (
            "pptx",
            package("/ppt/presentation.xml", PRESENTATION),
            "a PowerPoint presentation, not a workbook: open it with oxoffice pptx",
        ),
    ];
    for (case, bytes, message) in cases {
        match Workbook::from_bytes(&bytes) {
            Err(error) => assert_eq!(error.to_string(), message, "{case}"),
            Ok(_) => panic!("{case}: opened"),
        }
    }
}
