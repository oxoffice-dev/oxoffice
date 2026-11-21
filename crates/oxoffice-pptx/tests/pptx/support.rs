//! Helpers shared by the modules of this binary.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use oxoffice_pptx::Presentation;

/// The corpus folder, `tests/corpus` at the repository root.
pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus")
}

/// One file of the corpus manifest, `tests/corpus/manifest.toml`.
pub struct CorpusFile {
    /// Path under `tests/corpus/files`, such as `word/report.docx`.
    pub path: String,
    /// The issue that makes a save without an edit rewrite some parts of
    /// this file today.
    pub known_failure: Option<u32>,
    /// The parts that known failure rewrites, by ZIP entry name.
    pub rewritten: Vec<String>,
}

impl CorpusFile {
    pub fn bytes(&self) -> Vec<u8> {
        let path = corpus_root().join("files").join(&self.path);
        fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }
}

/// Every entry of the corpus manifest, in its order. The manifest is written
/// by `tests/corpus/generate/manifest.py` with one `key = value` per line, so
/// the keys the tests need are read line by line.
pub fn corpus_manifest() -> Vec<CorpusFile> {
    let text =
        fs::read_to_string(corpus_root().join("manifest.toml")).expect("manifest is readable");
    let mut files: Vec<CorpusFile> = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("path = ") {
            files.push(CorpusFile {
                path: path.trim_matches('"').to_owned(),
                known_failure: None,
                rewritten: Vec::new(),
            });
        } else if let Some(parts) = line.strip_prefix("rewritten = ") {
            let file = files.last_mut().expect("rewritten follows a path");
            file.rewritten = parts
                .trim_matches(['[', ']'])
                .split(',')
                .map(|part| part.trim().trim_matches('"').to_owned())
                .collect();
        } else if let Some(issue) = line.strip_prefix("known_failure = ") {
            let file = files.last_mut().expect("known_failure follows a path");
            file.known_failure = Some(
                issue
                    .trim()
                    .parse()
                    .expect("known_failure is an issue number"),
            );
        }
    }
    assert!(!files.is_empty(), "the corpus manifest lists no file");
    files
}

/// The manifest entries of the files with this extension.
pub fn corpus_files(extension: &str) -> Vec<CorpusFile> {
    corpus_manifest()
        .into_iter()
        .filter(|file| file.path.ends_with(&format!(".{extension}")))
        .collect()
}

/// `f` over every item, on as many threads as the machine has, each with the
/// 8 MiB stack the test threads get, the results in item order. A panic in
/// `f` stays on its item: it comes back as `None`.
pub fn map_in_parallel<T: Sync, R: Send>(
    items: &[T],
    f: impl Fn(&T) -> R + Sync,
) -> Vec<Option<R>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, Option<R>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.min(items.len()))
            .map(|_| {
                std::thread::Builder::new()
                    .stack_size(8 * 1024 * 1024)
                    .spawn_scoped(scope, || {
                        let mut done = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(item) = items.get(index) else {
                                break done;
                            };
                            done.push((index, catch_unwind(AssertUnwindSafe(|| f(item))).ok()));
                        }
                    })
                    .expect("worker thread starts")
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("worker thread ends"))
            .collect()
    });
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
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

/// `lines` equal `tests/snapshots/<name>.txt` of this crate, one line each.
/// A change shows as a diff of that file in the pull request that makes it;
/// `OXOFFICE_SNAPSHOTS_UPDATE=1` rewrites it, outside CI only. Line ends are
/// read either way: a Windows checkout may turn them into CRLF.
#[allow(dead_code)]
pub fn assert_snapshot(name: &str, lines: &[String]) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    let update = std::env::var("OXOFFICE_SNAPSHOTS_UPDATE").is_ok_and(|value| value == "1");
    assert!(
        !(update && std::env::var_os("CI").is_some()),
        "OXOFFICE_SNAPSHOTS_UPDATE is for a local run, never under CI"
    );
    if update {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>();
        fs::write(&path, text).unwrap();
        return;
    }
    let stored = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; write it with OXOFFICE_SNAPSHOTS_UPDATE=1",
            path.display()
        )
    });
    let stored = stored.lines().map(str::to_owned).collect::<Vec<_>>();
    assert!(
        stored == lines,
        "{} differs (rewrite it with OXOFFICE_SNAPSHOTS_UPDATE=1 if the change is meant):\n{}",
        path.display(),
        stored
            .iter()
            .zip(lines)
            .filter(|(old, new)| old != new)
            .map(|(old, new)| format!("- {old}\n+ {new}"))
            .chain(
                (stored.len() != lines.len())
                    .then(|| { format!("{} lines stored, {} now", stored.len(), lines.len()) })
            )
            .collect::<Vec<_>>()
            .join("\n")
    );
}
