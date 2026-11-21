---
date: 2026-11-21
status: shipped
scope: docx
---

# Comparison: a footnote whose reference moved

## Context

`compare` refused two versions where the reference of a footnote moved to
another place, even within its paragraph (issue #4): "its reference moved to
another site". The body comparison deletes the old reference and inserts the
new one, both naming the same note id, so the redline would reference one
note twice. Word for Mac cannot open such a file, and the note's own changes
would be recorded against both sites. The check that refuses it runs after
the body is compared and before any note is written.

## Decision

As Word shows it: the note is deleted with its old reference and inserted
again with its new one. When that check finds such notes, the comparison
starts again from the inputs, the edited version's notes under new ids
(footnotes part and references). The comparison already records a note only
the original holds as deleted and a note only the edited version holds as
inserted, so the second pass needs nothing new. Nothing is written to the
document before the second pass.

Accepting gives the edited version, the moved notes under their new ids;
rejecting gives the original exactly. Word numbers notes by position, so the
ids are not seen.

Deliberately not done:

- A pair whose moved note is referenced outside the main story, or whose
  edited version holds the note custody of an earlier comparison, keeps the
  refusal.
- Endnotes: the check and the note custody are footnote only. A moved
  endnote reference is not refused today, and its redline references the
  endnote twice, one deleted and one inserted reference. Left for its own
  issue.

## Plan

One pull request: the check returns the notes instead of refusing,
`compare_with_options` runs again with them renumbered, behaviour tests in
`tests/docx/revisions_and_compare.rs` (moved to another paragraph, within its
paragraph, swapped references, moved and edited), and the limitation entry
removed. The second pass costs one more comparison, only for these pairs.

## Links

- Issue #4, limitation `docx-compare-moved-footnote`.
- [Comparison: bookmark ids and generated TOC names are no change](2026-11-21-comparison-bookmark-identities.md),
  the same idea for bookmarks.
