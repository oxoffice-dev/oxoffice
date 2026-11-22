---
date: 2026-11-22
status: shipped
scope: docx, pptx, python
---

# Characters XML cannot carry: refused or dropped at the setter, never at the save

## Context

XML 1.0 cannot carry some characters, not even as a character reference: the C0 controls other than tab, line feed
and carriage return, U+FFFE and U+FFFF. Before this change, a value holding one could get into the model unchecked
(#181).

A probe of every public text setter on main showed three outcomes:

- Word: about 35 setters accepted such a value, and every later save then failed. These were mostly infallible run,
  paragraph, table, header, list, hyperlink, footnote and core-property setters. Some setters that return `Result`
  did the same: `Paragraph::set_text`, `Table::set_caption` / `set_description`, `set_text_watermark`, `add_style`.
- Word: six setters panicked: `set_header`, `set_footer`, `set_first_page_header`, `set_first_page_footer`,
  `add_bullet_list_item` and `add_numbered_list_item`. `Cell::set_text` silently did nothing.
- PowerPoint: run text is stored as `_xHHHH_`, as python-pptx stores it. Names, titles and properties already
  refused such a value. Only `TextBulletCharacter::new` let one through.

## Decision

Maintainer decision 2026-11-22 (option A on #181):

- A Rust setter that returns `Result` refuses the value at the call. The error names the value and the character.
  The document is left unchanged.
- An infallible Rust setter drops the character, and its doc comment says so. No signatures change, so no Rust
  change breaks. Its `try_*` twin, where one exists, refuses instead.
- Python refuses the value at the call with `XmlError`, which now also derives from `ValueError`, in
  `oxoffice.docx` and `oxoffice.pptx`. Code that catches `DocxError`, `PptxError` or `XmlError` still works, and
  code written for python-docx or python-pptx that catches `ValueError` works too. Any refusal of such a character
  is an `XmlError`, wherever the Rust side wrapped it. This includes opening a file whose part holds one, which
  raised `PackageError` before.
- The CLI already calls only checked setters.
- PowerPoint run text keeps `_xHHHH_`.

Not done:

- No checked twin is added for every infallible setter. The `Result` forms that exist are the way to be told.
- A raw public field (such as `core_properties_mut().title` in pptx) is not guarded. The save still names the
  part and the character.

## Plan

One pull request, stacked on the #80/#100 one:

- Rust: shared helpers in `oxoffice_core::xml`:
  - `without_non_xml_characters` for infallible setters
  - `reject_non_xml_characters` for checked ones
  - `NON_XML_CHARACTER_REFUSAL`, the phrase every refusal carries
- Python: an explicit refusal before the edit where Python reaches an infallible setter:
  - `Run.text`
  - `Paragraph.add_run` and hyperlinks
  - `Cell.add_paragraph`
  - font and paragraph formatting values
- Rust, at the root of two classes:
  - every external relationship target is checked where the relationship is added
  - a style is checked as it will be saved, so every value a `StyleBuilder` sets is covered
- Python: the error mappers turn every refusal of such a character into `XmlError`. They recognise one by the shared
  words, which every refusal carries (a test keeps the package crate's copy the same).
- Tests:
  - Rust: one test drives every infallible setter with U+0007 and U+FFFE, then saves and reads back. One test checks
    that every changed checked setter refuses and changes nothing.
  - Python: a refusal for each kind of setter, and the error hierarchy.
  - Each changed file is mutation-checked.
- Removes the limitation `docx-property-control-characters`.

## Links

- #181, and the maintainer decision comment on it.
- #80 and #100, which made the reading and writing of tab, line feed and carriage return follow XML.
