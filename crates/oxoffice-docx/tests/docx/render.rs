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
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use lopdf::content::Content;
use lopdf::{Object, ObjectId};
use oxoffice_docx::{Document, StoryKind, TableBorderEdge};
use sha2::{Digest, Sha256};

use crate::pdf::{Drawn, Pdf};
use crate::support::{CorpusFile, corpus_files, corpus_root, map_in_parallel};

/// Page images are rendered at this resolution, as small as still shows a
/// moved line.
const DPI: f64 = 72.0;

// ---- Which files, and what to do with them ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    Subset,
    Full,
}

/// The reason of a baseline update, when this run rewrites the baselines.
fn update_reason() -> Option<String> {
    let reason = std::env::var("OXOFFICE_RENDER_UPDATE").ok()?;
    let reason = reason.trim();
    assert!(
        !reason.is_empty(),
        "OXOFFICE_RENDER_UPDATE needs the reason of the update"
    );
    Some(reason.to_owned())
}

fn render_set() -> Option<Set> {
    if update_reason().is_some() {
        return Some(Set::Full);
    }
    match std::env::var("OXOFFICE_RENDER").ok().as_deref() {
        Some("subset") => Some(Set::Subset),
        Some("full") => Some(Set::Full),
        Some("off") => None,
        Some(other) => panic!("OXOFFICE_RENDER={other}: expected subset, full or off"),
        None if std::env::var_os("CI").is_some() => {
            panic!("OXOFFICE_RENDER is not set: CI must choose subset, full or off")
        }
        None => None,
    }
}

fn render_root() -> PathBuf {
    corpus_root().join("../render")
}

/// The baseline file of a corpus file: `word/report.docx` has
/// `tests/render/baseline/word/report.docx.json`.
fn baseline_path(corpus_path: &str) -> PathBuf {
    render_root()
        .join("baseline")
        .join(format!("{corpus_path}.json"))
}

// ---- What a render is checked for ----

/// A check every render passes, unless `tests/render/known.toml` lists the
/// file and the pages it fails on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Check {
    /// The PDF opens, has a page image for each page, and its page sizes
    /// follow those of the sections, in order.
    Pages,
    /// A page draws text or a picture away from its top and bottom edges.
    EmptyPage,
    /// Every word of the document is drawn.
    Text,
    /// No text crosses the edge of its page.
    OffPage,
    /// No glyph is the font's .notdef box, and every glyph maps to text.
    MissingGlyph,
    /// No text is drawn over other text.
    Overlap,
    /// Every picture is drawn, at a size, inside its page.
    Pictures,
    /// A table with borders draws its rules.
    Tables,
    /// The renderer drew everything: it gives no diagnostic but the ones
    /// known to change nothing drawn.
    Dropped,
}

impl Check {
    const ALL: [Check; 9] = [
        Check::Pages,
        Check::EmptyPage,
        Check::Text,
        Check::OffPage,
        Check::MissingGlyph,
        Check::Overlap,
        Check::Pictures,
        Check::Tables,
        Check::Dropped,
    ];

    fn name(self) -> &'static str {
        match self {
            Check::Pages => "pages",
            Check::EmptyPage => "empty-page",
            Check::Text => "text",
            Check::OffPage => "off-page",
            Check::MissingGlyph => "missing-glyph",
            Check::Overlap => "overlap",
            Check::Pictures => "pictures",
            Check::Tables => "tables",
            Check::Dropped => "dropped",
        }
    }
}

/// What a check found wrong: on which page (`0` for the whole document)
/// and why.
type Finding = (usize, String);

/// One corpus file rendered.
struct Rendered {
    path: String,
    fingerprint: Fingerprint,
    findings: BTreeMap<Check, Vec<Finding>>,
}

/// The hashes of a render: one entry per page, then the parts of the PDF
/// that belong to no page.
#[derive(Clone, Debug, PartialEq)]
struct Fingerprint {
    pages: Vec<PageHashes>,
    /// What the content streams name: fonts with their widths, ToUnicode
    /// maps, pictures, graphics states and shadings, metadata left out.
    resources: String,
    /// The outline and the structure tree.
    outline_and_tags: String,
    /// The catalog's own entries.
    catalog: String,
}

#[derive(Clone, Debug, PartialEq)]
struct PageHashes {
    /// The decoded pixels of the page image, with its size.
    pixels: String,
    /// The page's content stream, inflated.
    content: String,
    /// Its link annotations.
    annotations: String,
    /// The rest of its page dictionary.
    page: String,
}

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

fn is_long(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("long-"))
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

/// The decoded pixels of a page image.
struct Pixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Pixels {
    fn decode(png_bytes: &[u8]) -> Self {
        let mut reader = png::Decoder::new(Cursor::new(png_bytes))
            .read_info()
            .expect("the page image is a PNG");
        let mut buffer = vec![0; reader.output_buffer_size().expect("the image fits")];
        let info = reader.next_frame(&mut buffer).expect("the PNG decodes");
        assert_eq!(
            (info.color_type, info.bit_depth),
            (png::ColorType::Rgba, png::BitDepth::Eight),
            "page images are 8-bit RGBA"
        );
        buffer.truncate(info.buffer_size());
        Self {
            width: info.width,
            height: info.height,
            rgba: buffer,
        }
    }

