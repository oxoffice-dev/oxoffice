//! Every Word file of the corpus rendered as `convert` and `render` do it,
//! with the bundled fonts: the checks a coherent render passes, read back
//! from the PDF and the page images, and the hashes that say whether the
//! render changed. See `specs/2026-11-21-render-fidelity.md`.
//!
//! `OXOFFICE_RENDER` chooses the files: `subset` (all but the `long-*`
//! family), `full`, or `off`. Unset means `off`, and fails under CI, so that
//! CI always says which. `OXOFFICE_RENDER_UPDATE="<reason>"` renders every
//! file and rewrites the baseline files that changed instead of checking
//! them:
//!
//! ```text
//! OXOFFICE_RENDER_UPDATE="<reason>" cargo test --profile ci -p oxoffice-docx --test docx render
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read};
use std::sync::OnceLock;

use lopdf::Object;
use oxoffice_docx::{Document, StoryKind, TableBorderEdge};
use oxoffice_render_test::checks::{
    Check, Finding, Rendered, Set, all_words, assert_baselines, assert_check,
    assert_every_file_has_a_baseline, assert_render_files_within_budget, check_empty_pages,
    check_glyphs, check_off_page, check_overlap, drawn_lines, drawn_pictures, horizontal_rules,
    is_long, parse_known, render_set, report_page, table_rules,
};
use oxoffice_render_test::fingerprint::{
    Fingerprint, PageHashes, Pixels, fingerprint, moved, sha256,
};
use oxoffice_render_test::pdf::{Drawn, Pdf};
use oxoffice_render_test::xml::{attribute, elements, elements_with_body, on, resolve, without};

use crate::support::{CorpusFile, corpus_files, corpus_manifest, corpus_root, map_in_parallel};

/// The paths of the Word files of the corpus.
fn docx_corpus() -> BTreeSet<String> {
    corpus_files("docx")
        .into_iter()
        .map(|file| file.path)
        .collect()
}

/// The paths of every corpus file, which `known.toml` entries name.
fn all_corpus() -> BTreeSet<String> {
    corpus_manifest()
        .into_iter()
        .map(|file| file.path)
        .collect()
}

/// Page images are rendered at this resolution, as small as still shows a
/// moved line.
const DPI: f64 = 72.0;

// ---- Which files, and what to do with them ----

// ---- What a render is checked for ----

// ---- Rendering ----

/// Every file of the chosen set rendered once per test binary run, shared
/// by the tests of this module.
fn rendered() -> &'static [Result<Rendered, String>] {
    static RENDERED: OnceLock<Vec<Result<Rendered, String>>> = OnceLock::new();
    RENDERED.get_or_init(|| {
        let Some(set) = render_set() else {
            return Vec::new();
        };
        let files: Vec<CorpusFile> = corpus_files("docx")
            .into_iter()
            .filter(|file| set == Set::Full || !is_long(&file.path))
            .collect();
        map_in_parallel(&files, render)
            .into_iter()
            .zip(&files)
            .map(|(result, file)| {
                result.unwrap_or_else(|| Err(format!("{}: the render panicked", file.path)))
            })
            .collect()
    })
}

/// A document drawn as `convert` and `render` draw it: its PDF read back,
/// its page images decoded, and the layout's diagnostics.
struct Drawing {
    pdf: Pdf,
    pixels: Vec<Pixels>,
    diagnostics: Vec<String>,
}

fn draw(doc: &Document) -> Result<Drawing, String> {
    let layout = doc
        .layout_deterministic()
        .map_err(|e| format!("layout: {e}"))?;
    let pdf_bytes = doc
        .to_pdf_deterministic()
        .map_err(|e| format!("PDF: {e}"))?;
    let pixels = (0..layout.layout.pages.len())
        .map(|page| {
            oxoffice_pdf::render_page_to_png(&layout.layout, page, DPI)
                .map(|image| Pixels::decode(&image))
                .ok_or_else(|| format!("PNG: page {} has no image", page + 1))
        })
        .collect::<Result<_, _>>()?;
    Ok(Drawing {
        pdf: Pdf::read(&pdf_bytes),
        pixels,
        diagnostics: layout
            .layout
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect(),
    })
}

fn render(file: &CorpusFile) -> Result<Rendered, String> {
    let bytes = file.bytes();
    let doc = Document::from_bytes(&bytes).map_err(|e| format!("{}: open: {e}", file.path))?;
    let Drawing {
        pdf,
        pixels,
        diagnostics,
    } = draw(&doc).map_err(|error| format!("{}: {error}", file.path))?;
    let drawn: Vec<Vec<Drawn>> = (0..pdf.page_count()).map(|page| pdf.drawn(page)).collect();

    let mut findings: BTreeMap<Check, Vec<Finding>> = BTreeMap::new();
    let mut find = |check: Check, page: usize, why: String| {
        findings.entry(check).or_default().push((page, why));
    };
    let package = Package::read(&bytes);
    check_pages(&doc, &pdf, &pixels, &mut find);
    check_empty_pages(&pdf, &drawn, &mut find);
    check_text(&doc, &package, &drawn, &mut find);
    let sizes: Vec<(f64, f64)> = (0..pdf.page_count())
        .map(|page| pdf.page_size(page))
        .collect();
    check_off_page(&sizes, &drawn, &mut find);
    check_glyphs(&drawn, &mut find);
    check_overlap(&drawn, &mut find);
    check_pictures(&package, &pdf, &mut find);
    check_tables(&doc, &package, &pdf, &mut find);
    check_dropped(&diagnostics, &mut find);

    Ok(Rendered {
        path: file.path.clone(),
        fingerprint: fingerprint(&pdf, &pixels),
        findings,
    })
}

// ---- The checks ----

