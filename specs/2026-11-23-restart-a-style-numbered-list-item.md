---
date: 2026-11-23
status: accepted
scope: docx
---

# Restarting a list item numbered through its style

## Context

`Document::restart_numbering(paragraph, start)` (Python `Document.restart_numbering(paragraph, start=1)`) gives one
body paragraph a new numbering instance, a copy of its own with a `w:startOverride` at its level, and moves nothing
else (issue #82):

- It reads only the paragraph's own `w:numPr`. Word and python-docx number their List Number items through the
  style (`w:pStyle` and no `w:numPr`): restarting one fails with "paragraph has no explicit numbering level", on
  every item of those lists. LibreOffice writes a direct `w:numPr` on its items, which restart.
- The items after the restarted one stay on the old instance. They still show the new count: Word, LibreOffice and
  the layout count one definition's level across its instances, and only the first item of an instance with a
  `w:startOverride` restarts it (checked on a hand-built file with LibreOffice 26: `1. 2. 1. 2.`). But the list
  is now split over two instances: a later change to one instance (its levels, another restart) reaches only part
  of what reads as one list.
- Word 16 (object model, `ApplyListTemplate` from this point forward, which the macro recorder writes for "Restart
  at 1") forks the list instead: a new definition copied from the old one takes the style's link, the style
  points at a new instance without any override, the items before the restart get a direct `w:numPr` on the old
  instance, and the restarted and later items lose theirs (`handoff/pcwe/word-checks/list-restart.md` of run
  PCWE).

## Decision

**A restart takes the numbering the paragraph shows, and moves the paragraph and the rest of its list to a new
instance that starts at `start`.**

- **Which numbering**: the paragraph's own `w:numPr`, else its style chain's, read as
  `effective_paragraph_properties` and the layout read it (a `w:numId` without `w:ilvl` at the level its style chain
  gives). A paragraph that shows no list, a bullet or a `none` level is refused, as today.
- **The new instance**: as today, a copy of the paragraph's instance, with a `w:startOverride` of `start` at its
  level. An instance whose data the copy would lose is refused, as today.
- **The rest of the list**: every later body paragraph that shows the same instance, at any level, in tables and
  content controls too (document order), moves to the new instance and keeps the level it shows. A paragraph numbered through its style gets a direct `w:numPr`
  (`w:numId` and `w:ilvl`) and keeps its style. Items before the restarted one keep the old instance.
- **Return value**: the new instance's id, as today.
- **Python and CLI**: no new call and no new argument; Python's `restart_numbering` follows. No CLI command
  restarts a list today.

**What we do not do**:

- Word's fork. Pointing the style at a new list moves every paragraph of that style, in other lists, headers,
  notes and table cells, unless each is pinned, and it rewrites two definitions and the style's link. The override
  is OOXML's restart, what `add_numbered_list_item(restart=True)` already writes, and it stays inside the list.
  Word 16 and LibreOffice 26 draw the result `1. 1. 2.`, opened without repair, whether the items were numbered by
  their style or directly (`handoff/pcwe/word-checks/oxoffice-restart.md` of run PCWE).
- List items of the same instance in another story (headers, footers, notes) are not moved: they keep the old
  instance and its count. `restart_numbering` takes a direct body paragraph.
- No "continue numbering" call (joining an item to the list before it).
- A restart at a level other than the paragraph's own: the level is the one it shows.

## Plan

One pull request, `Closes #82`, which also removes the limitation `docx-restart-style-numbered-item`. Tests in
`tests/docx/lists.rs`:

- On Word's report, restarting "Move the remaining customers" (List Number, numbered by its style) moves it and
  "Review warehouse contracts" to one new instance, both with a direct `w:numPr` and their style; "Hire two" keeps
  its style numbering; the markers stay `1. 1. 2.` through a reopen, and an item added after them continues the
  new count.
- On a list with direct numbering over two levels, restarting the third item moves it and every later item, at
  both levels, in a table cell and in a content control too, to the new instance, each at its level; earlier
  items and another instance of the same definition keep theirs.
- The refusals (a plain paragraph, a bullet, a missing paragraph) change nothing.

## Links

- Issue #82, found by the independent review of #79.
- #421 (a missing `w:ilvl` read at the level Word shows), backlog #477, theme WRD.