    fn hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.width.to_be_bytes());
        hasher.update(self.height.to_be_bytes());
        hasher.update(&self.rgba);
        hex(&hasher.finalize())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
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

/// An inch, the band at the top and the bottom of a page that holds its
/// header and footer: what `pdf.rs` calls the body text is below and above.
const EDGE: f64 = 72.0;

fn check_empty_pages(pdf: &Pdf, drawn: &[Vec<Drawn>], find: &mut impl FnMut(Check, usize, String)) {
    for (page, pieces) in drawn.iter().enumerate() {
        let (_, height) = pdf.page_size(page);
        let body = |y: f64| y > EDGE && y < height - EDGE;
        let text = pieces
            .iter()
            .any(|piece| body(piece.y) && !piece.text.trim().is_empty());
        let picture = pdf
            .pictures(page)
            .iter()
            .any(|picture| body(picture.rect[1] + picture.rect[3] / 2.0));
        if !text && !picture {
            find(
                Check::EmptyPage,
                page + 1,
                "draws nothing more than an inch from its top and bottom edges".to_owned(),
            );
        }
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
            for word in words(&paragraph.text) {
                *expected.entry(word).or_default() += 1;
            }
        }
    }
    if expected.is_empty() {
        find(Check::Text, 0, "no text read from the document".to_owned());
        return;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for pieces in drawn {
        for line in drawn_lines(pieces) {
            for word in words(&line) {
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

/// The words of a text: runs of letters, runs of digits, and each Chinese,
/// Japanese or Korean character on its own. Numbers are left out: page
/// numbers, sequence numbers and page counts are computed by the render, so
/// the text of their fields is not what the page draws. A digit next to a
/// letter starts a new word, so that a note mark drawn against its word
/// (`rate¹`) still reads as the word.
fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, words: &mut Vec<String>| {
        if !word.is_empty() && !is_number(word) {
            words.push(std::mem::take(word));
        }
        word.clear();
    };
    for ch in text.chars() {
        if is_cjk(ch) {
            flush(&mut word, &mut words);
            words.push(ch.to_string());
        } else if ch.is_alphanumeric() {
            let digit = ch.is_ascii_digit();
            if word
                .chars()
                .last()
                .is_some_and(|last| last.is_ascii_digit() != digit)
            {
                flush(&mut word, &mut words);
            }
            word.push(ch);
        } else {
            flush(&mut word, &mut words);
        }
    }
    flush(&mut word, &mut words);
    words
}

fn is_cjk(ch: char) -> bool {
    matches!(u32::from(ch),
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// Whether a word is a number in digits, or a page number in small Roman
/// numerals as Word writes front matter (`iv`, `xii`): only up to `xxxix`,
/// so that `mix`, `cm` or the pronoun `I` stay words.
fn is_number(word: &str) -> bool {
    if word.chars().all(|ch| ch.is_ascii_digit()) {
        return true;
    }
    if !word.chars().all(|ch| matches!(ch, 'i' | 'v' | 'x')) {
        return false;
    }
    (1..40).any(|value| roman(value) == word)
}

fn roman(mut value: u32) -> String {
    const DIGITS: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (step, digits) in DIGITS {
        while value >= step {
            out.push_str(digits);
            value -= step;
        }
    }
    out
}

/// The text of each line of a page: the pieces drawn one after the other
/// within half a line of each other (a superscript joins its word), joined
/// by a space where a gap separates them.
fn drawn_lines(pieces: &[Drawn]) -> Vec<String> {
    struct Line {
        y: f64,
        size: f64,
        end: f64,
        text: String,
    }
    let mut lines: Vec<Line> = Vec::new();
    for piece in pieces {
        match lines.last_mut() {
            Some(line) if (line.y - piece.y).abs() < 0.5 * line.size.max(piece.size).max(1.0) => {
                if piece.x - line.end > 0.15 * piece.size.max(1.0) {
                    line.text.push(' ');
                }
                line.text.push_str(&piece.text);
                line.end = piece.end;
                line.size = line.size.max(piece.size);
            }
            _ => lines.push(Line {
                y: piece.y,
                size: piece.size,
                end: piece.end,
                text: piece.text.clone(),
            }),
        }
    }
    lines.into_iter().map(|line| line.text).collect()
}

/// One finding for a page (`0` for the whole document) that has any: how
/// many, and the first few.
fn report_page(
    check: Check,
    page: usize,
    what: &str,
    found: &[String],
    find: &mut impl FnMut(Check, usize, String),
) {
    if !found.is_empty() {
        let examples: Vec<&str> = found.iter().take(3).map(String::as_str).collect();
        find(
            check,
            page,
            format!("{} {what}, such as {}", found.len(), examples.join(", ")),
        );
    }
}

fn check_off_page(
    sizes: &[(f64, f64)],
    drawn: &[Vec<Drawn>],
    find: &mut impl FnMut(Check, usize, String),
) {
    for (page, pieces) in drawn.iter().enumerate() {
        let (width, height) = sizes[page];
        let off: Vec<String> = pieces
            .iter()
            .filter(|piece| !piece.text.trim().is_empty())
            .filter(|piece| {
                let (bottom, top) = (piece.y - 0.25 * piece.size, piece.y + 0.75 * piece.size);
                piece.x < -0.5 || piece.end > width + 0.5 || bottom < -0.5 || top > height + 0.5
            })
            .map(|piece| format!("{:?} at ({:.0}, {:.0})", piece.text, piece.x, piece.y))
            .collect();
        report_page(
            Check::OffPage,
            page + 1,
            "pieces of text cross the page edge",
            &off,
            find,
        );
    }
}

fn check_glyphs(drawn: &[Vec<Drawn>], find: &mut impl FnMut(Check, usize, String)) {
    for (page, pieces) in drawn.iter().enumerate() {
        let boxes: Vec<String> = pieces
            .iter()
            .filter(|piece| piece.notdef > 0 || piece.glyphs.contains('\u{FFFD}'))
            .map(|piece| format!("{:?}", piece.text))
            .collect();
        report_page(
            Check::MissingGlyph,
            page + 1,
            "pieces of text with an empty box or a glyph that maps to no text",
            &boxes,
            find,
        );
    }
}

fn check_overlap(drawn: &[Vec<Drawn>], find: &mut impl FnMut(Check, usize, String)) {
    for (page, pieces) in drawn.iter().enumerate() {
        let pieces: Vec<&Drawn> = pieces
            .iter()
            .filter(|piece| !piece.text.trim().is_empty() && piece.size > 0.0)
            .collect();
        let mut over = Vec::new();
        for (index, first) in pieces.iter().enumerate() {
            for second in &pieces[index + 1..] {
                let em = first.size.min(second.size);
                let across = first.end.min(second.end) - first.x.max(second.x);
                if (first.y - second.y).abs() < 0.5 * em && across > em {
                    over.push(format!("{:?} over {:?}", first.text, second.text));
                }
            }
        }
        report_page(
            Check::Overlap,
            page + 1,
            "pieces of text drawn over others",
            &over,
            find,
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
    let drawn = drawn_pictures(pdf);
    if drawn.len() < written.len() {
        find(
            Check::Pictures,
            0,
            format!(
                "{} distinct pictures drawn, {} in the document ({})",
                drawn.len(),
                written.len(),
                written.iter().cloned().collect::<Vec<_>>().join(", ")
            ),
        );
    }
}

/// The distinct pictures a PDF draws, by the hash of their samples.
fn drawn_pictures(pdf: &Pdf) -> HashSet<String> {
    (0..pdf.page_count())
        .flat_map(|page| pdf.pictures(page))
        .map(|picture| sha256(&picture.data))
        .collect()
}

/// A rule is at least half an inch long: a table's rows are wider, an
/// underline under a short word is not counted.
const RULE: f64 = 36.0;

fn check_tables(
    doc: &Document,
    package: &Package,
    pdf: &Pdf,
    find: &mut impl FnMut(Check, usize, String),
) {
    let wanted = rules_wanted(doc, package);
    let rules: usize = (0..pdf.page_count())
        .map(|page| horizontal_rules(pdf, page))
        .sum();
    if rules < wanted {
        find(
            Check::Tables,
            0,
            format!("{rules} horizontal rules drawn, the tables' borders need {wanted}"),
        );
    }
}

/// How many horizontal rules the tables of a document need at least: one a
/// row for a table with borders between its rows (its own or its style's),
/// two for one with a top or bottom border only.
fn rules_wanted(doc: &Document, package: &Package) -> usize {
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
        .sum()
}

fn visible(style: &str) -> bool {
    !matches!(style, "none" | "nil")
}

/// How many horizontal rules a page draws: its stroked horizontal segments
/// and rectangle edges, and its thin filled rectangles, joined where they
/// meet on one line (a table border is drawn one cell edge at a time), and
/// counted when the joined rule is at least [`RULE`] long.
fn horizontal_rules(pdf: &Pdf, page: usize) -> usize {
    let content = pdf.doc.get_page_content(pdf.page_ids()[page]);
    let operations = Content::decode(&content)
        .expect("content stream decodes")
        .operations;
    let number = |object: &Object| f64::from(object.as_float().expect("a number"));
    let apply =
        |m: [f64; 6], (x, y): (f64, f64)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
    let mut ctm = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut saved = Vec::new();
    let mut point = (0.0, 0.0);
    // The path being built: its segments and its rectangles, as two corners.
    let mut segments: Vec<((f64, f64), (f64, f64))> = Vec::new();
    let mut rects: Vec<((f64, f64), (f64, f64))> = Vec::new();
    // Every horizontal piece painted: its height, its left and right ends.
    let mut pieces: Vec<(f64, f64, f64)> = Vec::new();
    for operation in &operations {
        let operands = &operation.operands;
        match operation.operator.as_str() {
            "q" => saved.push(ctm),
            "Q" => ctm = saved.pop().unwrap_or(ctm),
            "cm" => {
                let m: Vec<f64> = operands.iter().map(number).collect();
                ctm = [
                    m[0] * ctm[0] + m[1] * ctm[2],
                    m[0] * ctm[1] + m[1] * ctm[3],
                    m[2] * ctm[0] + m[3] * ctm[2],
                    m[2] * ctm[1] + m[3] * ctm[3],
                    m[4] * ctm[0] + m[5] * ctm[2] + ctm[4],
                    m[4] * ctm[1] + m[5] * ctm[3] + ctm[5],
                ];
            }
            "m" => point = apply(ctm, (number(&operands[0]), number(&operands[1]))),
            "l" => {
                let next = apply(ctm, (number(&operands[0]), number(&operands[1])));
                segments.push((point, next));
                point = next;
            }
            "re" => {
                let [x, y, w, h] = [0, 1, 2, 3].map(|index| number(&operands[index]));
                rects.push((apply(ctm, (x, y)), apply(ctm, (x + w, y + h))));
            }
            "S" | "s" | "B" | "B*" | "b" | "b*" => {
                for (a, b) in segments.drain(..) {
                    if (a.1 - b.1).abs() < 0.5 {
                        pieces.push((a.1, a.0.min(b.0), a.0.max(b.0)));
                    }
                }
                for (a, b) in rects.drain(..) {
                    let (left, right) = (a.0.min(b.0), a.0.max(b.0));
                    pieces.push((a.1, left, right));
                    pieces.push((b.1, left, right));
                }
            }
            "f" | "F" | "f*" => {
                segments.clear();
                for (a, b) in rects.drain(..) {
                    if (a.1 - b.1).abs() <= 3.0 {
                        pieces.push(((a.1 + b.1) / 2.0, a.0.min(b.0), a.0.max(b.0)));
                    }
                }
            }
            "n" => {
                segments.clear();
                rects.clear();
            }
            _ => {}
        }
    }
    pieces.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut rules: Vec<(f64, f64, f64)> = Vec::new();
    for (y, left, right) in pieces {
        let joined = rules
            .iter_mut()
            .find(|rule| (rule.0 - y).abs() < 0.5 && left <= rule.2 + 1.0);
        match joined {
            Some(rule) => rule.2 = rule.2.max(right),
            None => rules.push((y, left, right)),
        }
    }
    rules
        .iter()
        .filter(|(_, left, right)| right - left >= RULE)
        .count()
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

    /// The pictures of the stories a reader sees, by the hash of their
    /// bytes: the body, the shown headers and footers, and the notes.
    fn shown_pictures(&self) -> BTreeSet<String> {
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
        let mut pictures = BTreeSet::new();
        for story in stories {
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
                    pictures.insert(sha256(bytes));
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

/// An XML text without the elements `<name>...</name>`, which do not nest.
fn without(xml: &str, name: &str) -> String {
    let mut out = xml.to_owned();
    for element in elements_with_body(xml, name) {
        out = out.replacen(element, "", 1);
    }
    out
}

/// A part name relative to the folder of the part that names it.
fn resolve(folder: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        folder.split('/').filter(|part| !part.is_empty()).collect()
    };
    for part in target.trim_start_matches('/').split('/') {
        match part {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// The start tags `<name ...>` of an XML text, attributes included.
fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name}");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        if after.starts_with([' ', '>', '/', '\n', '\r', '\t']) {
            let end = after.find('>').map_or(after.len(), |end| end + 1);
            found.push(&rest[start..start + open.len() + end]);
        }
        rest = after;
    }
    found
}

/// The elements `<name ...>...</name>` of an XML text, start tag and body,
/// assuming they do not nest; an empty element `<name .../>` alone.
fn elements_with_body<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    for start_tag in elements(xml, name) {
        let Some(offset) = rest.find(start_tag) else {
            continue;
        };
        let from = &rest[offset..];
        let element = if start_tag.ends_with("/>") {
            start_tag
        } else {
            from.find(&close)
                .map_or(start_tag, |end| &from[..end + close.len()])
        };
        found.push(element);
        rest = &from[start_tag.len()..];
    }
    found
}

fn attribute<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = element.find(&key)? + key.len();
    let end = element[start..].find('"')?;
    Some(&element[start..start + end])
}

/// Whether an on/off element is present and on (`<w:titlePg/>`,
/// `<w:titlePg w:val="true"/>`).
fn on(xml: &str, name: &str) -> bool {
    elements(xml, name)
        .iter()
        .any(|element| !matches!(attribute(element, "w:val"), Some("0" | "false" | "off")))
}

// ---- Fingerprints ----

fn fingerprint(pdf: &Pdf, pixels: &[Pixels]) -> Fingerprint {
    let page_ids = pdf.page_ids();
    let pages = page_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let dict = pdf.doc.get_dictionary(*id).expect("page dictionary");
            let annotations = dict
                .get(b"Annots")
                .map(|annots| canonical(pdf, annots, &[b"P", b"Parent"]))
                .unwrap_or_default();
            let mut rest = dict.clone();
            for key in [&b"Contents"[..], b"Annots", b"Parent", b"Resources"] {
                rest.remove(key);
            }
            PageHashes {
                pixels: pixels.get(index).map_or_else(String::new, Pixels::hash),
                content: sha256(&pdf.doc.get_page_content(*id)),
                annotations: sha256(&annotations),
                page: sha256(&canonical(pdf, &Object::Dictionary(rest), &[])),
            }
        })
        .collect();
    let catalog = pdf.doc.catalog().expect("catalog");
    let outline_and_tags: Vec<u8> = [&b"Outlines"[..], b"StructTreeRoot"]
        .into_iter()
        .flat_map(|key| {
            catalog
                .get(key)
                .map(|object| canonical(pdf, object, &[b"P", b"Parent", b"Prev", b"Last"]))
                .unwrap_or_default()
        })
        .collect();
    let mut own = catalog.clone();
    for key in [&b"Pages"[..], b"Outlines", b"StructTreeRoot", b"Metadata"] {
        own.remove(key);
    }
    Fingerprint {
        pages,
        resources: resources_hash(pdf),
        outline_and_tags: sha256(&outline_and_tags),
        catalog: sha256(&canonical(pdf, &Object::Dictionary(own), &[])),
    }
}

/// What the content streams name and the pixels do not show: for each page,
/// its resource dictionary with every font (widths included), picture,
/// graphics state and shading it refers to, then every other stream
/// (annotation appearances, colour profiles), metadata left out. Streams are
/// inflated, and nothing depends on object numbers.
fn resources_hash(pdf: &Pdf) -> String {
    let mut streams = HashMap::new();
    let mut parts: Vec<String> = pdf
        .page_ids()
        .into_iter()
        .map(|id| {
            let page = pdf.doc.get_dictionary(id).expect("page dictionary");
            page.get(b"Resources")
                .map(|resources| sha256(&canonical_with(pdf, resources, &[], &mut streams)))
                .unwrap_or_default()
        })
        .collect();
    let contents: HashSet<ObjectId> = pdf
        .page_ids()
        .into_iter()
        .flat_map(|page| pdf.doc.get_page_contents(page))
        .collect();
    let mut others: Vec<String> = pdf
        .doc
        .objects
        .iter()
        .filter(|(id, _)| !contents.contains(id) && !streams.contains_key(*id))
        .filter(|(_, object)| {
            object.as_stream().is_ok_and(|stream| {
                let kind = stream.dict.get(b"Type").and_then(Object::as_name).ok();
                !matches!(kind, Some(b"Metadata" | b"XRef" | b"ObjStm"))
            })
        })
        .map(|(_, object)| sha256(&canonical(pdf, object, &[])))
        .collect();
    others.sort();
    parts.push(String::new());
    parts.extend(others);
    sha256(parts.join("\n").as_bytes())
}

/// A PDF object written out with what it refers to, independent of object
/// numbers: a page is written as its index, an object met before as the
/// order it was first met in, a stream as its dictionary and the hash of its
/// inflated bytes (how it is compressed left out), and `skip`ped keys are
/// left out (back references that would loop).
fn canonical(pdf: &Pdf, object: &Object, skip: &[&[u8]]) -> Vec<u8> {
    canonical_with(pdf, object, skip, &mut HashMap::new())
}

/// [`canonical`], with the hashes of the stream bytes already inflated, so
/// that the pages sharing a font inflate it once.
fn canonical_with(
    pdf: &Pdf,
    object: &Object,
    skip: &[&[u8]],
    streams: &mut HashMap<ObjectId, String>,
) -> Vec<u8> {
    struct Writer<'a> {
        pdf: &'a Pdf,
        pages: HashMap<ObjectId, usize>,
        met: HashMap<ObjectId, usize>,
        skip: &'a [&'a [u8]],
        streams: &'a mut HashMap<ObjectId, String>,
        out: Vec<u8>,
    }
    impl Writer<'_> {
        fn write(&mut self, object: &Object, depth: usize) {
            // An outline is a chain of `Next` items, one level each.
            assert!(depth < 4096, "a PDF object nests too deep to hash");
            match object {
                Object::Reference(id) => {
                    if let Some(page) = self.pages.get(id) {
                        self.out.extend(format!("page {page}").as_bytes());
                    } else if let Some(order) = self.met.get(id) {
                        self.out.extend(format!("@{order}").as_bytes());
                    } else {
                        self.met.insert(*id, self.met.len());
                        match self.pdf.doc.get_object(*id) {
                            Ok(Object::Stream(stream)) => {
                                let bytes = self
                                    .streams
                                    .entry(*id)
                                    .or_insert_with(|| stream_bytes_hash(stream))
                                    .clone();
                                self.write_stream(stream, &bytes, depth + 1);
                            }
                            Ok(target) => self.write(target, depth + 1),
                            Err(_) => self.out.extend(b"null"),
                        }
                    }
                }
                Object::Dictionary(dict) => {
                    let mut entries: Vec<_> = dict
                        .iter()
                        .filter(|(key, _)| !self.skip.contains(&key.as_slice()))
                        .collect();
                    entries.sort_by(|a, b| a.0.cmp(b.0));
                    self.out.push(b'<');
                    for (key, value) in entries {
                        self.out.push(b'/');
                        self.out.extend(key.as_slice());
                        self.out.push(b' ');
                        self.write(value, depth + 1);
                        self.out.push(b' ');
                    }
                    self.out.push(b'>');
                }
                Object::Array(items) => {
                    self.out.push(b'[');
                    for item in items {
                        self.write(item, depth + 1);
                        self.out.push(b' ');
                    }
                    self.out.push(b']');
                }
                Object::Stream(stream) => {
                    self.write_stream(stream, &stream_bytes_hash(stream), depth + 1)
                }
                Object::String(bytes, _) => {
                    self.out.push(b'(');
                    self.out.extend(bytes);
                    self.out.push(b')');
                }
                Object::Name(name) => {
                    self.out.push(b'/');
                    self.out.extend(name);
                }
                other => self.out.extend(format!("{other:?}").as_bytes()),
            }
        }

        fn write_stream(&mut self, stream: &lopdf::Stream, bytes: &str, depth: usize) {
            let mut dict = stream.dict.clone();
            for key in [&b"Length"[..], b"Filter", b"DecodeParms"] {
                dict.remove(key);
            }
            self.write(&Object::Dictionary(dict), depth);
            self.out.extend(bytes.as_bytes());
        }
    }
    let pages = pdf
        .page_ids()
        .into_iter()
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect();
    let mut writer = Writer {
        pdf,
        pages,
        met: HashMap::new(),
        skip,
        streams,
        out: Vec::new(),
    };
    writer.write(object, 0);
    writer.out
}

/// The hash of a stream's bytes, inflated when they are compressed.
fn stream_bytes_hash(stream: &lopdf::Stream) -> String {
    sha256(
        &stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone()),
    )
}

