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

/// The name the workflow tests give [`small_thesis`].
pub const SMALL_THESIS: &str = "small-thesis";

/// A few pages with the landmarks of the corpus thesis that the thesis walks
/// read: Word's TOC field over levels 1 to 3 in a content control, with
/// entries whose pages are stale, chapters starting a page (so their entries
/// read 2, 3 and 4), a running header naming the degree, a "Page X of Y"
/// footer, a discussion section of two paragraphs and a last section. A
/// thesis copy costs minutes of layout in a debug build, so pull requests
/// walk this one.
pub fn small_thesis() -> Vec<u8> {
    let run = |text: &str| format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#);
    let field = |instruction: &str, result: &str| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
            run(result)
        )
    };
    let body_text = "Each site keeps a safety stock sized from the forecast of the coming weeks.";
    let (mut entries, mut body) = (Vec::new(), String::new());
    for (chapter, sections) in [
        ("Introduction", &["Motivation", "Research questions"][..]),
        ("Results", &["Discussion", "Threats to validity"]),
        ("Conclusion", &["Future work"]),
    ] {
        for (level, title) in std::iter::once((1, chapter)).chain(sections.iter().map(|s| (2, *s)))
        {
            let id = entries.len() + 1;
            let name = format!("_Toc21900{id:04}");
            let page_break = if level == 1 {
                "<w:pageBreakBefore/>"
            } else {
                ""
            };
            body += &format!(
                r#"<w:p><w:pPr><w:pStyle w:val="Heading{level}"/>{page_break}</w:pPr><w:bookmarkStart w:id="{id}" w:name="{name}"/>{}<w:bookmarkEnd w:id="{id}"/></w:p>"#,
                run(title)
            );
            entries.push((level, title, name));
            let texts = if level == 1 {
                vec![body_text.to_owned()]
            } else {
                vec![
                    format!("{title} first: {body_text}"),
                    format!("{title} second: {body_text}"),
                ]
            };
            for text in texts {
                body += &format!("<w:p>{}</w:p>", run(&text));
            }
        }
    }
    // Every entry says page 9 until a pass gives it its page.
    let mut toc = String::new();
    for (index, (level, title, name)) in entries.iter().enumerate() {
        let opening = if index == 0 {
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#
        } else {
            ""
        };
        toc += &format!(
            r#"<w:p><w:pPr><w:pStyle w:val="TOC{level}"/><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9026"/></w:tabs></w:pPr>{opening}<w:hyperlink w:anchor="{name}" w:history="1">{}<w:r><w:tab/></w:r>{}</w:hyperlink></w:p>"#,
            run(title),
            field(&format!(r"PAGEREF {name} \h"), "9")
        );
    }
    toc += r#"<w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    let mut doc = doc_with_body(&format!(
        r#"<w:sdt><w:sdtPr><w:id w:val="219000001"/><w:docPartObj><w:docPartGallery w:val="Table of Contents"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent><w:p><w:pPr><w:pStyle w:val="Heading1"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="0"/></w:numPr></w:pPr>{}</w:p>{toc}</w:sdtContent></w:sdt>{body}"#,
        run("Contents")
    ));
    doc.set_header("Sophie Laurent - MSc thesis");
    let footer = format!(
        r#"<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:jc w:val="right"/></w:pPr>{}{}{}{}</w:p></w:ftr>"#,
        run("Page "),
        field("PAGE", "1"),
        run(" of "),
        field("NUMPAGES", "1")
    );
    doc.set_raw_footer_with_images(footer.into_bytes(), &[], oxoffice_docx::HdrFtrType::Default);
    doc.to_bytes().expect("the small thesis saves")
}

/// The thesis copies a workflow test walks: the small thesis on a pull
/// request, and every producer's corpus copy too with [`every_copy`].
pub fn thesis_copies() -> Vec<String> {
    let mut copies = every_thesis_copy();
    if !every_copy() {
        copies.truncate(1);
    }
    copies
}

/// The copies the page-number pass walks: the small thesis on a pull
/// request, every thesis and record copy with [`every_copy`].
pub fn page_numbers_copies() -> Vec<String> {
    if every_copy() {
        every_page_numbers_copy()
    } else {
        thesis_copies()
    }
}

/// The small thesis, then every producer's corpus copy of the thesis.
pub fn every_thesis_copy() -> Vec<String> {
    std::iter::once(SMALL_THESIS.to_owned())
        .chain(
            ["word", "libreoffice", "authored", "google"]
                .map(|producer| format!("{producer}/thesis.docx")),
        )
        .collect()
}

/// Whether a workflow walk renders this copy (a PDF, page images): the
/// small thesis and the Word copy. A render of a thesis copy costs a layout
/// of a few dozen pages in the `ci` profile, and every corpus file's render is
/// checked as saved by the render tests; the other copies walk the edits,
/// comparisons and page fields only.
pub fn walk_renders(copy: &str) -> bool {
    copy == SMALL_THESIS || copy.starts_with("word/")
}

