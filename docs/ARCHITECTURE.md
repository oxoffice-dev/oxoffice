# Architecture

oxoffice reads, edits, writes and renders Office Open XML files in pure Rust,
with no Office suite and no conversion service. This page is a snapshot of how
the code is organised today. Decisions and their history live in `specs/`.

## Crates

One workspace: shared crates, three document families, the entry points, and
two crates of test support.

```
crates/
  # Format-neutral infrastructure
  oxoffice-core          units, XML helpers, lexical validation, raw XML
                         capture, core / app / custom document properties
  oxoffice-opc           ZIP and OPC package, relationships, content types,
                         embedded-content and package-signature checks,
                         what a compound file holds (legacy or encrypted)
  oxoffice-media         image and media sniffing, dimensions, DPI, MIME
  oxoffice-drawing       DrawingML: colour, transforms, geometry, fills,
                         lines, effects, theme, text body
  oxoffice-layout        layout output types, font manager, 33 bundled
                         fonts, line breaking and shaping
  oxoffice-pdf           PDF writer and the tiny-skia rasteriser (PNG)
  oxoffice-numfmt        number format codes to the text Excel displays,
                         with the month and day names Word's date controls share
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

  # SpreadsheetML
  oxoffice-xlsx-oxml     the parts of a workbook: workbook, shared strings,
                         styles, worksheets (the row index and the splice
                         writer, column widths, frozen panes), page setup;
                         Strict read as Transitional
  oxoffice-xlsx          the facade: Workbook, Worksheet, WorksheetMut; open
                         and save without loss, refusals told by content, a
                         new workbook from the text files under
                         assets/default/, sheets added, column widths, row
                         heights and frozen panes
  oxoffice-xlsx-cli      the `oxoffice xlsx` commands (library)
  oxoffice-xlsx-py       the `oxoffice.xlsx` bindings (library)

  # Entry points
  oxoffice-cli           the `oxoffice` binary
  oxoffice-py            the `oxoffice` Python extension and package

  # Test support (dev-dependency only, never published)
  oxoffice-render-test   the render tests' shared pieces: a PDF read back,
                         page hashes, baseline files and the checks
  oxoffice-xml-test      equivalent writings of a part (another prefix, an
                         end tag, indentation...), the canonical form that
                         proves them the same, and the writings check
```

## The dependency rule

The graph is acyclic and layered. **No `oxoffice-*` shared crate depends on a
`-docx`, `-pptx` or `-xlsx` crate.** Format crates depend on shared ones,
never the other way, and Word, PowerPoint and Excel never depend on each
other. `oxoffice-sml` (the chart workbook writer) and `oxoffice-chart` keep
no edge to the xlsx family.

The one subtle edge: the Word `Theme` type is an adapter over the DrawingML
style sheet, and its `From` impl lives in `oxoffice-docx-oxml`, so the edge
runs from Word to `oxoffice-drawing` like every other.

## How a document flows

```
bytes ─→ oxoffice-opc (package, parts, relationships)
      ─→ *-oxml typed models (CT_* types, unmodelled XML kept verbatim)
      ─→ facade (Document / Presentation): reads and edits
      ─→ save: models serialised back, untouched parts copied as they were

workbook: the workbook part read at open; a sheet indexed at its first read
  (one pass: each row's range, shared formula masters, array ranges); a row
  read when read or edited; an edit stages the rows and children of the
  root it changed beside the sheet (a call that changes nothing stages
  nothing) and the first one marks the workbook edited; a save copies every
  part no edit changed, splices the staged rows and children into the
  sheet's source bytes, writes the workbook part with its new records and,
  when the workbook holds a formula, asks Excel to calculate it on open
  (`fullCalcOnLoad`) with the calculation chain removed; a workbook nobody
  edited saves as it was read
  new workbook: Workbook::new assembles a package from text files and the
  package writer's sorted entries and fixed ZIP date give the same bytes
  every time

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

Word edits written as tracked changes go through `Document::track_changes`,
which returns a `TrackedDocument` holding the document mutably, so nothing
else can edit it while the session lives. Each of its calls makes the edit as
the untracked call would, then the redliner (`document/redline.rs`) compares
each paragraph the edit changed with the paragraph before it, word by word,
and writes the difference as `w:ins` and `w:del` of the session's author;
resolving the result both ways must give the edit and the paragraph before it,
or the edit is refused. Revision ids come from the identifier registry, which
reads every tracked change and move range at open.

## Model conventions

- **quick-xml pull parsing, written by hand.** No serde, no derive, no code
  generation for the XML.
- Types follow the schema names, `CT_*` and `ST_*`.
- Root parts have `from_xml(&[u8])` and `to_xml(&self)`. Nested elements take
  a `&mut Reader` and a `&mut Writer`.
- **Tolerant on read, fixed on write**: any namespace prefix is accepted, the
  canonical one is written. The roots of the main part, headers and footers
  keep the declarations they were read with, a default namespace included:
  the unmodelled XML kept without a prefix stays in it when the part is
  written again, beside the modelled elements written under `w:`. Comments,
  numbering and styles keep their root's attributes, settings are edited in
  their source, and an existing notes part takes typed additions into its
  own root. A SpreadsheetML element keeps its start tag as read; a row an
  edit writes again takes the fixed writing of the attributes the schema
  types (`1`/`0`, the shortest number), and a new element takes the prefix
  the part's root binds to the SpreadsheetML namespace.
- **Unmodelled XML is preserved verbatim** through
  `oxoffice_core::raw_xml::capture_element`. Parse what you use, keep the
  rest.
- **An element with nothing inside reads the same in both writings.** Every
  reader of a document part goes through `oxoffice_core::xml_reader`
  (`Reader`, `NsReader`), which delivers `<x></x>` as one `Event::Empty`,
  as it delivers `<x/>`: an `Empty` arm models the element whatever its
  writing. `empty_writing()` says how it was written;
  `raw_xml::capture_empty(e, reader.empty_writing())` keeps an unmodelled
  one in its writing, beside `capture_element`, and a loop that writes back
  an `Event::Empty` it read uses `raw_xml::write_empty`. A loop that copies
  events and models none, or that reads XML the model just wrote, reads
  through `Reader::verbatim()`. Where a tag or its content is, is read with
  `oxoffice_core::xml_element` (`start_tag`, `element_span`), never by
  taking `/>` as the close. `crates/oxoffice-core/tests/xml_element.rs`
  refuses a quick-xml reader in the crates that read document parts, and a
  `/>` assumption in `crates/*/src`.
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
- **A block location is read by one resolver** (`document/block_locations.rs`)
  for every operation that inserts, copies, moves or removes whole blocks.
  Four shapes, the same in every story: `[i]`, story item `i` (a direct
  child of the story owner, or a block control written in another block
  control's content); `[c, k]`, paragraph `k` of block control `c`, or
  its table `k` with the kind table (the kind says what `k` counts);
  `ContentLocation::end(story)`, before the body's section properties; and
  `ContentLocation::control_end(story, c)`, after the last child of `c`'s
  content. The container of a location is the story owner or that
  control's `w:sdtContent`, and the bookmark rule above applies among the
  container's children. A container keeps a block (a block control, a
  header, footer, note, comment or text box; a cell keeps a paragraph), a
  repeating section takes blocks only in its items and its items stay in
  it, a plain text, date, list, picture or check box control takes no
  second block, and a copy holds whole complex fields. A refusal names the
  route that works.
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
