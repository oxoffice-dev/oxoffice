---
date: 2026-11-21
status: accepted
scope: python
---

# Python core property dates and shared helpers

## Context

Plan item 23 of the [code inspection spec](2026-11-21-code-inspection-and-refactoring.md) moves the helpers that the
two Python bindings copy from each other into `oxoffice-py-support`, "once the `datetime` question is settled". It is
settled: the user decided on 2026-11-21 that the core property dates are a `datetime` in both packages.

Today, on main 78f44de:

- **The dates.** `oxoffice.docx.CoreProperties.created`, `modified` and `last_printed` read as an aware UTC `datetime`
  (or `None`) and take a `datetime`. `oxoffice.pptx.CoreProperties` reads and writes the same three as the stored
  W3CDTF `str`.
- **The conversion** lives only in the Word binding (`crates/oxoffice-docx-py/src/document.rs`: `w3cdtf_fields`,
  `w3cdtf_to_datetime`, `datetime_to_w3cdtf`, about 120 lines). It follows python-docx:
  - it reads a date and time, a date, a year and month, or a year, then an optional `+hh:mm` or `-hh:mm` offset;
  - it reads `None` when the value cannot be read or falls outside `datetime`'s range in UTC;
  - it writes `YYYY-MM-DDThh:mm:ssZ`, and takes a naive value as UTC;
  - it drops fractional seconds;
  - it reads a wrong instant in silence when an offset follows fractional seconds:
    `…T10:00:00.503+02:00` reads as 10:00 UTC instead of 08:00, because everything after the 19th character is
    ignored unless it is exactly 6 characters long (#249).
- **The `CoreProperties` classes** have the same 30 accessors. 23 are identical; the other 7 are `author` and the six
  date accessors.
- **`author`** reads `""` when absent in `oxoffice.docx` (as python-docx) and `None` in `oxoffice.pptx`. Every other
  text property reads `""` in both.
- **Helpers with the same text** in `oxoffice-docx-py` and `oxoffice-pptx-py`: `normalize_index` and
  `checked_font_input` (79 lines).
- **`public_error` and `stale_to_pyerr` read the same but do not behave the same.** Each looks the exception class up
  in its own package (`oxoffice.docx` or `oxoffice.pptx`). `StaleElementError`, `PackageError` and `XmlError` are
  separate classes in the two packages. `test_save.py` checks `PackageError` on both, and no test checks
  `StaleElementError` or `XmlError`.
- **The font helpers:**
  - `checked_font_kind`, `checked_font_license` and `font_kind_name` (32 lines) differ only by their `oxoffice_docx::`
    or `oxoffice_pptx::` types;
  - `collect_caller_fonts` (65 lines) uses `oxoffice_layout::FontFile` in both bindings and differs only by the line
    that maps its error. A folder holding a font that cannot be read raises `oxoffice.docx.LayoutError` in Word and
    `oxoffice.pptx.PptxError` in PowerPoint.
- **Signing** is already shared (`oxoffice_py_support::signature_report_to_py`).
- **Comment dates** (`date=` in Word, `created=` in PowerPoint) are `str` in both packages. They are not core
  properties, and they stay as they are.

## Decision

- **`oxoffice.pptx` core property dates become `datetime`, exactly as in `oxoffice.docx`.**
  - The getter returns an aware UTC `datetime`, or `None` when the property is absent or cannot be read.
  - The setter takes a `datetime`: a naive value counts as UTC, an aware one is converted to UTC, and `None` removes
    the property. A `str` raises `TypeError`.
  - The stored text keeps its W3CDTF form, `YYYY-MM-DDThh:mm:ssZ`.
  - No silent wrong instant in a new API: the shared conversion reads the right instant or `None`, never a shifted
    time. Fractional seconds are accepted and dropped, and an offset after them is applied. Parity with python-docx
    is not required.
    - The value is trimmed of XML whitespace first, as the W3CDTF schema types collapse it.
    - An offset must be exactly `±hh:mm`, with hours 00-23 and minutes 00-59. `Z` may also be lowercase.
    - The one accepted exception: a date and time without `Z` or an offset is taken as UTC, as python-docx and the
      old copy do. It has no zone to apply.
  - This is a breaking change for Python callers: `feat(pptx)!` with a `BREAKING CHANGE:` footer. The footer names
    the three properties, their new types, the `TypeError` on `str`, the loss of fractional seconds, a stored value
    that is not a readable W3CDTF date reading `None` instead of its text, and `author` reading `""` when absent
    (below).
- **The conversion moves to `oxoffice-py-support`** (a `core_properties` module), with that tail rule rewritten,
  together with the property text limit (`CORE_TEXT_LIMIT`, 255 characters). Both bindings call it. One conversion
  means the two packages cannot drift again.
- **`normalize_index` and `checked_font_input` move to `oxoffice-py-support` as they are.**
- **`public_error` and `stale_to_pyerr` move with the package as a parameter.** The shared helper takes the module
  name (`"oxoffice.docx"` or `"oxoffice.pptx"`), so each package keeps raising its own classes.
- **This amends the parent spec's table:** the `CoreProperties` classes stay one per binding instead of moving.
  - They are two Python classes, `oxoffice.docx.CoreProperties` and `oxoffice.pptx.CoreProperties`, on two hosts.
  - Their identical accessors are three-line calls into the shared helpers. A macro would save less than it hides.
- **`collect_caller_fonts` moves in step 2** with the package's exception as a parameter, as `public_error` does, so
  each package keeps raising its own class. **The three type-dependent font helpers follow item 21.**
- **Not done here:**
  - the Rust facades and the CLI, which keep W3CDTF text: only Python changes;
  - comment dates.

## Plan

Each step is reviewed on the local branch before its one push.

1. `feat(pptx)!: core property dates are datetime objects`
   - Touches `oxoffice-py-support` (a new `core_properties` module, the conversion moved from the Word binding),
     `oxoffice-pptx-py` (`presentation.rs`), the `oxoffice/pptx/_native.pyi` stub, the Python tests, and
     `docs/ARCHITECTURE.md` (py-support gains the dates).
   - It does not touch the Word binding, whose `document.rs` and `lib.rs` belong to wave 3. Until step 2, Word keeps
     its own copy of the conversion, the old tail rule.
   - Tests (behaviour, through the facade), run on both packages with the same answers, so the two copies cannot
     drift before step 2:
     - a W3CDTF date written by PowerPoint reads as the right UTC `datetime`, with and without an offset;
     - a naive and an aware `datetime` save as the same UTC text, which reopens unchanged;
     - a date with fractional seconds and an offset (`…T10:00:00.503+02:00`) reads as the right instant (08:00 UTC);
     - an unreadable stored date reads as `None`;
     - a `str` raises `TypeError`;
     - `None` removes the property.
   - Word keeps its old reading until step 2. The test that runs the cases on both packages marks each case where Word
     still differs as a strict known failure on Word, citing #249, so it flips to a pass the day Word is fixed.
   - Removing a date leaves an empty element in PowerPoint's `core.xml` (#303, the facade's writer). The test marks
     it as a strict known failure on PowerPoint.
   - `test_pptx_scaling.py` passes `datetime` values.
   - It waits until item 15 (debug work counters) is done with pptx-py `presentation.rs` and py-support `lib.rs`;
     its first pull request, #211, edits py-support `lib.rs`.