/// Every producer's corpus copy of the record, the co-edited one first.
pub fn every_record_copy() -> Vec<String> {
    ["word-google", "word", "libreoffice", "authored"]
        .map(|producer| format!("{producer}/record.docx"))
        .to_vec()
}

/// The bytes of a workflow copy: the small thesis, or a corpus file.
pub fn workflow_source(copy: &str) -> Vec<u8> {
    if copy == SMALL_THESIS {
        small_thesis()
    } else {
        fs::read(corpus_root().join("files").join(copy)).expect("the corpus copy is readable")
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
/// A workflow's name and the copies it walks with `OXOFFICE_WORKFLOWS=full`.
pub type Workflow = (&'static str, fn() -> Vec<String>);

/// The workflows the tests walk, with the copies each walks under
/// `OXOFFICE_WORKFLOWS=full`: an entry of `workflows-known.toml` must name
/// one of them and one of its copies, so that a renamed walk or a dropped
/// copy shows.
pub const WORKFLOWS: [Workflow; 6] = [
    ("page-numbers", every_page_numbers_copy),
    ("redline", every_thesis_copy),
    ("redline-rebuilt-toc", every_thesis_copy),
    ("record-edit", every_record_copy),
    ("record-review", every_record_copy),
    ("restyle", every_record_copy),
];

/// The page-number pass walks the thesis and the record.
fn every_page_numbers_copy() -> Vec<String> {
    let mut copies = every_thesis_copy();
    copies.extend(every_record_copy());
    copies
}

/// A corpus copy a workflow test fails on today, from
/// `tests/corpus/workflows-known.toml`.
pub struct WorkflowKnownFailure {
    pub issue: u32,
    /// Text the failure message holds, so that another failure still shows.
    pub message: String,
}

/// Every entry of `workflows-known.toml`: (file, workflow, known failure).
pub fn workflow_known_failures() -> Vec<(String, String, WorkflowKnownFailure)> {
    let text = fs::read_to_string(corpus_root().join("workflows-known.toml"))
        .expect("workflows-known.toml is readable");
    let mut entries = Vec::new();
    for entry in text.split("[[known]]").skip(1) {
        let value = |key: &str| {
            entry
                .lines()
                .find_map(|line| {
                    line.strip_prefix(key)
                        .and_then(|rest| rest.trim_start().strip_prefix('='))
                        .map(|v| v.trim().trim_matches('"').replace("\\\"", "\""))
                })
                .unwrap_or_else(|| panic!("an entry of workflows-known.toml has its {key}"))
        };
        let (path, name) = (value("file"), value("workflow"));
        let issue = value("issue").parse::<u32>().expect("an issue number");
        assert!(
            entries
                .iter()
                .all(|(file, workflow, _)| (file, workflow) != (&path, &name)),
            "{path} is listed twice for {name}"
        );
        entries.push((
            path,
            name,
            WorkflowKnownFailure {
                issue,
                message: value("message"),
            },
        ));
    }
    entries
}

/// The known failure of this workflow on this copy, if it is listed.
pub fn workflow_known_failure(file: &str, workflow: &str) -> Option<WorkflowKnownFailure> {
    workflow_known_failures()
        .into_iter()
        .find(|(path, name, _)| path == file && name == workflow)
        .map(|(_, _, known)| known)
}

/// Runs `walk` on each copy: a copy listed in `workflows-known.toml` for
/// this workflow must fail it with the listed message, and any other must
/// pass, so a fix shows.
pub fn run_workflow(workflow: &str, copies: &[String], walk: impl Fn(&str) + Sync) {
    // The copies walk on threads of their own: a thesis copy takes minutes in
    // the `ci` profile, and one after another they would set the time of the
    // whole binary on `main`. Each outcome keeps its panic for the checks below;
    // every copy walks to its end, and the first mismatch in copy order fails.
    let outcomes = std::thread::scope(|scope| {
        let walk = &walk;
        let threads = copies
            .iter()
            .map(|copy| {
                std::thread::Builder::new()
                    .name(format!("{workflow} {copy}"))
                    .stack_size(8 * 1024 * 1024)
                    .spawn_scoped(scope, move || catch_unwind(AssertUnwindSafe(|| walk(copy))))
                    .expect("a walk thread starts")
            })
            .collect::<Vec<_>>();
        threads
            .into_iter()
            .map(|thread| thread.join().expect("a walk thread ends"))
            .collect::<Vec<_>>()
    });
    for (copy, outcome) in copies.iter().zip(outcomes) {
        match (workflow_known_failure(copy, workflow), outcome) {
            (None, Err(panic)) => std::panic::resume_unwind(panic),
            (Some(known), Ok(())) => panic!(
                "{copy} passes the {workflow} walk now: remove its line (#{}) from workflows-known.toml",
                known.issue
            ),
            (Some(known), Err(panic)) => {
                let text = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or_default();
                assert!(
                    text.contains(&known.message),
                    "{copy} fails the {workflow} walk, but not on #{} ({:?}): {text}",
                    known.issue,
                    known.message
                );
            }
            (None, Ok(())) => {}
        }
    }
}
