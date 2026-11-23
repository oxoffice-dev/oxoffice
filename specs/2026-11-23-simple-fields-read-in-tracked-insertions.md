---
date: 2026-11-23
status: accepted
scope: docx
---

# A tracked insertion holds its simple fields as runs

## Context

A simple field typed with tracked changes on sits in a `w:ins`:
`<w:r>B</w:r><w:ins><w:fldSimple QUOTE><w:r>12</w:r></w:fldSimple><w:r>C</w:r></w:ins>`. The revision reader
(`CT_Revision::try_from_raw_with_read_output`, through `CT_P::from_xml_with_read_output` with revision source facts)
deferred every `w:fldSimple`, in the paragraph and in a hyperlink. It kept the field as raw `extra_xml` of the stored
content paragraph and set `semantic_fields_pending`. Three places then re-parsed the raw revision XML:
`CT_Revision::accepted_content` on every read (paragraph text, the accepted view: about 16 times the cost of the
stored read), `physical_content_paragraph_mut` for the layout and story scans, and the refresh after an in-place edit.

Readers that borrow the stored paragraph missed the field (#150, what is left after #422, #489 and #492):

- `Paragraph::runs()`, Python `Paragraph.runs` and the CLI run JSON list `B`, `C`, while the text reads `B12C`.
- An insertion inside an inline content control (`append_accepted_control_runs`) reads its stored paragraph:
  `Paragraph.text` and HTML read `ABC`, without `12`.
- Markdown counts an inline control's bookmarks on the stored runs (`control_bookmark_starts`) but writes the
  expanded field result: an anchor after the field lands inside it (`B1<a id="mark"></a>2CD`).
- An equation in an inline control is dropped by both exports: the control's view keeps runs only.

Nothing needs the deferral. The revision read already has what the paragraph read uses to parse a field eagerly: the
field input cursor that registers the field's origin, and the source maps. The deferral came with the rdocx import.

## Decision

**The revision read parses simple fields as the paragraph read does, so the stored content paragraph is the
reader's view.** One read, one paragraph, no re-parse.

- `paragraph_read.rs` and the hyperlink reader (`paragraph_projection.rs`) take the same `w:fldSimple` branch with or
  without revision source facts: a field run, its origin registered, its source kept. A field is not a direct run of
  the revision, so `RevisionContent::Runs` (the physical runs) does not change.
- `RevisionSourceFacts::simple_fields_deferred` and `CT_Revision::semantic_fields_pending` go. `accepted_content`
  returns the stored paragraph. `physical_content_paragraph_mut` parses only a revision that has no stored paragraph
  (a deletion without a field owner). The refresh after an edit reads the field once, in the same read.
- **An inline content control's view is built on the runs it writes.** `append_accepted_inline_control` serves both
  exports. It reads an insertion's content and a paragraph through their own accepted view, simple fields expanded,
  as the paragraph's own insertions are. It records each bookmark start and each equation (a raw `m:oMath` child,
  read in the control's namespace scope) at its place among those runs. `control_bookmark_starts`, which counted on
  stored runs, goes.
- The writer does not change: the revision's raw XML stays the serializer's authority, so a document opened and
  saved writes the insertion as it was read.

Not changed: the layout (family 3 of #150) already read the re-parsed view, so it reads the same paragraph. The tracked
view, deletions and move sources keep their current reads.

## Invariants

- For an insertion or a move destination, `accepted_content()` is the stored content paragraph, borrowed.
- `Paragraph::runs()`, Python `Paragraph.runs`, the CLI run JSON and `Paragraph.text` read the same runs.
- In an inline control, HTML and Markdown write every anchor and equation at its place among the runs they write.
- A simple field's origin is registered by the read that stores it, as in a paragraph; the debug cross-checks of
  every edit hold.

## Plan

One pull request (`oxoffice-docx-oxml`: `text/paragraph_read.rs`, `text/paragraph_projection.rs`, `revision.rs`,
the accepted-view part of `text.rs`). Tests: `tests/docx/reader_view.rs` covers runs of an inserted field, saved
unchanged; an inline control's insertion in text and HTML; an anchor after a two-run inserted field in an inline
control in both exports; an equation in an inline control and in an insertion inside one, in both exports. The CLI
test checks `text --json` runs, and `test_reader_view.py` checks Python `Paragraph.runs`. Each test fails on main.

## Links

- Issue #150 (what is left); earlier: #60, #422, #439, #489, #492.
