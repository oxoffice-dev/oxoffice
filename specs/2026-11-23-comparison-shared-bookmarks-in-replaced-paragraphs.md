---
date: 2026-11-23
status: accepted
scope: docx
---

# Comparison: a bookmark both versions hold, in a paragraph replaced whole

## Context

- No tracked change records a bookmark. A paired paragraph whose bookmarks differ (a heading that keeps its bookmark
  and gains a `_Toc` one from a contents rebuilt in one version, or a `_Ref` one from a cross-reference added in Word)
  cannot be revised in place. Replacing it whole (deleted, then inserted) would hold the kept bookmark twice. So
  `compare` refused it ("cannot revise paragraph boundary structures"), limitation `docx-compare-bookmark-change`
  (#448). The Google thesis walk with rebuilt contents met it.
- A first layout wrote the shared bookmark whole, untracked, in the inserted paragraph. Our own engines resolved it
  right, but Word's Reject All removes the inserted paragraph with its mark and drops every bookmark whose start and
  end both sit in it (OX-M, Word 16.115, 5 of 6 files). A layout with the start in the insertion and the end in the
  deletion passed in Word.

## Decision

- A pair whose bookmarks differ is replaced whole, **the insertion first** (also when it is the story's last entry).
- A bookmark both versions hold (same id and name after `reconcile_body_bookmarks`) is written **once**, outside the
  revision marks: its **start** in the inserted paragraph, where the edited version has it; its **end** in the deleted
  paragraph, where the original has it. A bookmark the edited version holds as a point (start then end, nothing
  between) stays whole in the insertion.
- Rejecting leaves such a start at the restored paragraph's start; accepting leaves such an end just after the inserted
  paragraph's mark, as Word does. The text the bookmark covers does not change.
- The postconditions accept that, and only that: for those bookmarks, each resolution holds each once under its name
  and over the same text as the version it gives (`bookmark_coverage`), and the stories compare exactly without them.
- Still refused, deliberately: a shared bookmark that starts or ends **inside** the replaced paragraph's text (no layout
  keeps it there through both resolutions), one under another id, or nested in a hyperlink or content control. The
  limitation `docx-compare-bookmark-change` is narrowed to these.
- Not done: tracking bookmark changes (no markup exists), moving one-sided bookmarks (they go with their paragraph's
  content, as before).

## Plan

One pull request on `comparison/` (`bookmarks.rs`, `alignment.rs`, `paragraph_properties.rs`, `paragraphs.rs`, the
postconditions in `comparison.rs`). Checked by:

- tests per story (body, cell, header, footnote, last paragraph), a link change, a bookmark ending in a replaced
  paragraph, the Google thesis walk with rebuilt contents, and the refusals;
- the full corpus walk (`OXOFFICE_WORKFLOWS=full`);
- Word for Mac (OX-M): Accept All and Reject All on the redlines of those tests.

## Links

- #448, limitation `docx-compare-bookmark-change`, #455 (the insert side), #458.
- [Comparison: bookmark ids and generated TOC names are no change](2026-11-21-comparison-bookmark-identities.md).