fn check_pages(
    doc: &Document,
    pdf: &Pdf,
    pixels: &[Pixels],
    find: &mut impl FnMut(Check, usize, String),
) {
    if pdf.page_count() != pixels.len() {
        find(
            Check::Pages,
            0,
            format!(
                "{} PDF pages, {} page images",
                pdf.page_count(),
                pixels.len()
            ),
        );
    }
    // The sizes of the sections, in order, and those of the pages: each
    // section gives one or more pages of its size, so the two agree once
    // repeats are folded. A section without a size takes Letter, as Word
    // does.
    let close =
        |a: &mut (f64, f64), b: &mut (f64, f64)| (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5;
    let mut sections: Vec<(f64, f64)> = doc
        .sections()
        .map(|section| {
            section
                .page_size()
                .map_or((612.0, 792.0), |(width, height)| {
                    (width.to_pt(), height.to_pt())
                })
        })
        .collect();
    sections.dedup_by(|a, b| close(a, b));
    let mut pages: Vec<(f64, f64)> = (0..pdf.page_count())
        .map(|page| pdf.page_size(page))
        .collect();
    pages.dedup_by(|a, b| close(a, b));
    let same = sections.len() == pages.len()
        && sections
            .iter()
            .zip(&pages)
            .all(|(a, b)| close(&mut a.clone(), &mut b.clone()));
    if !same {
        find(
            Check::Pages,
            0,
            format!("pages come in sizes {pages:?}, the sections in {sections:?}"),
        );
    }
}

fn check_text(
    doc: &Document,
    package: &Package,
    drawn: &[Vec<Drawn>],
    find: &mut impl FnMut(Check, usize, String),
) {
    let shown_parts = package.shown_header_footer_parts();
    let paragraphs = match doc.story_paragraphs() {
        Ok(paragraphs) => paragraphs,
        Err(error) => {
            find(
                Check::Text,
                0,
                format!("the stories cannot be read: {error}"),
            );
            return;
        }
    };
    let mut expected: HashMap<String, usize> = HashMap::new();
    for paragraph in &paragraphs {
        let shown = match paragraph.kind {
            StoryKind::Comment => false,
            StoryKind::Header | StoryKind::Footer => shown_parts.contains(paragraph.part_name()),
            _ => true,
        };
        if shown {
            for word in all_words(&paragraph.text) {
                *expected.entry(word).or_default() += 1;
            }
        }
    }
    // Page numbers, sequence numbers, page counts and dates are computed by
    // the render: the numbers a field result holds are not what the page
    // draws. Every other number is.
    for (number, count) in package.field_result_numbers() {
        if let Some(expected) = expected.get_mut(&number) {
            *expected = expected.saturating_sub(count);
        }
    }
    expected.retain(|_, count| *count > 0);
    if expected.is_empty() {
        find(Check::Text, 0, "no text read from the document".to_owned());
        return;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for pieces in drawn {
        for line in drawn_lines(pieces) {
            for word in all_words(&line) {
                *seen.entry(word).or_default() += 1;
            }
        }
    }
    let mut missing: Vec<String> = expected
        .iter()
        .filter(|(word, count)| seen.get(*word).copied().unwrap_or(0) < **count)
        .map(|(word, count)| {
            format!(
                "{word} ({} of {count})",
                seen.get(word).copied().unwrap_or(0)
            )
        })
        .collect();
    if !missing.is_empty() {
        missing.sort();
        find(
            Check::Text,
            0,
            format!(
                "{} words not drawn as often as written: {}",
                missing.len(),
                missing.join(", ")
            ),
        );
    }
}

fn check_pictures(package: &Package, pdf: &Pdf, find: &mut impl FnMut(Check, usize, String)) {
    let written = package.shown_pictures();
    for page in 0..pdf.page_count() {
        let (width, height) = pdf.page_size(page);
        for picture in pdf.pictures(page) {
            let [left, bottom, w, h] = picture.rect;
            let (x, y) = (left + w / 2.0, bottom + h / 2.0);
            if w <= 0.0 || h <= 0.0 || x < 0.0 || x > width || y < 0.0 || y > height {
                find(
                    Check::Pictures,
                    page + 1,
                    format!(
                        "a picture at {:?} is outside its page or has no size",
                        picture.rect
                    ),
                );
            }
        }
    }
    // A JPEG reaches the PDF as written, so it is found by its bytes; other
    // pictures are decoded and re-encoded, so they are counted against the
    // pictures drawn that are no written JPEG.
    let drawn = drawn_pictures(pdf);
    let mut lost: Vec<&str> = written
        .iter()
        .filter(|(hash, jpeg)| **jpeg && !drawn.contains(*hash))
        .map(|(hash, _)| hash.as_str())
        .collect();
    let others = written.values().filter(|jpeg| !**jpeg).count();
    let others_drawn = drawn
        .iter()
        .filter(|hash| written.get(*hash) != Some(&true))
        .count();
    if !lost.is_empty() || others_drawn < others {
        lost.sort_unstable();
        find(
            Check::Pictures,
            0,
            format!(
                "{} JPEG pictures not drawn ({}), {others_drawn} other pictures drawn for {others}",
                lost.len(),
                lost.join(", ")
            ),
        );
    }
}

fn check_tables(
    doc: &Document,
    package: &Package,
    pdf: &Pdf,
    find: &mut impl FnMut(Check, usize, String),
) {
    let wanted = rules_wanted(doc, package);
    let drawn = table_rules(pdf);
    if drawn.len() != wanted.len() {
        find(
            Check::Tables,
            0,
            format!(
                "{} tables tagged in the PDF, {} in the body",
                drawn.len(),
                wanted.len()
            ),
        );
        return;
    }
    // A table that draws no text cannot be placed: it is not compared.
    let short: Vec<String> = drawn
        .iter()
        .zip(&wanted)
        .enumerate()
        .filter_map(|(index, (drawn, wanted))| {
            drawn
                .filter(|drawn| drawn < wanted)
                .map(|drawn| format!("table {} ({drawn} of {wanted})", index + 1))
        })
        .collect();
    if !short.is_empty() {
        find(
            Check::Tables,
            0,
            format!(
                "{} tables draw fewer horizontal rules than their borders need: {}",
                short.len(),
                short.join(", ")
            ),
        );
    }
}

/// How many horizontal rules each table of the body needs at least, in
/// order: one a row for a table with borders between its rows (its own or
/// its style's), two for one with a top or bottom border only.
fn rules_wanted(doc: &Document, package: &Package) -> Vec<usize> {
    let styles = package.table_style_borders();
    doc.tables()
        .iter()
        .map(|table| {
            let own = |edge| table.border(edge).map(|border| visible(border.style()));
            let style = table
                .style_id()
                .and_then(|id| styles.get(id))
                .copied()
                .unwrap_or_default();
            let inside = own(TableBorderEdge::InsideHorizontal).unwrap_or(style.inside);
            let outside = [TableBorderEdge::Top, TableBorderEdge::Bottom]
                .into_iter()
                .any(|edge| own(edge).unwrap_or(style.outside));
            if inside {
                table.row_count()
            } else if outside {
                2
            } else {
                0
            }
        })
        .collect()
}

fn visible(style: &str) -> bool {
    !matches!(style, "none" | "nil")
}

/// Whether a diagnostic changes nothing drawn: a font replaced by its
/// bundled metric twin, or chart labels written in the General format
/// because theirs is not supported. Any other diagnostic says the render
/// left something out (an equation, a picture it could not decode, a chart
/// drawn as a placeholder) and fails its file, until it is classified here
/// or listed in `known.toml`.
fn harmless(message: &str) -> bool {
    (message.starts_with("Font '") && message.contains("' substituted: selected '"))
        || (message.starts_with("Word chart relationship ")
            && message.ends_with("is not supported and its labels use General"))
}

fn check_dropped(diagnostics: &[String], find: &mut impl FnMut(Check, usize, String)) {
    let mut dropped: Vec<String> = diagnostics
        .iter()
        .filter(|message| !harmless(message))
        .cloned()
        .collect();
    dropped.sort();
    dropped.dedup();
    report_page(
        Check::Dropped,
        0,
        "diagnostics say content was not drawn",
        &dropped,
        find,
    );
}

// ---- The package around the render ----

/// The fields whose results the render computes: page numbers and counts,
/// section numbers, page references, sequence numbers, and the text of a
/// bookmark or of a style on the page (whose numbers are checked where they
/// are written). Their cached results are not what the page draws.
const COMPUTED_FIELDS: [&str; 8] = [
    "PAGE",
    "NUMPAGES",
    "SECTION",
    "SECTIONPAGES",
    "PAGEREF",
    "SEQ",
    "REF",
    "STYLEREF",
];

/// Each field of a story part with its result: its code (a simple field's
/// `w:instr`, a complex field's instruction text) and the text of its
/// result, a nested field's result going to the nested field.
fn field_results(xml: &str) -> Vec<(String, String)> {
    struct Open {
        code: String,
        result: Option<String>,
        simple: bool,
    }
    let mut open: Vec<Open> = Vec::new();
    let mut done = Vec::new();
    let mut inside: Option<&str> = None;
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        let text = &rest[..start];
        match inside {
            Some("w:t") => {
                if let Some(result) = open.last_mut().and_then(|field| field.result.as_mut()) {
                    result.push_str(text);
                }
            }
            Some("w:instrText") => {
                if let Some(field) = open.last_mut().filter(|field| field.result.is_none()) {
                    field.code.push_str(text);
                }
            }
            _ => {}
        }
        let Some(length) = rest[start..].find('>') else {
            break;
        };
        let tag = &rest[start + 1..start + length];
        rest = &rest[start + length + 1..];
        let closing = tag.starts_with('/');
        let empty = tag.ends_with('/');
        let name = tag
            .trim_start_matches('/')
            .split([' ', '/', '\t', '\n', '\r'])
            .next()
            .unwrap_or_default();
        let finish = |open: &mut Vec<Open>, done: &mut Vec<(String, String)>| {
            if let Some(field) = open.pop() {
                done.push((
                    field.code.trim().to_owned(),
                    field.result.unwrap_or_default(),
                ));
            }
        };
        match name {
            "w:t" | "w:instrText" => {
                inside = (!closing && !empty).then_some(if name == "w:t" {
                    "w:t"
                } else {
                    "w:instrText"
                });
            }
            "w:tab" | "w:br" | "w:cr" | "w:p" => {
                if let Some(result) = open.last_mut().and_then(|field| field.result.as_mut()) {
                    result.push(' ');
                }
            }
            "w:fldSimple" if closing => {
                if open.last().is_some_and(|field| field.simple) {
                    finish(&mut open, &mut done);
                }
            }
            "w:fldSimple" if !empty => open.push(Open {
                code: attribute(tag, "w:instr").unwrap_or_default().to_owned(),
                result: Some(String::new()),
                simple: true,
            }),
            "w:fldChar" => match attribute(tag, "w:fldCharType") {
                Some("begin") => open.push(Open {
                    code: String::new(),
                    result: None,
                    simple: false,
                }),
                Some("separate") => {
                    if let Some(field) = open.last_mut().filter(|field| !field.simple) {
                        field.result = Some(String::new());
                    }
                }
                Some("end") if open.last().is_some_and(|field| !field.simple) => {
                    finish(&mut open, &mut done);
                }
                _ => {}
            },
            _ => {}
        }
    }
    done
}

/// The parts of a Word package the checks read: which headers and footers
/// a reader sees, which pictures the shown stories hold, and the borders of
/// the table styles.
struct Package {
    parts: BTreeMap<String, String>,
    media: BTreeMap<String, Vec<u8>>,
}

#[derive(Clone, Copy, Default)]
struct StyleBorders {
    /// A visible top or bottom border.
    outside: bool,
    /// A visible border between rows.
    inside: bool,
}

impl Package {
    fn read(bytes: &[u8]) -> Self {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("package is a ZIP");
        let mut parts = BTreeMap::new();
        let mut media = BTreeMap::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).expect("ZIP entry");
            let name = entry.name().expect("ZIP entry name").into_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).expect("ZIP entry reads");
            if name.ends_with(".xml") || name.ends_with(".rels") {
                parts.insert(name, String::from_utf8_lossy(&data).into_owned());
            } else {
                media.insert(name, data);
            }
        }
        Self { parts, media }
    }

    /// The relationships of a part: id to target part name.
    fn relationships(&self, part: &str) -> HashMap<String, String> {
        let (folder, file) = part.rsplit_once('/').unwrap_or(("", part));
        let rels = format!("{folder}/_rels/{file}.rels");
        let Some(xml) = self.parts.get(&rels) else {
            return HashMap::new();
        };
        elements(xml, "Relationship")
            .into_iter()
            .filter_map(|element| {
                let id = attribute(element, "Id")?;
                let target = attribute(element, "Target")?;
                Some((id.to_owned(), resolve(folder, target)))
            })
            .collect()
    }

    /// The header and footer parts some page shows, as story part names
    /// (`/word/header1.xml`): each section's own or inherited reference of
    /// the default kind, the first-page one when the section has a title
    /// page, the even-page one when the settings turn them on.
    fn shown_header_footer_parts(&self) -> HashSet<String> {
        let document = &self.parts["word/document.xml"];
        let relationships = self.relationships("word/document.xml");
        let even = self
            .parts
            .get("word/settings.xml")
            .is_some_and(|settings| on(settings, "w:evenAndOddHeaders"));
        let mut shown = HashSet::new();
        let mut current: HashMap<(String, String), String> = HashMap::new();
        for section in elements_with_body(document, "w:sectPr") {
            for kind in ["w:headerReference", "w:footerReference"] {
                for reference in elements(section, kind) {
                    let (Some(page), Some(id)) =
                        (attribute(reference, "w:type"), attribute(reference, "r:id"))
                    else {
                        continue;
                    };
                    if let Some(target) = relationships.get(id) {
                        current.insert((kind.to_owned(), page.to_owned()), target.clone());
                    }
                }
            }
            let title = on(section, "w:titlePg");
            for ((_, page), target) in &current {
                let seen = match page.as_str() {
                    "default" => true,
                    "first" => title,
                    "even" => even,
                    _ => false,
                };
                if seen {
                    shown.insert(format!("/{target}"));
                }
            }
        }
        shown
    }

    /// The parts of the stories a reader sees: the body, the shown headers
    /// and footers, and the notes.
    fn shown_stories(&self) -> Vec<String> {
        let mut stories = vec!["word/document.xml".to_owned()];
        stories.extend(
            self.shown_header_footer_parts()
                .into_iter()
                .map(|part| part.trim_start_matches('/').to_owned()),
        );
        stories.extend([
            "word/footnotes.xml".to_owned(),
            "word/endnotes.xml".to_owned(),
        ]);
        stories
    }

    /// The numbers the results of the computed fields of the shown stories
    /// hold (see [`COMPUTED_FIELDS`]), with how often: in digits, or in Roman
    /// numerals as a page number field writes front matter.
    fn field_result_numbers(&self) -> HashMap<String, usize> {
        let mut numbers = HashMap::new();
        for story in self.shown_stories() {
            let Some(xml) = self.parts.get(&story) else {
                continue;
            };
            // The render draws the choice of an alternate content, never its
            // fallback, which repeats the fields of a text box.
            let xml = without(xml, "mc:Fallback");
            for (code, result) in field_results(&xml) {
                let name = code.split_whitespace().next().unwrap_or_default();
                if !COMPUTED_FIELDS
                    .iter()
                    .any(|computed| computed.eq_ignore_ascii_case(name))
                {
                    continue;
                }
                for word in all_words(&result) {
                    let digits = word.chars().all(|ch| ch.is_ascii_digit());
                    let roman = word.chars().all(|ch| "ivxlcdmIVXLCDM".contains(ch));
                    if digits || roman {
                        *numbers.entry(word).or_default() += 1;
                    }
                }
            }
        }
        numbers
    }

    /// The pictures of the stories a reader sees, by the hash of their
    /// bytes, each with whether it is a JPEG: the body, the shown headers
    /// and footers, and the notes.
    fn shown_pictures(&self) -> BTreeMap<String, bool> {
        let mut pictures = BTreeMap::new();
        for story in self.shown_stories() {
            let Some(xml) = self.parts.get(&story) else {
                continue;
            };
            // The render draws the choice of an alternate content, never
            // its fallback (a picture of a text box, for one).
            let xml = &without(xml, "mc:Fallback");
            let relationships = self.relationships(&story);
            let ids = elements(xml, "a:blip")
                .into_iter()
                .filter_map(|blip| attribute(blip, "r:embed"))
                .chain(
                    elements(xml, "v:imagedata")
                        .into_iter()
                        .filter_map(|data| attribute(data, "r:id")),
                );
            for id in ids {
                if let Some(bytes) = relationships
                    .get(id)
                    .and_then(|target| self.media.get(target))
                {
                    pictures.insert(sha256(bytes), bytes.starts_with(&[0xFF, 0xD8, 0xFF]));
                }
            }
        }
        pictures
    }

    /// The borders each table style declares, its base styles' included.
    fn table_style_borders(&self) -> HashMap<String, StyleBorders> {
        let Some(styles) = self.parts.get("word/styles.xml") else {
            return HashMap::new();
        };
        let mut own: HashMap<String, (Option<String>, StyleBorders)> = HashMap::new();
        for style in elements_with_body(styles, "w:style") {
            if attribute(style, "w:type") != Some("table") {
                continue;
            }
            let Some(id) = attribute(style, "w:styleId") else {
                continue;
            };
            let based_on = elements(style, "w:basedOn")
                .first()
                .and_then(|element| attribute(element, "w:val"))
                .map(str::to_owned);
            // The table-wide borders, not those of a conditional format.
            let table_wide = style.split("<w:tblStylePr").next().unwrap_or_default();
            let mut borders = StyleBorders::default();
            for block in elements_with_body(table_wide, "w:tblBorders") {
                let shows = |edge: &str| {
                    elements(block, edge)
                        .iter()
                        .any(|element| attribute(element, "w:val").is_some_and(visible))
                };
                borders.outside |= shows("w:top") || shows("w:bottom");
                borders.inside |= shows("w:insideH");
            }
            own.insert(id.to_owned(), (based_on, borders));
        }
        own.keys()
            .map(|id| {
                let mut borders = StyleBorders::default();
                let mut next = Some(id.clone());
                let mut depth = 0;
                while let Some(id) = next.take()
                    && depth < 16
                {
                    let Some((based_on, style)) = own.get(&id) else {
                        break;
                    };
                    borders.outside |= style.outside;
                    borders.inside |= style.inside;
                    next = based_on.clone();
                    depth += 1;
                }
                (id.clone(), borders)
            })
            .collect()
    }
}

