//! Helpers shared by the modules of this binary.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxoffice_opc::OpcPackage;
use oxoffice_xlsx_oxml::Element;
use oxoffice_xlsx_oxml::namespaces::content_types;
use oxoffice_xlsx_oxml::shared_strings::SharedStrings;
use oxoffice_xlsx_oxml::styles::StylesXml;
use oxoffice_xlsx_oxml::workbook::WorkbookXml;
use oxoffice_xlsx_oxml::worksheet::{CT_Row, PageSetupElements, SheetEdits, SheetIndex, splice};

/// The corpus folder, `tests/corpus` at the repository root.
pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus")
}

/// One file of the corpus manifest, `tests/corpus/manifest.toml`.
pub struct CorpusFile {
    /// Path under `tests/corpus/files`, such as `excel/review.xlsx`.
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

/// The workbooks of the corpus manifest, in its order. The manifest is
/// written by `tests/corpus/generate/manifest.py` with one `key = value` per
/// line, so the keys the tests need are read line by line.
pub fn corpus_workbooks() -> Vec<CorpusFile> {
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
            file.known_failure = Some(issue.trim().parse().expect("an issue number"));
        }
    }
    files.retain(|file| file.path.ends_with(".xlsx"));
    assert!(!files.is_empty(), "the corpus manifest lists no workbook");
    files
}

/// A corpus file by its path under `tests/corpus/files`.
pub fn corpus_bytes(path: &str) -> Vec<u8> {
    fs::read(corpus_root().join("files").join(path)).expect("the corpus file is readable")
}

/// `f` over every item, on as many threads as the machine has, each with an
/// 8 MiB stack, the results in item order. A panic stays on its item: it
/// comes back as `None`.
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

/// Every ZIP entry of a package, by name, as stored.
pub fn package_parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
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

/// A ZIP holding these entries, in order.
pub fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .expect("a ZIP entry starts");
        zip.write_all(data).expect("a ZIP entry is written");
    }
    zip.finish().expect("the ZIP ends").into_inner()
}

/// A typed part of a workbook: which model reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typed {
    Workbook,
    Worksheet,
    Styles,
    SharedStrings,
}

/// The typed parts of a package, by ZIP entry name, told by their content
/// types.
pub fn typed_parts(package: &OpcPackage) -> BTreeMap<String, Typed> {
    let main = [
        content_types::WORKBOOK,
        content_types::WORKBOOK_MACRO_ENABLED,
        content_types::TEMPLATE,
        content_types::TEMPLATE_MACRO_ENABLED,
    ];
    package
        .parts
        .keys()
        .filter_map(|name| {
            let content_type = package.content_types.content_type_for(name)?;
            let typed = match content_type {
                ct if main.contains(&ct) => Typed::Workbook,
                content_types::WORKSHEET => Typed::Worksheet,
                content_types::STYLES => Typed::Styles,
                content_types::SHARED_STRINGS => Typed::SharedStrings,
                _ => return None,
            };
            Some((name.trim_start_matches('/').to_owned(), typed))
        })
        .collect()
}

/// A worksheet's model: its children, each typed one read and the others
/// as written, and every row read.
#[derive(Debug, PartialEq)]
pub struct SheetModel {
    pub children: Vec<(String, Option<Vec<u8>>)>,
    pub page_setup: PageSetupElements,
    pub rows: Vec<CT_Row>,
    pub shared_formulas: Vec<String>,
    pub range_formulas: Vec<String>,
}

pub fn sheet_model(xml: &[u8]) -> Result<SheetModel, String> {
    let index = SheetIndex::build(xml).map_err(|error| error.to_string())?;
    let page_setup = PageSetupElements::read(xml, &index).map_err(|error| error.to_string())?;
    let typed = |name: &str| {
        name == "sheetData" || oxoffice_xlsx_oxml::worksheet::PAGE_SETUP_CHILDREN.contains(&name)
    };
    let rows = index
        .rows
        .iter()
        .map(|row| CT_Row::parse(&xml[row.range.clone()]))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(SheetModel {
        children: index
            .children
            .iter()
            .map(|child| {
                let bytes = (!typed(&child.name)).then(|| xml[child.range.clone()].to_vec());
                (child.name.clone(), bytes)
            })
            .collect(),
        page_setup,
        rows,
        shared_formulas: index
            .shared_formulas
            .values()
            .map(|f| format!("{f:?}"))
            .collect(),
        range_formulas: index
            .range_formulas
            .iter()
            .map(|f| format!("{f:?}"))
            .collect(),
    })
}

