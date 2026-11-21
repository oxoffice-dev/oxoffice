//! Helpers shared by the modules of this binary.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use oxoffice_docx::Document;
use oxoffice_opc::OpcPackage;

/// Every file of the corpus (`tests/corpus/files`) with this extension,
/// sorted.
pub fn corpus_files(extension: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("corpus folder is readable") {
            let path = entry.expect("corpus entry is readable").path();
            if path.is_dir() {
                walk(&path, extension, out);
            } else if path.extension().is_some_and(|ext| ext == extension) {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files");
    let mut files = Vec::new();
    walk(&root, extension, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no .{extension} file in {}",
        root.display()
    );
    files
}

/// A corpus document, by its path under `tests/corpus/files`:
/// `corpus_doc("word/report.docx")`.
pub fn corpus_doc(name: &str) -> Document {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(name);
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Document::from_bytes(&bytes).unwrap_or_else(|e| panic!("{name} opens: {e}"))
}

/// The document after a save and a reopen, as the next session sees it.
pub fn reopen(doc: &mut Document) -> Document {
    let bytes = doc.to_bytes().expect("document saves");
    Document::from_bytes(&bytes).expect("saved document reopens")
}

/// A new document holding these paragraphs, in order.
pub fn doc_with(paragraphs: &[&str]) -> Document {
    let mut doc = Document::new();
    for text in paragraphs {
        doc.add_paragraph(text);
    }
    doc
}

/// The texts of the body paragraphs, in order.
pub fn texts(doc: &Document) -> Vec<String> {
    doc.paragraphs().iter().map(|p| p.text()).collect()
}

/// A new document whose body holds this raw `w:` XML, before the section
/// properties: markup as Word, Google Docs or LibreOffice write it, which
/// the facade would not write itself.
pub fn doc_with_body(body: &str) -> Document {
    let bytes = Document::new().to_bytes().expect("new document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let part = package.main_document_part().expect("main document part");
    let xml = String::from_utf8(package.get_part(&part).expect("main part").to_vec())
        .expect("main part is UTF-8");
    let start = xml.find("<w:body>").expect("body start") + "<w:body>".len();
    let end = xml
        .find("<w:sectPr")
        .or_else(|| xml.find("</w:body>"))
        .expect("body end");
    let xml = format!("{}{body}{}", &xml[..start], &xml[end..]);
    package.set_part(&part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document with this body opens")
}