// ---- Fingerprints ----

// ---- Baselines ----

// ---- Known failures ----

// ---- Tests ----

#[test]
fn every_corpus_render_matches_its_baseline() {
    assert_baselines(rendered(), "docx", &docx_corpus());
}

#[test]
fn every_corpus_file_has_a_baseline_and_every_baseline_a_corpus_file() {
    assert_every_file_has_a_baseline("docx", &docx_corpus());
}

#[test]
fn the_render_files_stay_within_their_budget() {
    assert_render_files_within_budget();
}

#[test]
fn a_page_whose_pixels_or_content_change_is_reported_on_that_page() {
    let page = |pixels: &Pixels| PageHashes {
        pixels: pixels.hash(),
        content: sha256(b"BT (Hello) Tj ET"),
        annotations: sha256(b""),
        page: sha256(b"<>"),
    };
    let white = |width: u32, height: u32| Pixels {
        width,
        height,
        rgba: vec![255; (width * height * 4) as usize],
    };
    let expected = Fingerprint {
        pages: vec![page(&white(4, 4)), page(&white(4, 4))],
        resources: sha256(b"fonts"),
        outline_and_tags: sha256(b""),
        catalog: sha256(b"<>"),
    };

    let mut one_pixel = white(4, 4);
    one_pixel.rgba[5] ^= 1;
    let mut found = expected.clone();
    found.pages[1] = page(&one_pixel);
    assert_eq!(moved(&expected, &found), ["page 2: pixels"]);

    let mut found = expected.clone();
    found.pages[0].content = sha256(b"BT (Hellp) Tj ET");
    assert_eq!(moved(&expected, &found), ["page 1: content"]);

    assert!(moved(&expected, &expected.clone()).is_empty());
}

