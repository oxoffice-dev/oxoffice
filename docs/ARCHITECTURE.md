# Architecture

oxoffice reads, edits, writes and renders Office Open XML files in pure Rust,
with no Office suite and no conversion service. This page is a snapshot of how
the code is organised today. Decisions and their history live in `specs/`.

## Crates

One workspace, three families, and one crate of test support.

```
crates/
  # Format-neutral infrastructure
  oxoffice-core          units, XML helpers, lexical validation, raw XML
                         capture, core / app / custom document properties
  oxoffice-opc           ZIP and OPC package, relationships, content types
  oxoffice-media         image and media sniffing, dimensions, DPI, MIME
  oxoffice-drawing       DrawingML: colour, transforms, geometry, fills,
                         lines, effects, theme, text body
  oxoffice-layout        layout output types, font manager, 32 bundled
                         fonts, line breaking and shaping
  oxoffice-pdf           PDF writer and the tiny-skia rasteriser (PNG)
  oxoffice-chart         ChartML model and renderer, for Word and PowerPoint
  oxoffice-sml           minimal SpreadsheetML writer for chart workbooks
  oxoffice-cli-support   shared CLI pieces: ranges, JSON output, paths
  oxoffice-py-support    shared PyO3 pieces: errors, indexes, lengths, colours, core property
                         dates, caller fonts

  # WordprocessingML
  oxoffice-docx-oxml     the w: types and OfficeMath
  oxoffice-docx-layout   flow engine, paginator, tables, style resolver
  oxoffice-docx-html     HTML and Markdown output (list numbers from docx-layout)
  oxoffice-docx          the facade: Document, Paragraph, Run, Table...,
                         shaped after python-docx, plus HTML import,
                         comparison, fields, templates, conversions
  oxoffice-docx-cli      the `oxoffice docx` commands (library)
  oxoffice-docx-py       the `oxoffice.docx` bindings (library)
  oxoffice-docx-wasm     WebAssembly bindings

  # PresentationML
  oxoffice-pptx-oxml     the p: types, and which layout and master
                         placeholders a slide placeholder follows
  oxoffice-pptx-layout   placeholder inheritance for rendering (master,
                         layout, slide), chart routing, flattening
  oxoffice-pptx-render   resolved slides to page frames
  oxoffice-pptx          the facade: Presentation, Slide, Shape..., shaped
                         after python-pptx, plus the bundled default.pptx
  oxoffice-pptx-cli      the `oxoffice pptx` commands (library)
  oxoffice-pptx-py       the `oxoffice.pptx` bindings (library)
  oxoffice-pptx-wasm     WebAssembly bindings

  # Entry points
  oxoffice-cli           the `oxoffice` binary
  oxoffice-py            the `oxoffice` Python extension and package

  # Test support (dev-dependency only, never published)
  oxoffice-render-test   the render tests' shared pieces: a PDF read back,
                         page hashes, baseline files and the checks
```

## The dependency rule

The graph is acyclic and layered. **No `oxoffice-*` shared crate depends on a
`-docx` or `-pptx` crate.** Format crates depend on shared ones, never the
other way, and Word and PowerPoint never depend on each other.

The one subtle edge: the Word `Theme` type is an adapter over the DrawingML
style sheet, and its `From` impl lives in `oxoffice-docx-oxml`, so the edge
runs from Word to `oxoffice-drawing` like every other.

## How a document flows

```
bytes ─→ oxoffice-opc (package, parts, relationships)
      ─→ *-oxml typed models (CT_* types, unmodelled XML kept verbatim)
      ─→ facade (Document / Presentation): reads and edits
      ─→ save: models serialised back, untouched parts copied as they were

render:
  Word        facade ─→ docx-layout (flow, pagination) ─→ layout output
  PowerPoint  facade ─→ pptx-layout (inheritance) ─→ pptx-render ─→ layout output
  layout output ─→ oxoffice-pdf ─→ PDF, or rasterised PNG
```

