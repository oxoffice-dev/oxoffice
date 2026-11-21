---
date: 2026-11-21
status: draft
scope: python
---

# Python core property dates and shared helpers

## Context

Plan item 23 of the [code inspection spec](2026-11-21-code-inspection-and-refactoring.md) moves the helpers that the
two Python bindings copy from each other into `oxoffice-py-support`, "once the `datetime` question is settled". It is
settled: the user decided on 2026-11-21 that the core property dates are a `datetime` in both packages.

Today, on main 56b485b:

- `oxoffice.docx.CoreProperties.created`, `modified` and `last_printed` read as an aware UTC `datetime` (or `None`)
  and take a `datetime`. `oxoffice.pptx.CoreProperties` reads and writes the same three as the stored W3CDTF `str`.
- The conversion lives only in the Word binding (`crates/oxoffice-docx-py/src/document.rs`: `w3cdtf_fields`,
  `w3cdtf_to_datetime`, `datetime_to_w3cdtf`, about 120 lines). It follows python-docx: it reads a date and time, a
  date, a year and month, or a year, then an optional `+hh:mm` or `-hh:mm` offset. It reads `None` when the value
  cannot be read or falls outside `datetime`'s range in UTC. It writes `YYYY-MM-DDThh:mm:ssZ`, and it takes a naive
  value as UTC.
- The two `CoreProperties` classes have the same 30 accessors. 23 are identical; the other 7 are `author` and the
  six date accessors.
- Other identical functions between `oxoffice-docx-py` and `oxoffice-pptx-py`: `normalize_index`, `public_error`,
  `stale_to_pyerr` and `checked_font_input` (79 lines).
- The font helpers (`checked_font_kind`, `checked_font_license`, `font_kind_name`, `collect_caller_fonts`) differ only
  by `oxoffice_docx::` against `oxoffice_pptx::` types. They become identical once item 21 gives both facades one font
  embedding model.
- Signing is already shared (`oxoffice_py_support::signature_report_to_py`).
- One more difference: `CoreProperties.author` reads `""` when absent in `oxoffice.docx` (as python-docx) and `None` in
  `oxoffice.pptx`. Every other text property reads `""` in both.
- Comment dates (`date=` in Word, `created=` in PowerPoint) are `str` in both packages; they are not core properties
  and stay as they are.

## Decision

- **`oxoffice.pptx` core property dates become `datetime`, exactly as in `oxoffice.docx`.**
  - The getter returns an aware UTC `datetime`, or `None` when the property is absent or cannot be read.
  - The setter takes a `datetime`: a naive value counts as UTC, an aware one is converted to UTC, and `None` removes
    the property. A `str` raises `TypeError`.
  - The stored text keeps its W3CDTF form, `YYYY-MM-DDThh:mm:ssZ`.
  - This is a breaking change for Python callers: `feat(pptx)!` with a `BREAKING CHANGE:` footer naming the three
    properties and the new types.
- **The conversion moves to `oxoffice-py-support`** (a `dates` module), unchanged, with the property text limit
  (`CORE_TEXT_LIMIT`, 255 characters). Both bindings call it. One conversion means the two packages cannot drift
  again.
- **The other identical helpers move to `oxoffice-py-support` too:** `normalize_index`, `public_error`,
  `stale_to_pyerr` and `checked_font_input`.
  - The `CoreProperties` classes stay one per binding: they are two Python classes, `oxoffice.docx.CoreProperties`
    and `oxoffice.pptx.CoreProperties`, on two hosts. Their identical getters are three-line calls into the shared
    helpers; a macro would save less than it hides.
  - The font helpers follow item 21.
- **Not done here:**
  - the Rust facades and the CLI: they keep W3CDTF text, and only Python changes;
  - comment dates;
  - `author`'s `None`, unless the orchestrator decides otherwise (open question below).

## Plan

1. `feat(pptx)!: core property dates are datetime objects`
   - Touches `oxoffice-py-support` (the new `dates` module, moved from the Word binding), `oxoffice-pptx-py`
     (`presentation.rs`), the `oxoffice/pptx/_native.pyi` stub and the Python tests.
   - It does not touch the Word binding, whose `document.rs` and `lib.rs` belong to wave 3: until step 2, Word keeps
     its own copy of the conversion. Both copies are the same text, and a test pins their behaviour.
   - Tests (behaviour, through the facade):
     - a W3CDTF date written by PowerPoint reads as the right UTC `datetime`, with and without an offset;
     - a naive and an aware `datetime` save as the same UTC text, which reopens unchanged;
     - an unreadable stored date reads as `None`;
     - a `str` raises `TypeError`;
     - `None` removes the property.
   - `test_pptx_scaling.py` passes `datetime` values.
   - The same date cases run on both packages and give the same answers, so the two copies cannot drift before step 2.
2. `refactor(python): the Word binding uses the shared date conversion and helpers`
   - After item 17 splits `oxoffice-docx-py/src/document.rs`, inside the docx freeze.
   - Drops the Word copy of the conversion and both bindings' copies of `normalize_index`, `public_error`,
     `stale_to_pyerr` and `checked_font_input`.
   - It changes no behaviour: identical copies only, and the step 1 tests cover the dates.
3. After item 21: the font helpers (`checked_font_kind`, `checked_font_license`, `font_kind_name`,
   `collect_caller_fonts`) move to `oxoffice-py-support`, with item 21's shared types.

Each step is reviewed on the local branch before its one push.

## Open question

`oxoffice.pptx.CoreProperties.author` reads `None` when absent, where `oxoffice.docx` and every other text property
read `""`. Recommendation: read `""` in step 1, in the same breaking change, so a caller migrates once. That widens the
user's decision, so it needs the orchestrator's yes. Without it, step 1 leaves `author` as it is.

## Links

- [Code inspection and refactoring](2026-11-21-code-inspection-and-refactoring.md), items 21 and 23.
- [Embedded content and signatures in oxoffice-opc](2026-11-21-embedded-content-and-signatures-in-opc.md), item 22.