#[test]
fn a_word_changed_on_one_page_moves_the_baseline_of_that_page_only() {
    let mut doc = crate::support::corpus_doc("word/report.docx");
    let before = draw(&doc).expect("the report draws");
    assert_eq!(doc.replace_text("handled", "answered"), 1);
    let after = draw(&doc).expect("the edited report draws");
    let moved = moved(
        &fingerprint(&before.pdf, &before.pixels),
        &fingerprint(&after.pdf, &after.pixels),
    );
    assert_eq!(
        moved.first().map(String::as_str),
        Some("page 2: pixels, content")
    );
    assert!(
        moved[1..].iter().all(|line| !line.starts_with("page")),
        "only page 2 moved: {moved:?}"
    );
}

#[test]
fn every_page_of_a_corpus_render_has_a_page_size_of_its_document() {
    assert_check(Check::Pages, rendered(), &all_corpus());
}

#[test]
fn no_page_of_a_corpus_render_draws_only_its_header_and_footer() {
    assert_check(Check::EmptyPage, rendered(), &all_corpus());
}

#[test]
fn every_word_of_a_corpus_document_is_drawn() {
    assert_check(Check::Text, rendered(), &all_corpus());
}

#[test]
fn no_text_of_a_corpus_render_crosses_its_page_edge() {
    assert_check(Check::OffPage, rendered(), &all_corpus());
}

