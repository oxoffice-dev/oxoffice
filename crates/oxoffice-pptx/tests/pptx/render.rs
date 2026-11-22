//! Every PowerPoint file of the corpus rendered as `convert` and `render` do
//! it, with the bundled fonts: the checks a coherent render passes, read back
//! from the PDF and the slide images, and the hashes that say whether the
//! render changed. See `specs/2026-11-21-render-fidelity.md`; the shared
//! pieces live in `oxoffice-render-test`.
//!
//! `OXOFFICE_RENDER` chooses the files: `subset` (all but the `long-*`
//! family), `full`, or `off`. Unset means `off`, and fails under CI.
//! `OXOFFICE_RENDER_UPDATE="<reason>"` renders every file and rewrites the
//! baseline files that changed instead of checking them:
//!
//! ```text
//! OXOFFICE_RENDER_UPDATE="<reason>" cargo test --profile ci -p oxoffice-pptx --test pptx render
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Cursor, Read};
use std::sync::OnceLock;

use oxoffice_pptx::Presentation;
use oxoffice_render_test::checks::{
    Check, Finding, Rendered, Set, all_words, assert_baselines, assert_check,
    assert_every_file_has_a_baseline, check_glyphs, check_off_page, check_overlap, drawn_lines,
    horizontal_rules, is_long, render_set, report_page,
};
use oxoffice_render_test::fingerprint::{Pixels, fingerprint, moved};
use oxoffice_render_test::pdf::{Drawn, Pdf};
use oxoffice_render_test::xml::{attribute, elements, elements_with_body, resolve, without};

use crate::support::{CorpusFile, corpus_deck, corpus_files, corpus_manifest, map_in_parallel};

/// Slide images are rendered at this resolution, as small as still shows a
/// moved line.
const DPI: f64 = 72.0;

