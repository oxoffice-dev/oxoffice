//! Helpers shared by the modules of this binary.

use std::fs;
use std::path::{Path, PathBuf};

use oxoffice_pptx::Presentation;

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

/// A corpus deck, by its path under `tests/corpus/files`:
/// `corpus_deck("powerpoint/deck.pptx")`.
pub fn corpus_deck(name: &str) -> Presentation {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(name);
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Presentation::from_bytes(&bytes).unwrap_or_else(|e| panic!("{name} opens: {e}"))
}

/// The deck after a save and a reopen, as the next session sees it.
pub fn reopen(prs: &Presentation) -> Presentation {
    let bytes = prs.to_bytes().expect("deck saves");
    Presentation::from_bytes(&bytes).expect("saved deck reopens")
}

/// A new deck from the default template, with no slide.
pub fn new_deck() -> Presentation {
    Presentation::new().expect("the default template opens")
}

/// The visible text of every slide, in order.
pub fn slide_texts(prs: &Presentation) -> Vec<String> {
    prs.slides().map(|slide| slide.text()).collect()
}
