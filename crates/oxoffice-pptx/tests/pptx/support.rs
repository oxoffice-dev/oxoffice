//! Helpers shared by the modules of this binary.

use std::fs;
use std::path::{Path, PathBuf};

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
