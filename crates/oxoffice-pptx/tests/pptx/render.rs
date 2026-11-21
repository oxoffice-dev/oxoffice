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
    check_slide_text(&deck, &drawn, &mut find);
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

/// Every word of a slide's own shapes and tables (its notes left out) is
/// drawn on that slide's page, numbers included: a deck computes only its
/// slide numbers and dates, and a slide number reads as cached.
fn check_slide_text(
    deck: &Presentation,
    drawn: &[Vec<Drawn>],
    find: &mut impl FnMut(Check, usize, String),
) {
    let mut read = 0;
    for (index, slide) in deck.slides().enumerate() {
        let mut expected: HashMap<String, usize> = HashMap::new();
        for word in all_words(&slide.text()) {
            *expected.entry(word).or_default() += 1;
            read += 1;
        }
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

/// What the checks read from a slide part: the pixel size of each picture
/// its visible shapes show, and the horizontal rules its tables need.
#[derive(Debug, Default)]
struct SlideFacts {
    pictures: Vec<(u32, u32)>,
    rules: usize,
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
    let styles = table_styles(&text("ppt/tableStyles.xml"));
    elements(&text("ppt/presentation.xml"), "p:sldId")
        .into_iter()
        .filter_map(|slide| presentation.get(attribute(slide, "r:id")?).cloned())
        .map(|part| {
            // The render draws the choice of an alternate content, never its
            // fallback, and no hidden picture.
            let xml = without(&text(&part), "mc:Fallback");
            let targets = relationships(&part);
            let pictures = elements_with_body(&xml, "p:pic")
                .into_iter()
                .filter(|picture| {
                    !elements(picture, "p:cNvPr")
                        .iter()
                        .any(|name| matches!(attribute(name, "hidden"), Some("1" | "true")))
                })
                .flat_map(|picture| elements(picture, "a:blip"))
                .filter_map(|blip| parts.get(targets.get(attribute(blip, "r:embed")?)?))
                .filter_map(|media| oxoffice_media::probe(media))
                .map(|info| (info.width_px, info.height_px))
                .collect();
            let rules = elements_with_body(&xml, "a:tbl")
                .into_iter()
                .map(|table| table_rules(table, &styles))
                .sum();
            SlideFacts { pictures, rules }
        })
        .collect()
}

/// The table styles a deck defines, by id: whether their whole-table
/// borders draw lines between rows, and above or below the table.
fn table_styles(xml: &str) -> HashMap<String, (bool, bool)> {
    elements_with_body(xml, "a:tblStyle")
        .into_iter()
        .filter_map(|style| {
            let id = attribute(style, "styleId")?.to_owned();
            let borders = elements_with_body(style, "a:wholeTbl")
                .first()
                .map(|whole| {
                    let draws = |edge: &str| {
                        elements_with_body(whole, edge)
                            .first()
                            .is_some_and(|edge| draws_a_line(edge))
                    };
                    (draws("a:insideH"), draws("a:top") || draws("a:bottom"))
                })
                .unwrap_or_default();
            Some((id, borders))
        })
        .collect()
}

/// PowerPoint's default table style, Medium Style 2 - Accent 1, built in
/// and not written in the deck: its whole table has white lines around and
/// between its cells.
const MEDIUM_STYLE_2_ACCENT_1: &str = "{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}";

/// How many horizontal rules a table needs at least: one between each two
/// rows when its cells or its style draw lines between rows, and two more
/// when they draw lines above or below it.
fn table_rules(table: &str, styles: &HashMap<String, (bool, bool)>) -> usize {
    let rows = elements(table, "a:tr").len();
    let style = elements_with_body(table, "a:tableStyleId")
        .first()
        .and_then(|element| element.split('>').nth(1))
        .and_then(|rest| rest.split('<').next())
        .map(str::trim)
        .and_then(|id| {
            if id == MEDIUM_STYLE_2_ACCENT_1 {
                Some((true, true))
            } else {
                styles.get(id).copied()
            }
        })
        .unwrap_or_default();
    let cells = |edge: &str| {
        elements_with_body(table, edge)
            .iter()
            .any(|line| draws_a_line(line))
    };
    let (inside, outside) = if cells("a:lnB") || cells("a:lnT") {
        (true, true)
    } else {
        style
    };
    usize::from(inside) * rows.saturating_sub(1) + usize::from(outside) * 2
}

/// Whether a line element draws: it holds a fill that is not `a:noFill` and
/// a width that is not zero.
fn draws_a_line(line: &str) -> bool {
    let start = line.split('>').next().unwrap_or_default();
    !line.contains("<a:noFill")
        && line.contains("Fill")
        && !matches!(attribute(&format!("{start}>"), "w"), Some("0"))
}

/// Every picture a slide shows is drawn on its page, recognised by its pixel
/// size, at a size and inside the page.
fn check_slide_pictures(
    slides: &[SlideFacts],
    pdf: &Pdf,
    find: &mut impl FnMut(Check, usize, String),
) {
    for page in 0..pdf.page_count() {
        let (width, height) = pdf.page_size(page);
        let mut drawn: Vec<(i64, i64)> = Vec::new();
        for picture in pdf.pictures(page) {
            let [left, bottom, w, h] = picture.rect;
            let (x, y) = (left + w / 2.0, bottom + h / 2.0);
            if w <= 0.0 || h <= 0.0 || x < 0.0 || x > width || y < 0.0 || y > height {
                find(
                    Check::Pictures,
                    page + 1,
                    format!(
                        "a picture at {:?} is outside its slide or has no size",
                        picture.rect
                    ),
                );
            }
            drawn.push(picture.pixels);
        }
        let missing: Vec<String> = slides
            .get(page)
            .map(|slide| slide.pictures.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|(w, h)| {
                let found = drawn
                    .iter()
                    .position(|size| *size == (i64::from(*w), i64::from(*h)));
                found.map(|index| drawn.remove(index)).is_none()
            })
            .map(|(w, h)| format!("{w} x {h} px"))
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
    check_slide_text(&deck, &drawn, &mut |check, page, _| {
        found.push((check, page))
    });
    assert_eq!(
        found,
        [(Check::Text, 1), (Check::Text, 2)],
        "each slide's words on the other's page"
    );
}
