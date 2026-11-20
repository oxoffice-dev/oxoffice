//! `oxoffice docx replace` and `oxoffice pptx replace` refuse to write over
//! their input file and leave it as it was.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_of(corpus: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oxoffice-cli-output-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch folder");
    let path = dir.join(name);
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/files")
            .join(corpus),
        &path,
    )
    .expect("copy of a corpus file");
    path
}

fn assert_replace_refuses_its_input(family: &str, path: &Path) {
    let before = fs::read(path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args([family, "replace"])
        .arg(path)
        .args(["-p", "a", "-v", "b", "-o"])
        .arg(path)
        .output()
        .expect("oxoffice runs");
    assert!(
        !output.status.success(),
        "{family} replace wrote over its input"
    );
    assert_eq!(fs::read(path).unwrap(), before, "{family}: input changed");
}

#[test]
fn replace_refuses_to_write_over_its_input() {
    let report = copy_of("word/report.docx", "report.docx");
    assert_replace_refuses_its_input("docx", &report);
    assert_replace_refuses_its_input("pptx", &copy_of("powerpoint/deck.pptx", "deck.pptx"));
    let _ = fs::remove_dir_all(report.parent().unwrap());
}
