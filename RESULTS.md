# Word checks for #469: what Word writes for tracked changes and modern comments

Word for Mac 16.115.2 (French UI), driven by AppleScript (`osascript`) on 2026-11-27, 18:46-18:58 CET (UTC+1).
Word 2016+ for Mac has no `do Visual Basic`; every action below is a call of Word's AppleScript dictionary, which
maps to the VBA object model (named in each section). Scripts: `scripts/`. Each `wNN-*` folder holds the probe
`.docx` files and, under `xml/<file>/`, the `document.xml`, `comments*.xml`, `people.xml` (and `numbering.xml` for
W5) that Word saved, byte for byte.

## Read this first

- **Authors.** Setting `Application.UserName` (AppleScript `user name`) to "Mac" does not change the author: Word for
  Mac writes the signed-in Microsoft account's name, `Hadrien Mary`, as `w:author` of every revision and comment,
  even after a restart. So "Mac" below is Word's own identity (`w:author="Hadrien Mary"` in the files), and "Other"
  is a revision written by hand in the XML before Word opened the file (`*-input.docx`, `*-hand-*.docx`). The user
  name was restored to its original value; Word was quit at the end.
- **Dates.** `w:date` on `w:ins`, `w:del`, `w:rPrChange`, `w:pPrChange`, `w:tcPrChange`, `w:moveFrom`... and on
  `w:comment` is the **local wall-clock time with a `Z` suffix** (`18:46:00Z` when UTC was 17:46), seconds zeroed.
  The true UTC time is in `w16du:dateUtc` (revisions) and `w16cex:dateUtc` (comments). `w:moveFromRangeStart` /
  `w:moveToRangeStart` carry `w:author`, `w:date` and `w:name` but no `w16du:dateUtc`.
- **Redaction.** `people.xml` held the account's Microsoft (Windows Live) CID as `w15:userId`; it is replaced by
  `0000000000000000` (same length) in every file here. Nothing else was changed.
- French UI: Word writes French style ids (`Titre1`, `Paragraphedeliste`, `TM1`, `Commentaire`,
  `Marquedecommentaire`).

## Answers

**W1. Own insertion, edited later by the same author.** Step 1 (18:46): "Mac" inserts " alpha beta gamma". Step 2
(reopened, 18:47): typing "XYZ " inside it **splits the insertion**: `w:ins`(18:46) " alpha " / new `w:ins`(18:47)
"XYZ " / `w:ins`(18:46) "beta " (the object model still reports one merged insertion). Deleting "gamma", inserted
earlier by the same author, **drops it**: no `w:del`. Side effect: Word's smart spacing also deleted one original
space, tracked as `w:del " "`. Calls: `Selection.TypeText`, `Selection.TypeBackspace` on a selected range.

**W2. "Mac" edits inside "Other"'s insertion.** The insertion is **split in three**: `w:ins` Other "one two " /
`w:ins` Mac "MAC " / `w:ins` Other "three ... five". The deletion is **nested, `w:del` inside `w:ins`**:
`<w:ins Other><w:r>three </w:r><w:del Mac><w:r><w:delText>four </w:delText></w:r></w:del><w:r>five</w:r></w:ins>`.
Calls: `Selection.TypeText`, `Selection.TypeBackspace`.

**W3. "Mac" bolds part of its own insertion.** **Yes**, an `w:rPrChange` (author Mac, old `<w:rPr/>`) is written on
the bold run inside Mac's own `w:ins`, in the same session and again after reopening. Call:
`Range.Font.Bold = True`.

**W4. Two formatting changes on one word.** **One `w:rPrChange`**. Only Word's own identity can act in Word, so the
order was swapped: "Other" bolds (hand-written `rPrChange` by Other), then "Mac" italicizes in Word. Result:
`<w:b/><w:i/>` with a single `rPrChange` whose **author and date are the last editor's** (Mac, new date) and whose
**old properties are the state before the first change** (`<w:rPr/>`). Other's authorship is lost. Same author
(bold at 18:49, italic at 18:52): still one `rPrChange`, date moved to 18:52, old `<w:rPr/>`. Call:
`Range.Font.Italic` / `.Bold = True`.

**W5. Numbering and style.** **`w:pPrChange` only** (old `<w:pPr/>`): the new `w:pPr` holds `w:numPr` (plus
`pStyle` List Paragraph for the numbered default) or `pStyle Titre1`, or both. **No `w:numberingChange`**, no
`rPrChange`, nothing tracked in `numbering.xml`. Calls: `ListFormat.ApplyNumberDefault`,
`ListFormat.ApplyBulletDefault`, `Range.Style = wdStyleHeading1`.

**W6. oxoffice 0.8.0 compare, paragraph 1 moved to the end.** `oxoffice docx compare --author Mac --timestamp
2026-11-27T12:00:00Z -o redline.docx A.docx B.docx`. The redline has `w:moveFrom` / `w:moveTo` (run wrappers and
paragraph marks: moveFrom on Alpha's mark, moveTo on Charlie's mark) but **no `moveFromRangeStart/End` nor
`moveToRangeStart/End`**, all four elements share **`w:id="0"`**, and the two moved runs repeat about 30 `xmlns`
declarations each. Word opens it by script with no repair prompt, but **reads it as a deletion plus an insertion, not a move** (object model: `wdRevisionDelete` +
`wdRevisionInsert`, so no green double underline). **Accept All and Reject All give the right text** (B order, A
order, 3 paragraphs each). Variants by hand (`redline-variant-*`): unique ids alone, still delete + insert; with
range markers starting in the moved paragraph, move from + move to plus an extra insertion for Charlie's moveTo
mark; with **`moveToRangeStart` placed in Charlie's paragraph after its run, so the range covers the moveTo mark,
exactly two revisions, move from and move to**. Accept and Reject were right in every variant. Calls:
`Revisions.AcceptAll`, `Revisions.RejectAll`.