// ---- Baselines ----

/// A baseline file as JSON, one page a line, so that a diff shows which
/// pages moved.
fn baseline_json(path: &str, reason: &str, fingerprint: &Fingerprint) -> String {
    let quote = |text: &str| serde_json::to_string(text).expect("a JSON string");
    let mut out = String::from("{\n");
    out.push_str(&format!("  \"file\": {},\n", quote(path)));
    out.push_str(&format!("  \"reason\": {},\n", quote(reason)));
    out.push_str(&format!(
        "  \"resources\": \"{}\",\n",
        fingerprint.resources
    ));
    out.push_str(&format!(
        "  \"outline_and_tags\": \"{}\",\n",
        fingerprint.outline_and_tags
    ));
    out.push_str(&format!("  \"catalog\": \"{}\",\n", fingerprint.catalog));
    out.push_str("  \"pages\": [\n");
    let last = fingerprint.pages.len().saturating_sub(1);
    for (index, page) in fingerprint.pages.iter().enumerate() {
        out.push_str(&format!(
            "    {{\"page\": {}, \"pixels\": \"{}\", \"content\": \"{}\", \"annotations\": \"{}\", \"dictionary\": \"{}\"}}{}\n",
            index + 1,
            page.pixels,
            page.content,
            page.annotations,
            page.page,
            if index == last { "" } else { "," }
        ));
    }
    out.push_str("  ]\n}\n");
    out
}

