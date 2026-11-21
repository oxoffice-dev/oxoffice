//! `Document::save` refuses to replace the file the document was opened from,
//! whatever the spelling of the path and after any edit, unless asked, and
//! saves anywhere else as before.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use oxoffice_docx::{Document, Error, SaveOptions};

/// A folder of its own under the system temporary folder, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("oxoffice-docx-save-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch folder");
        Self(dir)
    }

    /// A copy of the Word-saved report of the corpus, named `name`.
    fn report(&self, name: &str) -> PathBuf {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files/word/report.docx");
        let path = self.0.join(name);
        fs::copy(source, &path).expect("copy of the corpus report");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_refused(doc: &mut Document, path: &Path) {
    let before = fs::read(path).expect("source is readable");
    match doc.save(path) {
        Err(Error::OverwritesSource { .. }) => {}
        other => panic!("{}: expected a refusal, got {other:?}", path.display()),
    }
    assert_eq!(
        fs::read(path).expect("source is readable"),
        before,
        "a refused save left {} as it was",
        path.display()
    );
}

#[test]
fn saving_over_the_opened_file_is_refused() {
    let scratch = Scratch::new("refused");
    let path = scratch.report("report.docx");
    let mut doc = Document::open(&path).unwrap();
    doc.add_paragraph("added");
    assert_refused(&mut doc, &path);
    let message = doc.save(&path).unwrap_err().to_string();
    assert!(message.contains("overwrite_source"), "{message}");
}

#[test]
fn every_spelling_of_the_source_is_refused() {
    let scratch = Scratch::new("spellings");
    fs::create_dir(scratch.0.join("sub")).unwrap();
    let path = scratch.report("report.docx");
    let mut doc = Document::open(&path).unwrap();
    assert_refused(&mut doc, &scratch.0.join("./report.docx"));
    assert_refused(&mut doc, &scratch.0.join("sub/../report.docx"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&path, scratch.0.join("link.docx")).unwrap();
        assert_refused(&mut doc, &scratch.0.join("link.docx"));
        fs::hard_link(&path, scratch.0.join("hard.docx")).unwrap();
        assert_refused(&mut doc, &scratch.0.join("hard.docx"));
    }
}

#[cfg(unix)]
#[test]
fn a_document_opened_through_a_link_refuses_the_file_it_names() {
    let scratch = Scratch::new("opened-link");
    let path = scratch.report("report.docx");
    let link = scratch.0.join("link.docx");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let mut doc = Document::open(&link).unwrap();
    assert_refused(&mut doc, &path);
    assert_refused(&mut doc, &link);
}

#[test]
fn the_source_is_still_refused_after_edits() {
    let scratch = Scratch::new("edits");
    let path = scratch.report("report.docx");
    let mut doc = Document::open(&path).unwrap();
    let word = doc
        .text()
        .split_whitespace()
        .find(|word| word.len() > 3 && word.chars().all(char::is_alphabetic))
        .expect("a word of the report")
        .to_owned();

    doc.add_paragraph("added paragraph");
    assert_refused(&mut doc, &path);
    doc.set_title("A title");
    assert_refused(&mut doc, &path);
    doc.add_table(2, 2);
    assert_refused(&mut doc, &path);
    doc.add_comment_on_text(
        &word,
        0,
        "Reviewer",
        None,
        "A comment",
        oxoffice_docx::CommentDate::At("2026-11-20T00:00:00Z"),
    )
    .unwrap();
    assert_refused(&mut doc, &path);
    assert_eq!(
        doc.replace_all(&HashMap::from([("added paragraph", "changed paragraph")])),
        1
    );
    assert_refused(&mut doc, &path);
    doc.to_bytes().unwrap();
    assert_refused(&mut doc, &path);
    doc.save(scratch.0.join("elsewhere.docx")).unwrap();
    assert_refused(&mut doc, &path);
}

#[test]
fn overwrite_source_replaces_the_opened_file() {
    let scratch = Scratch::new("overwrite");
    let path = scratch.report("report.docx");
    let mut doc = Document::open(&path).unwrap();
    doc.add_paragraph("written over the source");
    doc.save_with_options(
        &path,
        SaveOptions {
            overwrite_source: true,
        },
    )
    .unwrap();
    let reopened = Document::open(&path).unwrap();
    assert!(reopened.text().contains("written over the source"));
}

#[cfg(unix)]
#[test]
fn the_replaced_source_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("mode");
    let path = scratch.report("report.docx");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let mut doc = Document::open(&path).unwrap();
    doc.save_with_options(
        &path,
        SaveOptions {
            overwrite_source: true,
        },
    )
    .unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

#[test]
fn saving_elsewhere_leaves_the_source_as_it_was() {
    let scratch = Scratch::new("elsewhere");
    let path = scratch.report("report.docx");
    let before = fs::read(&path).unwrap();
    let mut doc = Document::open(&path).unwrap();
    doc.add_paragraph("only in the copy");
    let copy = scratch.0.join("copy.docx");
    doc.save(&copy).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(
        Document::open(&copy)
            .unwrap()
            .text()
            .contains("only in the copy")
    );
}

#[test]
fn a_document_from_bytes_or_new_has_no_source() {
    let scratch = Scratch::new("bytes");
    let path = scratch.report("report.docx");
    let mut doc = Document::from_bytes(&fs::read(&path).unwrap()).unwrap();
    doc.add_paragraph("from bytes");
    doc.save(&path).unwrap();
    let mut new = Document::new();
    new.add_paragraph("new");
    new.save(&path).unwrap();
}
