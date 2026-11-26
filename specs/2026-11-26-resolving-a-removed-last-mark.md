---
date: 2026-11-26
status: accepted
scope: docx
---

# Resolving a removed last mark and the controls a resolution empties

## Context

`accept_all` refused two shapes Google Docs writes for suggestions, on 13 of 140 real Google-to-Word files (#680), and
`reject_all` their inserted forms: the body's last paragraph with its mark deleted ("cannot remove the final paragraph
mark"), and a paragraph removed whole whose text sits in an inline control tagged `goog_rdk_<n>` ("a content control
removed whole requires its paragraphs to keep no content"). A removed mark merges its paragraph into the next one, as
Word does; with no paragraph after it there was no rule. And the walker that removes a paragraph whole disagreed with
the predicate that decides a block control goes whole (`inline_content_removes` counts an emptied control as nothing).

## Decision

**A. The last mark of a story stays.** A removed mark with no owner after it in its story (the body, a cell, a header,
a note, a comment, a text box, or a block control ending one, nested ones included) stays, untracked: its paragraph
keeps its properties and what the resolution keeps of its content, and the removed marks before it merge into it. The
Word check of 2026-11-23 found Word cannot remove the body's last mark either (Accept All left that change, Reject All
an empty tracked paragraph). The comparison never writes such a mark; only other producers' redlines hold one.

**B. A paragraph removed whole takes the wrappers the resolution empties.** Where a paragraph goes whole (a block
control removed whole, a paragraph before a table or block control that stays, the end of the body after one), an
inline control, hyperlink, smart tag or custom XML element left with nothing inside goes with it, as a block control
whose content all goes does (decision A of 2026-11-23). A Google wrapper then changes nothing. A control that still
holds anything (text, the placeholder run of an unfilled control, a field, a picture) keeps its paragraph. Where the
paragraph merges into the next one instead, its controls move with its content, as Word's merge does.

**Range markers and comments.** A paragraph removed whole leaves its range markers at block level where it was: those
in its emptied wrappers and the comment marks its removed text keeps (Google moves comment marks inside `w:del` and
`w:ins`). A bookmark whose two ends go with it is dropped. A comment whose start, end and reference all go, in removed
revisions or in the content removed whole, goes whole with its definition.

**The end of the body after a table or block control** is unchanged: a removed last paragraph that keeps nothing goes,
and the body ends with that table or control, as the comparison needs to give back a body that ended with one. Rule A
applies after a paragraph. Before preserved XML only (`w:altChunk`, a block `w:customXml`) the mark now stays (rule A).

**Stays refused**, with a message that says why:

- a block control's last mark followed by a table, by a paragraph whose mark is removed too, or by the end of an
  enclosing control that a paragraph follows: only a paragraph right after the control whose mark stays can take it;
- a removed last paragraph after a table or block control that keeps content;
- a paragraph removed whole holding the reference of a comment that starts or ends outside the content removed with it;
- a simple field emptied by the resolution follows the retiring-field rule, as before.

## Open questions (Word check, none on Linux)

1. Rule A against Google's intent ("delete the last paragraph"): a block control ending the body removed whole leaves
   the body ending at the mark before it, a plain last paragraph leaves an empty last paragraph. Recommendation: keep A,
   it is Word's result.
2. Rule B against Word, which never removes a control through a revision (an empty control lands in the next
   paragraph, limitation `docx-compare-block-control-in-word`). Recommendation: keep B, consistent with decision A.
3. A user's control holding nothing in a paragraph removed whole goes with its properties (date, drop-down, alias).
   Recommendation: keep, unless a real file shows an empty typed control in a deleted paragraph.
4. Rejecting a Google-inserted last paragraph leaves an empty last paragraph, as Word's Reject All does.

## Plan

One pull request (#686): rules A and B in `revision.rs`; one table of shapes in `revisions_and_compare.rs` (body,
block control, nested controls, cell, header, footnote, the mark alone by id, the kept refusal, wrapped and unwrapped
comment marks, a comment ending in a link) accepted as deletions and rejected as insertions, each failing on main; the
issue's repro in Python.

## Links

- #680; [compare adds, removes and moves block content controls whole](2026-11-23-compare-block-content-controls.md).