#[test]
fn no_glyph_of_a_corpus_render_is_an_empty_box() {
    assert_check(Check::MissingGlyph, rendered(), &all_corpus());
}

#[test]
fn no_text_of_a_corpus_render_is_drawn_over_other_text() {
    assert_check(Check::Overlap, rendered(), &all_corpus());
}

#[test]
fn every_picture_of_a_corpus_document_is_drawn_inside_its_page() {
    assert_check(Check::Pictures, rendered(), &all_corpus());
}

#[test]
fn the_tables_of_a_corpus_render_draw_their_borders() {
    assert_check(Check::Tables, rendered(), &all_corpus());
}

#[test]
fn a_corpus_render_leaves_nothing_out() {
    assert_check(Check::Dropped, rendered(), &all_corpus());
}

#[test]
fn words_split_digits_from_letters_and_chinese_by_character() {
    assert_eq!(
        all_words("Page 12 of iv: rate1 2Kyoto CO2"),
        [
            "Page", "12", "of", "iv", "rate", "1", "2", "Kyoto", "CO", "2"
        ]
    );
    assert_eq!(
        all_words("会议纪要 Hello"),
        ["会", "议", "纪", "要", "Hello"]
    );
}

#[test]
fn field_results_are_read_from_simple_complex_and_nested_fields() {
    let xml = concat!(
        "<w:p><w:r><w:t>Revenue 2024</w:t></w:r>",
        r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>7</w:t></w:r></w:fldSimple>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        "<w:r><w:instrText> REF total </w:instrText></w:r>",
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
        "<w:r><w:t>see</w:t></w:r><w:r><w:tab/></w:r>",
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        "<w:r><w:instrText xml:space=\"preserve\"> PAGEREF total </w:instrText></w:r>",
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
        "<w:r><w:t>12</w:t></w:r>",
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        "<w:r><w:t>1402</w:t></w:r></w:p>",
    );
    assert_eq!(
        field_results(xml),
        [
            ("PAGE".to_owned(), "7".to_owned()),
            ("PAGEREF total".to_owned(), "12".to_owned()),
            ("REF total".to_owned(), "see ".to_owned()),
        ]
    );
}

#[test]
fn a_field_in_a_text_box_counts_once_not_again_in_its_fallback() {
    let field = r#"<w:p><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>"#;
    let mut doc = crate::support::doc_with_body(&format!(
        "<w:p>{}</w:p>",
        crate::support::text_box_run(1, field)
    ));
    let package = Package::read(&doc.to_bytes().expect("document saves"));
    assert_eq!(package.field_result_numbers().get("1"), Some(&1));
}