**W7. Word moves two paragraphs.** `Range.Relocate wdRelocateDown` in outline view (moves P1+P2 below P3). Both
ranges **start inside the first moved paragraph** (after its `w:pPr`, before the `w:moveFrom`/`w:moveTo` run
wrapper) and **end at body level after the last moved paragraph's `</w:p>`**, so they cover both paragraph marks.
Each moved paragraph's mark carries `w:pPr/w:rPr/w:moveFrom` at the source and `w:moveTo` at the destination.
`w:name="move246697827"` pairs the two ranges; every element has its own `w:id`.

**W8. Tables.**
- Row height (exact 40 pt): `w:trHeight` changes with **no `w:trPrChange`**; Word writes `w:tblPrChange` and
  `w:tblGridChange` (old = new) and an identical `w:tcPrChange` on each cell of the row (1 "table property"
  revision). Reject All still brings the row back to auto height.
- Cell shading (cell r2c2): **`w:tcPrChange`** on that cell (old `tcPr` without `w:shd`), and an identical
  `tcPrChange` on every other cell of rows 1 and 2 (row 3 has none), plus `tblPrChange` and `tblGridChange`. Reject
  All removes the shading.
- Insert column (`Selection.InsertCells wdInsertCellsEntireColumn`; `InsertColumns` is not reachable by
  AppleScript): **no `w:cellIns`**; the new cells are bare, every cell gets a `tcPrChange`, the grid a
  `tblGridChange`. **Reject All keeps the new column.**
- Delete column (`Column.Delete`) and merge two cells (`Cell.Merge`): **not tracked at all**, 0 revisions; the
  column is gone, the merged cell has `w:gridSpan="2"` and both paragraphs.
- Bonus, rows: insert and delete rows are tracked with `w:trPr/w:ins` or `w:trPr/w:del`, plus the paragraph marks and
  `w:delText`.
- No `w:cellIns`, `w:cellDel` or `w:cellMerge` anywhere.

**W9. Comments inside revisions.** **The comment goes with its anchor.** Reference run inside a `w:ins`: **Reject
All removes the comment**, Accept All keeps it. Inside a `w:del`: **Accept All removes it**, Reject All keeps it
(`w9-hand-anchor-and-ref-inside`). The reference run alone is enough: it holds even when the comment's range covers
untracked text that survives (`w9-hand-ref-only-inside`). When Word itself comments on revised text it puts the
range markers and the reference run **outside**, around the `w:ins`/`w:del` (`w9-word-made`); the comment is still
removed when all of its range is (inserted text on Reject All, deleted text on Accept All). Calls: `Comments.Add` on
the revision's range, `Revisions.AcceptAll` / `RejectAll`.

**W10. Comment, reply, resolve.** Word's AppleScript has no `Replies`, `Done` or `Resolve` (a comment exposes only
author, initials, scope, text and date), so only the root comment is Word's own; the reply and the resolved state
were written by hand and round-tripped through Word (open, add a comment, save). What Word writes:
- `commentsIds.xml`: `w16cid:paraId` (the comment's paragraph `w14:paraId`) to `w16cid:durableId` (8 hex digits).
- `commentsExtensible.xml`: keyed by **`durableId`**, `w16cex:dateUtc` = **true UTC**; `comments.xml` `w:date` =
  local time with `Z`.
- `commentsExtended.xml`: keyed by **`paraId`**, `w15:done`, replies add `w15:paraIdParent`.
- `people.xml`: `w15:person w15:author="Hadrien Mary"` with `w15:presenceInfo w15:providerId="Windows Live"
  w15:userId="<16-hex CID>"`.
- **`w15:done`: input `done="1"` on the root only; Word saves `done="1"` on the root and on the reply** (the whole
  thread).
- Reply anchors (written by hand, kept as is by Word): the reply's `commentRangeStart` right after the root's, over
  the same text, each `commentRangeEnd` followed by its own reference run, root first.
- Not observed: what Word writes when a person clicks Reply or Resolve; that needs the UI.

**W11. Reply to a reply.** Input: reply 2 with `w15:paraIdParent` = reply 1. **Word rewrites it to the root's
`paraId` on save**: threads are flat. Replying from the UI was not possible by script.

**W12. Find and Replace, last paragraph.** Replace All "30 days" to "45 days" with tracking: **the whole match is
deleted and the whole replacement inserted** (`w:del "30 days"` + `w:ins "45 days"`), no trimming to "30"/"45".
Deleting the last paragraph: selecting it (with the final mark) and Backspace marks only its text deleted, **no
paragraph mark**; selecting from before P2's mark to the end marks **P2's mark deleted** (`w:pPr/w:rPr/w:del` on the
previous paragraph) and the last paragraph's text; the final mark is never marked. Calls: `Find.Execute
Replace:=wdReplaceAll`, `Selection.TypeBackspace`.

**W13. TOC update with tracking on.** **Tracked**, as a whole: Word inserts the full new list of entries (every
entry, unchanged ones too: `w:ins` on runs and paragraph marks) and deletes the full old list (`w:del`), no
per-entry diff; the field's begin / instruction / separate / end are not tracked, and new `_Toc` bookmarks pile up.
TOC from `TablesOfContents.Add` (instruction ` TOC \o "1-3" \n \p " " `), heading added with tracking on, then
`TableOfContents.Update`. The field end sits in the "Intro" heading paragraph because of where the script inserted
the TOC, not because of tracking.