/// A baseline file read back: its reason and fingerprint.
fn read_baseline(path: &str) -> Option<(String, Fingerprint)> {
    let text = fs::read_to_string(baseline_path(path)).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).expect("a baseline file is JSON");
    let field = |value: &serde_json::Value, key: &str| {
        value[key]
            .as_str()
            .unwrap_or_else(|| panic!("{path}: baseline field {key}"))
            .to_owned()
    };
    let pages = json["pages"]
        .as_array()
        .expect("baseline pages")
        .iter()
        .map(|page| PageHashes {
            pixels: field(page, "pixels"),
            content: field(page, "content"),
            annotations: field(page, "annotations"),
            page: field(page, "dictionary"),
        })
        .collect();
    Some((
        field(&json, "reason"),
        Fingerprint {
            pages,
            resources: field(&json, "resources"),
            outline_and_tags: field(&json, "outline_and_tags"),
            catalog: field(&json, "catalog"),
        },
    ))
}

/// What moved between a baseline and a render, one line per page and
/// entry; empty when nothing did.
fn moved(expected: &Fingerprint, found: &Fingerprint) -> Vec<String> {
    let mut lines = Vec::new();
    if expected.pages.len() != found.pages.len() {
        lines.push(format!(
            "{} pages, the baseline has {}",
            found.pages.len(),
            expected.pages.len()
        ));
    }
    for (index, (was, now)) in expected.pages.iter().zip(&found.pages).enumerate() {
        let entries = [
            ("pixels", &was.pixels, &now.pixels),
            ("content", &was.content, &now.content),
            ("annotations", &was.annotations, &now.annotations),
            ("dictionary", &was.page, &now.page),
        ];
        let changed: Vec<&str> = entries
            .iter()
            .filter(|(_, was, now)| was != now)
            .map(|(name, ..)| *name)
            .collect();
        if !changed.is_empty() {
            lines.push(format!("page {}: {}", index + 1, changed.join(", ")));
        }
    }
    for (name, was, now) in [
        ("resources", &expected.resources, &found.resources),
        (
            "outline and tags",
            &expected.outline_and_tags,
            &found.outline_and_tags,
        ),
        ("catalog", &expected.catalog, &found.catalog),
    ] {
        if was != now {
            lines.push(name.to_owned());
        }
    }
    lines
}

