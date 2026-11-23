---
date: 2026-11-23
status: shipped
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
  body may end with it. A control goes whole only when the text of its paragraphs goes too.
- **The last mark of a control**: removing the mark of a control's last paragraph merges the paragraph after the
  control into it, inside the control, as Word does (its own Compare and Reject All, checked on Word 16.115).
- **Range markers** of a control that goes: a bookmark whose both ends it holds goes with it, as Word drops a
  bookmark whose whole range is deleted (a heading's `_Toc` bookmark, `_GoBack`). Every other bookmark, comment or
  permission marker of its paragraphs stays where the control was, at block level (a start in place, an end after
  a paragraph merged across the control), so that a range that only starts or ends in it keeps both ends. A table
  inside the control goes with its markers, as a table removed whole does.
- **Comparison**: a control added, removed or moved whole keeps its properties and has each paragraph and table
  inside marked as added, removed or moved whole, nested controls alike: the OOXML has no mark for the control
  itself, so the marks go on what it holds. In the body, in a cell and inside another control. It is an owner a
  change of the same resolution crosses, and a control both versions hold is kept like a table: a paragraph
  before it carries its own change.
- **A control added or removed whole that ends the body** after a paragraph keeps its last paragraph's mark, the
  body's last, untracked, and the mark of the paragraph before it carries the change, as for a final paragraph; resolving it
  merges that paragraph with the control's last one, and the control goes (Word check round 3: no revision left).
- **A final paragraph added or removed after a control both versions hold** puts its change on the mark of the
  control's last paragraph, and the body's final mark stays untracked: Word's own shape. Word cannot resolve a
  change on the body's final mark (first Word check: one revision left after Accept All, an empty tracked paragraph
  after Reject All).

**Word and a plain control added or removed whole** (decision of 2026-11-23, option A of the Word check): Word cannot
remove a block control by Accept All or Reject All, whatever the marks. Accepting the removal (or rejecting the
addition) of a control other than a table of contents leaves an empty inline control at the start of the next
paragraph in Word, or at the end of the previous one when the control ended the body. Word's own Compare does no better: it leaves an empty block control after accepting a removal,
and drops the control from an addition. We keep the shape that oxoffice resolves to either version exactly, and
name the Word behaviour in the limitation `docx-compare-block-control-in-word`. A table of contents control
(gallery `Table of Contents`) resolves cleanly in Word.

We deliberately do not:

- Track an unmodelled body node (`RawXml`): its XML cannot carry a mark. It stays refused, with its own message.
- Track a control holding runs, rows or cells, or one with nothing inside, added or removed whole: they are not
  block controls, or there is nothing to mark. Refused with a message that says so. Whitespace, proofing marks
  and range markers between its paragraphs are carried as they are.
- Give the copy of a moved control new properties: until the move is resolved, the redline holds both copies with
  the same `w:id`, tag and gallery; not part of the Word check.
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
3. Word check (Mac session), round 1 (2026-11-23): every redline opens without repair; the TOC case resolves both
   ways; cases with a plain control fail as described above; a final paragraph after a control needs Word's shape.
   Round 2: the final-paragraph shape passes both ways; the plain cases and a control of several paragraphs leave
   the known empty control; a control ending the body left a revision on the body's last mark. Round 3 (hand-made
   redlines): with that change on the mark of the paragraph before, no revision is left, the text never moves into
   the control, and only the known empty control remains.

## Links

- Issues #460, #433.
- The final-table rule this follows: `a_paragraph_deleted_before_a_final_table_deleted_by_an_earlier_redline_is_compared`.
