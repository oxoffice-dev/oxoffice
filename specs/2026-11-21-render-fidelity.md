---
date: 2026-11-21
status: accepted
scope: tests
---

# Render fidelity: a baseline on every pull request, Word, PowerPoint and LibreOffice every night

## Context

oxoffice renders Word and PowerPoint files to PDF and to page images (`convert`,
`render`, the Python and Rust calls behind them). By default it lays out with
its bundled fonts only (Carlito, Caladea, Liberation, Noto subsets), which are
metric-compatible with the fonts the corpus uses (Calibri, Cambria, Arial,
Times New Roman).

No test compares a render with Word, PowerPoint or LibreOffice, and none
notices that a render changed. What exists: the docx PDF behaviour tests (#173,
35 tests, facts written by hand, 7 known failures on #156 to #158), the PDF
forms tests, and one pptx chart test that checks a PNG header. Nothing for
pptx rendering.

Measured on `main` 4b8fc13 with the corpus of #199 (64 files, 44 docx and 20
pptx, 1,056 pages):

- **The render is deterministic everywhere.** Two processes on one machine
  write the same PDF and PNG bytes; so do the `ci`, `scaling` and `release`
  profiles. A one-off run on `ubuntu-latest`, `macos-latest` (arm64) and
  `windows-latest` (run 44885565795, 56 files and 333 pages, the `long-*`
  family left out) wrote byte-identical PDFs and 72 DPI PNGs on the three
  platforms, twice on each, and the same bytes as a local Linux machine.
- **Layout is the cost.** One layout, then a PDF and every page at 72 DPI, in
  the `ci` profile on a fast Linux machine, one file after another:

  | Set | Files | Pages | Layout | PDF | PNG | Total | Slowest file |
  | --- | --- | --- | --- | --- | --- | --- | --- |
  | Whole corpus | 64 | 1,056 | 133 s | 9 s | 41 s | 182 s | 41 s (Google long report) |
  | Without `long-*` | 56 | 333 | 41 s | 2 s | 11 s | 53 s | 11 s (LibreOffice thesis) |
  | docx / pptx of that set | 40 / 16 | 209 / 124 | 38 / 3 s | | | 46 / 7 s | |

  The same 56 files rendered through the CLI (two layouts a file) took 103 s
  here, 138 s on `ubuntu-latest`, 192 s on `macos-latest` and 195 s on
  `windows-latest`: runners are 1.3 (Linux) to 1.9 times slower. A
  `release` build renders 9 times faster (4.2 s instead of 37 s for the
  LibreOffice long report) but costs a fat-LTO build; `scaling` saves 6 to
  30 %.
- **Page images are too big to commit.** A 72 DPI page is 62 KB as written
  and still 39 KB as optimised greyscale: 65 MB, or 41 MB, for the corpus. A
  hash is 64 bytes.