/// A workbook or styles part's model: its lists and single elements, and
/// the children it does not type as written.
pub fn list_part_model(part: &oxoffice_xlsx_oxml::TypedPart) -> String {
    let typed: Vec<String> = (part.lists().iter().map(|list| list.name().to_owned()))
        .chain(
            part.located_singles()
                .map(|single| single.value.tag.local_name()),
        )
        .collect();
    let mut model = String::new();
    for list in part.lists() {
        model.push_str(&format!(
            "{} {:?}\n",
            list.name(),
            list.iter().collect::<Vec<_>>()
        ));
    }
    for single in part.located_singles() {
        model.push_str(&format!("{:?}\n", single.value));
    }
    for (name, range) in part.children() {
        if !typed.contains(name) {
            model.push_str(&format!("{name} {:?}\n", &part.source()[range.clone()]));
        }
    }
    model
}

/// The model of a typed part, as text: what reading it gives.
pub fn part_model(typed: Typed, xml: &[u8]) -> Result<String, String> {
    let bytes = Arc::new(xml.to_vec());
    let error = |error: oxoffice_xlsx_oxml::OxmlError| error.to_string();
    Ok(match typed {
        Typed::Workbook => list_part_model(WorkbookXml::parse(bytes).map_err(error)?.part()),
        Typed::Styles => list_part_model(StylesXml::parse(bytes).map_err(error)?.part()),
        Typed::SharedStrings => {
            let strings = SharedStrings::parse(bytes).map_err(error)?;
            let items = (0..strings.len())
                .map(|index| strings.item(index).expect("an item").map_err(error))
                .collect::<Result<Vec<_>, _>>()?;
            format!("{items:?}")
        }
        Typed::Worksheet => format!("{:?}", sheet_model(xml)?),
    })
}

/// The part written again by its writers: every record, single element,
/// item, row and page setup element, through the writer a save uses.
pub fn through_writers(typed: Typed, xml: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let bytes = Arc::new(xml.to_vec());
    let error = |error: oxoffice_xlsx_oxml::OxmlError| error.to_string();
    Ok(match typed {
        Typed::Workbook => {
            let part = WorkbookXml::parse(bytes).map_err(error)?;
            let part = part.part();
            (
                part.to_xml_through_writers(),
                part.written_through_writers(),
            )
        }
        Typed::Styles => {
            let part = StylesXml::parse(bytes).map_err(error)?;
            let part = part.part();
            (
                part.to_xml_through_writers(),
                part.written_through_writers(),
            )
        }
        Typed::SharedStrings => {
            let strings = SharedStrings::parse(bytes).map_err(error)?;
            (
                strings.to_xml_through_writers().map_err(error)?,
                strings.len(),
            )
        }
        Typed::Worksheet => {
            let index = SheetIndex::build(xml).map_err(error)?;
            let model = sheet_model(xml)?;
            let mut edits = SheetEdits::default();
            for (span, row) in index.rows.iter().zip(model.rows) {
                edits.rows.insert(span.row, Some(row));
            }
            for (name, element) in model.page_setup.by_name() {
                if let Some(element) = element {
                    edits
                        .children
                        .insert(name.to_owned(), Some(element.to_xml()));
                }
            }
            let count = edits.rows.len() + edits.children.len();
            (splice(xml, &index, &edits).map_err(error)?, count)
        }
    })
}

