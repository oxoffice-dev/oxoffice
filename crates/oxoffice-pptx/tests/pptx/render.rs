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
    Check, Finding, Rendered, Set, assert_baselines, assert_check,
    assert_every_file_has_a_baseline, check_glyphs, check_off_page, check_overlap, drawn_lines,
    drawn_pictures, is_long, render_set, report_page, words,
};
use oxoffice_render_test::fingerprint::{Pixels, fingerprint, moved, sha256};
use oxoffice_render_test::pdf::{Drawn, Pdf};
use oxoffice_render_test::xml::{attribute, elements, resolve, without};

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
    check_slides(&deck, &pdf, &pixels, &mut find);
    check_slide_text(&deck, &drawn, &mut find);
    check_off_page(&sizes, &drawn, &mut find);
    check_glyphs(&drawn, &mut find);
    check_overlap(&drawn, &mut find);
    check_slide_pictures(&bytes, &pdf, &mut find);
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
    // A deck without a size is 16:9, PowerPoint's default.
    let (width, height) = deck.slide_size().map_or((960.0, 540.0), |(width, height)| {
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
/// drawn on that slide's page.
fn check_slide_text(
    deck: &Presentation,
    drawn: &[Vec<Drawn>],
    find: &mut impl FnMut(Check, usize, String),
) {
    let mut read = 0;
    for (index, slide) in deck.slides().enumerate() {
        let mut expected: HashMap<String, usize> = HashMap::new();
        for word in words(&slide.text()) {
            *expected.entry(word).or_default() += 1;
            read += 1;
        }
        let mut seen: HashMap<String, usize> = HashMap::new();
        for line in drawn
            .get(index)
            .map(|pieces| drawn_lines(pieces))
            .unwrap_or_default()
        {
            for word in words(&line) {
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

/// The pictures the slides themselves hold (not their layouts' or masters'),
/// by the hash of their bytes, must all be drawn somewhere.
fn check_slide_pictures(bytes: &[u8], pdf: &Pdf, find: &mut impl FnMut(Check, usize, String)) {
    let written = slide_pictures(bytes);
    let drawn = drawn_pictures(pdf);
    if drawn.len() < written.len() {
        find(
            Check::Pictures,
            0,
            format!(
                "{} distinct pictures drawn, the slides hold {}",
                drawn.len(),
                written.len()
            ),
        );
    }
}

/// The distinct pictures of the slide parts of a package, by the hash of
/// their bytes: the targets of the `a:blip` elements outside the fallbacks
/// of alternate content.
fn slide_pictures(bytes: &[u8]) -> BTreeSet<String> {
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
    };
    let mut pictures = BTreeSet::new();
    for slide in parts
        .keys()
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
    {
        let Some(xml) = text(slide) else {
            continue;
        };
        let xml = without(&xml, "mc:Fallback");
        let (folder, file) = slide.rsplit_once('/').expect("a slide part has a folder");
        let rels = text(&format!("{folder}/_rels/{file}.rels")).unwrap_or_default();
        let targets: HashMap<&str, String> = elements(&rels, "Relationship")
            .into_iter()
            .filter_map(|element| {
                Some((
                    attribute(element, "Id")?,
                    resolve(folder, attribute(element, "Target")?),
                ))
            })
            .collect();
        for blip in elements(&xml, "a:blip") {
            if let Some(media) = attribute(blip, "r:embed")
                .and_then(|id| targets.get(id))
                .and_then(|target| parts.get(target))
            {
                pictures.insert(sha256(media));
            }
        }
    }
    pictures
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
fn every_picture_of_a_slide_is_drawn() {
    assert_check(Check::Pictures, rendered(), &all_corpus());
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
