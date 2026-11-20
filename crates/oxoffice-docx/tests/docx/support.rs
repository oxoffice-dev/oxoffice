//! Helpers shared by the modules of this binary.

use std::fs;
use std::path::{Path, PathBuf};

use oxoffice_docx::Document;

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
