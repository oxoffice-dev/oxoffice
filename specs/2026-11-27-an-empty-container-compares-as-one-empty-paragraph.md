---
date: 2026-11-27
status: accepted
scope: docx
---

# A container empty in one version compares as one empty paragraph

## Context

`compare` refused a block content control, a header, a footer, a text box or a note that holds no block in one
version and paragraphs in the other: "comparison cannot give an empty ... content, nor take all of it" (limitation
`docx-compare-empty-container`, #665, what #681 left). No revision element records the container itself, and the
last paragraph mark of its content cannot be taken out ([block controls](2026-11-23-compare-block-content-controls.md)),
so no redline gave an empty container back. A note goes whole with its reference since #681; the refusal stayed for
the notes compare pairs by id in place: a reference in a tracked change, a note referenced twice or outside the main
story, one a version does not reference, notes of an earlier comparison still pending. Found on synthetic cases
(#656, #659); no corpus copy holds an empty container.

## Decision

**A container that holds no block reads as holding one empty paragraph, its normal form.**

- **The schema asks for one block.** In ISO/IEC 29500-4 `wml.xsd`, `CT_HdrFtr`, `CT_FtnEdn` and `CT_TxbxContent`
  (and `CT_Tc`) hold `EG_BlockLevelElts minOccurs="1"`. Every particle inside that group is optional, so the grammar
  still admits an empty sequence: an empty one is readable, not the form the type asks for. `CT_SdtContentBlock` is
  `minOccurs="0"`, but a block control's last paragraph mark is never taken out (removing it merges the next
  paragraph into the control, the 2026-11-23 rule), so the same normal form applies to it.
- **Comparison.** The empty side gets one empty paragraph (`<w:p/>` under the prefix its scope binds to
  WordprocessingML) before the pair is compared: the other side's content is tracked against it, its runs inserted
  or deleted, its properties as a property change, its other blocks inserted or deleted. Accepting and rejecting give
  the version that holds content exactly, and the empty one with one empty paragraph. Body, cells, headers, footers,
  notes, text boxes and nested controls alike.
- **Postconditions.** The guard reads every container holding nothing (a story, a text box, a block control's
  content) as holding one empty paragraph, so the empty version and the resolution that gives it back compare equal.
  A field after a container read so keeps its place in the part (the guard's part projection).
- **A story root written `<w:hdr></w:hdr>`** took the redline's content after its end tag (the reader reads it as
  `<w:hdr/>`, #689); its content now goes between its tags, and a root written self-closing gets an end tag.

We deliberately do not:

- Fill a container empty in both versions: it stays as written.
- Revise in place a comment one version leaves empty: `CT_Comment` asks for no block (`minOccurs="0"`). The comment
  parts are replaced whole, as before, and resolve to either version exactly.
- Pair by id a note empty in one version that #681 sends whole: it still goes whole, exactly.
- Change a block control holding nothing added or removed whole: refused with its own message (2026-11-23).

## Plan

One pull request, closing #665: the comparison and guard changes, the limitation file removed. Checked by one table,
block control, header, footer, text box, note (a reference in a tracked change), each filled and emptied, plus a
comment; the redline validates, accepting and rejecting give each version's story texts (the empty one as one empty
paragraph, the comment exactly). One row keeps an empty control both versions hold as written, before a field whose
paragraph is added whole. The compare walks with `OXOFFICE_WORKFLOWS=full`.

## Learnings

- Not checked in Word (no Word on Linux): how Word shows a redline that gives a header, a text box or a block control
  its first paragraph. Its Reject All should leave the empty paragraph, as oxoffice does; worth one Mac check.

## Links

- #665, #681; [block controls](2026-11-23-compare-block-content-controls.md),
  [empty elements](2026-11-26-empty-elements-with-end-tags.md).
