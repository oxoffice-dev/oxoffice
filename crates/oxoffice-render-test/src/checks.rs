//! The checks a coherent render passes, read from its PDF, the files of the
//! render tests under `tests/render/`, and the runner of their tests.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use lopdf::Object;
use lopdf::content::Content;

use crate::fingerprint::{Fingerprint, baseline_json, moved, read_baseline, sha256};
use crate::pdf::{Drawn, Pdf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Set {
    Subset,
    Full,
}

/// The reason of a baseline update, when this run rewrites the baselines.
pub fn update_reason() -> Option<String> {
    let reason = std::env::var("OXOFFICE_RENDER_UPDATE").ok()?;
    let reason = reason.trim();
    assert!(
        !reason.is_empty(),
        "OXOFFICE_RENDER_UPDATE needs the reason of the update"
    );
    Some(reason.to_owned())
}

pub fn render_set() -> Option<Set> {
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

pub fn render_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/render")
}

/// The baseline file of a corpus file: `word/report.docx` has
/// `tests/render/baseline/word/report.docx.json`.
pub fn baseline_path(corpus_path: &str) -> PathBuf {
    render_root()
        .join("baseline")
        .join(format!("{corpus_path}.json"))
}

/// A check every render passes, unless `tests/render/known.toml` lists the
/// file and the pages it fails on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
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
    pub const ALL: [Check; 9] = [
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

    pub fn name(self) -> &'static str {
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
pub type Finding = (usize, String);

/// One corpus file rendered.
pub struct Rendered {
    pub path: String,
    pub fingerprint: Fingerprint,
    pub findings: BTreeMap<Check, Vec<Finding>>,
}

pub fn is_long(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("long-"))
}

/// An inch, the band at the top and the bottom of a page that holds its
/// header and footer: what `pdf.rs` calls the body text is below and above.
pub const EDGE: f64 = 72.0;

pub fn check_empty_pages(
    pdf: &Pdf,
    drawn: &[Vec<Drawn>],
    find: &mut impl FnMut(Check, usize, String),
) {
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

/// The words of a text: runs of letters, runs of digits, and each Chinese,
/// Japanese or Korean character on its own. A digit next to a letter starts
/// a new word, so that a note mark drawn against its word (`rate¹`) still
/// reads as the word.
pub fn all_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, words: &mut Vec<String>| {
        if !word.is_empty() {
            words.push(std::mem::take(word));
        }
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

pub fn is_cjk(ch: char) -> bool {
    matches!(u32::from(ch),
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// The text of each line of a page: the pieces drawn one after the other
/// within half a line of each other (a superscript joins its word), joined
/// by a space where a gap separates them.
pub fn drawn_lines(pieces: &[Drawn]) -> Vec<String> {
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
pub fn report_page(
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

pub fn check_off_page(
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

pub fn check_glyphs(drawn: &[Vec<Drawn>], find: &mut impl FnMut(Check, usize, String)) {
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

pub fn check_overlap(drawn: &[Vec<Drawn>], find: &mut impl FnMut(Check, usize, String)) {
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

/// The distinct pictures a PDF draws, by the hash of their samples.
pub fn drawn_pictures(pdf: &Pdf) -> HashSet<String> {
    (0..pdf.page_count())
        .flat_map(|page| pdf.pictures(page))
        .map(|picture| sha256(&picture.data))
        .collect()
}

/// A rule is at least half an inch long: a table's rows are wider, an
/// underline under a short word is not counted.
pub const RULE: f64 = 36.0;

/// How many horizontal rules a page draws: see [`horizontal_rule_heights`].
pub fn horizontal_rules(pdf: &Pdf, page: usize) -> usize {
    horizontal_rule_heights(pdf, page).len()
}

/// The height of each horizontal rule a page draws, in default user space:
/// its stroked horizontal segments and rectangle edges, and its thin filled
/// rectangles, joined where they meet on one line (a table border is drawn
/// one cell edge at a time), and kept when the joined rule is at least
/// [`RULE`] long.
pub fn horizontal_rule_heights(pdf: &Pdf, page: usize) -> Vec<f64> {
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
        .map(|(y, _, _)| *y)
        .collect()
}

/// How many horizontal rules each table of a tagged PDF draws, its tables
/// in reading order, a nested table counted with the table around it. A
/// table's rules are those of each page it draws text on, between the text
/// drawn just above its own and the text drawn just below: so the borders of
/// its rows without text count, and those of another table do not while
/// text stands between the two.
pub fn table_rules(pdf: &Pdf) -> Vec<usize> {
    let elements = pdf.structure();
    let pages: Vec<(Vec<Drawn>, Vec<f64>)> = (0..pdf.page_count())
        .map(|page| (pdf.drawn(page), horizontal_rule_heights(pdf, page)))
        .collect();
    let mut counts = Vec::new();
    let mut index = 0;
    while index < elements.len() {
        let (role, depth, _, marks) = &elements[index];
        if role != "Table" {
            index += 1;
            continue;
        }
        let mut own: HashSet<(usize, i64)> = marks.iter().copied().collect();
        let mut end = index + 1;
        while end < elements.len() && elements[end].1 > *depth {
            own.extend(elements[end].3.iter().copied());
            end += 1;
        }
        let mut count = 0;
        for (page, (drawn, rules)) in pages.iter().enumerate() {
            let (inside, outside): (Vec<&Drawn>, Vec<&Drawn>) = drawn
                .iter()
                .partition(|piece| piece.mcid.is_some_and(|mcid| own.contains(&(page, mcid))));
            let Some(top) = inside.iter().map(|piece| piece.y).reduce(f64::max) else {
                continue;
            };
            let bottom = inside.iter().map(|piece| piece.y).fold(top, f64::min);
            let above = outside
                .iter()
                .map(|piece| piece.y)
                .filter(|y| *y > top)
                .fold(f64::INFINITY, f64::min);
            let below = outside
                .iter()
                .map(|piece| piece.y)
                .filter(|y| *y < bottom)
                .fold(f64::NEG_INFINITY, f64::max);
            count += rules.iter().filter(|y| below < **y && **y < above).count();
        }
        counts.push(count);
        index = end;
    }
    counts
}

/// An entry of `tests/render/known.toml`: a file that fails a check on some
/// pages, because of an issue or by design.
#[derive(Debug, Default)]
pub struct Known {
    pub file: String,
    pub check: String,
    pub pages: Vec<usize>,
    /// For the whole document (page 0), how its finding starts, such as
    /// `43 words`: a finding that grows or shrinks is not the known one.
    pub finding: Option<String>,
    /// Whether it names its issue or says why it is by design.
    pub explained: bool,
}

/// The entries of a `known.toml` text, which `[[known]]` starts and one
/// `key = value` a line fills, each checked: a corpus file, a check, pages,
/// an issue or a reason, and the finding of a whole-document entry.
pub fn parse_known(text: &str, corpus: &BTreeSet<String>) -> Result<Vec<Known>, String> {
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

/// The entries of `tests/render/known.toml`, checked against the paths of
/// the corpus files.
pub fn known(corpus: &BTreeSet<String>) -> Vec<Known> {
    let text = fs::read_to_string(render_root().join("known.toml")).unwrap_or_default();
    parse_known(&text, corpus).unwrap_or_else(|error| panic!("tests/render/known.toml: {error}"))
}

/// Runs one check over the rendered files: every finding must be a known
/// failure, and every known failure of this check must still be found.
/// `corpus` holds the paths of every corpus file.
pub fn assert_check(
    check: Check,
    rendered: &[Result<Rendered, String>],
    corpus: &BTreeSet<String>,
) {
    if rendered.is_empty() {
        return;
    }
    let known: Vec<Known> = known(corpus)
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

/// The corpus paths that have a baseline file.
pub fn baseline_files() -> Vec<String> {
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

/// Compares each rendered file with its baseline file, or rewrites the
/// baseline files that moved when `OXOFFICE_RENDER_UPDATE` gives a reason
/// (and removes those of corpus files that are gone). `extension` is the
/// format of the files (`docx`), `corpus` the paths of its corpus files.
pub fn assert_baselines(
    rendered: &[Result<Rendered, String>],
    extension: &str,
    corpus: &BTreeSet<String>,
) {
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
        for path in baseline_files() {
            if path.ends_with(&format!(".{extension}")) && !corpus.contains(&path) {
                fs::remove_file(baseline_path(&path)).expect("stale baseline removed");
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the render moved ({} files); if that is intended, rewrite the baselines with \
         OXOFFICE_RENDER_UPDATE=\"<reason>\" cargo test --profile ci -p oxoffice-{extension} --test {extension} render\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// Every corpus file of a format has a baseline file, and every baseline
/// file a corpus file.
pub fn assert_every_file_has_a_baseline(extension: &str, corpus: &BTreeSet<String>) {
    if render_set().is_none() || update_reason().is_some() {
        return;
    }
    let baselines: BTreeSet<String> = baseline_files()
        .into_iter()
        .filter(|path| path.ends_with(&format!(".{extension}")))
        .collect();
    assert_eq!(
        corpus.difference(&baselines).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "corpus files without a baseline file"
    );
    assert_eq!(
        baselines.difference(corpus).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "baseline files without a corpus file"
    );
}

/// `tests/render/` stays within its 1 MiB budget, its references aside.
pub fn assert_render_files_within_budget() {
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