// ---- Known failures ----

/// An entry of `tests/render/known.toml`: a file that fails a check on some
/// pages, because of an issue or by design.
#[derive(Debug, Default)]
struct Known {
    file: String,
    check: String,
    pages: Vec<usize>,
    /// For the whole document (page 0), how its finding starts, such as
    /// `43 words`: a finding that grows or shrinks is not the known one.
    finding: Option<String>,
    /// Whether it names its issue or says why it is by design.
    explained: bool,
}

/// The entries of a `known.toml` text, which `[[known]]` starts and one
/// `key = value` a line fills, each checked: a corpus file, a check, pages,
/// an issue or a reason, and the finding of a whole-document entry.
fn parse_known(text: &str, corpus: &BTreeSet<String>) -> Result<Vec<Known>, String> {
    let mut entries: Vec<Known> = Vec::new();
    for line in text.lines().map(str::trim) {
        let value = |key: &str| {
            line.strip_prefix(key)
                .filter(|rest| rest.trim_start().starts_with('='))
                .map(|rest| {
                    rest.trim_start_matches([' ', '='])
                        .trim()
                        .trim_matches('"')
                        .to_owned()
                })
        };
        if line == "[[known]]" {
            entries.push(Known::default());
            continue;
        }
        let Some(entry) = entries.last_mut() else {
            continue;
        };
        if let Some(file) = value("file") {
            entry.file = file;
        } else if let Some(check) = value("check") {
            entry.check = check;
        } else if let Some(pages) = value("pages") {
            entry.pages = pages
                .trim_matches(['[', ']'])
                .split(',')
                .map(str::trim)
                .filter(|page| !page.is_empty())
                .map(|page| page.parse().map_err(|_| format!("pages {pages}")))
                .collect::<Result<_, _>>()?;
        } else if let Some(finding) = value("finding") {
            entry.finding = Some(finding);
        } else if value("issue").is_some() || value("by_design").is_some() {
            entry.explained = true;
        }
    }
    for entry in &entries {
        let name = format!("{} {}", entry.file, entry.check);
        if !corpus.contains(&entry.file) {
            return Err(format!("{name}: no corpus file is named {}", entry.file));
        }
        if !Check::ALL.iter().any(|check| check.name() == entry.check) {
            return Err(format!("{name}: no check is named {:?}", entry.check));
        }
        if entry.pages.is_empty() {
            return Err(format!("{name}: no pages"));
        }
        if entry.pages.contains(&0) && entry.finding.is_none() {
            return Err(format!(
                "{name}: a whole-document entry says how its finding starts"
            ));
        }
        if !entry.explained {
            return Err(format!("{name}: neither an issue nor why it is by design"));
        }
    }
    Ok(entries)
}

