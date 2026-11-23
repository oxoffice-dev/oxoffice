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
  shell, and an `oxoffice` Python package that covers nearly all of it (a few calls, such as bibliographies and
  encryption, are Rust only for now) for when the shell is not enough.
- 📄 **Word and PowerPoint as equals**, with spreadsheets on the [roadmap](https://github.com/oxoffice-dev/oxoffice/issues?q=is%3Aissue%20state%3Aopen%20label%3Aroadmap).
- ✍️ **Edits that keep the file intact**: unmodelled XML is carried over verbatim, child order follows the
  schema, and text changes keep their formatting.
- 🔍 **Real review tools**: list, accept and reject tracked changes, compare two versions into a redline,
  work with comments.
- 🖨️ **Rendering without an office suite**: PDF, PNG, HTML and Markdown, with bundled fonts so the output is
  the same on every machine; a Word render takes a system font only for characters they lack, such as most
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
with a few that touch whole files: edits and form operations that cost more as a Word document grows, and a deck
refused at open when one of its parts holds a control character.

### 📝 Word

| Family | Read | Edit | Create | Notes |
| --- | :---: | :---: | :---: | --- |
| Text, runs, find and replace | ⚠️ | ⚠️ | ✅ | counted replacements that keep formatting and refuse text they cannot reach; non-breaking hyphens and positional tabs are not read yet |
| Styles | ✅ | ✅ | ✅ | |
| Lists and numbering | ✅ | ⚠️ | ✅ | a list numbered through its style cannot be restarted |
| Tables | ✅ | ✅ | ✅ | merges, borders, shading, nested tables |
| Sections, headers and footers | ✅ | ⚠️ | ✅ | the document-wide header or footer setter reaches the last section only |
| Footnotes and endnotes | ✅ | ⚠️ | ✅ | removing a paragraph leaves its notes behind, and removing a note leaves its custom mark |
| Comments and replies | ✅ | ✅ | ⚠️ | anchored on a piece of text, moved, resolved, dated; in a file saved by Word in another language, a new comment does not take the file's comment styles yet |
| Tracked changes | ✅ | ✅ | ⚠️ | accept and reject by id, author or date; compare two versions into a redline, which refuses a few edits around simple fields; writing your own edits as tracked changes is on the [roadmap](https://github.com/oxoffice-dev/oxoffice/issues?q=is%3Aissue%20state%3Aopen%20label%3Aroadmap) |
| Fields and tables of contents | ✅ | ✅ | ✅ | page numbers, page references, dates, sequences, cross-references |
| Content controls and forms | ⚠️ | ⚠️ | ✅ | fill, validate, reset and protect in every story, export to fillable PDF fields; a date control shows month and day names in English, French, German, Spanish, Italian, Portuguese and Dutch, numeric dates in any language, and a time where its format shows one |
| Pictures and watermarks | ✅ | ✅ | ✅ | a replaced picture keeps its frame |
| Charts | ✅ | ⚠️ | ✅ | an existing chart's legend can change, not its data yet |
| Hyperlinks and bookmarks | ✅ | ✅ | ✅ | |
| Mail merge | ✅ | ✅ | ✅ | from Rust and Python |
| Bibliography | ⚠️ | ⚠️ | ⚠️ | sources, citations and the bibliography from Rust only for now; more on the [roadmap](https://github.com/oxoffice-dev/oxoffice/issues?q=is%3Aissue%20state%3Aopen%20label%3Aroadmap) |
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
- **What stays static.** Controls outside the main body (headers, footers, text boxes...) stay static. So do body controls split across pages or columns, in a rotated cell or a vertical section, or around a floating picture, each with a diagnostic. See `oxoffice limitations`.

## 🧭 How the library is meant to be used

- **Rust first.** Every capability exists in the Rust API first.
- **The CLI covers everything that makes sense from a shell**: inspect,
  convert, render, find and replace, fill a template, compare, accept or
  reject changes.
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
