---
date: 2026-11-23
status: shipped
scope: docx
---

# One font record addressed by its index

## Context

A font table may carry several records with the same name: LibreOffice
writes `Symbol` twice in `tests/corpus/files/libreoffice/report.docx`. The
font calls find a record by its name, so `set_font`, `remove_font`,
`embed_font` and `remove_embedded_font` refuse such a name ("ambiguous font
record") and nothing can change or remove either record (#238).

## Decision

- **An index in `try_fonts()` order** addresses one record, the index
  `font_part_metadata` already takes. Rust: `set_font_at(index, font)`,
  `remove_font_at(index)`, `embed_font_at(index, face)`,
  `remove_embedded_font_at(index, kind)`, and `font_record_count()`, the
  bound of the index. Python: the same four methods; an index out of range
  raises `IndexError`, as the other indexed calls do.
  CLI: `font remove` and `font embed` take `--index N` in place of `--name`;
  `font list --json` gives each record its `index` (the text listing shows
  it too). There is no CLI command for `set_font` today, none is added.
- **The name calls keep refusing a name several records carry**, with an
  error that counts them and points at the index. Their behaviour is
  unchanged otherwise.
- `set_font_at` changes the fields of the record, not its name: a
  definition with another name is refused (a rename would be a new feature).
- `remove_font_at` returns the removed record (an index out of range is an
  error); `remove_embedded_font_at` returns the face, or nothing when the
  record has no face of that kind.
- Inside, the font-table model (oxml) addresses records by index only; the
  facade resolves a name to its one index first.

## Plan

One pull request. Tests: the LibreOffice report in Rust (each name call
refused, the second `Symbol` changed, given a face, the face and then the
record removed by index, the first `Symbol` untouched), in the CLI
(`--index` for embed and remove, `--name` with `--index` refused) and in
Python; the scaling test times the four new calls.

## Links

- Issue #238.
