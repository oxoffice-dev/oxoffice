---
date: 2026-11-23
status: accepted
scope: pptx
---

# A deck draws the characters its fonts lack with a system font

## Context

A Word render draws a character that no embedded, caller or bundled face has
with a system font found by glyph coverage, by default, since #457. A deck
does not: `configured_font_manager` builds a bundled-only `FontManager`, and the
only way out is a font file (`--font-dir`, `fonts=`). The Chinese slide (4), the Japanese one (5)
and the mixed one (6: Arabic, Hebrew, Chinese, Japanese) of every copy of
`multilingual.pptx` are drawn with empty boxes. The bundled Noto Sans SC, JP
and KR faces are fixture subsets of a few characters. `tests/render/known.toml`
lists the boxes and the "3 diagnostics" of those copies under #157.

The font layer is done: `FontManager::new_with_system_fallback` scans the
host's fonts once per process at the first character no other face has,
never selects a system font by family name, and the multilingual shaping
gives a system face only to the graphemes the bundled faces lack. Decks use
it for Chinese and Japanese only: `needs_multilingual_layout` in
`oxoffice-pptx-render/src/text.rs` leaves out Hangul, the fullwidth forms and
the CJK extensions past U+9FFF, whose runs are shaped with one face per run.
The CLI (`pptx convert` to images, `render`, `thumbnail`) and every Python
render method call the `*_deterministic` helpers today. What is missing is the
deck side: the option, its default, the split for every East Asian run, the
CLI, Python and the diagnostic.

## Decision

- **Renders** of a deck (PDF, PDF/A, slide images, notes and handout pages,
  thumbnails) draw what the embedded, caller and bundled fonts lack with a
  system font, **by default**, as a Word render does. A family name never
  selects a system font; text the bundled fonts draw is drawn by them on
  every machine. Every East Asian run, Korean included, takes the
  multilingual split, with the same East Asian set the Word layout uses, so
  a system face draws only the graphemes the bundled faces lack.
- **Bundled fonts only** keeps today's output: Rust `PptxRenderOptions`
  (`bundled_fonts_only`, built from `Default` field by field like the docx
  `RenderOptions`), CLI `--bundled-fonts-only`, Python
  `bundled_fonts_only=True`. The `*_deterministic` helpers and the render
  tests are bundled-only and stay so.
- **The CLI and Python renders move to the new entries**: `pptx convert`
  (PDF, PDF/A and images), `render` and `thumbnail`, and every Python render
  method (`to_pdf`, `to_pdfa`, `render_slide_to_png`, `render_all_slides`,
  `render_slides`, notes and handouts, with or without diagnostics) render with
  the fallback by default; their help and docstrings stop calling the output
  deterministic.
- **No `--system-fonts` for decks**: the facade, the CLI and Python offer no
  "system fonts first" mode. PowerPoint decks name few families the bundled
  fonts do not stand for, and a system family taking Latin text would make
  the output machine-dependent for no case seen. The render crate's own
  `layout_presentation` and `layout_slide` (`FontManager::new()`, system
  fonts first) stay as they are: the facade does not call them.
- **Fit checks stay bundled-only**: Rust `fit_report`, `fit_text_at`,
  `refresh_autofit` and `text_layout_deterministic*`, Python `fit_report`,
  `fit_text`, `refresh_autofit` and `text_layout` measure text to decide sizes
  written into the file; their answer must not depend on the machine. The
  Rust-only animation and timeline exports, deterministic by name, stay
  bundled-only too.
- **The diagnostic** names the system font that drew the characters, or says
  no font was found, with `--bundled-fonts-only` named as the way to a
  machine-independent render; the boxes diagnostic keeps naming a font file
  as the way out with bundled fonts only.
- A build without the `system-fonts` feature renders as bundled-only (the
  WebAssembly build already does).

Not done: bundling full CJK faces (package size), PowerPoint line metrics
with a fallback face (the deck keeps measuring on the face that draws).

## Plan

One pull request, not the two first planned: the boxes diagnostic names
`--bundled-fonts-only`, so the Rust option, the CLI switch and the Python
argument land together.

- **Rust facade** (`oxoffice-pptx`, `oxoffice-pptx-render`):
  `PptxRenderOptions`, a last argument of every `*_with_diagnostics` entry
  rather than a sibling `*_with_options` entry per family, and
  `Presentation::render_with_options` for the layout itself;
  `configured_font_manager` takes the tier; `needs_multilingual_layout` covers
  every East Asian character; the deck layout and its notes and handout
  exports name the system font through `glyph_coverage_diagnostics`. A run
  keeps the face of its family when that face draws part of it, and a run
  with glyphs that face lacks takes the grapheme split too, so only those
  graphemes look for a face (a system one included) and the family keeps the
  rest: a face picked for a whole run used to take its Latin text as well,
  and draw it as boxes when it had none (an emoji face, Noto Sans Symbols 2
  for a check box beside words). Tests: with a fixed fallback font, a deck draws only what its
  fonts lack with it and names it; on any machine, the default render takes a
  system face or says none was found, bundled fonts only names the switch,
  and Latin text is drawn alike in both modes; Korean, fullwidth and
  supplementary-plane runs take the split.
- **CLI and Python**: `pptx convert`, `render`, `thumbnail` and every Python
  render method use the options; `--bundled-fonts-only` and
  `bundled_fonts_only=` everywhere, the stubs, README, `ARCHITECTURE.md`, a
  `pptx-pdf-cjk` limitation beside `docx-pdf-cjk`, and the
  `multilingual.pptx` entries of `known.toml` become `by_design`
  (bundled-only render tests), as the minutes are.

## Links

- #157 (PowerPoint side, unparked 2026-11-23), #457 (Word side), #431
  (boxes diagnostic), #273 (render tests).