#[test]
fn a_number_outside_fields_must_be_drawn_and_one_in_a_field_need_not() {
    let mut doc = crate::support::doc_with_body(concat!(
        "<w:p><w:r><w:t xml:space=\"preserve\">Revenue 2024 was 1402 on page </w:t></w:r>",
        r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>99</w:t></w:r></w:fldSimple>"#,
        r#"<w:fldSimple w:instr=" MERGEFIELD Units "><w:r><w:t> 315</w:t></w:r></w:fldSimple>"#,
        r#"<w:fldSimple w:instr=" REF figure "><w:r><w:t> 77</w:t></w:r></w:fldSimple></w:p>"#,
    ));
    let package = Package::read(&doc.to_bytes().expect("document saves"));
    let findings = |line: &str| {
        let mut found = Vec::new();
        check_text(
            &doc,
            &package,
            &[vec![piece(line, 72.0, 700.0, 400.0)]],
            &mut |check, _, why| found.push((check, why)),
        );
        found
    };
    assert!(findings("Revenue 2024 was 1402 on page 1 315").is_empty());
    // A number of plain text, or of a field the render does not compute,
    // must be drawn.
    let found = findings("Revenue 2024 was 1420 on page 1 351");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].1.contains("1402 (0 of 1)") && found[0].1.contains("315 (0 of 1)"),
        "{}",
        found[0].1
    );
}

/// A piece of text as the PDF reader reads it back.
fn piece(text: &str, x: f64, y: f64, end: f64) -> Drawn {
    Drawn {
        text: text.to_owned(),
        glyphs: text.to_owned(),
        x,
        y,
        end,
        size: 11.0,
        mcid: None,
        notdef: 0,
    }
}

#[test]
fn the_text_checks_catch_text_over_text_off_its_page_or_in_empty_boxes() {
    let line = |y| piece("Quarterly results ", 72.0, y, 160.0);
    let mut boxed = line(500.0);
    boxed.notdef = 2;
    let pages = vec![
        vec![line(700.0), line(686.0), piece("grew", 162.0, 700.0, 185.0)],
        vec![line(700.0), piece("results", 100.0, 702.0, 140.0)],
        vec![piece("Footnote", 560.0, 400.0, 640.0)],
        vec![boxed],
    ];
    let mut found = Vec::new();
    let mut find = |check: Check, page: usize, _: String| found.push((check, page));
    check_overlap(&pages, &mut find);
    check_off_page(&[(612.0, 792.0); 4], &pages, &mut find);
    check_glyphs(&pages, &mut find);
    assert_eq!(
        found,
        [
            (Check::Overlap, 2),
            (Check::OffPage, 3),
            (Check::MissingGlyph, 4)
        ]
    );
}

#[test]
fn a_bordered_table_needs_a_rule_a_row_and_a_borderless_one_none() {
    let cell = |row: usize, column: usize| {
        format!(
            "<w:tc><w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/></w:tcPr>\
             <w:p><w:r><w:t>Row {row} column {column}</w:t></w:r></w:p></w:tc>"
        )
    };
    let rows: String = (1..=4)
        .map(|row| {
            format!(
                "<w:tr>{}</w:tr>",
                (1..=3).map(|column| cell(row, column)).collect::<String>()
            )
        })
        .collect();
    for (border, wanted) in [("single", 4), ("none", 0)] {
        let mut doc = crate::support::doc_with_body(&format!(
            "<w:tbl><w:tblPr><w:tblBorders>\
             <w:top w:val=\"{border}\" w:sz=\"4\"/><w:bottom w:val=\"{border}\" w:sz=\"4\"/>\
             <w:insideH w:val=\"{border}\" w:sz=\"4\"/></w:tblBorders></w:tblPr>\
             <w:tblGrid><w:gridCol w:w=\"3000\"/><w:gridCol w:w=\"3000\"/>\
             <w:gridCol w:w=\"3000\"/></w:tblGrid>{rows}</w:tbl><w:p/>"
        ));
        let package = Package::read(&doc.to_bytes().expect("document saves"));
        assert_eq!(rules_wanted(&doc, &package), [wanted], "{border} borders");
        let pdf = draw(&doc).expect("the table draws").pdf;
        // Three cells a row, each with its own top and bottom edge: one rule
        // above each row and one below the last.
        let rules = if wanted == 0 { 0 } else { wanted + 1 };
        assert_eq!(horizontal_rules(&pdf, 0), rules, "{border} borders");
        assert_eq!(table_rules(&pdf), [Some(rules)], "{border} borders");
    }
}

/// A body of tables of `rows` rows each, `border` on all their edges, a
/// paragraph between two.
fn tables(specs: &[(&str, usize)]) -> Document {
    let table = |(border, rows): &(&str, usize)| {
        let rows: String = (1..=*rows)
            .map(|row| {
                format!(
                    "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"4000\" w:type=\"dxa\"/></w:tcPr>\
                     <w:p><w:r><w:t>Row {row}</w:t></w:r></w:p></w:tc></w:tr>"
                )
            })
            .collect();
        format!(
            "<w:tbl><w:tblPr><w:tblBorders>\
             <w:top w:val=\"{border}\" w:sz=\"4\"/><w:bottom w:val=\"{border}\" w:sz=\"4\"/>\
             <w:insideH w:val=\"{border}\" w:sz=\"4\"/></w:tblBorders></w:tblPr>\
             <w:tblGrid><w:gridCol w:w=\"4000\"/></w:tblGrid>{rows}</w:tbl>\
             <w:p><w:r><w:t>Between the tables</w:t></w:r></w:p>"
        )
    };
    crate::support::doc_with_body(&specs.iter().map(table).collect::<String>())
}

