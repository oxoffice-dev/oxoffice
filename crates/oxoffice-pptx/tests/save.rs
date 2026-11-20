//! `Presentation::save` refuses to replace the file the presentation was
//! opened from, whatever the spelling of the path and after any edit, unless
//! asked, and saves anywhere else as before.

use std::fs;
use std::path::{Path, PathBuf};

use oxoffice_pptx::{Error, Presentation, SaveOptions};

/// A folder of its own under the system temporary folder, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("oxoffice-pptx-save-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch folder");
        Self(dir)
    }

    /// A copy of the PowerPoint-saved deck of the corpus, named `name`.
    fn deck(&self, name: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/files/powerpoint/deck.pptx");
        let path = self.0.join(name);
        fs::copy(source, &path).expect("copy of the corpus deck");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_refused(prs: &Presentation, path: &Path) {
    let before = fs::read(path).expect("source is readable");
    match prs.save(path) {
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
    let path = scratch.deck("deck.pptx");
    let prs = Presentation::open(&path).unwrap();
    assert_refused(&prs, &path);
    let message = prs.save(&path).unwrap_err().to_string();
    assert!(message.contains("overwrite_source"), "{message}");
}

#[test]
fn every_spelling_of_the_source_is_refused() {
    let scratch = Scratch::new("spellings");
    fs::create_dir(scratch.0.join("sub")).unwrap();
    let path = scratch.deck("deck.pptx");
    let prs = Presentation::open(&path).unwrap();
    assert_refused(&prs, &scratch.0.join("./deck.pptx"));
    assert_refused(&prs, &scratch.0.join("sub/../deck.pptx"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&path, scratch.0.join("link.pptx")).unwrap();
        assert_refused(&prs, &scratch.0.join("link.pptx"));
        fs::hard_link(&path, scratch.0.join("hard.pptx")).unwrap();
        assert_refused(&prs, &scratch.0.join("hard.pptx"));
    }
}

#[test]
fn the_source_is_still_refused_after_edits() {
    let scratch = Scratch::new("edits");
    let path = scratch.deck("deck.pptx");
    let mut prs = Presentation::open(&path).unwrap();
    let last_layout = prs.layout_count() - 1;

    prs.add_slide(last_layout).unwrap();
    assert_refused(&prs, &path);
    prs.duplicate_slide(0).unwrap();
    assert_refused(&prs, &path);
    prs.move_slide(0, 1).unwrap();
    assert_refused(&prs, &path);
    prs.remove_slide(prs.len() - 1).unwrap();
    assert_refused(&prs, &path);
    prs.set_slide_number_start(Some(3)).unwrap();
    assert_refused(&prs, &path);
    prs.core_properties_mut().title = Some("A title".to_owned());
    assert_refused(&prs, &path);
    prs.replace_text("a", "a");
    assert_refused(&prs, &path);
    let clone = prs.clone();
    assert_refused(&clone, &path);
    prs.save(scratch.0.join("elsewhere.pptx")).unwrap();
    assert_refused(&prs, &path);
}

#[test]
fn overwrite_source_replaces_the_opened_file() {
    let scratch = Scratch::new("overwrite");
    let path = scratch.deck("deck.pptx");
    let mut prs = Presentation::open(&path).unwrap();
    let slides = prs.len();
    prs.duplicate_slide(0).unwrap();
    prs.save_with_options(
        &path,
        SaveOptions {
            overwrite_source: true,
        },
    )
    .unwrap();
    assert_eq!(Presentation::open(&path).unwrap().len(), slides + 1);
}

#[cfg(unix)]
#[test]
fn the_replaced_source_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("mode");
    let path = scratch.deck("deck.pptx");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let prs = Presentation::open(&path).unwrap();
    prs.save_with_options(
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
    let path = scratch.deck("deck.pptx");
    let before = fs::read(&path).unwrap();
    let mut prs = Presentation::open(&path).unwrap();
    let slides = prs.len();
    prs.duplicate_slide(0).unwrap();
    let copy = scratch.0.join("copy.pptx");
    prs.save(&copy).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(Presentation::open(&copy).unwrap().len(), slides + 1);
}

#[test]
fn a_presentation_from_bytes_has_no_source() {
    let scratch = Scratch::new("bytes");
    let path = scratch.deck("deck.pptx");
    let prs = Presentation::from_bytes(&fs::read(&path).unwrap()).unwrap();
    prs.save(&path).unwrap();
}