Charts in both families go through `oxoffice-chart`. A Word render resolves
every font family in the embedded, caller and bundled fonts, so a document
they cover is the same on every machine. With the default `system-fonts` feature, a
character none of them can draw, such as most Chinese, Japanese or Korean
text, takes a system font found by its glyph coverage (`FontManager::
new_with_system_fallback`: the host's fonts are scanned once per process, at
the first such character, and no family name ever selects a system font); a
diagnostic names the system font. In body text, which the multilingual layout
splits by grapheme (`FontManager::resolve_font_for_split_text`), a run keeps
the bundled face that draws part of it and only the missing graphemes take a
system font; a label, list marker or watermark shaped with one face takes the
system font for all of its text when it holds such a character. `RenderOptions::with_bundled_fonts_only`
(CLI `--bundled-fonts-only`, Python `bundled_fonts_only=True`) never takes a
system font, and `RenderOptions::with_system_fonts` (CLI `--system-fonts`,
Python `system_fonts=True`) resolves families in the system fonts first; the
`*_deterministic` helpers always use the bundled fonts only. A deck render
(`PptxRenderOptions`, the `*_with_diagnostics` entries and
`Presentation::render_with_options`) takes the same fallback for what its
embedded, caller and bundled fonts lack, every East Asian run being split by
grapheme; `PptxRenderOptions::with_bundled_fonts_only` (CLI
`--bundled-fonts-only`, Python `bundled_fonts_only=True`) never takes one, a
deck has no system-fonts-first mode, and its fit checks and `*_deterministic`
helpers use the bundled fonts only. Text no font in use can draw is drawn as
empty boxes and named by a diagnostic with the way out of its family.

The Rust facades are the reference. The CLI crates and the Python crates are
thin layers over them: a feature lands in the facade first, then reaches the
CLI where it makes sense and Python everywhere.

## Model conventions

- **quick-xml pull parsing, written by hand.** No serde, no derive, no code
  generation for the XML.
- Types follow the schema names, `CT_*` and `ST_*`.
- Root parts have `from_xml(&[u8])` and `to_xml(&self)`. Nested elements take
  a `&mut Reader` and a `&mut Writer`.
- **Tolerant on read, fixed on write**: any namespace prefix is accepted, the
  canonical one is written.
- **Unmodelled XML is preserved verbatim** through
  `oxoffice_core::raw_xml::capture_element`. Parse what you use, keep the
  rest.
- **Child order is part of the contract.** OOXML schemas use `xsd:sequence`.
  A wrong order makes PowerPoint refuse the file and Word "repair" it.
- **A bookmark start between blocks belongs to the block after it**, as
  Google Docs writes a heading's bookmark. Reading (bookmarks, contents
  rebuilds) and block operations agree: an insertion before a block goes
  before its leading starts, and a block that moves, goes or is copied
  takes the leading starts whose bookmark ends inside it or in the
  bookmark ends written just after it (a bookmark around a whole table),
  with those ends, and the ends just after it that close a bookmark
  started inside it. A cut fragment carries them and an insertion writes
  them back.
- `thiserror` for errors, never `anyhow`.
- Edition 2024, MSRV and toolchain pinned in `rust-toolchain.toml`.

## Things that are deliberately wrong

Do not "fix" these without a spec:

- **`oxoffice_docx_oxml::theme::apply_tint_shade`** uses Word's 0 to 255
  convention and a naive sRGB interpolation, because that is what Word draws.
  `oxoffice-drawing` has spec-correct functions under other names for
  PowerPoint.
- **Unit constructors truncate** with `as i64`. Rounding would shift every
  twip, and every layout with it.
- **The `wp:` drawing code in `oxoffice-docx-oxml`'s `drawing.rs`** stays
  Word-only.

## Known hot spot

Edits made through the facade, and above all through Python, re-check and
re-publish much more than the element they touch (owner inventories, staged
save comparisons, whole-body proofs). On documents saved by Word or Google
Docs this makes common edits cost the whole body. See
[#466](https://github.com/oxoffice-dev/oxoffice/issues/466).
