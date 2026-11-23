---
date: 2026-11-23
status: accepted
scope: docx
---

# Comparison: endnotes compared like footnotes

## Context

`compare` refuses an endnote added, removed or moved between two versions
(issue #68): "endnotes story shell count changed", "document comparison
requires identical related-story shells" for the first endnote, "its
reference moved to another site" for a moved reference, and also a table or
paragraph holding an endnote that the comparison replaces whole. With
endnotes ignored, an edited reference to an endnote the original lacks is
refused too. The limitation `docx-compare-endnotes` documents it.

Footnotes do all of it through the note custody
([moved footnotes](2026-11-21-comparison-moved-footnotes.md)): the ids are
reconciled before comparing, a note only one version holds is deleted or
inserted whole with `_rdocxCmpNote` marker pairs binding the note to its
reference, a moved reference makes the comparison start again with the
edited note under a new id, and accepting or rejecting settles the custody.
Endnotes go another way: the endnotes part is a plain related story whose
owners must match one to one. Every custody function is typed to footnotes
(`FootnotePart`, `footnoteReference`, the footnotes relationship and
content type, the `F` of the marker names), about 2,800 lines in
`comparison/footnotes.rs`, read from `comparison.rs`, `revision.rs`,
`postconditions.rs`, `relationships.rs`, `text_boxes.rs`, `guard.rs` and
`field_conversions.rs`.

## Decision

1. **One custody, a note kind.** `FootnotePart` becomes `NotePart` with a
   `NoteKind` (footnote, endnote) that gives the part's root and owner
   names, the reference element, the relationship type, the content type,
   the ignore option (`ComparisonStoryKind::Footnote` or `Endnote`) and the
   marker letter. Every custody step takes the kind and runs once per kind:
   id reconciliation, preflight, comparison of the note parts, the moved
   reference check, the marker inventory, validation, settlement on
   accept and reject, retirement of completed custody, the relationship
   remap of the note part. The endnotes part leaves the plain story path,
   as the footnotes part did.
2. **Markers.** An endnote pair is named `_rdocxCmpNoteE{token}P{o}{e}{B|N}`;
   `_rdocxCmpNoteF...` keeps meaning a footnote, so a redline written before
   this change still resolves. Tokens and pair ids come from one allocator,
   so the two kinds never share one.
3. **Moved references.** A pass that finds moved notes hands back the
   edited input with them under new ids, per kind; the next pass leaves both
   lists out of the pairing. Each pass gives at least one more note a new
   id, so the passes still end.
4. **Ignoring endnotes** keeps the original's endnotes whole, as ignoring
   footnotes does; an edited reference to an endnote the original lacks
   stays refused then ("ignored endnotes cannot supply ...").

Accepting gives the edited version, rejecting the original, endnotes
included, under the original's ids or new ones, as for footnotes.
Accepting and rejecting run the endnote custody only when the document has
an endnotes part (a relationship from the main part), so a document without
one, even holding a stray endnote reference, resolves as before.

Deliberately not done:

- The custody limits of footnotes apply to endnotes as they are (a note
  referenced twice or outside the main story, a reference inside a tracked
  change, the order of the notes both versions hold).
- A footnote turned into an endnote, or back, is a deletion and an
  insertion of two notes, not a conversion.
- No public API change: `compare` and `compare_with_options` keep their
  signatures; the doc comment names endnotes with footnotes.

## Plan

Two pull requests, each with the full corpus walk
(`OXOFFICE_WORKFLOWS=full`) before merging:

1. `refactor(compare)`: the note kind through the custody, footnotes only,
   no behaviour change. Checked by the existing footnote tests and the
   corpus walk unchanged; `revision.rs` resolves custody per kind.
2. `fix(compare)`: endnotes go through the custody. Behaviour tests in
   `tests/docx/revisions_and_compare.rs`, the footnote set run for endnotes
   (added, removed, the first endnote, moved within and across paragraphs,
   moved and edited, numbered by position, a paragraph moved with its
   endnote, a table replaced whole holding one), each accepted to the
   edited version and rejected to the original; a document with both kinds
   changed at once, which also checks that footnote redlines keep their
   `F` markers and resolve.
   The limitation `docx-compare-endnotes` removed. Closes #68.

## Links

- Issue #68; earlier: #4, #65, [moved footnotes](2026-11-21-comparison-moved-footnotes.md).