- **LibreOffice is a usable third witness only with our fonts.** LibreOffice
  26.2 exports the 56 files in 21 s. Without Carlito and Caladea on the
  machine it draws Calibri with Noto Sans and line breaks move; with the
  bundled fonts as its only fonts, a first prototype of the scores below
  gives equal page counts on 33 of 56 files and, on docx, a median of 0.91
  per file for the share of line starts that agree (0.9 or more on 23 of 40
  files). The differences point at real gaps: a
  blank page before a section break in every copy of the report (#156),
  equations not drawn (`OfficeMath content ... could not be rendered`, the
  paper), floating text boxes (the newsletter), a hidden slide that
  LibreOffice leaves out by default (the training deck).

What rdocx (upstream, eb980e94) does, read for this spec:

| rdocx | What | Cost per pull request |
| --- | --- | --- |
| `hash_harness.py` | 7 generated samples: 3 XML parts, the page-one PNG and a PDF fingerprint (inflated page content, inflated resources, file bytes); `--update --reason` | 1.3 min |
| `golden_png_harness.py` | page one of the 7 samples rasterised by a source-built Poppler 26.01.0 at 150 DPI, decoded RGBA hash, no committed PNG | 1.8 min plus 1.1 to 1.5 min to build Poppler, in each job |
| `docx_ssim_harness.py`, `pptx_ssim_harness.py` | pinned LibreOffice 26.2.5.2 as the oracle, global luminance SSIM at 150 DPI, union of pages, target 0.95 on 80 % of pages, advisory | 5 min (5 documents), 28 min (50 decks) |
| `test_rendering_threads.py` | Python render calls release the GIL, run faster in parallel, read back by Poppler | part of its test, Python binding and MSRV jobs (25 to 80 min each) |

Its own results argue against its fidelity metric: 1 of 18 Word pages and 25
of 421 slides (5.9 %) reach its target, and PowerPoint itself scores a median
of 0.65 against LibreOffice on a 34-slide deck, so its SSIM gate is
review-only.

## Decision

The goals, in order (the user's): a render that is coherent, at least not
broken; no change that nobody explained; then a render close to Word's and
PowerPoint's.

### Layer A: every pull request

**Where.** A `render` module in each facade binary,
`crates/oxoffice-docx/tests/docx/render.rs` and
`crates/oxoffice-pptx/tests/pptx/render.rs`, table-driven from
`tests/corpus/manifest.toml`. Its files live in a new `tests/render/`, never
in `tests/corpus/` (agreed with the tests and corpus session). It runs in the
existing `cargo test --profile ci --test '*'` step: no new build, no new job.
The PDF behaviour tests of #173 stay in `pdf.rs`; `render` does not repeat
them. What both binaries share and needs no facade (the PDF reader, the
hashes, the baseline files and the checks) lives in a dev-only crate,
`oxoffice-render-test` (never published, depending on neither facade).

**What each corpus file goes through.** Open, lay out with the bundled fonts
(the default route of `convert` and `render`), write the PDF and one PNG per
page at 72 DPI from that layout, read the PDF back with lopdf (the reader
`pdf.rs` already uses). docx in the accepted view; pptx every slide the CLI
renders, hidden ones included.

**Invariants**, read from the PDF bytes and the PNG pixels, not from the
layout model (the corpus spec's learning: a test reads what it checks from
where it can fail). Invariant 9 is the one exception: a dropped object leaves
nothing in the PDF to read, so it reads the renderer's diagnostics.

1. The PDF opens, has as many pages as there are PNGs, and each page has the
   size of its section or of the slide.
2. No page is empty: every docx page draws text or a picture more than an
   inch from its top and bottom edges, not only its header and footer (the
   `body_text` of `pdf.rs`, which reads no section from the layout); a slide
   is checked through 3 and 7, slide by slide. The blank page of #156 breaks
   this in every copy of the report: those four files are the first entries
   of `known.toml`, and the proof that the check catches a real file. A page
   left blank on purpose, before a section that starts on an odd or even
   page, is listed there too, as by design instead of with an issue.
3. The text is there: every word the render must draw is drawn, read back
   through the PDF's ToUnicode maps. For docx, the body with its tables, text
   boxes and content controls, the headers and footers each section uses, the
   footnotes and endnotes, in the accepted view; hidden text, comments (never
   drawn) and the numbers of field results (the render computes page
   numbers, counts and dates) are left out, every other number is checked,
   and words are compared over the whole document. For pptx, the text of each
   slide's own shapes, notes left out, found on that slide's page. Words are
   compared as a multiset; CJK text one character at a time.
4. The text is inside its page: no glyph box crosses the page edge (0.5 pt).
5. No glyph is missing: no `.notdef` glyph (the empty box), and every drawn
   glyph maps back to text.
6. No text is drawn over other text: no two lines of a page overlap by more
   than half the smaller line's height and more than one em across.
7. Every picture of the document is drawn at least once, at a non-zero size,
   inside its page; for pptx, every picture of a slide on that slide's page.
   A JPEG reaches the PDF as written and is found by its bytes; another
   picture is decoded again, so those are counted against the pictures
   drawn that are no written JPEG.
8. Every table is drawn: the text of each cell is there (3), and a table with
   borders draws at least as many horizontal rules as it has rows, table by
   table: the rules between the text drawn above it and the text drawn below
   it, found through the tagged PDF's `Table` elements.
9. Nothing is dropped: a renderer diagnostic that says content was not drawn
   (an equation that could not be rendered, a dropped shape) fails the file.

A file that breaks an invariant today gets an issue and an entry in
`tests/render/known.toml` (file, check, pages, issue), written by hand and
read by the two test binaries only. Like the rest of the suite, a known
failure is strict: once the file passes, the test fails until the entry goes.

**Exact baseline.** One file per corpus file, at the corpus path under
`tests/render/baseline/` (`word/report.docx.json`), written by the test
binary of its format: two pull requests that change the render of different
files never touch the same baseline file (the lesson of `limitations.json`,
where one shared file made every pull request conflict). Each holds the
file's page count and, per page, the SHA-256 of the decoded PNG pixels, of the page's inflated PDF
content stream, of its link annotations (rectangles and targets) and of the
rest of its page dictionary (its own entries, references left out), plus,
per file, one hash of the inflated PDF resources (font subsets, ToUnicode
maps, images; metadata left out), one of the outline and the structure tree
(references resolved) and one of the catalog's other entries. The pixels are
what a reader sees; the PDF entries cover the PDF writer, a code path of its
own (rdocx's reason for its fingerprint). Unlike rdocx, the file's bytes are
not hashed: a change of compression or metadata alone is not a render
change. Form fields, off on the default route, are left to their own tests.
About 250 bytes a page: under 300 KB for the corpus, within a
budget of 1 MB for `tests/render/` without the references, checked by a
test.

- A render that moves fails the test, which names each file, page and entry
  that moved. Because the bytes are the same on every platform, one baseline
  serves Linux, macOS and Windows, and there is no tolerance to tune.
- `OXOFFICE_RENDER_UPDATE="<reason>" cargo test --profile ci -p oxoffice-docx --test docx render`
  (and `-p oxoffice-pptx --test pptx`) renders the whole corpus whatever
  `OXOFFICE_RENDER` says, rewrites the baseline files of that format and
  records the reason in each file it changes; an empty reason is refused. The update lands
  in the pull request that changes the render, in a commit of its own whose
  message states the expected change.
- A test proves the check bites: one changed pixel, and one changed byte of
  a content stream, each fail it on the right page.

**Before and after for the review.** When a pull request changes the baseline
(or the render test fails), the Linux build job renders the pages whose hash
moved twice: with the pull request's wheel (after) and with `main`'s latest
Linux wheel, the `wheel-linux` artifact the build job already keeps (before;
kept only when its decoded-pixel hash is the one the base's baseline holds,
`git show <base>:tests/render/baseline/<file>.json`, otherwise the report
says that `main` moved). The step fetches the base commit first (the
checkout is one commit deep), runs even when the test step failed, once the
wheel was built (`!cancelled()` and the build step's success), and its job
declares `contents: read` and `actions: read` to fetch another run's
artifact. `tests/render/diff.py` (Pillow, already in `ci.txt`)
writes before, after and a picture of the changed pixels in red for each such
page, uploads them as the `render-diff` artifact, and puts a table in the job
summary: file, page, share of pixels changed, box of the change. A reviewer
reads the table on the run page and opens the pictures with
`gh run download <run> -n render-diff`.

**What runs where, and the cost.**

| Run | Set | Platforms | Added time (estimate) |
| --- | --- | --- | --- |
| Pull request into `main`, push to `main` | the corpus without `long-*`: 56 files, 333 pages | Linux | about 70 s of CPU on a runner, spread over the test threads with the other tests: well under a minute of wall time; the slowest file about 15 s |
| Nightly run (02:23 UTC) and `workflow_dispatch` | the whole corpus: 64 files, 1,056 pages | Linux, macOS, Windows | about 4 min of CPU on Linux, 6 on macOS and Windows: 2 to 3 min of wall time, the slowest file about 1.5 min |

These are the render's numbers; the read-back and the invariants come on top
and are measured by the first pull request. The limit: 2 minutes of wall
time added to the Linux test step of a pull request.

An environment variable chooses the set: `OXOFFICE_RENDER=subset`, `full` or
`off`. CI sets it: `subset` on Linux for pull requests and pushes to `main`,
from the first layer A pull request on, `off` on macOS and Windows for pull
requests, `full` on the three platforms at night, which then catches a
platform that drifts, a day later at most. Under CI (`CI` set), a missing
variable fails the render test instead of skipping it, so that dropping it
from `ci.yml` cannot turn layer A off. Outside CI, unset means `off`: a plain
`cargo test` builds every dependency unoptimised, which none of the
measurements covers. The local run is
`OXOFFICE_RENDER=subset cargo test --profile ci -p oxoffice-docx --test docx render`;
the first pull request measures the test profile and makes `subset` the
default if it is cheap enough.

Options weighed for the binary, as asked:

| Option | Extra build | Render speed | Kept |
| --- | --- | --- | --- |
| A module of the test binaries, `ci` profile, in the existing test step | none: that step compiles the binary anyway | 1x | yes |
| A job of its own on the `wheel-linux` artifact, like the skill examples | none, but a job, its setup and a Python harness | 1x | no |
| The `scaling` wheel, in the scaling job | none on Linux | 1.1 to 1.3x | no: a second home for little |
| A `release` build | a fat-LTO build, minutes on a runner | 9x | no |

If the subset passes the limit, the first lever is the four thesis copies,
about half of its time (the rest takes 24 s of the 53 s), which then run at
night only. Choosing files by the crates a pull request touches (rdocx's path
filters) comes after: a layout change shows anywhere, and pptx is only 7 s of
the 53 s.

### Layer B: every night

**References.**

- **Word for docx, PowerPoint for pptx**: PDF exports of every corpus file
  outside `long-*` (each producer's copy, opened as a user would open it),
  made on the Mac by a new `--export-pdf` route of
  `tests/corpus/generate/produce.py`, with the rules of the other Office
  routes: every document closed after its export, the app quit even on
  error, 5 minutes at most a file, the frozen pilot exported only when named.
  Word exports the final view (no markup). PowerPoint exports hidden slides
  too, as ours and LibreOffice's do, when its route allows it; otherwise
  `references.toml` says so and the scorer leaves hidden slides out on our
  side (the training deck has one in each copy). They are committed under
  `tests/render/references/word/` and `.../powerpoint/`, since only a Mac can
  make them, with `tests/render/references.toml`: app and version, date, and
  the SHA-256 of the source file, so a reference whose source changed shows
  as stale. Budget: 10 MB, checked by the budget test (the LibreOffice
  exports of the same 56 files take 5.9 MB).
- **LibreOffice**, the third witness: exported in the nightly job, not
  committed, by a pinned LibreOffice (official archive, checked SHA-256, the
  version recorded), with the bundled fonts as its only fonts through its
  own fontconfig file, and hidden slides exported like ours.

**Scores**, for each file and reference, both PDFs read by the same reader
(pypdfium2, pinned with hashes in `.github/requirements/`):

1. **Pages**: the counts, then every index of the union, a page missing on
   one side scored against a blank page (rdocx).
2. **Text on its page**: the words of both documents aligned in reading
   order; the share of reference words found, and the share found on the
   same page.
3. **Line breaks**: the share of reference line starts that also start a
   line in ours. Glyph shapes do not move it; metric-compatible fonts make
   it a measure of the layout.
4. **Position**: median and 95th percentile distance, in points, between
   matched line starts on the same page.
5. **Visual**: mean SSIM over 7 by 7 windows of 50 DPI greyscale pages after
   a light blur, to see what text cannot (pictures, shapes, fills, borders,
   backgrounds). Not rdocx's global SSIM, which it found unstable.

pptx is scored per slide the same way.

**Thresholds.** The scoring pull request runs the whole set against Word and
PowerPoint once and puts the distribution in its body. Each file's scores
are then committed as its floor (`tests/render/scores/<file>.json`, one
file per corpus file like the baseline, written by the scorer with an update
reason). A night fails when a score
falls below its floor by more than 0.02 (a share) or by one page; a better
score raises the floor at the next update. Independently of history, a file
is a gap when its page count differs, or fewer than 98 % of the reference
words are found, 95 % on the same page, 90 % of line starts agree, or the
visual score is under 0.70. These defaults may move with the calibration, by
the orchestrator's acceptance of the scoring pull request.

**A gap becomes an issue.** Every gap is either a known gap in
`tests/render/gaps.toml` (file, reference, score, pages, issue; read by the
scorer only, strict like `known.toml`), or a new issue. The session that
reads the night opens one issue per cause (for example "a blank page before a
section break", whatever the files), never the job itself. A gap is filed
when oxoffice differs from Word or PowerPoint; LibreOffice agreeing with the
Office app makes it stronger, and LibreOffice alone never decides
(PowerPoint and LibreOffice disagree too often, rdocx measured).

**Output.** `render-scores.json` (file, page, reference, score) and
`render-scores.md` (per reference: files, pages with equal counts, line
breaks, visual score, below floor, known), as the job's artifact and summary.
The orchestrator reads the JSON for its progress bars.

**Where it runs.** A job of the nightly run of `ci.yml`, Linux, on the Linux
wheel of the same run. Estimate: under a minute to render the 56 files with
four processes, 2 min to install LibreOffice and export, 3 to 5 min to score:
about 10 minutes. `ci.yml` belongs to the CI session, which writes or reviews
this job.

### What we take from rdocx, and what we leave

| Taken | Left, and why |
| --- | --- |
| A baseline of hashes, rewritten by one command that requires a reason | a source-built Poppler in every job: our layer A rasterises with its own backend; the nightly uses pdfium wheels |
| A proof that the check fails on one changed pixel | its global SSIM: unstable, and not discriminating between renderers |
| The PDF fingerprint split into page content and resources | fidelity gates on pull requests (28 min for the decks): nightly here |
| The union of pages, a missing page against a blank one, one white canvas for both sizes | page one of seven synthetic samples: every page of a corpus saved by four producers |
| Pinned oracle versions, recorded and checked | LibreOffice as the judge of PowerPoint |
| References in the final view, hidden slides aligned | third-party corpora fetched at test time: ours are committed and ours to share |
| The evidence kept as an artifact; dropped shapes counted | GIL and thread timing tests: they test the bindings, not the render |
| Reading our PDF with another reader | |

### Not now

- Google Docs and Slides, Word and PowerPoint for Windows as references.
- The system-fonts route: it differs between machines by design.
- SVG, HTML, notes pages, handouts and animation timelines.
- Renders after edits: the behaviour suites cover edits; a later step can
  render the edited corpus.
- PDF/A and PDF/UA validation (veraPDF).
- Fonts the bundle lacks (Aptos in one file): scored, but a font gap, not a
  layout bug.

## Plan

One pull request in CI at a time, each reviewed before its push, two reviews
at most.

1. This spec, documentation only.
2. `test(docx)`: layer A for docx: `render.rs`, its baseline files,
   `known.toml` with issues for what breaks today, the update command, the
   budget test, the proof that the check fails, and `OXOFFICE_RENDER=subset`
   on the Linux test step of `ci.yml` (with the CI session). Its body gives
   the measured time of the Linux test step before and after, against the
   2-minute limit; above it, the thesis copies move to the nightly set in the
   same pull request. It also measures the test profile.
3. `test(pptx)`: layer A for pptx. The perf and refactoring sessions hear when
   2 and 3 land.
4. `ci`: the before and after report, `off` on macOS and Windows for pull
   requests and `full` at night, with the CI session (and a longer retention
   for `wheel-linux`, kept one day today, if the report often finds it gone).
5. `test(corpus)`: references: `produce.py --export-pdf` run on the Mac,
   `references.toml`, the LibreOffice exporter.
6. `test(render)`: the scorer, its calibration, the floors, `gaps.toml` and
   its issues.
7. `ci`: the nightly job, with the CI session.
8. This spec's outcome, and a "Render tests" section in `docs/DEVELOPMENT.md`.

## Links

- [A test suite from scratch](2026-11-20-test-suite-from-scratch.md): its
  "Rendering" layer, which this spec replaces
- [Corpus at scale and behaviour tests by area](2026-11-21-corpus-at-scale-and-behaviour-tests.md)
- [Nightly scaling on macOS and Windows](2026-11-21-nightly-scaling-on-macos-and-windows.md)
- #173 (docx PDF behaviour), #199 (the corpus copies), #156, #157, #158
- rdocx eb980e94: `scripts/hash_harness.py`, `scripts/golden_png_harness.py`,
  `scripts/docx_ssim_harness.py`, `scripts/pptx_ssim_harness.py`,
  `docs/hld/12-testing-strategy.md`