/// The paths of the PowerPoint files of the corpus.
fn pptx_corpus() -> BTreeSet<String> {
    corpus_files("pptx")
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

// ---- Rendering ----

/// Every file of the chosen set rendered once per test binary run, shared
/// by the tests of this module.
fn rendered() -> &'static [Result<Rendered, String>] {
    static RENDERED: OnceLock<Vec<Result<Rendered, String>>> = OnceLock::new();
    RENDERED.get_or_init(|| {
        let Some(set) = render_set() else {
            return Vec::new();
        };
        let files: Vec<CorpusFile> = corpus_files("pptx")
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

/// A deck drawn as `convert` and `render` draw it, from one layout: its PDF
/// read back, its slide images decoded, and the renderer's diagnostics.
struct Drawing {
    pdf: Pdf,
    pixels: Vec<Pixels>,
    diagnostics: Vec<String>,
}

fn draw(deck: &Presentation) -> Result<Drawing, String> {
    let (_, layout) = deck
        .render_deterministic()
        .map_err(|e| format!("layout: {e}"))?;
    let pdf_bytes =
        oxoffice_pdf::render_to_pdf_checked(&layout).map_err(|e| format!("PDF: {e}"))?;
    let pixels = (0..layout.pages.len())
        .map(|page| {
            oxoffice_pdf::render_page_to_png(&layout, page, DPI)
                .map(|image| Pixels::decode(&image))
                .ok_or_else(|| format!("PNG: slide {} has no image", page + 1))
        })
        .collect::<Result<_, _>>()?;
    Ok(Drawing {
        pdf: Pdf::read(&pdf_bytes),
        pixels,
        diagnostics: layout
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect(),
    })
}

fn render(file: &CorpusFile) -> Result<Rendered, String> {
    let bytes = file.bytes();
    let deck = Presentation::from_bytes(&bytes).map_err(|e| format!("{}: open: {e}", file.path))?;
    let Drawing {
        pdf,
        pixels,
        diagnostics,
    } = draw(&deck).map_err(|error| format!("{}: {error}", file.path))?;
    let drawn: Vec<Vec<Drawn>> = (0..pdf.page_count()).map(|page| pdf.drawn(page)).collect();

    let mut findings: BTreeMap<Check, Vec<Finding>> = BTreeMap::new();
    let mut find = |check: Check, page: usize, why: String| {
        findings.entry(check).or_default().push((page, why));
    };
    let sizes: Vec<(f64, f64)> = (0..pdf.page_count())
        .map(|page| pdf.page_size(page))
        .collect();
    let slides = slide_facts(&bytes);
    check_slides(&deck, &pdf, &pixels, &mut find);
    check_slide_text(&deck, &slides, &drawn, &mut find);
    check_off_page(&sizes, &drawn, &mut find);
    check_glyphs(&drawn, &mut find);
    check_overlap(&drawn, &mut find);
    check_slide_pictures(&slides, &pdf, &mut find);
    check_slide_tables(&slides, &pdf, &mut find);
    check_dropped(&diagnostics, &mut find);

    Ok(Rendered {
        path: file.path.clone(),
        fingerprint: fingerprint(&pdf, &pixels),
        findings,
    })
}

// ---- The checks ----

/// One page and one image per slide, hidden slides included as the render
/// draws them, each of the slide size.
fn check_slides(
    deck: &Presentation,
    pdf: &Pdf,
    pixels: &[Pixels],
    find: &mut impl FnMut(Check, usize, String),
) {
    let slides = deck.slides().len();
    if pdf.page_count() != slides || pixels.len() != slides {
        find(
            Check::Pages,
            0,
            format!(
                "{} PDF pages and {} images for {slides} slides",
                pdf.page_count(),
                pixels.len()
            ),
        );
    }
    // A deck without a size takes 10 by 7.5 inches, as the layout does.
    let (width, height) = deck.slide_size().map_or((720.0, 540.0), |(width, height)| {
        (width.0 as f64 / 12_700.0, height.0 as f64 / 12_700.0)
    });
    for page in 0..pdf.page_count() {
        let (w, h) = pdf.page_size(page);
        if (w - width).abs() > 0.5 || (h - height).abs() > 0.5 {
            find(
                Check::Pages,
                page + 1,
                format!("{w} x {h} pt, the slides are {width} x {height}"),
            );
        }
    }
}

/// Every word of a slide's visible shapes and tables (its notes and hidden
/// shapes left out) is drawn on that slide's page, numbers included: a slide
/// number field draws the slide's number whatever its cached text says, as
/// the render computes it; dates read as cached.
fn check_slide_text(
    deck: &Presentation,
    slides: &[SlideFacts],
    drawn: &[Vec<Drawn>],
    find: &mut impl FnMut(Check, usize, String),
) {
    let mut read = 0;
    for (index, slide) in deck.slides().enumerate() {
        let expected = expected_words(&slide.text(), slides.get(index));
        read += expected.values().sum::<usize>();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for line in drawn
            .get(index)
            .map(|pieces| drawn_lines(pieces))
            .unwrap_or_default()
        {
            for word in all_words(&line) {
                *seen.entry(word).or_default() += 1;
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
        missing.sort();
        report_page(
            Check::Text,
            index + 1,
            "words not drawn on their slide as often as written",
            &missing,
            find,
        );
    }
    if read == 0 {
        find(Check::Text, 0, "no text read from the deck".to_owned());
    }
}

/// The words a slide's text must draw, with their counts: its text, less
/// the words of its hidden shapes and of its slide number fields' cache,
/// plus the slide's number once per slide number field.
fn expected_words(text: &str, facts: Option<&SlideFacts>) -> HashMap<String, usize> {
    let mut expected: HashMap<String, usize> = HashMap::new();
    for word in all_words(text) {
        *expected.entry(word).or_default() += 1;
    }
    let skipped = facts
        .into_iter()
        .flat_map(|facts| facts.hidden_words.iter().chain(&facts.slide_number_words));
    for word in skipped {
        if let Some(count) = expected.get_mut(word) {
            *count = count.saturating_sub(1);
        }
    }
    if let Some(facts) = facts {
        for _ in 0..facts.slide_numbers {
            for word in all_words(&facts.number.to_string()) {
                *expected.entry(word).or_default() += 1;
            }
        }
    }
    expected.retain(|_, count| *count > 0);
    expected
}

/// What the checks read from a slide part: each picture its visible shapes
/// show, how many of them it places wholly outside the slide, the
/// horizontal rules its tables need, the words of the shapes it hides, and
/// its slide number fields with the number the slide takes.
#[derive(Debug, Default)]
struct SlideFacts {
    pictures: Vec<PictureFact>,
    parked: usize,
    rules: usize,
    hidden_words: Vec<String>,
    slide_numbers: usize,
    slide_number_words: Vec<String>,
    number: usize,
}

/// A picture by its pixel size, and whether it is a metafile, which the
/// render draws as a raster a whole number of times its size.
#[derive(Clone, Copy, Debug)]
struct PictureFact {
    pixels: (u32, u32),
    emf: bool,
}

impl PictureFact {
    /// Whether a picture drawn with these pixels is this one: the same size,
    /// or for an EMF the size times the render's scale, 1024 over its larger
    /// side, kept within 1 to 8 (`decode_emf` in `oxoffice-pdf/src/image.rs`).
    fn drawn_as(&self, (w, h): (i64, i64)) -> bool {
        let (pw, ph) = (i64::from(self.pixels.0), i64::from(self.pixels.1));
        let scale = if self.emf {
            (1024 / pw.max(ph).max(1)).clamp(1, 8)
        } else {
            1
        };
        (w, h) == (scale * pw, scale * ph)
    }
}

/// The facts of each slide, in the order of the deck.
fn slide_facts(bytes: &[u8]) -> Vec<SlideFacts> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("package is a ZIP");
    let mut parts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).expect("ZIP entry");
        let name = entry.name().expect("ZIP entry name").into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("ZIP entry reads");
        parts.insert(name, data);
    }
    let text = |name: &str| {
        parts
            .get(name)
            .map(|data| String::from_utf8_lossy(data).into_owned())
            .unwrap_or_default()
    };
    let relationships = |part: &str| -> HashMap<String, String> {
        let (folder, file) = part.rsplit_once('/').unwrap_or(("", part));
        let rels = text(&format!("{folder}/_rels/{file}.rels"));
        elements(&rels, "Relationship")
            .into_iter()
            .filter_map(|element| {
                Some((
                    attribute(element, "Id")?.to_owned(),
                    resolve(folder, attribute(element, "Target")?),
                ))
            })
            .collect()
    };
    let presentation = relationships("ppt/presentation.xml");
    let presentation_xml = text("ppt/presentation.xml");
    let styles = table_styles(&text("ppt/tableStyles.xml"));
    // A deck without a size takes 10 by 7.5 inches, as the layout does.
    let size = elements(&presentation_xml, "p:sldSz")
        .first()
        .and_then(|size| Some((emu(size, "cx")?, emu(size, "cy")?)))
        .unwrap_or((9_144_000, 6_858_000));
    let first = elements(&presentation_xml, "p:presentation")
        .first()
        .and_then(|element| attribute(element, "firstSlideNum")?.parse().ok())
        .unwrap_or(1);
    elements(&presentation_xml, "p:sldId")
        .into_iter()
        .filter_map(|slide| presentation.get(attribute(slide, "r:id")?).cloned())
        .enumerate()
        .map(|(index, part)| {
            // The render draws the choice of an alternate content, never its
            // fallback, and nothing a slide hides.
            let (xml, hidden) = split_hidden(&without(&text(&part), "mc:Fallback"));
            let targets = relationships(&part);
            let pictures = elements_with_body(&xml, "p:pic")
                .into_iter()
                .flat_map(|picture| elements(picture, "a:blip"))
                .filter_map(|blip| parts.get(targets.get(attribute(blip, "r:embed")?)?))
                .filter_map(|media| oxoffice_media::probe(media))
                .map(|info| PictureFact {
                    pixels: (info.width_px, info.height_px),
                    emf: info.format == oxoffice_media::ImageFormat::Emf,
                })
                .collect();
            // A picture in a group sits in the group's frame: a group the
            // slide places wholly off it parks all its pictures, and a picture
            // inside a group on the slide counts as on it.
            let top_level = groups_removed(&xml);
            let parked = elements_with_body(&top_level, "p:pic")
                .into_iter()
                .filter(|picture| outside(picture, size))
                .count()
                + top_level_groups(&xml)
                    .into_iter()
                    .filter(|group| outside(group, size))
                    .map(|group| elements_with_body(group, "p:pic").len())
                    .sum::<usize>();
            let rules = elements_with_body(&xml, "a:tbl")
                .into_iter()
                .map(|table| table_rules(table, &styles))
                .sum();
            let hidden_words = hidden
                .iter()
                .flat_map(|fragment| all_words(&texts(fragment)))
                .collect();
            // The render computes a slidenum field, whatever the case of its
            // type, and an untyped field in a slide number placeholder.
            let slide_number_shapes: Vec<&str> = elements_with_body(&xml, "p:sp")
                .into_iter()
                .filter(|shape| {
                    elements(shape, "p:ph")
                        .first()
                        .is_some_and(|ph| attribute(ph, "type") == Some("sldNum"))
                })
                .collect();
            let fields: Vec<&str> = elements_with_body(&xml, "a:fld")
                .into_iter()
                .filter(|field| {
                    let kind = elements(field, "a:fld")
                        .first()
                        .and_then(|start| attribute(start, "type"));
                    match kind {
                        Some(kind) => kind.eq_ignore_ascii_case("slidenum"),
                        None => slide_number_shapes.iter().any(|shape| {
                            let start = shape.as_ptr() as usize;
                            let at = field.as_ptr() as usize;
                            (start..start + shape.len()).contains(&at)
                        }),
                    }
                })
                .collect();
            SlideFacts {
                pictures,
                parked,
                rules,
                hidden_words,
                slide_numbers: fields.len(),
                slide_number_words: fields
                    .iter()
                    .flat_map(|field| all_words(&texts(field)))
                    .collect(),
                number: first + index,
            }
        })
        .collect()
}

/// An EMU attribute of an element.
fn emu(element: &str, name: &str) -> Option<i64> {
    attribute(element, name)?.parse().ok()
}

/// Whether a shape's own frame (the `a:off` and `a:ext` of its first
/// `a:xfrm`, not an extension list's `a:ext`) lies wholly outside a slide
/// of `size`. A group's frame is the one in its own `p:grpSpPr`, which
/// may have none.
fn outside(shape: &str, (width, height): (i64, i64)) -> bool {
    let properties = if shape.starts_with("<p:grpSp") {
        let Some(properties) = elements_with_body(shape, "p:grpSpPr").first().copied() else {
            return false;
        };
        properties
    } else {
        shape
    };
    let Some(frame) = elements_with_body(properties, "a:xfrm").first().copied() else {
        return false;
    };
    let off = elements(frame, "a:off").first().copied();
    let ext = elements(frame, "a:ext").first().copied();
    let (Some(off), Some(ext)) = (off, ext) else {
        return false;
    };
    let (Some(x), Some(y), Some(w), Some(h)) =
        (emu(off, "x"), emu(off, "y"), emu(ext, "cx"), emu(ext, "cy"))
    else {
        return false;
    };
    x >= width || y >= height || x + w <= 0 || y + h <= 0
}

/// The text of an XML fragment as the facade reads it: the runs of a
/// paragraph joined as they are (a word may span two runs), a line break as
/// a new line, paragraphs on lines of their own, and the character
/// references read.
fn texts(xml: &str) -> String {
    let paragraph = |body: &str| {
        let body = elements_with_body(body, "a:br")
            .into_iter()
            .fold(body.to_owned(), |body, br| {
                body.replacen(br, "<a:t>\n</a:t>", 1)
            });
        body.split("<a:t")
            .skip(1)
            .filter_map(|run| {
                let (head, rest) = run.split_once('>')?;
                let starts_a_run = head.is_empty() || head.starts_with(' ');
                (starts_a_run && !head.ends_with('/'))
                    .then(|| rest.split("</a:t>").next().unwrap_or_default())
            })
            .map(unescape)
            .collect::<String>()
    };
    let paragraphs = elements_with_body(xml, "a:p");
    if paragraphs.is_empty() {
        paragraph(xml)
    } else {
        paragraphs
            .into_iter()
            .map(paragraph)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// XML text with its predefined entities and character references read.
fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find(';') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        let read = match name {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "amp" => Some('&'),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match read {
            Some(c) => out.push(c),
            None => out.push_str(&rest[start..start + end + 2]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The shapes of a slide tree, by element name. A group nests groups, and
/// every other shape element sits in a group or the tree, never in itself.
const SHAPES: [&str; 5] = ["p:grpSp", "p:sp", "p:pic", "p:graphicFrame", "p:cxnSp"];

/// The byte range of the element `name` that starts at `start`, nested
/// elements of the same name included.
fn element_range(xml: &str, start: usize, name: &str) -> std::ops::Range<usize> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let head_end = xml[start..]
        .find('>')
        .map_or(xml.len(), |end| start + end + 1);
    if xml[start..head_end].ends_with("/>") {
        return start..head_end;
    }
    // The next start tag of the element itself, not of one whose name only
    // begins like it (`p:grpSpPr` after `p:grpSp`).
    let next_start = |from: usize| {
        let mut search = from;
        while let Some(offset) = xml[search..].find(&open) {
            let at = search + offset;
            if xml[at + open.len()..].starts_with([' ', '>', '/', '\n', '\r', '\t']) {
                return Some(at);
            }
            search = at + open.len();
        }
        None
    };
    let mut depth = 1;
    let mut at = head_end;
    while depth > 0 {
        let next_open = next_start(at);
        let Some(next_close) = xml[at..].find(&close).map(|offset| at + offset) else {
            return start..xml.len();
        };
        match next_open {
            Some(open_at) if open_at < next_close => {
                depth += 1;
                at = open_at + open.len();
            }
            _ => {
                depth -= 1;
                at = next_close + close.len();
            }
        }
    }
    start..at
}

/// A slide tree without the shapes it hides (`p:cNvPr hidden`, a group with
/// all it holds), and those shapes, as the render skips them
/// (`oxoffice-pptx-layout/src/context.rs`).
fn split_hidden(xml: &str) -> (String, Vec<String>) {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for name in SHAPES {
        for start_tag in elements(xml, name) {
            let start = start_tag.as_ptr() as usize - xml.as_ptr() as usize;
            let range = element_range(xml, start, name);
            let hidden = elements(&xml[range.clone()], "p:cNvPr")
                .first()
                .is_some_and(|own| matches!(attribute(own, "hidden"), Some("1" | "true")));
            if hidden {
                ranges.push(range);
            }
        }
    }
    ranges.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
    let mut visible = String::new();
    let mut hidden = Vec::new();
    let mut at = 0;
    for range in ranges {
        if range.start < at {
            continue; // inside a hidden group already left out
        }
        visible.push_str(&xml[at..range.start]);
        hidden.push(xml[range.clone()].to_owned());
        at = range.end;
    }
    visible.push_str(&xml[at..]);
    (visible, hidden)
}

/// The groups of a slide tree that sit in no other group.
fn top_level_groups(xml: &str) -> Vec<&str> {
    let mut groups = Vec::new();
    let mut at = 0;
    for start_tag in elements(xml, "p:grpSp") {
        let start = start_tag.as_ptr() as usize - xml.as_ptr() as usize;
        if start < at {
            continue;
        }
        let range = element_range(xml, start, "p:grpSp");
        at = range.end;
        groups.push(&xml[range]);
    }
    groups
}

/// A slide tree without its groups and all they hold.
fn groups_removed(xml: &str) -> String {
    let mut out = xml.to_owned();
    for group in top_level_groups(xml) {
        out = out.replacen(group, "", 1);
    }
    out
}

/// The horizontal lines a table style draws: its whole table's above the
/// first row, between two rows and below the last row, and the top and
/// bottom its first-row and last-row regions set, which win when the table
/// turns the region on.
#[derive(Clone, Copy, Debug, Default)]
struct StyleLines {
    top: bool,
    inside: bool,
    bottom: bool,
    first_row: RegionLines,
    last_row: RegionLines,
}

/// The top and bottom lines one style region sets, drawing or not.
#[derive(Clone, Copy, Debug, Default)]
struct RegionLines {
    top: Option<bool>,
    bottom: Option<bool>,
}

/// The table styles a deck defines, by id, with the lines they draw.
fn table_styles(xml: &str) -> HashMap<String, StyleLines> {
    elements_with_body(xml, "a:tblStyle")
        .into_iter()
        .filter_map(|style| {
            let id = attribute(style, "styleId")?.to_owned();
            let region = |name: &str| elements_with_body(style, name).first().copied();
            let edge = |region: Option<&str>, edge: &str| {
                region
                    .and_then(|region| elements_with_body(region, edge).first().copied())
                    .map(draws_a_line)
            };
            let whole = region("a:wholeTbl");
            // A region's top and bottom fall back to its inside line, as the
            // render's `apply_borders` reads them.
            let lines = |name: &str| RegionLines {
                top: edge(region(name), "a:top").or(edge(region(name), "a:insideH")),
                bottom: edge(region(name), "a:bottom").or(edge(region(name), "a:insideH")),
            };
            Some((
                id,
                StyleLines {
                    top: edge(whole, "a:top").unwrap_or(false),
                    inside: edge(whole, "a:insideH").unwrap_or(false),
                    bottom: edge(whole, "a:bottom").unwrap_or(false),
                    first_row: lines("a:firstRow"),
                    last_row: lines("a:lastRow"),
                },
            ))
        })
        .collect()
}

/// The lines of the style `id`: the deck's own definition, else PowerPoint's
/// built-in one (`CT_TableStyle::builtin`), which a deck may name without
/// defining it.
fn style_lines(id: &str, styles: &HashMap<String, StyleLines>) -> Option<StyleLines> {
    styles.get(id).copied().or_else(|| {
        let style = oxoffice_drawing::table::CT_TableStyle::builtin(id)?;
        let mut list = oxoffice_drawing::table::CT_TableStyleList::from_xml(
            br#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"/>"#,
        )
        .expect("an empty style list");
        list.styles.push(style);
        let xml =
            String::from_utf8(list.to_xml().expect("a built-in style writes")).expect("UTF-8");
        table_styles(&xml).get(id).copied()
    })
}

/// Whether a table cell's own top and bottom lines draw, where it sets
/// them, and whether it continues the merged cell above it.
struct TableCell {
    top: Option<bool>,
    bottom: Option<bool>,
    continues: bool,
}

/// PowerPoint's default table style, Medium Style 2 - Accent 1, built in
/// and not written in the deck: its whole table has white lines around and
/// between its cells.
#[cfg(test)]
const MEDIUM_STYLE_2_ACCENT_1: &str = "{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}";

/// How many horizontal rules a table needs at least, one per boundary its
/// cells draw: above its first row, between each two rows, below its last
/// row. Each cell edge takes the cell's own `a:lnT` or `a:lnB` when set,
/// drawing or `a:noFill`, and the style's line otherwise: the first-row or
/// last-row region's when the table turns it on and the region sets it, the
/// whole table's else. Where a cell's
/// bottom meets the next cell's top, the renderer keeps a cell's own line
/// over the style's, then a drawn line over none. A boundary needs a rule
/// when it is drawn across every column, so the inside of a merged cell
/// needs none.
fn table_rules(table: &str, styles: &HashMap<String, StyleLines>) -> usize {
    let style = elements_with_body(table, "a:tableStyleId")
        .first()
        .and_then(|element| element.split('>').nth(1))
        .and_then(|rest| rest.split('<').next())
        .map(str::trim)
        .and_then(|id| style_lines(id, styles))
        .unwrap_or_default();
    let properties = elements(table, "a:tblPr")
        .first()
        .copied()
        .unwrap_or_default();
    let turned_on = |name: &str| matches!(attribute(properties, name), Some("1" | "true"));
    let (first_row, last_row) = (turned_on("firstRow"), turned_on("lastRow"));
    let rows: Vec<Vec<TableCell>> = elements_with_body(table, "a:tr")
        .into_iter()
        .map(|row| {
            elements_with_body(row, "a:tc")
                .into_iter()
                .map(|cell| {
                    let line = |name: &str| {
                        elements_with_body(cell, name)
                            .first()
                            .map(|line| draws_a_line(line))
                    };
                    let start = format!("{}>", cell.split('>').next().unwrap_or_default());
                    TableCell {
                        top: line("a:lnT"),
                        bottom: line("a:lnB"),
                        continues: matches!(attribute(&start, "vMerge"), Some("1" | "true")),
                    }
                })
                .collect()
        })
        .collect();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let last = rows.len().saturating_sub(1);
    // The style's line on an edge of row `row`, with the rank the render
    // gives it at a boundary: a region's (1) over the whole table's (0). The
    // last-row region applies after the first-row one, so it wins in a
    // one-row table.
    let style_edge = |row: usize, whole: bool, first: Option<bool>, last_line: Option<bool>| {
        let mut line = (0u8, whole);
        if let Some(draws) = first.filter(|_| first_row && row == 0) {
            line = (1, draws);
        }
        if let Some(draws) = last_line.filter(|_| last_row && row == last) {
            line = (1, draws);
        }
        line
    };
    let style_top = |row: usize| {
        let whole = if row == 0 { style.top } else { style.inside };
        style_edge(row, whole, style.first_row.top, style.last_row.top)
    };
    let style_bottom = |row: usize| {
        let whole = if row == last {
            style.bottom
        } else {
            style.inside
        };
        style_edge(row, whole, style.first_row.bottom, style.last_row.bottom)
    };
    (0..=rows.len())
        .filter(|&boundary| {
            // A cell's own line wins over the style's, a region's over the
            // whole table's, then a drawn line over none.
            let edge = |own: Option<bool>, (rank, style): (u8, bool)| match own {
                Some(draws) => (2u8, draws),
                None => (rank, style),
            };
            columns > 0
                && (0..columns).all(|column| {
                    let above = boundary
                        .checked_sub(1)
                        .and_then(|row| rows[row].get(column).map(|cell| (row, cell)));
                    let below = rows.get(boundary).and_then(|cells| cells.get(column));
                    if below.is_some_and(|cell| cell.continues) {
                        return false;
                    }
                    [
                        above.map(|(row, cell)| edge(cell.bottom, style_bottom(row))),
                        below.map(|cell| edge(cell.top, style_top(boundary))),
                    ]
                    .into_iter()
                    .flatten()
                    .max()
                    .is_some_and(|(_, draws)| draws)
                })
        })
        .count()
}

/// Whether a line element draws: it holds a fill that is not `a:noFill` and
/// a width that is not zero, or it refers to a theme line (`a:lnRef` with an
/// index above 0), as a table style border may.
fn draws_a_line(line: &str) -> bool {
    let start = line.split('>').next().unwrap_or_default();
    let theme_line = elements(line, "a:lnRef")
        .first()
        .is_some_and(|reference| !matches!(attribute(reference, "idx"), None | Some("0")));
    theme_line
        || (!line.contains("<a:noFill")
            && line.contains("Fill")
            && !matches!(attribute(&format!("{start}>"), "w"), Some("0")))
}

/// Every picture a slide shows is drawn on its page, recognised by its pixel
/// size, at a size and overlapping the page. The drawn rect of a cropped
/// picture is the whole image, so it may reach past the page; a picture the
/// slide itself places off it is drawn off it.
fn check_slide_pictures(
    slides: &[SlideFacts],
    pdf: &Pdf,
    find: &mut impl FnMut(Check, usize, String),
) {
    for page in 0..pdf.page_count() {
        let (width, height) = pdf.page_size(page);
        let parked = slides.get(page).map_or(0, |slide| slide.parked);
        let mut off_page = Vec::new();
        let mut drawn: Vec<(i64, i64)> = Vec::new();
        for picture in pdf.pictures(page) {
            let [left, bottom, w, h] = picture.rect;
            if w <= 0.0 || h <= 0.0 {
                find(
                    Check::Pictures,
                    page + 1,
                    format!("a picture at {:?} has no size", picture.rect),
                );
            } else if left >= width || bottom >= height || left + w <= 0.0 || bottom + h <= 0.0 {
                off_page.push(picture.rect);
            }
            drawn.push(picture.pixels);
        }
        if off_page.len() > parked {
            find(
                Check::Pictures,
                page + 1,
                format!(
                    "pictures at {off_page:?} are outside their slide, which places {parked} there"
                ),
            );
        }
        let missing: Vec<String> = slides
            .get(page)
            .map(|slide| slide.pictures.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|picture| {
                let found = drawn.iter().position(|size| picture.drawn_as(*size));
                found.map(|index| drawn.remove(index)).is_none()
            })
            .map(|picture| format!("{} x {} px", picture.pixels.0, picture.pixels.1))
            .collect();
        report_page(
            Check::Pictures,
            page + 1,
            "pictures of the slide not drawn on its page",
            &missing,
            find,
        );
    }
}

/// The tables of a slide draw the horizontal rules their borders need.
fn check_slide_tables(
    slides: &[SlideFacts],
    pdf: &Pdf,
    find: &mut impl FnMut(Check, usize, String),
) {
    for (page, slide) in slides
        .iter()
        .enumerate()
        .filter(|(_, slide)| slide.rules > 0)
    {
        let rules = horizontal_rules(pdf, page);
        if rules < slide.rules {
            find(
                Check::Tables,
                page + 1,
                format!(
                    "{rules} horizontal rules drawn, the tables' borders need {}",
                    slide.rules
                ),
            );
        }
    }
}

/// Whether a diagnostic of the slide renderer changes nothing a still slide
/// draws: what it says about transitions, animations, morph and media
/// playback only concerns a slide show, and chart labels written in the
/// General format because theirs is not supported are still drawn. Any other
/// diagnostic says the render left something out or drew a stand-in (a chart
/// drawn as a placeholder, content kept as bounds) and fails its file, until
/// it is classified here or listed in `known.toml`.
fn harmless(message: &str) -> bool {
    [
        "unsupported slide transition",
        "unsupported timing",
        "unsupported animate attribute",
        "unsupported set attribute",
        "unsupported motion path",
        "unsupported morph option",
        "unsupported playback trigger",
        "non-finite timeline state",
        "media position overflow",
    ]
    .iter()
    .any(|prefix| message.starts_with(prefix))
        || (message.starts_with("morph shape ") && message.ends_with("uses crossfade"))
        || (message.starts_with("chart number format ")
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

// ---- Tests ----

#[test]
fn every_corpus_render_matches_its_baseline() {
    assert_baselines(rendered(), "pptx", &pptx_corpus());
}

#[test]
fn every_corpus_file_has_a_baseline_and_every_baseline_a_corpus_file() {
    assert_every_file_has_a_baseline("pptx", &pptx_corpus());
}

#[test]
fn every_slide_of_a_corpus_render_has_a_page_of_the_slide_size() {
    assert_check(Check::Pages, rendered(), &all_corpus());
}

#[test]
fn every_word_of_a_slide_is_drawn_on_its_page() {
    assert_check(Check::Text, rendered(), &all_corpus());
}

#[test]
fn no_text_of_a_corpus_render_crosses_its_slide_edge() {
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
fn every_picture_of_a_slide_is_drawn_on_its_page() {
    assert_check(Check::Pictures, rendered(), &all_corpus());
}

#[test]
fn the_tables_of_a_slide_draw_their_borders() {
    assert_check(Check::Tables, rendered(), &all_corpus());
}

#[test]
fn a_corpus_render_leaves_nothing_out() {
    assert_check(Check::Dropped, rendered(), &all_corpus());
}

#[test]
fn a_word_changed_on_one_slide_moves_the_baseline_of_that_slide_only() {
    let mut deck = corpus_deck("powerpoint/pitch.pptx");
    let before = draw(&deck).expect("the pitch draws");
    let slide = deck
        .slides()
        .position(|slide| slide.text().contains("carmaker"))
        .expect("a slide of the pitch says carmaker");
    assert_eq!(
        deck.replace_text("carmaker", "automaker"),
        1,
        "carmaker is said once"
    );
    let after = draw(&deck).expect("the edited pitch draws");
    let moved = moved(
        &fingerprint(&before.pdf, &before.pixels),
        &fingerprint(&after.pdf, &after.pixels),
    );
    let pages: Vec<&String> = moved
        .iter()
        .filter(|line| line.starts_with("page"))
        .collect();
    assert_eq!(pages.len(), 1, "one slide moved: {moved:?}");
    assert!(
        pages[0].starts_with(&format!("page {}: pixels, content", slide + 1)),
        "{moved:?}"
    );
}

#[test]
fn a_diagnostic_about_a_slide_show_changes_nothing_drawn() {
    assert!(harmless("unsupported slide transition"));
    assert!(harmless(
        "morph shape Title 1 has no incoming match and uses crossfade"
    ));
    assert!(harmless(
        "chart number format `[$-1009]yyyy/mm/dd` is not supported and its labels use General"
    ));
    for dropped in [
        "unsupported chart rendered as labelled placeholder: no plot area",
        "unsupported smartArt content retained as bounds",
        "uncached automatic date field has no computed date for static rendering",
    ] {
        assert!(!harmless(dropped), "{dropped}");
    }
}

#[test]
fn the_pictures_and_tables_of_a_corpus_deck_are_found_on_their_slides() {
    for producer in ["authored", "powerpoint", "libreoffice", "google"] {
        let file = format!("{producer}/deck.pptx");
        let bytes = std::fs::read(crate::support::corpus_root().join("files").join(&file))
            .expect("corpus file");
        let slides = slide_facts(&bytes);
        let pictures: usize = slides.iter().map(|slide| slide.pictures.len()).sum();
        assert!(pictures > 0, "{file} shows pictures");
        // The deck's table has 4 rows, lines between them and around it,
        // whether a built-in style (PowerPoint), cell lines (LibreOffice) or
        // a style the deck defines (Google) draws them.
        assert_eq!(slides[2].rules, 5, "{file}: the table of slide 3");
        let deck = Presentation::from_bytes(&bytes).expect("the deck opens");
        let drawing = draw(&deck).expect("the deck draws");
        let mut found = Vec::new();
        check_slide_pictures(&slides, &drawing.pdf, &mut |check, page, why| {
            found.push((check, page, why))
        });
        check_slide_tables(&slides, &drawing.pdf, &mut |check, page, why| {
            found.push((check, page, why))
        });
        assert!(found.is_empty(), "{file}: {found:?}");

        // The same pictures expected one slide early are missed.
        let mut early = slides;
        early.rotate_left(1);
        check_slide_pictures(&early, &drawing.pdf, &mut |check, page, why| {
            found.push((check, page, why))
        });
        assert!(
            found.iter().any(|(check, ..)| *check == Check::Pictures),
            "{file}: a picture expected on the wrong slide is reported"
        );
    }
}

#[test]
fn the_words_of_a_slide_keep_their_numbers() {
    assert_eq!(
        all_words("Q3 revenue -412,000"),
        ["Q", "3", "revenue", "412", "000"]
    );
}

#[test]
fn the_slide_checks_catch_a_wrong_size_and_words_on_another_slide() {
    let deck = corpus_deck("powerpoint/pitch.pptx");
    let mut drawing = draw(&deck).expect("the pitch draws");
    let mut found = Vec::new();
    let id = drawing.pdf.page_ids()[1];
    drawing
        .pdf
        .doc
        .get_dictionary_mut(id)
        .expect("page dictionary")
        .set(
            "MediaBox",
            lopdf::Object::Array(vec![0.into(), 0.into(), 720.into(), 540.into()]),
        );
    check_slides(
        &deck,
        &drawing.pdf,
        &drawing.pixels,
        &mut |check, page, _| found.push((check, page)),
    );
    assert_eq!(found, [(Check::Pages, 2)], "a 4:3 page among 16:9 slides");

    let mut drawn: Vec<Vec<Drawn>> = (0..drawing.pdf.page_count())
        .map(|page| drawing.pdf.drawn(page))
        .collect();
    drawn.swap(0, 1);
    found.clear();
    let facts = slide_facts(
        &std::fs::read(
            crate::support::corpus_root()
                .join("files")
                .join("powerpoint/pitch.pptx"),
        )
        .expect("corpus file"),
    );
    check_slide_text(&deck, &facts, &drawn, &mut |check, page, _| {
        found.push((check, page))
    });
    assert_eq!(
        found,
        [(Check::Text, 1), (Check::Text, 2)],
        "each slide's words on the other's page"
    );
}

/// A two-column table of `rows` rows with the style `style` (none when
/// empty), each cell's properties given by its row.
fn table(style: &str, rows: usize, cell: impl Fn(usize) -> String) -> String {
    let style = if style.is_empty() {
        String::new()
    } else {
        format!("<a:tableStyleId>{style}</a:tableStyleId>")
    };
    let rows: String = (0..rows)
        .map(|row| {
            let cell = format!(
                "<a:tc><a:txBody><a:bodyPr/><a:p/></a:txBody><a:tcPr>{}</a:tcPr></a:tc>",
                cell(row)
            );
            format!("<a:tr h=\"370840\">{cell}{cell}</a:tr>")
        })
        .collect();
    format!(
        "<a:tbl><a:tblPr firstRow=\"1\">{style}</a:tblPr><a:tblGrid>\
         <a:gridCol w=\"3000000\"/><a:gridCol w=\"3000000\"/></a:tblGrid>{rows}</a:tbl>"
    )
}

#[test]
fn a_table_needs_a_rule_for_each_boundary_its_cells_or_its_style_draw() {
    let styles = HashMap::new();
    let line = |edge: &str| {
        format!(
            "<{edge} w=\"12700\"><a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill></{edge}>"
        )
    };
    let none = |edge: &str| format!("<{edge} w=\"12700\"><a:noFill/></{edge}>");
    let header_only = |cell: String| {
        move |row: usize| {
            if row == 0 {
                cell.clone()
            } else {
                String::new()
            }
        }
    };
    let rules = |table: String| table_rules(&table, &styles);

    // A line under each row: none above the first.
    assert_eq!(rules(table("", 6, |_| line("a:lnB"))), 6);
    // A line under the header only.
    assert_eq!(rules(table("", 6, header_only(line("a:lnB")))), 1);
    // The default style draws every boundary, until the cells say no line.
    assert_eq!(
        rules(table(MEDIUM_STYLE_2_ACCENT_1, 6, |_| String::new())),
        7
    );
    assert_eq!(
        rules(table(MEDIUM_STYLE_2_ACCENT_1, 6, |_| none("a:lnT")
            + &none("a:lnB"))),
        0
    );
    // A cell's own no line under the header wins over the style's line
    // above the next row.
    assert_eq!(
        rules(table(
            MEDIUM_STYLE_2_ACCENT_1,
            6,
            header_only(none("a:lnB"))
        )),
        6
    );
    // Inside a cell merged down its column, the boundary is not drawn
    // across the table.
    let merged = table(MEDIUM_STYLE_2_ACCENT_1, 6, |_| String::new())
        .replacen(
            "</a:tr><a:tr h=\"370840\"><a:tc>",
            "</a:tr><a:tr h=\"370840\"><a:tc vMerge=\"1\">",
            1,
        )
        .replacen("<a:tc>", "<a:tc rowSpan=\"2\">", 1);
    assert_eq!(rules(merged), 6);
}

/// The corpus deck `deck` with `from` replaced by `to` in its part `part`.
fn deck_with(deck: &str, part: &str, from: &str, to: &str) -> Vec<u8> {
    deck_with_all(deck, part, &[(from, to)])
}

/// The corpus deck `deck` with each `from` replaced by its `to` in its part
/// `part`.
fn deck_with_all(deck: &str, part: &str, replacements: &[(&str, &str)]) -> Vec<u8> {
    let replacements: Vec<(&str, &str, &str)> = replacements
        .iter()
        .map(|(from, to)| (part, *from, *to))
        .collect();
    deck_with_parts(deck, &replacements)
}

/// The corpus deck `deck` with each `(part, from, to)` replacement made.
fn deck_with_parts(deck: &str, replacements: &[(&str, &str, &str)]) -> Vec<u8> {
    use std::io::Write;
    let bytes =
        std::fs::read(crate::support::corpus_root().join("files").join(deck)).expect("corpus file");
    let mut source = zip::ZipArchive::new(Cursor::new(bytes)).expect("package is a ZIP");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut entry = source.by_index(index).expect("ZIP entry");
        let name = entry.name().expect("ZIP entry name").into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("ZIP entry reads");
        let mine: Vec<_> = replacements
            .iter()
            .filter(|(part, ..)| *part == name)
            .collect();
        if !mine.is_empty() {
            let mut xml = String::from_utf8(data).expect("XML part");
            for (part, from, to) in mine {
                assert!(xml.contains(from), "{deck} {part} has {from}");
                xml = xml.replace(from, to);
            }
            data = xml.into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::DEFAULT)
            .expect("ZIP entry starts");
        out.write_all(&data).expect("ZIP entry writes");
    }
    out.finish().expect("ZIP finishes").into_inner()
}

#[test]
fn a_table_drawn_without_its_borders_is_reported() {
    let file = "powerpoint/deck.pptx";
    let bytes =
        std::fs::read(crate::support::corpus_root().join("files").join(file)).expect("corpus file");
    let slides = slide_facts(&bytes);
    // The table of slide 3 takes No Style, No Grid instead of the default
    // style, so it draws no line while its borders still need 5.
    let borderless = deck_with(
        file,
        "ppt/slides/slide3.xml",
        MEDIUM_STYLE_2_ACCENT_1,
        "{2D5ABB26-0587-4C30-8999-92F81FD0307C}",
    );
    let deck = Presentation::from_bytes(&borderless).expect("the deck opens");
    let drawing = draw(&deck).expect("the deck draws");
    let mut found = Vec::new();
    check_slide_tables(&slides, &drawing.pdf, &mut |check, page, _| {
        found.push((check, page))
    });
    assert_eq!(found, [(Check::Tables, 3)]);
}

#[test]
fn a_deck_names_its_text_drawn_as_boxes_once_with_the_font_file_way_out() {
    let deck = crate::support::corpus_deck("powerpoint/multilingual.pptx");
    let han = |messages: &[oxoffice_layout::Diagnostic]| -> Vec<String> {
        messages
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .filter(|message| {
                message.starts_with("Chinese, Japanese or Korean ideograph (Han) text has no glyph")
            })
            .collect()
    };
    let (_, slides) = deck.to_pdf_with_diagnostics(&[]).unwrap();
    // The notes pages draw each slide again in their preview: the export
    // still names the boxes once, over all its pages.
    let (_, notes) = deck.to_notes_pdf_with_diagnostics(&[]).unwrap();
    for (export, messages) in [("slides", &slides), ("notes", &notes)] {
        let found = han(messages);
        assert_eq!(found.len(), 1, "{export}: {messages:#?}");
        assert!(found[0].contains("--font-dir"), "{export}: {}", found[0]);
        assert!(
            !found[0].contains("--system-fonts"),
            "{export}: {}",
            found[0]
        );
    }
}

#[test]
fn a_deck_pdf_names_the_language_of_its_default_text() {
    // PowerPoint writes the deck's language on its default text style.
    let deck = corpus_deck("powerpoint/deck.pptx");
    let pdf = Pdf::read(&deck.to_pdf_deterministic_with_fonts(&[]).unwrap());
    let language = pdf
        .catalog()
        .get(b"Lang")
        .map(|lang| String::from_utf8(pdf.resolve(lang).as_str().unwrap().to_vec()).unwrap());
    assert_eq!(language.ok().as_deref(), Some("en-US"));
}

/// The findings of the picture and text checks on a deck's render.
fn picture_and_text_findings(bytes: &[u8]) -> Vec<(Check, usize, String)> {
    let slides = slide_facts(bytes);
    let deck = Presentation::from_bytes(bytes).expect("the deck opens");
    let drawing = draw(&deck).expect("the deck draws");
    let drawn: Vec<Vec<Drawn>> = (0..drawing.pdf.page_count())
        .map(|page| drawing.pdf.drawn(page))
        .collect();
    let mut found = Vec::new();
    check_slide_pictures(&slides, &drawing.pdf, &mut |check, page, why| {
        found.push((check, page, why))
    });
    check_slide_text(&deck, &slides, &drawn, &mut |check, page, why| {
        found.push((check, page, why))
    });
    found
}

#[test]
fn an_emf_is_found_drawn_at_the_render_scale_of_its_size() {
    // 1024 over the larger side, within 1 to 8: 3 for 300 x 200, 1 for a
    // large one, 8 for a small one.
    let emf = |w, h| PictureFact {
        pixels: (w, h),
        emf: true,
    };
    assert!(emf(300, 200).drawn_as((900, 600)));
    assert!(!emf(300, 200).drawn_as((300, 200)) && !emf(300, 200).drawn_as((1200, 800)));
    assert!(emf(2000, 1000).drawn_as((2000, 1000)));
    assert!(emf(50, 40).drawn_as((400, 320)));
    let png = PictureFact {
        pixels: (300, 200),
        emf: false,
    };
    assert!(png.drawn_as((300, 200)) && !png.drawn_as((900, 600)));
}

#[test]
fn a_hidden_shape_or_group_is_left_out_with_all_it_holds() {
    let tree = concat!(
        r#"<p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="shown"/></p:nvSpPr><a:t>Shown</a:t></p:sp>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="hid" hidden="1"/></p:nvSpPr><a:t>Hidden &amp; gone</a:t></p:sp>"#,
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="4" name="group" hidden="true"/></p:nvGrpSpPr><p:grpSpPr/>"#,
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="5" name="inner"/></p:nvGrpSpPr><p:grpSpPr/>"#,
        r#"<p:pic><p:nvPicPr><p:cNvPr id="6" name="picture"/></p:nvPicPr></p:pic></p:grpSp>"#,
        r#"<p:sp><p:nvSpPr><p:cNvPr id="7" name="in group"/></p:nvSpPr><a:t>Grouped</a:t></p:sp></p:grpSp>"#,
        r#"<p:pic><p:nvPicPr><p:cNvPr id="8" name="kept"/></p:nvPicPr></p:pic></p:spTree>"#,
    );
    let (visible, hidden) = split_hidden(tree);
    assert!(
        visible.contains("Shown") && visible.contains(r#"name="kept""#),
        "{visible}"
    );
    assert!(
        !visible.contains("Hidden") && !visible.contains("picture") && !visible.contains("Grouped")
    );
    assert_eq!(
        hidden.len(),
        2,
        "the hidden shape and the hidden group, once: {hidden:?}"
    );
    let words: Vec<String> = hidden
        .iter()
        .flat_map(|fragment| all_words(&texts(fragment)))
        .collect();
    assert_eq!(words, ["Hidden", "gone", "Grouped"]);
    assert_eq!(elements_with_body(&groups_removed(tree), "p:pic").len(), 1);
}

#[test]
fn a_picture_parked_off_its_slide_or_cropped_past_its_edge_is_no_finding() {
    let file = "powerpoint/deck.pptx";
    let picture = r#"<a:off x="1371600" y="1463040"/>"#;
    // PowerPoint 365 writes an extension list in every shape's properties,
    // with an `a:ext` before the frame's.
    let named = r#"<p:cNvPr id="3" name="Picture 2" descr="image.png"/>"#;
    let with_extensions = r#"<p:cNvPr id="3" name="Picture 2" descr="image.png"><a:extLst><a:ext uri="{FF2B5EF4-FFF2-40B4-BE49-F238E27FC236}"><a16:creationId xmlns:a16="http://schemas.microsoft.com/office/drawing/2014/main" id="{00000000-0000-0000-0000-000000000001}"/></a:ext></a:extLst></p:cNvPr>"#;
    // The slide places the picture wholly off it, and the render draws it
    // there: expected, and not reported.
    let parked = deck_with_all(
        file,
        "ppt/slides/slide4.xml",
        &[
            (picture, r#"<a:off x="99999999" y="1463040"/>"#),
            (named, with_extensions),
        ],
    );
    assert_eq!(slide_facts(&parked)[3].parked, 1);
    assert_eq!(picture_and_text_findings(&parked), []);
    // Its frame crosses the right edge: the drawn rect reaches past the page
    // but overlaps it.
    let edge = deck_with(
        file,
        "ppt/slides/slide4.xml",
        picture,
        r#"<a:off x="11500000" y="1463040"/>"#,
    );
    assert_eq!(slide_facts(&edge)[3].parked, 0);
    assert_eq!(picture_and_text_findings(&edge), []);
}

#[test]
fn a_hidden_picture_or_text_box_is_not_expected_on_its_slide() {
    let file = "powerpoint/deck.pptx";
    let hidden = deck_with_all(
        file,
        "ppt/slides/slide4.xml",
        &[
            (
                r#"<p:cNvPr id="3" name="Picture 2" descr="image.png"/>"#,
                r#"<p:cNvPr id="3" name="Picture 2" descr="image.png" hidden="1"/>"#,
            ),
            (
                r#"<p:cNvPr id="4" name="TextBox 3"/>"#,
                r#"<p:cNvPr id="4" name="TextBox 3" hidden="1"/>"#,
            ),
        ],
    );
    let facts = slide_facts(&hidden);
    assert!(facts[3].pictures.is_empty());
    assert!(!facts[3].hidden_words.is_empty());
    assert_eq!(picture_and_text_findings(&hidden), []);
}

#[test]
fn a_slide_number_is_expected_as_the_slide_number_whatever_its_cache() {
    // PowerPoint's cache says 2 on slide 2; a stale one says 99. Google's
    // reads ‹#›, which holds no word.
    let stale = deck_with(
        "powerpoint/training.pptx",
        "ppt/slides/slide2.xml",
        "<a:t>2</a:t></a:fld>",
        "<a:t>99</a:t></a:fld>",
    );
    let facts = slide_facts(&stale);
    assert_eq!((facts[1].slide_numbers, facts[1].number), (1, 2));
    assert_eq!(facts[1].slide_number_words, ["99"]);
    assert_eq!(picture_and_text_findings(&stale), []);

    let google = std::fs::read(
        crate::support::corpus_root()
            .join("files")
            .join("google/training.pptx"),
    )
    .expect("corpus file");
    let facts = slide_facts(&google);
    assert_eq!((facts[1].slide_numbers, facts[1].number), (1, 2));
    assert!(facts[1].slide_number_words.is_empty());
    // Expected now, the numbers are found drawn; on the wrong page they are not.
    let deck = Presentation::from_bytes(&google).expect("the deck opens");
    let drawing = draw(&deck).expect("the deck draws");
    let mut drawn: Vec<Vec<Drawn>> = (0..drawing.pdf.page_count())
        .map(|page| drawing.pdf.drawn(page))
        .collect();
    let mut found = Vec::new();
    check_slide_text(&deck, &facts, &drawn, &mut |check, page, _| {
        found.push((check, page))
    });
    assert_eq!(found, []);
    drawn.swap(1, 2);
    check_slide_text(&deck, &facts, &drawn, &mut |check, page, _| {
        found.push((check, page))
    });
    assert!(found.contains(&(Check::Text, 2)), "{found:?}");
}

#[test]
fn a_table_takes_the_lines_of_any_built_in_style_and_of_its_first_and_last_rows() {
    let none = HashMap::new();
    let plain = |style: &str| table(style, 6, |_| String::new());
    // Built-in styles the deck does not define: no lines, every line.
    assert_eq!(
        table_rules(&plain("{2D5ABB26-0587-4C30-8999-92F81FD0307C}"), &none),
        0
    );
    assert_eq!(
        table_rules(&plain("{5940675A-B579-460E-94D1-54222C63F5DA}"), &none),
        7
    );

    let line = r#"<a:ln w="12700"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln>"#;
    let theme_line = r#"<a:lnRef idx="1"><a:schemeClr val="accent1"/></a:lnRef>"#;
    let style = |regions: &str| {
        table_styles(&format!(
            r#"<a:tblStyleLst><a:tblStyle styleId="{{S}}" styleName="s">{regions}</a:tblStyle></a:tblStyleLst>"#
        ))
    };
    // A line under the first row only, from the first-row region, while the
    // table turns that region on.
    let header = style(&format!(
        "<a:firstRow><a:tcStyle><a:tcBdr><a:bottom>{line}</a:bottom></a:tcBdr></a:tcStyle></a:firstRow>"
    ));
    assert_eq!(table_rules(&plain("{S}"), &header), 1);
    assert_eq!(
        table_rules(
            &plain("{S}").replace(r#"firstRow="1""#, r#"firstRow="0""#),
            &header
        ),
        0
    );
    // A line over the last row, from the last-row region.
    let total = style(&format!(
        "<a:lastRow><a:tcStyle><a:tcBdr><a:top>{line}</a:top></a:tcBdr></a:tcStyle></a:lastRow>"
    ));
    assert_eq!(table_rules(&plain("{S}"), &total), 0);
    assert_eq!(
        table_rules(
            &plain("{S}").replace(r#"firstRow="1""#, r#"firstRow="1" lastRow="1""#),
            &total
        ),
        1
    );
    // Lines between rows that refer to a theme line.
    let themed = style(&format!(
        "<a:wholeTbl><a:tcStyle><a:tcBdr><a:insideH>{theme_line}</a:insideH></a:tcBdr></a:tcStyle></a:wholeTbl>"
    ));
    assert_eq!(table_rules(&plain("{S}"), &themed), 5);
}

#[test]
fn a_slide_expects_its_number_in_place_of_a_stale_cache_and_no_hidden_word() {
    let facts = SlideFacts {
        hidden_words: vec!["Draft".to_owned()],
        slide_numbers: 1,
        slide_number_words: vec!["99".to_owned()],
        number: 7,
        ..SlideFacts::default()
    };
    let expected = expected_words("Revenue Draft 99", Some(&facts));
    let mut expected: Vec<(String, usize)> = expected.into_iter().collect();
    expected.sort();
    assert_eq!(expected, [("7".to_owned(), 1), ("Revenue".to_owned(), 1)]);
}

#[test]
fn a_fragment_reads_as_the_facade_reads_its_text() {
    let fragment = concat!(
        "<a:p><a:r><a:t>Confi</a:t></a:r><a:r><a:t>dential</a:t></a:r><a:br/>",
        "<a:r><a:t>draft</a:t></a:r></a:p><a:p><a:r><a:t>&#233;t&#xE9; &amp; co</a:t></a:r></a:p>",
    );
    assert_eq!(texts(fragment), "Confidential\ndraft\nété & co");
}

#[test]
fn a_group_placed_off_its_slide_parks_every_picture_it_holds() {
    let group = |x: i64| {
        format!(
            r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="4" name="group"/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="{x}" y="0"/><a:ext cx="100" cy="100"/><a:chOff x="0" y="0"/><a:chExt cx="100" cy="100"/></a:xfrm></p:grpSpPr><p:pic><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="10" cy="10"/></a:xfrm></p:spPr></p:pic></p:grpSp>"#
        )
    };
    let size = (12_192_000, 6_858_000);
    let tree = format!("<p:spTree>{}{}</p:spTree>", group(99_999_999), group(0));
    let groups = top_level_groups(&tree);
    assert_eq!(groups.len(), 2);
    assert!(outside(groups[0], size) && !outside(groups[1], size));
    assert_eq!(elements_with_body(&groups_removed(&tree), "p:pic").len(), 0);
}

#[test]
fn an_untyped_field_in_a_slide_number_placeholder_or_any_case_is_a_slide_number() {
    let field = r#"<a:fld id="{C1FF6DA9-008F-8B48-92A6-B652298478BF}" type="slidenum">"#;
    for to in [
        r#"<a:fld id="{C1FF6DA9-008F-8B48-92A6-B652298478BF}">"#,
        r#"<a:fld id="{C1FF6DA9-008F-8B48-92A6-B652298478BF}" type="SlideNum">"#,
    ] {
        let deck = deck_with(
            "powerpoint/training.pptx",
            "ppt/slides/slide2.xml",
            field,
            to,
        );
        assert_eq!(slide_facts(&deck)[1].slide_numbers, 1, "{to}");
    }
}

#[test]
fn a_table_style_border_given_as_a_theme_line_is_drawn() {
    // Themed Style 1 - Accent 1 gives its borders as `a:lnRef`, which the
    // render resolves against the theme as a shape's line reference.
    let themed = deck_with(
        "powerpoint/deck.pptx",
        "ppt/slides/slide3.xml",
        MEDIUM_STYLE_2_ACCENT_1,
        "{3C2FFA5D-87B4-456A-9821-1D502468CF0F}",
    );
    let slides = slide_facts(&themed);
    assert!(slides[2].rules > 0, "the style's theme lines need rules");
    let deck = Presentation::from_bytes(&themed).expect("the deck opens");
    let drawing = draw(&deck).expect("the deck draws");
    let mut found = Vec::new();
    check_slide_tables(&slides, &drawing.pdf, &mut |check, page, why| {
        found.push((check, page, why))
    });
    assert_eq!(found, []);
    assert!(
        !drawing
            .diagnostics
            .iter()
            .any(|message| message.contains("border")),
        "{:?}",
        drawing.diagnostics
    );
}

#[test]
fn a_group_parked_off_its_slide_counts_its_pictures_as_parked() {
    let grouped = deck_with_all(
        "powerpoint/deck.pptx",
        "ppt/slides/slide4.xml",
        &[
            (
                "<p:pic>",
                concat!(
                    r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="9" name="Group 9"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>"#,
                    r#"<p:grpSpPr><a:xfrm><a:off x="99999999" y="0"/><a:ext cx="9144000" cy="6858000"/>"#,
                    r#"<a:chOff x="0" y="0"/><a:chExt cx="9144000" cy="6858000"/></a:xfrm></p:grpSpPr><p:pic>"#,
                ),
            ),
            ("</p:pic>", "</p:pic></p:grpSp>"),
        ],
    );
    assert_eq!(slide_facts(&grouped)[3].parked, 1);
    assert_eq!(picture_and_text_findings(&grouped), []);
}

#[test]
fn a_theme_line_border_reads_into_its_edge_and_writes_back_as_read() {
    use oxoffice_drawing::table::CT_TableStyleList;
    let xml = concat!(
        r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{S}">"#,
        r#"<a:tblStyle styleId="{S}" styleName="s"><a:wholeTbl><a:tcStyle><a:tcBdr>"#,
        r#"<a:top><a:lnRef idx="2"><a:schemeClr val="accent1"/></a:lnRef></a:top>"#,
        r#"<a:bottom><a:ln w="12700"/><a:lnRef idx="1"/></a:bottom>"#,
        r#"</a:tcBdr></a:tcStyle></a:wholeTbl></a:tblStyle></a:tblStyleLst>"#,
    );
    let list = CT_TableStyleList::from_xml(xml.as_bytes()).unwrap();
    let borders = list.styles[0]
        .whole_table
        .as_ref()
        .and_then(|region| region.cell_style.as_ref())
        .and_then(|cell| cell.borders.as_ref())
        .unwrap();
    let top = borders
        .line_references
        .top
        .as_ref()
        .expect("a theme line on top");
    assert_eq!(top.index, 2);
    assert!(top.color.is_some());
    // Two children are no theme line: still a border form the model leaves.
    assert!(borders.line_references.bottom.is_none());
    assert_eq!(borders.unsupported, ["border form"]);
    let written = String::from_utf8(list.to_xml().unwrap()).unwrap();
    assert!(
        written
            .contains(r#"<a:top><a:lnRef idx="2"><a:schemeClr val="accent1"/></a:lnRef></a:top>"#),
        "{written}"
    );
    let again = CT_TableStyleList::from_xml(written.as_bytes()).unwrap();
    assert_eq!(String::from_utf8(again.to_xml().unwrap()).unwrap(), written);
}

#[test]
fn a_theme_line_of_index_0_is_no_line_and_one_the_theme_cannot_resolve_drops_only_its_edge() {
    // The deck's own style: lines between all rows, but no line (index 0)
    // under the first row, and a last-column border whose theme line has no
    // colour, which the theme's placeholder colour cannot take.
    let style = concat!(
        r#"<a:tblStyle styleId="{0C4F1A57-0000-4000-8000-000000000001}" styleName="custom">"#,
        r#"<a:wholeTbl><a:tcStyle><a:tcBdr><a:insideH><a:ln w="12700"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln></a:insideH></a:tcBdr></a:tcStyle></a:wholeTbl>"#,
        r#"<a:lastCol><a:tcStyle><a:tcBdr><a:left><a:lnRef idx="1"/></a:left></a:tcBdr></a:tcStyle></a:lastCol>"#,
        r#"<a:firstRow><a:tcStyle><a:tcBdr><a:bottom><a:lnRef idx="0"><a:schemeClr val="accent1"/></a:lnRef></a:bottom></a:tcBdr></a:tcStyle></a:firstRow>"#,
        r#"</a:tblStyle>"#,
    );
    let list = format!(r#"def="{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}">{style}</a:tblStyleLst>"#);
    let deck = deck_with_parts(
        "powerpoint/deck.pptx",
        &[
            (
                "ppt/tableStyles.xml",
                r#"def="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}"/>"#,
                &list,
            ),
            (
                "ppt/slides/slide3.xml",
                MEDIUM_STYLE_2_ACCENT_1,
                "{0C4F1A57-0000-4000-8000-000000000001}",
            ),
        ],
    );
    let slides = slide_facts(&deck);
    // Four rows: the boundaries below rows 2 and 3, not below the first.
    assert_eq!(slides[2].rules, 2);
    let presentation = Presentation::from_bytes(&deck).expect("the deck opens");
    let drawing = draw(&presentation).expect("the deck draws");
    assert_eq!(
        horizontal_rules(&drawing.pdf, 2),
        2,
        "{:?}",
        drawing.diagnostics
    );
    assert!(
        drawing
            .diagnostics
            .iter()
            .any(|message| message.contains("theme line reference")),
        "{:?}",
        drawing.diagnostics
    );
}
