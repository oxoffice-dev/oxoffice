//! Helpers shared by the modules of this binary.

use std::fs;
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use oxoffice_docx::Document;
use oxoffice_opc::OpcPackage;

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

/// Whether the workflow tests read every producer's copy (`main` and the
/// nightly run, `OXOFFICE_WORKFLOWS=full`) or one copy each (pull requests).
/// Read at run time, so that a build without the variable still compiles.
pub fn every_copy() -> bool {
    std::env::var("OXOFFICE_WORKFLOWS").is_ok_and(|value| value == "full")
}

/// The copies a workflow test reads: the first producer's on a pull request,
/// every producer's with [`every_copy`].
pub fn workflow_copies<'a>(producers: &[&'a str]) -> Vec<&'a str> {
    if every_copy() {
        producers.to_vec()
    } else {
        producers[..1].to_vec()
    }
}

/// Every part of a package by its ZIP entry name, with its bytes.
pub fn package_parts(bytes: &[u8]) -> std::collections::BTreeMap<String, Vec<u8>> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("package is a ZIP");
    (0..zip.len())
        .map(|index| {
            let mut entry = zip.by_index(index).expect("ZIP entry reads");
            let mut data = Vec::new();
            entry.read_to_end(&mut data).expect("ZIP entry inflates");
            (entry.name().expect("ZIP entry name").into_owned(), data)
        })
        .collect()
}
/// The issue a workflow test fails on for this corpus copy today, from
/// `tests/corpus/workflows-known.toml`.
pub fn workflow_known_failure(file: &str, workflow: &str) -> Option<u32> {
    let text = fs::read_to_string(corpus_root().join("workflows-known.toml"))
        .expect("workflows-known.toml is readable");
    let mut entries = Vec::new();
    let (mut path, mut name) = (None, None);
    for line in text.lines() {
        let value = |prefix: &str| {
            line.strip_prefix(prefix)
                .map(|v| v.trim().trim_matches('"').to_owned())
        };
        if let Some(v) = value("file = ") {
            path = Some(v);
        } else if let Some(v) = value("workflow = ") {
            name = Some(v);
        } else if let Some(v) = value("issue = ") {
            entries.push((
                path.take().unwrap(),
                name.take().unwrap(),
                v.parse::<u32>().unwrap(),
            ));
        }
    }
    entries
        .into_iter()
        .find(|(path, name, _)| path == file && name == workflow)
        .map(|(_, _, issue)| issue)
}

/// Runs `walk` on each copy: a copy listed in `workflows-known.toml` for
/// this workflow must fail it, any other must pass, so a fix shows.
pub fn run_workflow(workflow: &str, copies: &[String], walk: impl Fn(&str)) {
    for copy in copies {
        let outcome = catch_unwind(AssertUnwindSafe(|| walk(copy)));
        match (workflow_known_failure(copy, workflow), outcome) {
            (None, Err(panic)) => std::panic::resume_unwind(panic),
            (Some(issue), Ok(())) => panic!(
                "{copy} passes the {workflow} walk now: remove its line (#{issue}) from workflows-known.toml"
            ),
            _ => {}
        }
    }
}
