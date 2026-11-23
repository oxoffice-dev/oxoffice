---
date: 2026-11-23
status: accepted
scope: docx
---

# Compare adds, removes and moves block content controls whole

## Context

`compare` treats a table as a body owner: one added or removed whole has every row marked, the resolution that
removes its rows removes the table and lets the paragraphs on either side meet, and a table that stays can end the
body. A block content control (`w:sdt` holding paragraphs and tables) has no such rule:

- Added or removed whole, it is refused: `comparison cannot delete an unmatched content-control or opaque body
  node` (#460). Word's own table of contents (References > Table of Contents) is a gallery control, so removing a
  Word TOC is refused, as is an optional clause or a cover page of a template. Moved, it is refused too.
- A paragraph appended after a body that ends with a control is refused: `comparison needs an adjacent paragraph
  for a final paragraph change` (#433). So is a paragraph removed first in the body before a control, or between
  two controls: the comparison takes the control for the end of the story and looks for a paragraph before it.
- A control added or removed in a cell or inside another control is refused the same way.
- Accepting or rejecting cannot resolve such a control either: a control whose every run and paragraph mark is
  deleted (as Word writes one deleted under tracking) stops accept with `cannot remove the final paragraph mark`,
  because the last paragraph mark of the control has no paragraph after it inside the control.

## Decision

**A block content control is a body owner like a table, in the comparison and in the resolution.**

- **Resolution** (`revision.rs`): a block control whose every owner the resolution removes (the mark of each
  paragraph, every row of each table, each nested block control whole) goes whole, its paragraphs holding nothing
  else, as a table whose every row it removes does. In a chain of removed paragraph marks, such a control is
  crossed; a control that stays stops the chain as a table that stays does (the paragraphs before it go), and the
  body may end with it.
- **Range markers** of a control that goes: a bookmark whose both ends it holds goes with it, as Word drops a
  bookmark whose whole range is deleted (a heading's `_Toc` bookmark, `_GoBack`). Every other bookmark, comment or
  permission marker stays where the control was, at block level, so that a range that only starts or ends in it
  keeps both ends.
- **Comparison**: a control added, removed or moved whole keeps its properties and has each paragraph and table
  inside marked as added, removed or moved whole, nested controls alike: the OOXML has no mark for the control
  itself, so the marks go on what it holds. Whether Word's own Compare writes the same shape is the Word check of
  step 3. In the body, in a cell and inside another control. It is an owner a change of the same
  resolution crosses, and a control both versions hold is kept like a table: a paragraph before it carries its own
  change, and a final paragraph after it too.

We deliberately do not:

- Track an unmodelled body node (`RawXml`): its XML cannot carry a mark. It stays refused, with its own message.
- Track a control holding runs, rows or cells, or one with nothing inside, added or removed whole: they are not
  block controls, or there is nothing to mark. Refused with a message that says so. Whitespace, proofing marks
  and range markers between its paragraphs are carried as they are.
- Give the copy of a moved control new properties: until the move is resolved, the redline holds both copies with
  the same `w:id`, tag and gallery (Word check, step 3).
- Change how a control both versions hold is compared inside.

## Plan

One pull request, closing #460 and #433:

1. Resolution rule, checked by a control whose content is all deleted or inserted under tracking, in the middle and
   at the end of the body, resolving both ways.
2. Comparison, checked both ways at word and character granularity, each redline accepted and rejected back to
   either version: a control added or removed in the middle, at the end, alone, two at the end; holding a table and
   a nested control; a paragraph appended after a final control (with and without a change inside it); a
   paragraph removed first before a control, between two controls, before a final control; a moved control; a
   control in a cell and in another control; a Word TOC gallery control with its field and hyperlinked entries.
3. Word check (Mac session): Word resolves the redline the same way.

## Links

- Issues #460, #433.
- The final-table rule this follows: `a_paragraph_deleted_before_a_final_table_deleted_by_an_earlier_redline_is_compared`.
