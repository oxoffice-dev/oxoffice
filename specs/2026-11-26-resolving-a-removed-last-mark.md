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

**A. A removed mark with no paragraph to merge with stays.** At the end of its story (the body, a cell, a header, a
note, a comment, a text box, or a block control ending one, nested ones included), or before a table that stays, the
removed mark stays, untracked: its paragraph keeps its properties and what the resolution keeps of its content, and the
removed marks before it merge into it. The Word check of 2026-11-23 found Word cannot remove the body's last mark either
(Accept All left that change, Reject All an empty tracked paragraph). The comparison never writes such a mark; only
other producers' redlines hold one.

**Chains across block controls.** The removed last mark of a block control merges, as Word's merge rule does, with the
paragraph after the control, which joins the control (Word check of 2026-11-23); that paragraph may be the first one of
the next block control, nested ones too, or follow the end of an enclosing control. While the marks it meets are
removed, the merge goes on, across tables the resolution removes whole, until a paragraph whose mark stays, or rule A.
The walk starts after what the chain already crossed in its container. The merged paragraph takes the properties of the
last one and stays in the first control; a control the merge leaves with no owner goes, as one whose content all goes
does. Google Docs writes a suggestion that deletes across paragraphs this way, each paragraph in its own `goog_rdk`
control (up to 165 a file, in the body and in cells). The merge only enters a plain wrapper control: properties holding
nothing but an id, a tag, a lock and run properties, as Google writes them. The lock does not stop it: Google writes
`contentLocked` on its block wrappers, and oxoffice keeps a lock without enforcing it in any edit (spec of 2026-11-26 on
block control tables). A typed, data-bound or titled control, one showing a placeholder, or a building block (a table of
contents, a cover page) refuses the merge: its paragraph would leave it.

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

**Before a table, or ending the body after a table or block control**, removed paragraphs that keep nothing go, as
before (the body may end with that table or control, as the comparison needs to give back a body that ended with one);
ones that keep content merge into the last, whose mark stays (rule A). Before preserved XML only (`w:altChunk`, a block
`w:customXml`) the mark now stays (rule A).

**Stays refused**, with a message that says why:

- a paragraph outside block controls whose mark is removed and whose text stays, before a block control that stays:
  merging it into the control's first paragraph is not decided (see question 5);
- a block control's removed last mark whose merge would enter a control that is not a plain wrapper (above);
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
5. Which control keeps a merge across two controls. We keep it in the first, as Word keeps the paragraph after a
   control in that control; Word may keep it in the next one instead (the text is the same). Recommendation: keep the
   first, and decide the paragraph-before-a-control case with the same Word check.
6. Rule A before a table that stays: the Word check covers the body's last mark only. A paragraph that keeps text and
   whose mark is removed before a table keeps that mark here, as Word cannot merge a paragraph into a table; to check.

## Plan

1. #686: rules A and B in `revision.rs`; one table of shapes in `revisions_and_compare.rs` (body, block control,
   nested controls, cell, header, footnote, the mark alone by id, wrapped and unwrapped comment marks, a comment ending
   in a link) accepted as deletions and rejected as insertions, each failing on main; the issue's repro in Python.
2. The chains (`merge_after_control`), after a rerun of the real files left 7 of 13 failing: rows for a chain of
   controls in the body, in a cell and ending a cell, a control's last mark and a paragraph's before a table, a
   paragraph's ending the body after one, and the nested comment starts of two of the files (resolved by #686).
   Review of #688: the walk starts after the chain's crossed siblings (a marker or a control removed whole was taken
   twice); the rows read the controls, bookmarks and balanced range markers; the merge enters plain wrappers only.

## Links

- #680; [compare adds, removes and moves block content controls whole](2026-11-23-compare-block-content-controls.md).
