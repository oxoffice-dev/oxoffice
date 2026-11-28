<div align="center">

<img src="assets/logo.svg" alt="oxoffice logo" width="128">

# oxoffice

**Word and PowerPoint files in pure Rust: read, write, edit, compare and render `.docx` and `.pptx`,
from Rust, the command line or Python.**

[![CI](https://github.com/oxoffice-dev/oxoffice/actions/workflows/ci.yml/badge.svg)](https://github.com/oxoffice-dev/oxoffice/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/oxoffice-dev/oxoffice?label=release&color=4f46e5)](https://github.com/oxoffice-dev/oxoffice/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-22c55e)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-0ea5a4)](#-install)
[![Python](https://img.shields.io/badge/python-3.10%2B-f97316)](#-python)

</div>

| ![A redline of two versions of a lab report, rendered by oxoffice](assets/screenshot-docx.png) | ![A slide with a table, a process and three figures, rendered by oxoffice](assets/screenshot-pptx.png) |
|:---:|:---:|
| 📝 A redline of two versions of a report, in reviewer colours | 📊 A slide built and rendered from code |

<p align="center"><sub>Both pictures come straight out of oxoffice: no Word, no PowerPoint, no LibreOffice.</sub></p>

## ✨ Why oxoffice

- 🦀 **One engine, three doors**: a Rust API, an `oxoffice` command for everything that makes sense in a
  shell, and an `oxoffice` Python package that covers nearly all of it (a few calls, such as
  encryption, are Rust only for now) for when the shell is not enough.
- 📄 **Word and PowerPoint as equals**, with Excel workbooks joining them: they open, read, edit and save today, the rest is on the [roadmap](https://github.com/oxoffice-dev/oxoffice/issues?q=is%3Aissue%20state%3Aopen%20label%3Aroadmap).
- ✍️ **Edits that keep the file intact**: unmodelled XML is carried over verbatim, child order follows the
  schema, and text changes keep their formatting.
- 🔍 **Real review tools**: list, accept and reject tracked changes, compare two versions into a redline,
  work with comments.
- 🖨️ **Rendering without an office suite**: PDF, PNG, HTML and Markdown, with bundled fonts so the output is
  the same on every machine; a render takes a system font only for characters they lack, such as most
  Chinese, Japanese and Korean text (`--bundled-fonts-only` never does).
- 🚫 **Nothing to install around it**: no Office, no LibreOffice, no conversion service, no network.

## 🚀 Quick start

### 📦 Install

| Channel | Command |
| --- | --- |
| 🐚 Installer, Linux and macOS | `curl -fsSL https://github.com/oxoffice-dev/oxoffice/releases/latest/download/install.sh \| sh` |
| 🪟 Installer, Windows | `powershell -ExecutionPolicy Bypass -c "irm https://github.com/oxoffice-dev/oxoffice/releases/latest/download/install.ps1 \| iex"` |
| 🐍 PyPI | `pip install oxoffice`, `uv add oxoffice` or `uv tool install oxoffice` |
| 🟢 conda | `pixi global install -c https://prefix.dev/hadim/oxoffice -c conda-forge oxoffice` |
| 📦 GitHub release | binaries, wheels and checksums on the [releases page](https://github.com/oxoffice-dev/oxoffice/releases) |
| 🦀 Rust | `oxoffice-docx = { git = "https://github.com/oxoffice-dev/oxoffice", tag = "v0.1.0" }` |

The installers and the Python channels all give you the `oxoffice` command, and the Python channels also give
you the `oxoffice` package. Every release ships `SHA256SUMS`, an SBOM and a signed build provenance: check any
file with `gh attestation verify <file> --repo oxoffice-dev/oxoffice`. Details, pinning and verification:
[docs/INSTALL.md](docs/INSTALL.md).

### Command line

```bash
oxoffice docx inspect report.docx
oxoffice docx text report.docx
oxoffice docx convert report.docx --to pdf
oxoffice docx replace report.docx -p "{{client}}" -v "Acme" -o out.docx
oxoffice docx diff v1.docx v2.docx

oxoffice pptx outline deck.pptx
oxoffice pptx render deck.pptx -o slides/
oxoffice pptx convert deck.pptx --to md
```

### Python

```python
from oxoffice.docx import Document
from oxoffice.pptx import Presentation

doc = Document()
doc.add_heading("Quarterly report", level=1)
doc.add_paragraph("Revenue grew by 12 %.")
doc.save("report.docx")

deck = Presentation("deck.pptx")
for slide in deck.slides:
    for shape in slide.shapes:
        if shape.has_text_frame:
            print(shape.text_frame.text)
```

The Python API starts from the names of python-docx and python-pptx, so most code reads the same, but it is not bound to them: it differs where that makes it safer or simpler, for example `save` refuses to overwrite the file a document was opened from.

### Rust

```toml
[dependencies]
oxoffice-docx = { git = "https://github.com/oxoffice-dev/oxoffice", tag = "v0.1.0" }
```

```rust
use oxoffice_docx::Document;

let mut doc = Document::new();
doc.add_paragraph("Hello from Rust");
doc.save("hello.docx")?;
```

## 🧰 What it does

A tour by family rather than a full list, as `main` stands, a step ahead of the last release: each command's
`--help`, the Python type stubs and the examples of [oxoffice-skills](https://github.com/oxoffice-dev/oxoffice-skills)
go into the details. ✅ works, ⚠️ works with known gaps, ❌ not yet, — does not apply. The note says what a ⚠️
lacks; `oxoffice limitations` in a build of `main`, one file per gap in
[crates/oxoffice-cli/limitations](crates/oxoffice-cli/limitations), and, from the release after 0.1.0, a limitations
page attached to each [release](https://github.com/oxoffice-dev/oxoffice/releases) give each listed gap its workaround,
with a few that touch whole files: edits and form operations that cost more as a Word document grows.

### 📝 Word

| Family | Read | Edit | Create | Notes |
| --- | :---: | :---: | :---: | --- |
| Text, runs, find and replace | ⚠️ | ⚠️ | ✅ | counted replacements that keep formatting and refuse text they cannot reach; non-breaking hyphens and positional tabs are not read yet |
| Styles | ✅ | ✅ | ✅ | |
| Lists and numbering | ✅ | ✅ | ✅ | |
| Tables | ✅ | ✅ | ✅ | merges, borders, shading, nested tables |
| Sections, headers and footers | ✅ | ⚠️ | ✅ | the document-wide header or footer setter reaches the last section only |
| Footnotes and endnotes | ✅ | ⚠️ | ✅ | removing a paragraph leaves its notes behind, and removing a note leaves its custom mark |
| Comments and replies | ✅ | ✅ | ⚠️ | anchored on a piece of text, moved, resolved by thread, dated, with the durable ids and UTC dates Word keeps; in a file saved by Word in another language, a new comment does not take the file's comment styles yet |
| Tracked changes | ✅ | ✅ | ⚠️ | accept and reject by id, author or date; compare two versions into a redline, which refuses a few edits around simple fields; text edits and replacements written as tracked changes by a given author (`track_changes` in Rust and Python, CLI `replace --track-author`); for formatting, paragraphs, rows and moves, on the [roadmap](https://github.com/oxoffice-dev/oxoffice/issues?q=is%3Aissue%20state%3Aopen%20label%3Aroadmap) |
| Fields and tables of contents | ✅ | ✅ | ✅ | page numbers, page references, dates, sequences, cross-references |
| Content controls and forms | ⚠️ | ⚠️ | ✅ | fill, validate, reset and protect in every story, export to fillable PDF fields; a date control shows month and day names in English, French, German, Spanish, Italian, Portuguese and Dutch, numeric dates in any language, and a time where its format shows one |
| Pictures and watermarks | ✅ | ✅ | ✅ | a replaced picture keeps its frame |
| Charts | ✅ | ⚠️ | ✅ | an existing chart's legend can change, not its data yet |
| Hyperlinks and bookmarks | ✅ | ✅ | ✅ | |
| Mail merge | ✅ | ✅ | ✅ | from Rust and Python |
| Bibliography | ⚠️ | ⚠️ | ⚠️ | sources, citations and the bibliography from Rust, the CLI and Python; every source kind in APA 6th, a one-author book in the 11 other styles; only APA 6th in en-US is checked against Word |
| Conversions | ✅ | — | ✅ | from Markdown and HTML; to PDF and PDF/A, HTML, Markdown, ODT, RTF, EPUB and page images |
| Embedded fonts | ✅ | ✅ | ✅ | you state that the font's licence allows it |
| Signatures | ✅ | — | ✅ | sign and verify, no certificate chain trust |
| Encryption | ⚠️ | — | ⚠️ | open and save with a password, from Rust only for now |

### 📊 PowerPoint

| Family | Read | Edit | Create | Notes |
| --- | :---: | :---: | :---: | --- |
| Slides | ✅ | ⚠️ | ✅ | add, duplicate, import, move, hide and remove, sections following each slide; after a slide edit the save refreshes the slide counts and drops the title lists and word counts it does not recount; the thumbnail stays until a removal |
| Shapes and groups | ✅ | ✅ | ✅ | every shape walked, groups included; align, distribute, z-order, glued connectors |
| Text | ✅ | ✅ | ✅ | replacements that keep run formatting, bullets and numbering |
| Text fit | ✅ | ✅ | — | reports what overflows with oxoffice's own line breaks, shrinks text to fit |
| Tables | ✅ | ✅ | ✅ | merge and split cells, add and remove rows and columns, in tables from PowerPoint, Google and LibreOffice |
| Charts | ✅ | ✅ | ✅ | new data keeps the series formatting |
| Pictures | ✅ | ✅ | ✅ | a replaced picture keeps its frame and crop |
| Speaker notes | ✅ | ✅ | ✅ | notes pages and handouts as PDF |
| Comments and replies | ✅ | ✅ | ✅ | modern comments, on a slide or a shape |
| Themes, masters and layouts | ✅ | ✅ | ✅ | theme colours and fonts, a theme taken from another deck |
| Merging decks | — | ✅ | — | slides imported onto a layout of the deck, or with their own formatting and their notes |
| Rendering and conversions | ✅ | — | ✅ | from ODP; to PDF and PDF/A, PNG and JPEG slides, HTML, Markdown and ODP |
| Embedded fonts | ✅ | ✅ | ✅ | you state that the font's licence allows it |
| Signatures | ✅ | — | ✅ | sign and verify, no certificate chain trust |
| Encryption | ⚠️ | — | ⚠️ | open and save with a password, from Rust only for now |

### 📗 Excel

Workbooks arrive step by step ([#365](https://github.com/oxoffice-dev/oxoffice/issues/365)): opening, saving,
reading, editing cells and making a workbook first, then the evaluator, sheets moved and removed, rows inserted,
comments, tables and the render.

| Family | Read | Edit | Create | Notes |
| --- | :---: | :---: | :---: | --- |
| Workbooks | ✅ | ⚠️ | ⚠️ | `.xlsx`, `.xlsm`, `.xltx` and `.xltm` from Excel, Google Sheets and LibreOffice, Strict Open XML included: the sheets (name, state, kind, used range), the date system, each sheet's page setup; a save without an edit keeps every part byte for byte (a package signature that already fails verification gets its mark, as in docx and pptx); `.xls`, `.xlsb` and `.ods` are told by their content and refused with the way out. A new workbook (the same bytes every time), a sheet added, column widths, row heights and frozen panes |
| Cells, formulas and styles | ✅ | ⚠️ | ✅ | values, formulas as Excel displays them (shared ones moved to each cell) and as stored, cached values marked stale when the file asks for a recalculation or after an edit, the text Excel displays, dates, merges, the resolved style; the whole workbook as Markdown, a sheet as CSV. Values, dates, formulas (checked, stored as Excel stores them, spilling ones included), clears, bulk writes, CSV imports, styles and number formats, the records reused; a write into a shared formula's range keeps the other cells' formulas; what Excel would repair is refused. A spilling formula written without its range shows one value in Google Sheets (give the range). No evaluator yet: an edited workbook asks Excel and Google Sheets to calculate it on open, and `drop_cached_values` lets LibreOffice do it too (a function it lacks, such as `LAMBDA`, then shows an error) |
| Comments | ✅ | ❌ | ❌ | notes and threads with their replies, authors and resolution |
| Encryption | ✅ | — | ✅ | open and save with a password, from Rust and Python |

```bash
oxoffice xlsx text review.xlsx --markdown                 # every sheet as a table, each value at its address
oxoffice xlsx cells review.xlsx "Orders!F2:F5" --json     # values, formulas, number formats, styles
oxoffice xlsx csv review.xlsx --sheet Orders -o orders.csv
oxoffice xlsx comments review.xlsx
oxoffice xlsx set review.xlsx "Orders!D2" 12 -o edited.xlsx     # VALUE read as a CSV field: 007 stays text
oxoffice xlsx set review.xlsx G2 "D2*E2" --formula -o edited.xlsx
oxoffice xlsx import-csv review.xlsx orders.csv --sheet Orders --at A40 -o edited.xlsx
oxoffice xlsx edit review.xlsx --map edits.json -o edited.xlsx --drop-cached-values
#   [{"sheet": "Orders", "cell": "D2", "value": 12}, {"cell": "H2", "formula": "D2*E2"},
#    {"cell": "I2", "date": "2026-01-05"}, {"range": "A1:G1", "style": {"bold": true, "fill": "FFFF00"}},
#    {"range": "F2:F38", "number_format": "#,##0.00"}, {"range": "A39:G39", "clear": "contents"}]
oxoffice xlsx new --sheet Data --sheet Summary -o book.xlsx
oxoffice xlsx edit book.xlsx --map grid.json -o formatted.xlsx   # [{"columns": "A:C", "width": 20}, {"rows": "1", "height": 30}, {"freeze": "A2"}]
```

```python
from oxoffice.xlsx import Workbook

wb = Workbook("review.xlsx")
ws = wb.sheet("Orders")
total = ws.cell("F38")
print(total.formula, total.value, total.text)   # SUM(F2:F37) 206459.11 206,459.11
rows = ws.values("A1:G37")                      # nested lists, one native call

import datetime
from oxoffice.xlsx import Formula

ws.set_value("D2", 12)                          # the shared formulas of column F stay valid
ws.set_value("I2", datetime.date(2026, 1, 5))   # yyyy-mm-dd in a cell shown as General
ws.set_column_width("I", 12)                    # Excel shows ##### in a default-width column
ws.set_formula("H2", "XLOOKUP(A2,A3:A37,F3:F37)")   # stored with _xlfn., as Excel stores it
ws.write("A40", [["Total", None, None, Formula("SUM(D2:D37)")]])
ws.set_formula("J2", "SORT(D2:D37)", spill="J2:J37")   # the range lets Google Sheets spill it too
ws.set_style("A1:G1", bold=True, fill="FFFF00", border="thin")
ws.set_number_format("F2:F38", "#,##0.00")
ws.import_csv("north,12\nsouth,007\n", at="A50")  # 12 a number, 007 a str
ws.clear("A39:G39")                             # contents; formats=True for both
wb.save("edited.xlsx")                          # drop_cached_values=True for LibreOffice

wb = Workbook()                      # one empty sheet, Calibri 11
data = wb.add_sheet("Data")
data.set_column_width("A:C", 20)     # characters, as Excel's Column Width box
data.set_row_height("1", 30)         # points
data.freeze_panes("A2")              # the header row stays in view
wb.save("book.xlsx")
```

```rust
use oxoffice_xlsx::Workbook;

let workbook = Workbook::open("review.xlsx")?;
let sheet = workbook.sheet("Orders").expect("the Orders sheet");
for cell in sheet.cells("F2:F5")? {
    println!("{} {:?} {}", cell.address, cell.formula, cell.text);
}

use oxoffice_xlsx::{CellInput, DateValue, FormulaOptions, SaveOptions, StylePatch};

let mut workbook = Workbook::open("review.xlsx")?;
let mut orders = workbook.sheet_mut("Orders").expect("the Orders sheet");
orders.set_value("D2", 12.0)?;
orders.set_value("I2", DateValue::Date { year: 2026, month: 1, day: 5 })?;
orders.set_formula("H2", "D2*E2", FormulaOptions::default())?;
orders.write("A40", vec![vec!["Total".into(), CellInput::Formula("SUM(D2:D37)".into(), FormulaOptions::default())]])?;
orders.set_style("A1:G1", &StylePatch { bold: Some(true), fill: Some("FFFF00".into()), ..StylePatch::default() })?;
workbook.save_with_options("edited.xlsx", SaveOptions { drop_cached_values: true, ..SaveOptions::default() })?;

let mut workbook = Workbook::new();
let mut data = workbook.add_sheet("Data")?;
data.set_column_width("A:C", 20.0)?;
data.set_row_height("1", 30.0)?;
data.freeze_panes(Some("A2"))?;
workbook.save("book.xlsx")?;
```

### Fillable PDF forms

A Word form exports to a PDF that the recipient fills in, and signs, in Acrobat Reader or any PDF viewer. Its content controls and legacy form fields become fields: text, check boxes, lists and dates, with their current values. Word's own PDF export flattens forms.

```bash
oxoffice docx convert form.docx --to pdf --fillable-forms
```

```python
pdf = Document("form.docx").to_pdf(fillable_forms=True)
```

- **Signature field.** Tag a text or picture content control `signature`, or `signature:<name>` for several, in Word's control properties. It becomes an empty signature field, ready to sign in Acrobat Reader. A tag like `signature_date` stays a text field.
- **Names.** A field takes the control's tag, else its title, or the legacy field's name. A `.` becomes `_`, and a name used again takes `_2`, `_3`... The `signature` tag ignores case.
- **Values.** A control showing its placeholder exports empty, with the placeholder as the field's tooltip.
- **Notes, headers, footers and text boxes.** Controls in footnotes and endnotes become fields too, named after those of the body and of the headers and footers: footnotes first, then endnotes, in the order of their references. A control in a header or footer is one field with a widget on each page that draws it, so a value typed once shows on every page. Controls in a floating text box become fields where the box is anchored. In a rotated cell, a vertical section or a text box turned by a right angle, the widget turns with its text.
- **What stays static.** Controls split across pages or columns stay static, with a diagnostic. So do controls in stacked upright text, in a text box turned by another angle, one with vertical text, a text box inside another or a table inside a text box, or around a floating picture, and a signature in a header or footer that repeats on several pages. The controls of an inline text box are not fields yet, and say nothing. See `oxoffice limitations`.

## 🧭 How the library is meant to be used

- **Rust first.** Every capability exists in the Rust API first.
- **The CLI covers everything that makes sense from a shell**: inspect,
  convert, render, find and replace, fill a template, compare, accept or
  reject changes.
- **The CLI is made for agents and scripts first, people second.** Its
  `docx` and `pptx` commands give a `--json` record with a versioned
  `schema`, take their lists as JSON where they have one (`replace --map`),
  and name the cause when they refuse. The plain text output stays readable for a person
  checking by hand. When a choice has to be made, the agent's needs decide.
- **Python covers the whole Rust API**, so that when the CLI is not enough
  you switch to Python and keep going, without changing tools.
- Rust, CLI and Python go together. None is preferred over the others, and a
  feature is done when it reaches all three where it makes sense.
- Word and PowerPoint stand as equals, and spreadsheets will join them.
- **python-docx and python-pptx are a starting point, not a contract.**
  Their names make the API familiar, but we do not keep parity with them:
  the API differs wherever that makes it safer, simpler or faster.

## 🗺️ Status

oxoffice is young: 0.x, and the API can still move. It starts from rdocx 0.17.0 and rpptx 0.15.0, which
already cover a lot of Word and PowerPoint. What comes next lives in the issues labelled
[`roadmap`](https://github.com/oxoffice-dev/oxoffice/issues?q=is%3Aissue%20state%3Aopen%20label%3Aroadmap), one per workstream: edits that cost only
the element they touch, the remaining code splits, bibliographies, authoring tracked changes and completing
comments, and spreadsheets. Each gets a spec in `specs/` before work starts; nothing there is a promise of dates.

### Not planned

From the rdocx roadmap we leave out, unless a real need shows up: the
watermark and shadow qualification work, SmartArt and VML authoring, web
extensions and task panes, executable attachments, strict-conformance
qualification, the external benchmark campaign, and on the spreadsheet
side pivots, the Data Model, Power Query and Office Scripts.

## 🤝 Contributing

Read [CLAUDE.md](CLAUDE.md) and [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). Non-trivial changes start with
a short spec in [specs/](specs/). Commits and pull request titles follow
[conventional commits](https://www.conventionalcommits.org).

## 🙏 Credits

oxoffice is a friendly fork of [rdocx](https://github.com/tensorbee/rdocx) by Atul Sharma, who built the
Word and PowerPoint engine it starts from. See [NOTICE](NOTICE).

## 📄 License

[MIT](LICENSE).