#[test]
fn a_table_that_lost_its_borders_is_caught_though_another_draws_more() {
    // The document asks for two bordered tables of four rows; the PDF draws
    // the first without borders and the second with eight rows. Together
    // they draw more rules than the document needs.
    let mut wanted = tables(&[("single", 4), ("single", 4)]);
    let package = Package::read(&wanted.to_bytes().expect("document saves"));
    let drawn = draw(&tables(&[("none", 4), ("single", 8)]))
        .expect("the tables draw")
        .pdf;
    assert_eq!(table_rules(&drawn), [Some(0), Some(9)]);
    let mut found = Vec::new();
    check_tables(&wanted, &package, &drawn, &mut |check, _, why| {
        found.push((check, why))
    });
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.contains("table 1 (0 of 4)"), "{}", found[0].1);
    let mut found = Vec::new();
    let own = draw(&wanted).expect("the tables draw").pdf;
    check_tables(&wanted, &package, &own, &mut |check, _, why| {
        found.push((check, why))
    });
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn text_beside_a_table_does_not_hide_its_borders() {
    use std::sync::Arc;

    use oxoffice_layout::{
        Color, DocumentStructure, FontData, FontId, GlyphRun, LayoutResult, PageFrame, Point,
        PositionedElement, StructureId, StructureNode, StructureRole,
    };
    let id = |value| StructureId::new(value).expect("an identity");
    let node = |value, role, children: Vec<u32>| StructureNode {
        id: id(value),
        role,
        children: children.into_iter().map(id).collect(),
        alternate_text: None,
    };
    let text = |text: &str, x: f64, y: f64, structure: u32| PositionedElement::MarkedContent {
        structure: Some(id(structure)),
        children: vec![PositionedElement::Text(GlyphRun {
            origin: Point { x, y },
            font_id: FontId(1),
            font_size: 11.0,
            glyph_ids: (36..).take(text.chars().count()).collect(),
            advances: vec![6.0; text.chars().count()],
            text: text.to_owned(),
            source: None,
            color: Color::BLACK,
            bold: false,
            italic: false,
            field_kind: None,
            field_source: None,
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        })],
    };
    let rule = |y: f64| PositionedElement::Line {
        start: Point { x: 72.0, y },
        end: Point { x: 222.0, y },
        width: 0.5,
        color: Color::BLACK,
        dash_pattern: None,
    };
    // A table with a top and a bottom border in the first column; the line
    // of the second column stands between its top border and its first row.
    let page = PageFrame::new(
        1,
        612.0,
        792.0,
        vec![
            rule(100.0),
            text("Second column", 320.0, 112.0, 5),
            text("Row 1", 76.0, 125.0, 4),
            text("Row 2", 76.0, 150.0, 4),
            text("Row 3", 76.0, 175.0, 4),
            rule(185.0),
        ],
    );
    let font = FontData {
        id: FontId(1),
        family: "Liberation Sans".to_owned(),
        data: Arc::from(oxoffice_layout::bundled_fonts::LIBERATION_SANS_REGULAR),
        face_index: 0,
        bold: false,
        italic: false,
        variations: Vec::new(),
        synthetic_bold: false,
        synthetic_italic: false,
    };
    let mut layout = LayoutResult::new(vec![Arc::new(page)], vec![font], None, Vec::new());
    layout.structure = Some(DocumentStructure {
        root: id(1),
        nodes: vec![
            node(1, StructureRole::Document, vec![2, 5]),
            node(2, StructureRole::Table, vec![3]),
            node(3, StructureRole::TableRow, vec![4]),
            node(4, StructureRole::TableCell, Vec::new()),
            node(5, StructureRole::Paragraph, Vec::new()),
        ],
    });
    let pdf = Pdf::read(&oxoffice_pdf::render_to_pdf(&layout));
    assert_eq!(table_rules(&pdf), [Some(2)]);
}

#[test]
fn a_table_without_text_is_not_compared() {
    let rows =
        "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/></w:tcPr><w:p/></w:tc></w:tr>"
            .repeat(3);
    let mut doc = crate::support::doc_with_body(&format!(
        "<w:tbl><w:tblPr><w:tblBorders>\
         <w:top w:val=\"single\" w:sz=\"4\"/><w:bottom w:val=\"single\" w:sz=\"4\"/>\
         <w:insideH w:val=\"single\" w:sz=\"4\"/></w:tblBorders></w:tblPr>\
         <w:tblGrid><w:gridCol w:w=\"3000\"/></w:tblGrid>{rows}</w:tbl><w:p/>"
    ));
    let package = Package::read(&doc.to_bytes().expect("document saves"));
    let pdf = draw(&doc).expect("the table draws").pdf;
    assert_eq!(table_rules(&pdf), [None]);
    let mut found = Vec::new();
    check_tables(&doc, &package, &pdf, &mut |check, _, why| {
        found.push((check, why))
    });
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_empty_direct_border_container_keeps_the_table_style_borders() {
    // LibreOffice writes `<w:tblBorders/>` with no edge: the table style's
    // borders still draw, as they do without it.
    let rows: String = (1..=3)
        .map(|row| {
            format!(
                "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/></w:tcPr>\
                 <w:p><w:r><w:t>Row {row}</w:t></w:r></w:p></w:tc></w:tr>"
            )
        })
        .collect();
    let drawn = |direct: &str| {
        let doc = crate::support::doc_with_body(&format!(
            "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/>{direct}</w:tblPr>\
             <w:tblGrid><w:gridCol w:w=\"3000\"/></w:tblGrid>{rows}</w:tbl><w:p/>"
        ));
        horizontal_rules(&draw(&doc).expect("the table draws").pdf, 0)
    };
    let styled = drawn("");
    assert!(styled > 0, "the table style draws borders");
    assert_eq!(drawn("<w:tblBorders/>"), styled);
    assert_eq!(drawn("<w:tblBorders></w:tblBorders>"), styled);
}

#[test]
fn table_borders_come_from_the_table_style_whatever_its_language() {
    for (file, style) in [
        ("authored/report.docx", "TableGrid"),
        ("word/report.docx", "Grilledutableau"),
    ] {
        let bytes = fs::read(corpus_root().join("files").join(file)).expect("corpus file");
        let borders = Package::read(&bytes).table_style_borders();
        assert!(
            borders.get(style).is_some_and(|borders| borders.inside),
            "{file}: {style} has borders between its rows"
        );
    }
}

#[test]
fn the_pictures_of_a_corpus_report_are_found_in_its_package_and_its_pdf() {
    for file in [
        "authored/report.docx",
        "word/report.docx",
        "google/report.docx",
    ] {
        let bytes = fs::read(corpus_root().join("files").join(file)).expect("corpus file");
        let written = Package::read(&bytes).shown_pictures();
        let doc = Document::from_bytes(&bytes).expect("the report opens");
        let drawn = drawn_pictures(&draw(&doc).expect("the report draws").pdf);
        assert!(!written.is_empty(), "{file} has pictures");
        assert_eq!(drawn.len(), written.len(), "{file}");
        for (hash, _) in written.iter().filter(|(_, jpeg)| **jpeg) {
            assert!(drawn.contains(hash), "{file}: a JPEG is drawn as written");
        }
    }
}

/// A JPEG of the corpus, made distinct by a comment segment after its start
/// marker: the same picture, other bytes.
fn jpeg(comment: &[u8]) -> Vec<u8> {
    let bytes = fs::read(corpus_root().join("files/authored/contract.docx")).expect("corpus file");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("package is a ZIP");
    let mut jpeg = Vec::new();
    zip.by_name("docProps/thumbnail.jpeg")
        .expect("a thumbnail")
        .read_to_end(&mut jpeg)
        .expect("thumbnail inflates");
    let length = u16::try_from(comment.len() + 2).expect("a short comment");
    let segment = [&[0xFF, 0xFE][..], &length.to_be_bytes(), comment].concat();
    jpeg.splice(2..2, segment);
    jpeg
}

fn pictures(jpegs: &[&[u8]]) -> Document {
    let mut doc = Document::new();
    for (index, jpeg) in jpegs.iter().enumerate() {
        doc.add_picture(
            jpeg,
            &format!("picture{index}.jpeg"),
            oxoffice_docx::Length::inches(1.0),
            oxoffice_docx::Length::inches(1.0),
        );
    }
    doc
}

#[test]
fn a_dropped_jpeg_is_caught_though_another_picture_is_drawn_in_its_place() {
    let (a, b, c) = (jpeg(b"a"), jpeg(b"b"), jpeg(b"c"));
    let mut written = pictures(&[&a, &b]);
    let package = Package::read(&written.to_bytes().expect("document saves"));
    let check = |doc: &Document| {
        let pdf = draw(doc).expect("the pictures draw").pdf;
        let mut found = Vec::new();
        check_pictures(&package, &pdf, &mut |check, _, why| {
            found.push((check, why))
        });
        found
    };
    assert!(check(&written).is_empty());
    // As many distinct pictures drawn as written, but the second is not it.
    let found = check(&pictures(&[&a, &c]));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].1.starts_with("1 JPEG pictures not drawn"),
        "{}",
        found[0].1
    );
}

