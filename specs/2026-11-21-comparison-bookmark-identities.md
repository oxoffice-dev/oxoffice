---
date: 2026-11-21
status: shipped
scope: docx
---

# Comparison: bookmark ids and generated TOC names are no change

## Context

`compare` refused two versions of a document with a table of contents when
the `_Toc` bookmarks of its headings had been renamed, and any pair where a
bookmark id had moved (issue #3). Both happen without anyone editing a
bookmark:

- Word numbers bookmark ids in document order on every save, so one bookmark
  added near the top moves the id of every bookmark after it.
- Word, and `Document::rebuild_toc`, name the heading bookmarks of a table of
  contents `_Toc` + digits, and rebuilding the table renames them. The entries
  link to the new names (`w:hyperlink/@w:anchor`, `PAGEREF _Toc...`).

A paired paragraph whose bookmarks differ cannot be revised in place, since
no tracked change renames a bookmark. The comparison then replaces it (a
deleted and an inserted paragraph), unless the other version holds the same
bookmark id, which would be held twice: "cannot revise paragraph boundary
structures", or "duplicate bookmark id N" further on. The skills worked
around it by stripping every `_Toc` bookmark from both copies, which left the
table of contents linking to nothing.

## Decision

Before comparing, the edited input takes the original's bookmark identities
where they mean the same bookmark, in its main story:

- **Ids.** A bookmark name both versions hold takes the original's id. A
  bookmark only the edited version holds keeps its id, or takes a fresh one
  when the original uses that id in any story.
- **Generated names.** The blocks of the two bodies pair as the comparison
  pairs them (signatures, longest common sequence, then each gap by kind),
  the signatures read without the generated bookmarks, as the comparison will
  read them once the names agree. A `_Toc` bookmark of a body paragraph that
  pairs with one of the original holding as many takes the original's name,
  in order. An unpaired body heading keeps its name, or takes a fresh `_Toc`
  name when the original uses it; a nested one (table, text box) keeps its
  name unless a paired heading claims it. Pairing what the comparison pairs
  keeps a heading removed in one place from pairing with one added further
  on, and leaves a heading whose bookmark did not change as it is.
- **References follow.** Link anchors, simple field instructions and complex
  field instructions naming a renamed bookmark are rewritten with it, so the
  edited input reads the same.

The guarantee becomes: accepting every revision gives the edited version up
to its bookmark ids and the names of its `_Toc` bookmarks (paired ones take
the original's name, unpaired ones whose name the original uses take a fresh
one); rejecting gives the original exactly. Nothing changes when the identities already
agree.

Deliberately not done:

- Other bookmark names are content: a renamed bookmark of the author still
  differs, and its paragraph is replaced (or the pair refused, as before).
- When a reference cannot follow (a name split across two instruction runs,
  or named in another story), the edited input stays as it is and the
  comparison refuses as before, rather than leave a link pointing at nothing.
- LibreOffice's `__RefHeading___Toc` names are not covered yet.
- Inserting or deleting a paragraph that holds a `w:fldSimple` is still
  refused. `Document::insert_toc` writes its page numbers that way, so adding
  a heading to such a table of contents still refuses: a separate limitation
  (#66, listed in `limitations.json`).

## Plan

One pull request: the reconciliation in `comparison.rs` (beside the footnote
custody reconciliation, which does the same for its own markers), behaviour
tests in `tests/docx/revisions_and_compare.rs` (Word-shaped tables of
contents with a heading added, renamed, or removed with another added
further on, renamed bookmarks, renumbered ids, a renamed author bookmark,
simple and hyperlink fields that follow, a split reference and a reference
from another story that keep the refusal), and the limitation entry removed.

## Outcome

Checked beyond the tests on the authored thesis of the corpus at scale (26
`_Toc` bookmarks, every name renamed as Word renames them, one sentence
edited): refused before, two revisions after, and accept and reject give back
each version with every table of contents link resolved.

## Links

- Issue #3, limitation `docx-compare-toc-bookmarks`.
- [Skill helpers move into the library](2026-11-20-skill-helpers-in-the-library.md):
  `strip_generated_bookmarks` becomes unnecessary.
