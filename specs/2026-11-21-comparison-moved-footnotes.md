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

The same refusal hits a far more common pair: Word numbers footnotes by
position when it saves, so one note added near the top gives every later
note a new id, and the comparison, which pairs notes by id, sees every
later reference move.

## Decision

1. **Ids first.** Before comparing, the edited notes take the original's
   ids, as bookmark ids do (#3): the notes referenced in the main story are
   paired in reference order by their text (longest common sequence, then
   the notes left between two pairs, by position, only when both sides leave
   as many). An unpaired note keeps its id, or takes a new one when a
   paired note takes it or when the original references a note under it
   too; a note one version does not reference keeps its id and is revised
   in place. A pair that would take the id of a note the edited version
   does not reference is dropped, so that note keeps its id. A note added
   at the top is then the only note change.
2. **A reference that really moved.** As Word shows it: the note is deleted
   with its old reference and inserted again with its new one. When the
   check finds such notes, the comparison starts again from the inputs, the
   edited version's moved notes under new ids (footnotes part and
   references), and those notes are left out of step 1 in the next pass.
   The comparison already records a note only the original holds as deleted
   and a note only the edited version holds as inserted, so that pass needs
   nothing new. Nothing is written to the document before it. A paragraph
   moved with its reference is therefore deleted and inserted again with its
   note rather than recorded as a move, since both ends of a move would name
   the note.

Accepting gives the edited version, its notes under the original's ids or
new ones; rejecting gives the original exactly. Word numbers notes by
position, so the ids are not seen.

Deliberately not done:

- Step 1 changes nothing when a note is referenced twice or outside the
  main story, when a reference sits in a tracked change, or while note
  custody of an earlier comparison is pending.
- The notes both versions hold must still come in the same order in their
  footnotes parts ("footnote retained owner order changed" otherwise), as
  before.
- A pair whose moved note is referenced outside the main story, or whose
  edited version holds pending note custody, keeps the refusal. Completed
  custody is retired first, as the first pass retires it.
- Endnotes: the check and the note custody are footnote only. A moved
  endnote reference is not refused today, and its redline references the
  endnote twice, one deleted and one inserted reference. Left for its own
  issue.

## Plan

One pull request: the check returns the notes instead of refusing,
`compare_with_options` runs again with them renumbered, the ids reconciled
before each pass, behaviour tests in `tests/docx/revisions_and_compare.rs`
(moved to another paragraph, within its paragraph, swapped references also
numbered by position, moved and edited, a note added before the others, a
note added above with a moved reference, a moved note under a new id, a
paragraph moved with its note, a note neither version references, a note
only one version references, a note the edited version does not reference
under the id a pair would take), and the limitation entry removed. Each further
pass costs one more comparison, only for these pairs; the passes run in a
loop that drops each pass's state, at most one per note.

## Links

- Issue #4, limitation `docx-compare-moved-footnote`.
- [Comparison: bookmark ids and generated TOC names are no change](2026-11-21-comparison-bookmark-identities.md),
  the same idea for bookmarks.
