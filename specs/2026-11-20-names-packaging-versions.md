---
date: 2026-11-20
status: accepted
scope: release
---

# Names, packaging and versions

## Context

The fork cannot publish under `rdocx`, `rpptx` or `oxml-*`: those crates and
the Python distributions belong to upstream on crates.io and PyPI. Upstream
also ships two CLIs (`rdocx`, `rpptx`), two Python distributions and two
version trains (Word 0.17.0, PowerPoint and shared crates 0.15.0), with
cargo-release metadata to keep them apart. The GitHub name `oxoffice` is taken
by an empty account, `oxoffice-dev` is free, and `oxoffice` is free on
crates.io and PyPI.

The project has three entry points that stand as equals: the Rust API comes
first, the CLI covers everything that makes sense from a shell, and Python
covers the whole Rust API for the moment the CLI is not enough. Word and
PowerPoint stand as equals too, and spreadsheets will join them.

## Decision

- Organisation `oxoffice-dev`, repositories `oxoffice` and `oxoffice-skills`.
- Every crate is `oxoffice-*`:

  | Before | After |
  | --- | --- |
  | `oxml-core`, `-opc`, `-drawing`, `-layout`, `-media`, `-pdf`, `-sml`, `-chart`, `-cli-support`, `-py-support` | `oxoffice-core`, `-opc`, `-drawing`, `-layout`, `-media`, `-pdf`, `-sml`, `-chart`, `-cli-support`, `-py-support` |
  | `rdocx`, `rdocx-oxml`, `rdocx-layout`, `rdocx-html` | `oxoffice-docx`, `oxoffice-docx-oxml`, `oxoffice-docx-layout`, `oxoffice-docx-html` |
  | `rpptx`, `rpptx-oxml`, `rpptx-layout`, `rpptx-render` | `oxoffice-pptx`, `oxoffice-pptx-oxml`, `oxoffice-pptx-layout`, `oxoffice-pptx-render` |
  | `rdocx-cli`, `rpptx-cli` | `oxoffice-docx-cli`, `oxoffice-pptx-cli` (libraries) |
  | `rdocx-py`, `rpptx-py` | `oxoffice-docx-py`, `oxoffice-pptx-py` (libraries) |
  | `rdocx-wasm`, `rpptx-wasm` | `oxoffice-docx-wasm`, `oxoffice-pptx-wasm` (kept) |
  | `rdocx-opc`, `rdocx-pdf`, `rpptx-chart` | removed, they only re-exported a shared crate |

- One binary, `oxoffice`, from the new `oxoffice-cli` crate:
  `oxoffice docx <command>` and `oxoffice pptx <command>`, each with the full
  command tree of the old CLI.
- One Python distribution, `oxoffice`, from the new `oxoffice-py` crate, with
  `oxoffice.docx` and `oxoffice.pptx`. Each keeps the public API of the old
  `rdocx` and `rpptx` modules. `RdocxError` and `RpptxError` become
  `DocxError` and `PptxError`.
- The Word and PowerPoint APIs stay separate. Only the packaging is unified.
- SemVer with one version for the whole workspace and the wheel, starting at
  0.1.0. Cargo and pip read compatibility from SemVer, and 0.x says the API
  can still move. Tags are `vX.Y.Z`.
- Licence `MIT`, the only licence file upstream ships, with its copyright
  notice kept.
- Identifiers written into documents keep their `rdocx` spelling for now
  (custom XML namespaces, bookmark and attribute names, watermark ids), so
  documents written by rdocx keep working. Moving them to an `oxoffice`
  spelling needs readers for both and gets its own spec.

## Plan

1. Rename crates, paths and identifiers. Each commit builds.
2. Remove the three shim crates.
3. One workspace version and package metadata.
4. Merge the CLIs into `oxoffice`.
5. Merge the Python bindings into `oxoffice`.
6. Product strings: CLI help, PDF producer, `app.xml` application, error
   messages, docstrings.

Checked by fmt, clippy with warnings denied on all targets, the WASM check, a
CLI round trip and a wheel smoke test (import, create, edit, save, reopen).

## Kept document identifiers

To be listed when the rename lands.