2. `refactor(python): the bindings share the date conversion and the helpers`
   - After item 17 splits `oxoffice-docx-py/src/document.rs`, inside the docx freeze.
   - Drops the Word copy of the conversion and both bindings' copies of `normalize_index`, `checked_font_input`,
     `public_error`, `stale_to_pyerr` and `collect_caller_fonts`.
   - Adds a test on both packages: a stale handle and a package error raise that package's own class.
   - Closes #249, and its known failure marks go. Word's readings change, all toward the right instant or `None`:
     - an offset that is not exactly `±hh:mm` after the seconds (`+0200`, `+02`, `+2:00`), or out of range
       (`+00:60`): shifted before, `None` after;
     - an empty fraction (`.`, `.Z`): read before, `None` after;
     - leading whitespace, and a six-character ending that does not start with a sign (`.1234Z`, `.12345`): `None`
       before, the right instant after;
     - an offset after fractional seconds: shifted before, right after.
   - The PR names these changes, since a `refactor` title alone would hide them.
3. After item 21: `checked_font_kind`, `checked_font_license` and `font_kind_name` move to `oxoffice-py-support`, with
   item 21's shared types.

**Outside this repository:** oxoffice-skills documents the PowerPoint dates as RFC 3339 strings
(`plugin/skills/pptx/references/python-api.md`, `SKILL.md`). Its update goes with the release that ships step 1, as a
follow-up there.

## `author` (decided)

`oxoffice.pptx.CoreProperties.author` read `None` when absent, where `oxoffice.docx` and every other text property
read `""`. The user chose to align (option A, 2026-11-21): step 1 makes it read `""`, in the same breaking change, so
callers migrate once. A caller that tested `author is None` now sees `""`.

## Links

- [Code inspection and refactoring](2026-11-21-code-inspection-and-refactoring.md), items 15, 17, 21 and 23.
- [Embedded content and signatures in oxoffice-opc](2026-11-21-embedded-content-and-signatures-in-opc.md), item 22.