#[test]
fn a_known_failure_names_a_corpus_file_a_check_its_pages_and_why() {
    let corpus: BTreeSet<String> = ["word/report.docx".to_owned()].into();
    let entry = |lines: &str| parse_known(&format!("[[known]]\n{lines}"), &corpus);
    let good = "file = \"word/report.docx\"\ncheck = \"text\"\npages = [0]\nfinding = \"2 words\"\nissue = 1";
    assert_eq!(entry(good).expect("a good entry").len(), 1);
    for (broken, why) in [
        (good.replace("word/report", "word/gone"), "no corpus file"),
        (good.replace("\"text\"", "\"words\""), "no check"),
        (good.replace("[0]", "[]"), "no pages"),
        (
            good.replace("finding = \"2 words\"\n", ""),
            "says how its finding starts",
        ),
        (good.replace("issue = 1", ""), "neither an issue"),
    ] {
        let error = entry(&broken).expect_err(why);
        assert!(error.contains(why), "{error}");
    }
}

#[test]
fn a_diagnostic_fails_its_file_unless_it_changes_nothing_drawn() {
    assert!(harmless(
        "Font 'Calibri' substituted: selected 'Carlito' from the bundled tier"
    ));
    assert!(harmless(
        "Word chart relationship rId5: number format `[$-1009]yyyy/mm/dd` is not supported and its labels use General"
    ));
    for dropped in [
        "OfficeMath content at paragraph/0 was preserved but could not be rendered",
        "Word chart relationship rId5: the chart part is missing",
        "picture relationship rId7 could not be decoded",
    ] {
        assert!(!harmless(dropped), "{dropped}");
    }
}

#[test]
fn a_changed_font_width_moves_the_resources_and_nothing_else() {
    let doc = crate::support::doc_with(&["Quarterly results grew in every region."]);
    let mut drawing = draw(&doc).expect("the paragraph draws");
    let before = fingerprint(&drawing.pdf, &drawing.pixels);
    // The glyph widths of a font live in its CIDFont dictionary, an object
    // that is no stream: the content streams only name the font.
    let widths = drawing
        .pdf
        .doc
        .objects
        .values_mut()
        .filter_map(|object| object.as_dict_mut().ok())
        .find_map(|dict| {
            (dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"CIDFontType2"))
                .then(|| dict.get_mut(b"W").ok())
                .flatten()
        })
        .expect("a CIDFont with widths");
    let Object::Array(entries) = widths else {
        panic!("W is an array");
    };
    let Some(Object::Array(list)) = entries.get_mut(1) else {
        panic!("W starts with a list of widths");
    };
    list[0] = Object::Integer(list[0].as_float().expect("a width") as i64 + 1);
    let after = fingerprint(&drawing.pdf, &drawing.pixels);
    assert_eq!(moved(&before, &after), ["resources"]);
}

#[test]
fn a_landscape_section_drawn_portrait_is_caught() {
    let doc = crate::support::doc_with_body(concat!(
        "<w:p><w:pPr><w:sectPr>",
        r#"<w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/>"#,
        r#"<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>"#,
        "</w:sectPr></w:pPr><w:r><w:t>A wide table section</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>A tall closing section</w:t></w:r></w:p>",
    ));
    let mut drawing = draw(&doc).expect("the sections draw");
    let mut found = Vec::new();
    check_pages(
        &doc,
        &drawing.pdf,
        &drawing.pixels,
        &mut |check, page, _| found.push((check, page)),
    );
    assert!(
        found.is_empty(),
        "the pages have the sizes of their sections"
    );
    // The renderer loses the orientation of the landscape section: its page
    // comes out portrait.
    let (width, height) = drawing.pdf.page_size(0);
    assert!(width > height, "the first page is landscape");
    let id = drawing.pdf.page_ids()[0];
    let portrait = vec![
        0.into(),
        0.into(),
        Object::Real(height as f32),
        Object::Real(width as f32),
    ];
    drawing
        .pdf
        .doc
        .get_dictionary_mut(id)
        .expect("page dictionary")
        .set("MediaBox", Object::Array(portrait));
    check_pages(
        &doc,
        &drawing.pdf,
        &drawing.pixels,
        &mut |check, page, _| found.push((check, page)),
    );
    assert_eq!(found, [(Check::Pages, 0)]);
}
