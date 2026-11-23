---
date: 2026-11-23
status: draft
scope: pptx
---

# A deck draws the characters its fonts lack with a system font

## Context

A Word render draws a character that no embedded, caller or bundled face has
with a system font found by glyph coverage, by default, since #457. A deck
does not: `configured_font_manager` builds a bundled-only `FontManager`, so the
Chinese, Japanese and Korean slides (4 to 6) of every copy of
`multilingual.pptx` are drawn as empty boxes, and the only way out is a font
file (`--font-dir`, `fonts=`). The bundled Noto Sans SC, JP and KR faces are
fixture subsets of a few characters. `tests/render/known.toml` lists the boxes
and the "3 diagnostics" of those copies under #157.

The font layer is done: `FontManager::new_with_system_fallback` scans the
host's fonts once per process at the first character no other face has,
never selects a system font by family name, and the multilingual shaping
(which decks already use for CJK text) gives a system face only to the
graphemes the bundled faces lack. What is missing is the deck side: the
option, its default, the CLI, Python and the diagnostic.

## Decision

- **Renders** of a deck (PDF, PDF/A, slide images, notes and handout pages,
  thumbnails) draw what the embedded, caller and bundled fonts lack with a
  system font, **by default**, as a Word render does. A family name never
  selects a system font; text the bundled fonts draw is drawn by them on
  every machine.
- **Bundled fonts only** keeps today's output: Rust `PptxRenderOptions`
  (`bundled_fonts_only`, built from `Default` field by field like the docx
  `RenderOptions`), CLI `--bundled-fonts-only`, Python
  `bundled_fonts_only=True`. The `*_deterministic` helpers and the render
  tests are bundled-only and stay so.
- **No `--system-fonts` for decks**: no "system fonts first" mode. PowerPoint
  decks name few families the bundled fonts do not stand for, and a system
  family taking Latin text would make the output machine-dependent for no
  case seen.
- **Fit checks stay bundled-only**: `fit_report`, `fit_text`,
  `refresh_autofit` and `text_layout` measure text to decide sizes written
  into the file; their answer must not depend on the machine.
- **The diagnostic** names the system font that drew the characters, or says
  no font was found, with `--bundled-fonts-only` named as the way to a
  machine-independent render; the boxes diagnostic keeps naming a font file
  as the way out with bundled fonts only.
- A build without the `system-fonts` feature renders as bundled-only (the
  WebAssembly build already does).

Not done: bundling full CJK faces (package size), PowerPoint line metrics
with a fallback face (the deck keeps measuring on the face that draws).

## Plan

1. **Rust facade** (`oxoffice-pptx`, `oxoffice-pptx-render`):
   `PptxRenderOptions` and a `*_with_options` entry per render family, the
   non-deterministic entries calling them with the default;
   `configured_font_manager` takes the tier; the run shaper resolves with the
   split variant so a run the bundled faces partly draw keeps them. Tests:
   with fixed fallback fonts (`new_with_fallback_fonts`, as
   `crates/oxoffice-layout/tests/system_fallback.rs`), a CJK slide draws real
   glyphs by default and boxes with bundled fonts only, Latin text beside it
   draws alike in both modes, the diagnostic names the face.
2. **CLI and Python**: `--bundled-fonts-only` on `pptx convert`, `render`,
   `thumbnail`; `bundled_fonts_only=` on every render method and the stubs;
   README, `ARCHITECTURE.md`; the limitation becomes CJK-without-a-system-font
   for decks as for documents; `known.toml` keeps the `multilingual.pptx`
   entries as `by_design` (bundled-only render tests), as the minutes have.

Each pull request: the pptx tests, CLI `convert.rs` and Python
`test_missing_glyphs.py` cases for both modes.

## Links

- #157 (PowerPoint side, unparked 2026-11-23), #457 (Word side), #431
  (boxes diagnostic), #273 (render tests).