fn known() -> Vec<Known> {
    let text = fs::read_to_string(render_root().join("known.toml")).unwrap_or_default();
    let corpus: BTreeSet<String> = crate::support::corpus_manifest()
        .into_iter()
        .map(|file| file.path)
        .collect();
    parse_known(&text, &corpus).unwrap_or_else(|error| panic!("tests/render/known.toml: {error}"))
}

/// Runs one check over the rendered files: every finding must be a known
/// failure, and every known failure of this check must still be found.
fn assert_check(check: Check) {
    let rendered = rendered();
    if rendered.is_empty() {
        return;
    }
    let known: Vec<Known> = known()
        .into_iter()
        .filter(|entry| entry.check == check.name())
        .collect();
    let mut problems = Vec::new();
    for render in rendered {
        let render = match render {
            Ok(render) => render,
            Err(error) => {
                problems.push(error.clone());
                continue;
            }
        };
        let findings = render.findings.get(&check).cloned().unwrap_or_default();
        let found: BTreeSet<usize> = findings.iter().map(|(page, _)| *page).collect();
        let listed: BTreeSet<usize> = known
            .iter()
            .filter(|entry| entry.file == render.path)
            .flat_map(|entry| entry.pages.iter().copied())
            .collect();
        for (page, why) in &findings {
            if !listed.contains(page) {
                problems.push(format!("{} page {page}: {why}", render.path));
            }
            let expected = known
                .iter()
                .filter(|entry| entry.file == render.path && entry.pages.contains(page))
                .find_map(|entry| entry.finding.as_deref());
            if let Some(expected) = expected
                && !why.starts_with(expected)
            {
                problems.push(format!(
                    "{} page {page}: {why}; tests/render/known.toml expects {expected:?}",
                    render.path
                ));
            }
        }
        for page in listed.difference(&found) {
            problems.push(format!(
                "{} page {page}: passes now, remove it from tests/render/known.toml",
                render.path
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} ({}):\n{}",
        check.name(),
        problems.len(),
        problems.join("\n")
    );
}

// ---- Tests ----

#[test]
fn every_corpus_render_matches_its_baseline() {
    let rendered = rendered();
    if rendered.is_empty() {
        return;
    }
    let update = update_reason();
    let mut problems = Vec::new();
    for render in rendered {
        let render = match render {
            Ok(render) => render,
            Err(error) => {
                problems.push(error.clone());
                continue;
            }
        };
        let baseline = read_baseline(&render.path);
        let changes = match &baseline {
            Some((_, expected)) => moved(expected, &render.fingerprint),
            None => vec!["no baseline file".to_owned()],
        };
        if changes.is_empty() {
            continue;
        }
        match &update {
            Some(reason) => {
                let path = baseline_path(&render.path);
                fs::create_dir_all(path.parent().expect("a folder")).expect("baseline folder");
                fs::write(
                    &path,
                    baseline_json(&render.path, reason, &render.fingerprint),
                )
                .expect("baseline file written");
            }
            None => problems.push(format!("{}: {}", render.path, changes.join("; "))),
        }
    }
    if update.is_some() {
        // Baseline files of corpus files that are gone go with them.
        let docx: BTreeSet<String> = corpus_files("docx")
            .into_iter()
            .map(|file| file.path)
            .collect();
        for path in baseline_files() {
            if path.ends_with(".docx") && !docx.contains(&path) {
                fs::remove_file(baseline_path(&path)).expect("stale baseline removed");
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the render moved ({} files); if that is intended, rewrite the baselines with \
         OXOFFICE_RENDER_UPDATE=\"<reason>\" cargo test --profile ci -p oxoffice-docx --test docx render\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// The corpus paths that have a baseline file.
fn baseline_files() -> Vec<String> {
    fn walk(folder: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(folder) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("folder entry").path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if let Some(name) = path.strip_prefix(root).ok().and_then(|p| p.to_str()) {
                out.push(name.trim_end_matches(".json").replace('\\', "/"));
            }
        }
    }
    let root = render_root().join("baseline");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

#[test]
fn every_corpus_file_has_a_baseline_and_every_baseline_a_corpus_file() {
    if render_set().is_none() || update_reason().is_some() {
        return;
    }
    let docx: BTreeSet<String> = corpus_files("docx")
        .into_iter()
        .map(|file| file.path)
        .collect();
    let baselines: BTreeSet<String> = baseline_files()
        .into_iter()
        .filter(|path| path.ends_with(".docx"))
        .collect();
    assert_eq!(
        docx.difference(&baselines).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "corpus files without a baseline file"
    );
    assert_eq!(
        baselines.difference(&docx).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "baseline files without a corpus file"
    );
}

#[test]
fn the_render_files_stay_within_their_budget() {
    fn size(folder: &Path) -> u64 {
        fs::read_dir(folder).map_or(0, |entries| {
            entries
                .map(|entry| entry.expect("folder entry").path())
                .filter(|path| !path.ends_with("references"))
                .map(|path| {
                    if path.is_dir() {
                        size(&path)
                    } else {
                        fs::metadata(&path).map_or(0, |meta| meta.len())
                    }
                })
                .sum()
        })
    }
    let total = size(&render_root());
    assert!(
        total <= 1024 * 1024,
        "tests/render holds {total} bytes without its references, over its 1 MiB budget"
    );
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
    assert_check(Check::Pages);
}

#[test]
fn no_page_of_a_corpus_render_draws_only_its_header_and_footer() {
    assert_check(Check::EmptyPage);
}

#[test]
fn every_word_of_a_corpus_document_is_drawn() {
    assert_check(Check::Text);
}

#[test]
fn no_text_of_a_corpus_render_crosses_its_page_edge() {
    assert_check(Check::OffPage);
}

#[test]
fn no_glyph_of_a_corpus_render_is_an_empty_box() {
    assert_check(Check::MissingGlyph);
}

#[test]
fn no_text_of_a_corpus_render_is_drawn_over_other_text() {
    assert_check(Check::Overlap);
}

#[test]
fn every_picture_of_a_corpus_document_is_drawn_inside_its_page() {
    assert_check(Check::Pictures);
}

#[test]
fn the_tables_of_a_corpus_render_draw_their_borders() {
    assert_check(Check::Tables);
}

#[test]
fn a_corpus_render_leaves_nothing_out() {
    assert_check(Check::Dropped);
}

#[test]
fn words_leave_numbers_out_and_split_chinese_by_character() {
    assert_eq!(
        words("Page 12 of iv: mid civil XII, I mix 3 cm"),
        ["Page", "of", "mid", "civil", "XII", "I", "mix", "cm"]
    );
    assert_eq!(words("rate1 2Kyoto CO2"), ["rate", "Kyoto", "CO"]);
    assert_eq!(words("会议纪要 Hello"), ["会", "议", "纪", "要", "Hello"]);
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
        assert_eq!(rules_wanted(&doc, &package), wanted, "{border} borders");
        let drawn = horizontal_rules(&draw(&doc).expect("the table draws").pdf, 0);
        // Three cells a row, each with its own top and bottom edge: one rule
        // above each row and one below the last.
        let rules = if wanted == 0 { 0 } else { wanted + 1 };
        assert_eq!(drawn, rules, "{border} borders");
    }
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
    }
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
    let doc = crate::support::corpus_doc("word/report.docx");
    let mut drawing = draw(&doc).expect("the report draws");
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
    let doc = crate::support::corpus_doc("authored/thesis.docx");
    let mut drawing = draw(&doc).expect("the thesis draws");
    let mut found = Vec::new();
    check_pages(
        &doc,
        &drawing.pdf,
        &drawing.pixels,
        &mut |check, page, _| found.push((check, page)),
    );
    assert!(found.is_empty(), "the thesis has the sizes of its sections");
    // The renderer loses the orientation of the landscape section: all its
    // pages come out portrait.
    let landscape: Vec<usize> = (0..drawing.pdf.page_count())
        .filter(|page| {
            let (width, height) = drawing.pdf.page_size(*page);
            width > height
        })
        .collect();
    assert!(!landscape.is_empty(), "the thesis has a landscape section");
    for page in landscape {
        let id = drawing.pdf.page_ids()[page];
        let (width, height) = drawing.pdf.page_size(page);
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
    }
    check_pages(
        &doc,
        &drawing.pdf,
        &drawing.pixels,
        &mut |check, page, _| found.push((check, page)),
    );
    assert_eq!(found, [(Check::Pages, 0)]);
}