/// `bytes` with every typed part written again by its writers, and how many
/// elements the writers wrote.
pub fn package_through_writers(bytes: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let mut package =
        OpcPackage::from_reader(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut written = 0;
    for (name, typed) in typed_parts(&package) {
        let part = format!("/{name}");
        let xml = package.get_part(&part).expect("a typed part").to_vec();
        let (rewritten, count) =
            through_writers(typed, &xml).map_err(|error| format!("{name}: {error}"))?;
        written += count;
        package.set_part(&part, rewritten);
    }
    let mut out = Cursor::new(Vec::new());
    package
        .write_to(&mut out)
        .map_err(|error| error.to_string())?;
    Ok((out.into_inner(), written))
}

/// How a change moved the parts of a package, by ZIP entry name: the parts
/// whose bytes differ, the parts added and the parts removed.
#[derive(Debug, Default, PartialEq)]
pub struct PartsDiff {
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

pub fn parts_diff(before: &[u8], after: &[u8]) -> PartsDiff {
    let (before, after) = (package_parts(before), package_parts(after));
    PartsDiff {
        changed: before
            .iter()
            .filter(|(name, bytes)| after.get(*name).is_some_and(|other| other != *bytes))
            .map(|(name, _)| name.clone())
            .collect(),
        added: after
            .keys()
            .filter(|name| !before.contains_key(*name))
            .cloned()
            .collect(),
        removed: before
            .keys()
            .filter(|name| !after.contains_key(*name))
            .cloned()
            .collect(),
    }
}

/// The text of one part of a package.
pub fn part_text(bytes: &[u8], part: &str) -> String {
    let parts = package_parts(bytes);
    let bytes = parts.get(part).unwrap_or_else(|| panic!("no part {part}"));
    String::from_utf8(bytes.clone()).expect("a text part")
}

/// A new workbook whose only sheet is `xml`, a worksheet part.
pub fn workbook_with_sheet(xml: &str) -> oxoffice_xlsx::Workbook {
    workbook_with_sheets(&[xml], false)
}

/// A new workbook with a sheet for each of `xmls`, worksheet parts, and a
/// calculation chain when asked.
pub fn workbook_with_sheets(xmls: &[&str], chain: bool) -> oxoffice_xlsx::Workbook {
    let names: Vec<String> = (1..=xmls.len()).map(|n| format!("Sheet{n}")).collect();
    let bytes = oxoffice_xlsx::Workbook::with_sheet_names(&names)
        .expect("names")
        .to_bytes()
        .expect("saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("a package");
    for (number, xml) in (1..).zip(xmls) {
        package.set_part(
            &format!("/xl/worksheets/sheet{number}.xml"),
            xml.as_bytes().to_vec(),
        );
    }
    if chain {
        package.set_part("/xl/calcChain.xml", b"<calcChain/>".to_vec());
        package.get_or_create_part_rels("/xl/workbook.xml").add(
            oxoffice_xlsx_oxml::namespaces::rel_types::CALC_CHAIN,
            "calcChain.xml",
        );
        package
            .content_types
            .add_override("/xl/calcChain.xml", content_types::CALC_CHAIN);
    }
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).expect("writes");
    oxoffice_xlsx::Workbook::from_bytes(bytes.get_ref()).expect("opens")
}

/// The parts of a sheet an edit may change: every child of the root but
/// these and every row but the ones named, as bytes, with the root's start
/// tag.
pub fn kept_bytes(xml: &[u8], children: &[&str], rows: &[u32]) -> Vec<u8> {
    let index = SheetIndex::build(xml).expect("an index");
    let mut kept = xml[index.root.start_tag.clone()].to_vec();
    for child in &index.children {
        if child.name != "sheetData" && !children.contains(&child.name.as_str()) {
            kept.extend_from_slice(&xml[child.range.clone()]);
        }
    }
    for row in &index.rows {
        if !rows.contains(&row.row) {
            kept.extend_from_slice(&xml[row.range.clone()]);
        }
    }
    kept
}

/// The elements of a child of the root, and the children it holds.
pub fn child_of(xml: &[u8], name: &str) -> Option<Element> {
    let index = SheetIndex::build(xml).expect("an index");
    let child = index.child(name)?;
    Some(Element::parse(&xml[child.range.clone()]).expect("an element"))
}

pub fn row_of(xml: &[u8], number: u32) -> Option<CT_Row> {
    let index = SheetIndex::build(xml).expect("an index");
    let span = index.row(number)?;
    Some(CT_Row::parse(&xml[span.range.clone()]).expect("a row"))
}

/// Whether a package holds a formula, by the text of its worksheet parts
/// (the corpus writes `f` with no prefix).
pub fn holds_formula(parts: &BTreeMap<String, Vec<u8>>) -> bool {
    parts
        .iter()
        .filter(|(name, _)| name.starts_with("xl/worksheets/"))
        .any(|(_, bytes)| {
            let text = String::from_utf8_lossy(bytes);
            ["<f>", "<f ", "<f/>"]
                .iter()
                .any(|start| text.contains(start))
        })
}
