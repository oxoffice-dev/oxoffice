---
date: 2026-11-27
status: accepted
scope: docx
---

# Writing revisions and comments

## Context

#469 (ROADMAP item 6) asks for four things. First, author tracked changes, not only read, accept and reject them:
insertions, deletions, property changes and moves, with an author and a date the caller sets. Second, complete
comments with replies, resolution and modern comment metadata. Third, comparison output as real revisions. Related
closed issues: #443, #448 (part A'), #460. Surveyed on main 211ba592: the facade, the CLI, the binding, and probes with
the CLI on hand-made files and corpus files (probe scripts not committed).

### What exists, what is missing

| Capability | State | Where |
| --- | --- | --- |
| Read the revisions of every story (kind, author, timestamp, text) | exists | `Document::story_revisions` (`revision.rs`), Python `Document.revisions`, CLI `revision list` |
| Accept or reject all, by author, by date range, by id | exists | `revision.rs:424-473`, CLI `revision accept/reject` |
| The `w:trackRevisions` setting | exists | `settings.rs:580`, Python `Settings.track_revisions` |
| Read and resolve `w:trPrChange`, `w:tcPrChange`, `w:tblPrExChange`, `w:tblGridChange`, `w:numberingChange`, `w:cellIns`, `w:cellDel`, `w:cellMerge` | **missing**: not listed, and `accept_all`/`reject_all` leave them in place (probe F1) | `RevisionKind` has 8 kinds (`oxoffice-docx-oxml/src/revision.rs:208`); `modeled_child`, `revision_metadata` (`revision.rs:4575`, `4780`) |
| Resolve a move Word wrote (range marks, ids linked by `w:name`) | **partial**: by id, only the part with that id resolves; resolving everything leaves the four range marks, with their author and date (probe F2) | `revision.rs` |
| Author insertions, deletions, property changes, moves | **missing** | |
| An author and a date for edits | **missing**; compare takes them (author, RFC 3339 timestamp, both required) | `Document::compare` |
| A revision id pool | **missing** in the registry, which holds bookmark and comment ids (`document/identifiers.rs`); compare allocates its own, avoiding every `w:id` of both inputs | `comparison.rs:854`, `IdAllocator` |
| Comments: add (run range, story range, on text), reply, move, resolve/reopen, remove; read author, initials, date, parent, resolved, anchor | exists | `comments.rs`, CLI `comment ...`, Python |
| `commentsExtended` (`w15:paraIdParent`, `w15:done`) | exists, written on add, reply and resolve; `w15:done` is left out on new entries (Word writes `"0"`) | `comments.rs:2553`, `2705` |
| `commentsIds` (`w16cid:durableId`) | **partial**: read and checked, never written. A new comment gets no entry, even where the part exists (Word copies of record, report and contract; Google comments, reply, report and contract) | `comments.rs:4739` |
| `commentsExtensible` (`w16cex:dateUtc`) | **partial**: read for linkage, never written | same |
| `people.xml` (`w15:person`) | **missing**: neither read nor written; no corpus file holds one | |
| Comment paragraph ids | **defect**: `allocate_para_id_from_occupied` takes the highest id plus one, with no bound, and looks only at the comment parts. MS-DOCX requires 0 < `w14:paraId` < 0x80000000, unique within the part | `comments.rs:3649` |
| Compare: ins/del (run, word, character), `rPrChange`, `pPrChange`, `tblPrChange`, `sectPrChange`, paragraph marks, fields and links as Word tracks them, notes, block controls | exists | `comparison/` |
| Compare: row and cell property changes | **missing**: dropped, with the diagnostic "formatting differs and the original formatting was retained" (probe F4) | `comparison/tables.rs:489`, `629` |
| Compare: moves | **partial**: written without range marks, and one id shared by `moveFrom`, `moveTo` and both mark revisions (probe F3) | `comparison/move_pairs.rs:7` |
| Compare: comments | **partial**: changed comments go whole into a private part, `/customXml/rdocxComparisonComments.xml` (`urn:rdocx:comparison:comments:1`), that only oxoffice resolves. A comment added on unchanged text replaces its paragraph whole (probe F5) | `comparison/comments.rs` |

### Probes and evidence (main 211ba592, debug CLI)

- **F1.** A table whose row holds `w:trPrChange` and whose cell holds `w:tcPrChange`: `revision list` is empty, and
  accept and reject both resolve 0 and keep the marks. The same holds for `w:numberingChange` in `w:numPr` and for
  `w:cellIns`. Google Docs writes an id-only `w:tblGridChange` in every table: 15 corpus files under `google/`,
  `google-docs/` and `word-google/`, every Google copy that holds a table. Word writes `w:trPrChange` and `w:tcPrChange` for tracked row and cell formatting
  (to confirm, W8).
- **F2.** A move written as Word writes it: `revision accept --id <moveFrom id>` resolves 1 and leaves the
  `moveTo` pending. Accept All and Reject All leave `moveFromRangeStart/End` and `moveToRangeStart/End`.
- **F3.** Compare of "A B C" against "A C B" writes `<w:moveFrom w:id="0">` and `<w:moveTo w:id="0">`, with mark
  revisions also `w:id="0"`, and no range marks. ECMA-376 Part 1, `moveTo`: content outside a complete move container
  with a matching source "shall be treated as inserted, rather than moved". Word then most likely shows a plain
  insertion and deletion (W6).
- **F4.** A row gaining `w:cantSplit` and a cell gaining shading: the output keeps the original formatting, with
  two diagnostics.
- **F5.** `word/letter.docx` with one comment added on unchanged text: compare writes 8 deletions and 12 insertions
  (the paragraph replaced whole), at run and at word granularity, plus the private part.
- **Word's own writing** (`word/record.docx`, a python-docx source saved by Word 16):
  - One id sequence for bookmarks, comments and revisions, in document order (0 to 20), the paragraph mark's
    revision before its content's.
  - The deletion before the insertion that replaces it.
  - It added `commentsIds.xml` with `durableId` = `paraId`, and `w15:done="0"` or `"1"` on every entry.
  - No `people.xml` and no `commentsExtensible.xml` for comments it did not author.
- **Word's tracked links** (Word check of #541, `handoff/pcwe/word-checks/hyperlink-tracked.md`): a link inserted or
  deleted whole is a `HYPERLINK` complex field inside the `w:ins` or `w:del`, never a `w:hyperlink`.
- **Edit routes** (`paragraph_is_plain`, `document.rs:1161`; `plain_picture_free_paragraph`, `main_part.rs:2592`):
  the fast routes take only paragraphs with no revision. After a first tracked edit, a later edit of that paragraph
  would take the route that grows with the body.

## Decision

### 1. A tracked session, not a tracked twin of every call

Recommendation: a session. A twin per call would double three APIs, and most Python calls are setters
(`run.bold = True`, `paragraph.text = ...`) that cannot take an author. Word's Track Changes is a mode, with the user
as author. And one mechanism per owner kind covers whole families of calls at once (section 3).

**Rust.** The session borrows the document:

```rust
let changes = TrackedChanges::new("Ada", RevisionDate::At("2026-11-27T10:00:00Z"))?;
let mut tracked = document.track_changes(&changes);          // TrackedDocument<'_>, holds &mut Document
tracked.edit_paragraph(&location, |p| p.set_text("Six warehouses"))?;
tracked.try_replace_all_checked(&pairs, options)?;
let ids: &[i32] = tracked.revision_ids();                    // the ids this session wrote, in order
```

- **No edit escapes tracking, and the compiler checks it.** While a `TrackedDocument` lives, no direct handle
  (`paragraph_mut`, `last_paragraph_mut`, `table_mut`, `section_mut`, `section_properties_mut`) and no call the
  session does not offer can run. Rust needs no runtime guard.
- **Names.** Its methods take the names and arguments of the `Document` calls they track. Adders return locations,
  not `&mut` handles, which could edit without marks.
- **`RevisionDate`:** `Now` (UTC to the second, as `CommentDate::Now`), `At` (RFC 3339, checked as compare checks
  it) or `Undated` (no `w:date`, as Word leaves revisions after its Document Inspector).
- **Author:** not empty, XML 1.0 characters only, as for comments.

**Python.**

```python
with doc.track_changes("Ada", timestamp="2026-11-27T10:00:00Z") as tracked:
    doc.paragraphs[1].runs[0].text = "six"
    doc.replace_all([("30 days", "45 days")])
tracked.revision_ids          # (12, 13, 14, 15)
```

- `Document.track_changes(author, *, timestamp=None, undated=False) -> TrackedChanges` starts a session. It also
  works without `with`: `tracked.stop()` ends it.
- `Document.tracked_changes` is the active session or `None`. Starting a second one raises `DocxError`.
- The binding keeps the author and the date. It runs each tracked call through one Rust session.
- Handles: an edit that redlines a paragraph retires that paragraph's run handles, as `Paragraph.text` does today
  (`test_docx_paragraph_edits_in_place.py`). So does `Run.text` under a session, since the run splits.

**CLI.** `oxoffice docx replace ... --track-author NAME [--track-timestamp RFC3339 | --track-undated]` writes the
replacements as tracked changes, and `--json` adds `revision_ids`. No other command takes the flag in this series:
`fill`, `fields update`, `toc rebuild`, `bibliography` and `meta set` stay as they are, and Word does not track
comments.

**Independent of `w:trackRevisions`.** Starting a session does not set the setting, and the setting does not start a
session (Q3).

### 2. What a session tracks, runs untracked, or refuses

One table in the binding, keyed by the call names of the scaling registry (`TIMED`), puts every mutating call in one
of three classes. A test checks that the table lists every call.

- **Tracked:** the calls of each pull request (Plan). The redliner can still refuse one effect it cannot mark (see
  section 3), with the plain message below.
- **Untracked, as in Word**, which records no revision for them:
  - style definitions and defaults;
  - numbering definitions and instances;
  - settings, the core, custom and app properties, fonts;
  - comments (add, reply, move, resolve, remove), bookmarks, permission ranges, protection;
  - custom XML stores, bibliography sources and style, the mail merge package, signing;
  - accept and reject, which Word lets the user run while tracking.

  These run as they do without a session.
- **Refused:** every other edit until a pull request tracks it. The call raises `DocxError` "<call> is not recorded
  as a tracked change yet: end the tracked changes to make it", and the document stays unchanged. No limitation file:
  only a caller's own session can reach it (Known limitations rule). The refusals left after PR 5:
  - content control edits;
  - field, TOC and bibliography updates and insertions (Word tracks a TOC update whole, W13);
  - section and header or footer structure, watermarks, page colour;
  - picture replacement and resizing, equations;
  - table column insertion and deletion, cell merge and split (W8);
  - form fill, reset and protect;
  - compare into a session document.

  Each one can be another issue once a user needs it.

### 3. A tracked edit: the untracked edit, then a redline of the owner it touched

A tracked call runs the untracked edit on its owner, on the same route as today. A redliner then compares the owner
before and after the edit and writes the marks into the edited owner. An owner-scoped postcondition follows: resolve
the owner both ways and check I1 and I2 of section 5 on it. It costs that owner, runs in every build, and refuses the
edit (document unchanged) on a mismatch.

Owners:

- a paragraph: any story, a cell, a block control;
- table properties and grid, row properties, cell properties;
- section properties;
- a span of sibling blocks (a splice);
- a move (its source span and its destination).

**Why not the comparison engine.** It works on the whole document: it scans every story of both inputs for ids, and
its postconditions accept and reject the whole document, so a tracked call would grow with the body. It also refuses
any change to an owner already holding revisions, which is exactly what a second tracked edit of the same paragraph
is.

**What it reuses** (moved to a shared module where needed):

- the word tokenizer and alignment (`comparison/granular_units.rs`, `alignment::align`);
- the conversions Word's tracking needs (`field_conversions`: a link inserted or deleted whole as a `HYPERLINK`
  field, a simple field as a complex one, spec of 2026-11-21);
- the row and table markers (`tables::marked_row`, `marked_table`).

**The paragraph text redline.**

1. Flatten the paragraph before the edit into units, in document order:
   - words and separators, at the comparison's word granularity (Q2);
   - atomic run items: tab, break, symbol, drawing, note reference, field character;
   - range markers (comment, bookmark, permission), which are never marked.

   Each unit carries its run properties, its containers (link, inline control, smart tag, field) and its revision
   state: plain, inserted or deleted (with id and author), moved.
2. Flatten the paragraph after the edit the same way. The edit ran on the accepted view, so it never saw the deleted
   units.
3. Align the accepted units before the edit with the units after it, on text and run properties.
4. The output is the full unit sequence before the edit, changed as follows:
   - A kept unit keeps its state.
   - A removed plain unit goes into a new deletion. One inserted by this session is dropped, as undoing typing. One
     inserted by another author gets a deletion nested in that insertion (R4; the resolver already resolves nested
     deletions by id, probed).
   - New units go into a new insertion, after any deleted units of the same gap (deletion first). An insertion they
     fall inside is split (R1, R2).
   - Deleted units of other revisions stay where they were. That keeps the original order, so Reject restores it.
5. Units with the same state and run properties make one run. Each contiguous span of a state is one revision with
   its own id.

**Refused by effect** (PR 1; later pull requests narrow the list):

- a container made or removed around kept text, such as a link made on existing words or a control wrapped around
  them;
- a field partly removed;
- a whole link or field inserted or deleted (PR 3, with the field conversion);
- a property change (PR 2).

**The replace calls.** The match engine runs untracked on its staged candidate, as today. Each paragraph it changed
then goes through the redliner with that paragraph as it was before. The engine already knows the paragraphs it
rewrites.

### 4. The OOXML written

**Per change kind:**

| Change | Markup |
| --- | --- |
| Text inserted | `<w:ins w:id w:author w:date><w:r>{rPr}<w:t>..</w:t></w:r></w:ins>`. The new run takes the properties of the run before the insertion point; a replacement takes those of the first replaced run, as replace does today |
| Text deleted | `<w:del ..><w:r>{rPr}<w:delText xml:space="preserve">..</w:delText></w:r></w:del>`. `w:instrText` becomes `w:delInstrText`; tabs, breaks, symbols, drawings, note references and field characters stay as they are; a split run keeps its properties and rsid attributes on both halves |
| Inside a link or inline control | the `w:ins`/`w:del` goes inside the container (`w:hyperlink/w:ins/w:r`, `w:sdtContent/w:del/w:r`), which the schema allows |
| A link or field inserted or deleted whole | a complex field whose characters, code and result all sit in one `w:ins` or `w:del`, as Word writes it (#541 Word check); PR 3 |
| Paragraph inserted | its mark `w:pPr/w:rPr/w:ins` (the `w:ins` first in `w:rPr`, schema order) and its content in `w:ins`. Appended after a story's last paragraph: the previous paragraph's mark is inserted, and the new paragraph takes the story's last mark, as compare writes it |
| Paragraph deleted | content in `w:del`, mark `w:pPr/w:rPr/w:del` (it merges with the next paragraph on accept). A story's last mark is never marked: deleting the last paragraph marks the mark before it (spec of 2026-11-26, rule A) |
| Run properties | `w:rPr/w:rPrChange/w:rPr{old}`, the last child. "old" is the direct properties before the session's first change of that run, without `w:ins`, `w:del` or `w:rPrChange`. A paragraph mark's properties: the same inside `w:pPr/w:rPr` |
| Paragraph properties | `w:pPr/w:pPrChange/w:pPr{old}`, the last child; "old" leaves out `w:rPr`, `w:sectPr` and `w:pPrChange`. A style or numbering change is a `pPrChange` holding the old `w:pStyle` or `w:numPr` (Word writes no `w:numberingChange` for it; W5) |
| Table properties | `w:tblPr/w:tblPrChange/w:tblPr{old}`. Changed column widths add `w:tblGrid/w:tblGridChange w:id/w:tblGrid{old}`, which has an id and no author: it resolves with its table's `tblPrChange`, or alone by id or when everything is resolved (Google's id-only ones) |
| Row, cell | a row height change has no `w:trPrChange` in Word: it writes `w:tblPrChange` and `w:tblGridChange` (old = new) and a `w:tcPrChange` on each cell of the row (W8); PR 2 writes those and a `w:trPrChange` too, which a rejection undoes (Word's Reject All brings the row back, W8); `w:tcPr/w:tcPrChange/w:tcPr{old}`, the last child, for a cell's own change |
| Row inserted or deleted | `w:trPr/w:ins` or `w:trPr/w:del`, with every cell paragraph's content and mark marked, as `marked_row` writes them |
| Table inserted or deleted | every row marked, as `marked_table` writes it |
| Section | `w:sectPr/w:sectPrChange/w:sectPr{old}`, the last child |
| Move | see below |

**Moves**, written in Word's shape, which links the two sides by name. Compare will write them the same way (PR 4).

```xml
<w:p><w:pPr><w:rPr><w:moveFrom w:id="12" w:author="Ada" w:date=".."/></w:rPr></w:pPr>
  <w:moveFromRangeStart w:id="10" w:author="Ada" w:date=".." w:name="move1"/>
  <w:moveFrom w:id="11" w:author="Ada" w:date=".."><w:r><w:t>Beta.</w:t></w:r></w:moveFrom>
  <w:moveFromRangeEnd w:id="10"/></w:p>
...
<w:p><w:pPr><w:rPr><w:moveTo w:id="15" ../></w:rPr></w:pPr>
  <w:moveToRangeStart w:id="13" .. w:name="move1"/>
  <w:moveTo w:id="14" ..><w:r><w:t>Beta.</w:t></w:r></w:moveTo>
  <w:moveToRangeEnd w:id="13"/></w:p>
```

- Moved-from text keeps `w:t`, not `w:delText`.
- The name is `move` followed by a number no other range name in the document uses.
- The range starts inside the first moved paragraph, after its `w:pPr`, and ends at body level after the last moved
  paragraph, so it covers every moved paragraph mark; each paragraph's mark carries `w:moveFrom` or `w:moveTo`, and
  every element has its own id (W7). Without range marks covering the marks, Word reads a deletion and an insertion
  (W6).
- The resolver resolves a move as one change: the id of any of its parts selects the whole named move, and resolving
  it removes its range marks (fixes F2).
- A move of content that already holds revisions is refused in this series.

**Ids.**

- One pool for every revision-family element: the content and mark revisions, every `*PrChange`, `tblGridChange`,
  `numberingChange`, `cellIns`/`cellDel`/`cellMerge`, and the move range marks.
- Ids are unique across every story part, and never equal to a comment or bookmark id. Compare's allocator does the
  same, and Word writes one sequence for all three (record.docx).
- The registry collects these ids once, at open, in the scan it already makes (`DocumentIdentifiers`). A new id is
  the lowest free one, and reserving it goes in the reservation journal, so a refused edit gives it back. No edit
  scans the document.

**Author, date, rsid.**

- The author is written as given. `w:date` is the RFC 3339 value as given; `Now` writes UTC with `Z`, and `Undated`
  leaves the attribute out.
- No `w:rsidR`, `w:rsidDel` or `w:rsidRPr` is written, as compare writes none; Word assigns its own on its next save.
  The `w:rsids` of the settings stay as they are.

**Edits that meet existing revisions.** Checked in Word (W1-W4, see Word checks: results):

- **R1. Insert inside the author's insertion of an earlier session:** a new insertion, splitting the old one, as
  Word does (W1). Only this session's own insertions take new text into themselves.
- **R2. Insert inside another author's insertion:** split it, with the new insertion in between.
- **R3. Delete text this session inserted:** drop it. Delete text the same author inserted earlier: drop it too, no
  `w:del`, as Word does (W1).
- **R4. Delete text another author inserted:** `<w:ins A><w:del B><w:r><w:delText>`.
- **R5.** Deleted text is out of the accepted view, so no edit can reach it. A deletion across another's deletion
  marks only the visible units.
- **R6. Change the properties of inserted text:** an `rPrChange` inside the insertion, even the author's own, as
  Word does (W3).
- **R7. A second property change of the same owner:** keep the one `*PrChange` with the oldest "old" properties, and
  give it the latest author and date, as Word does (W4).

### 5. Interplay

**Round trips.** These hold for every tracked edit T of a document D, with U its untracked twin:

- **I1.** `accept_all(T(D)) == accept_all(U(D))`, range marks aside: the redline leaves a bookmark or comment
  range mark where it stood, between the words around it, where the untracked call may move it (Q12).
- **I2.** `reject_all(T(D)) == reject_all(D)`.
- **I3.** For D without revisions: `accept_all(T(D)) == U(D)` and `reject_all(T(D)) == D`.
- **I4.** `reject_revision_ids(session ids)` gives D back, unless the edit dropped text the same author had inserted
  in an earlier session (R3).

"==" is what the comparison's postconditions use to compare a resolved candidate with its input
(`comparison/postconditions.rs`, `normalized_package`), plus the accepted text of every story.

**Compare.** For edits on paragraphs without revisions, `story_revisions` of T(D) (kind and text, in order) equals
that of `compare(D, U(D), word)`. Where compare refuses (an owner holding revisions), only I1 and I2 apply.

**Readers.**

- `revisions`, `story_revisions` and the CLI `revision list` list the new revisions.
- Text views, exports and `text --json` read the accepted view, so they read the untracked result.
- The tracked render view (`revision_view="tracked"`) shows the marks.
- `validate` reports no new error, including its tracked-change id and author check.

**Cost (the `PERF` gate).**

- A tracked call keeps its route and adds the cost of its owner: the redliner, the id reservation and the
  owner-scoped postcondition. Nothing scans the body.
- PR 1 lets `paragraph_is_plain` and `plain_picture_free_paragraph` take paragraphs holding the wrappers the redliner
  writes: `w:ins` and `w:del` around plain runs, mark revisions, and later `rPrChange` and `pPrChange`. Otherwise a
  second edit of a redlined paragraph would take the route that grows with the body.
- `test_docx_scaling.py` gets a `"<Call> (tracked)"` entry for every tracked call (the variant syntax `TIMED`
  already reads), timed on a target that already holds one of the session's changes.
- A tracked entry is in `GROWING` only if its untracked call is (`Run.text`, `Paragraph.replace_text`,
  `Document.replace_text_at` are). No new `GROWING` entry.
- `test_every_mutating_call_is_timed` also checks that every tracked call has its twin.
- `QUICK` gets `Paragraph.text (tracked)` (PR 1), `Run.bold (tracked)` (PR 2) and
  `Document.insert_paragraph (tracked)` (PR 3).

**#660 (`WRD`, in flight).** PR 1 and PR 2 stay off `owner_snapshots.rs` and the `body_tables` rebuild. PR 3, which
covers rows of tables inside block controls, starts after the #660 series is merged.

### 6. Tests (lean)

- **Rust:** one module, `crates/oxoffice-docx/tests/docx/tracked_changes.rs`, with one table. A row holds a
  document builder, the tracked edit, its untracked twin, and an expected markup snippet or none. One driver checks
  I1 to I4, `validate`, a save and reopen, and, on rows without existing revisions, the compare equivalence. Markup
  snippets: one per change kind, nothing more.

  Rows per pull request:
  - PR 1, about 12: a word inserted mid-run; a word deleted; a replacement across two runs; inside a link; inside an
    inline control; `add_tab`; a second edit of the session's own insertion; inside another author's insertion,
    inserting and deleting; across another author's deletion; a header paragraph; a footnote paragraph; a cell
    paragraph.
  - PR 2, about 8; PR 3, about 8; PR 4, about 5.
  - PR 5: rows in `comments.rs`, and compare rows in `revisions_and_compare.rs`.

  Each row fails on main (it has no API there) or proves a refusal. Resolver fixes (F1, F2) get rows in
  `revisions_and_compare.rs`: one table of kinds accepted and rejected, plus the corpus walk on the Google copies
  (`tblGridChange`).
- **Python:** one file, `crates/oxoffice-py/tests/test_docx_tracked_changes.py`, for the binding layer only:
  - one test parametrized over the `TIMED` recipes under a session: a tracked call round-trips its text through
    `accept_all` and `reject_all`, an untracked one runs, and a refused one raises with `to_bytes()` unchanged;
  - the session object (context manager, `stop`, `revision_ids`, a second start refused);
  - run handles retired as by `Paragraph.text`.
- **CLI:** one case in `crates/oxoffice-cli/tests/docx_replace_reach.rs` (`--track-author`, then `revision list
  --json`).
- **Compare walks** (PR 2, PR 4, PR 5): `OXOFFICE_WORKFLOWS=full` locally, and the result in the pull request.

## Plan

Five pull requests, each about 800 product lines or fewer, and each useful on its own on all three surfaces.

| PR | Title | What it lands | Lines | Issue |
| --- | --- | --- | --- | --- |
| 1 | `feat(docx): tracked text edits` | the session (Rust `TrackedDocument`, Python `track_changes`, CLI `replace --track-*`); the revision id pool; the paragraph text redliner in every story, R1-R5, and the owner postcondition; tracked: `Run.text`, `Run.remove`, `Run.add_tab`, `Run.add_break`, `Paragraph.text`, `Paragraph.add_run`, `Paragraph.replace_text`, `Cell.text`, `Cell.replace_text`, `Document.set_story_text`, the replace family (`try_replace_text`, `replace_text_at`, `replace_all`, `replace_all_regex(_expected)`), `Paragraph.replace_xml` within what the redliner marks; the class table and its refusal; the fast-route predicates | ~800 | Refs #469 |
| 2 | `feat(docx)!: tracked property changes` | `rPrChange` and `pPrChange` through the paragraph route (`Font.*`, `Run.bold/italic/underline/style_id`, `ParagraphFormat.*`, `Paragraph.alignment/style/numbering`); `tblPrChange` with `tblGridChange` and `tcPrChange` through the table, row and cell property routes (`Table.*`, `Row.*`, cell setters), a row's height as Word writes it (W8); `sectPrChange` through `SectionHandle` setters; R6-R7. Reads and resolves `trPrChange`, `tcPrChange`, `tblPrExChange`, `tblGridChange` and `numberingChange` (F1): new `RevisionKind` variants and `#[non_exhaustive]`, hence the `!`. Compare writes `trPrChange` and `tcPrChange` instead of the diagnostic (F4) | ~750 | Refs #469 |
| 3 | `feat(docx): tracked insertions and deletions of blocks and rows` | splices in any story: `add_paragraph`, `insert_paragraph`, `insert_paragraph_before`, `add_heading`, the list item adders, `add_table`, `insert_table`, `remove_content(_range)`, `pop_content`, `insert_content`, `clone_content`, `insert_html`, `insert_document`, `import_fragment`; rows (`Table.add_row`, `clone_row`, `remove_row`); inline insertions that bring a part (`Run.add_picture`, `Paragraph.add_hyperlink` as a `HYPERLINK` field, `Run.add_field`, footnote and endnote references with their notes marked inserted); last-mark rules (the final mark is never marked, W12); `cellIns`/`cellDel`/`cellMerge` resolved when read (F1), never written: column insertion and deletion and cell merges are refused under a session, as Word does not track them (W8) | ~800 | Refs #469 |
| 4 | `feat(docx): tracked moves` | `move_content` in Word's shape (W7); the resolver links a move by name and removes its range marks (F2); compare writes its moves the same way, which Word then reads as moves (F3, W6) | ~600 | Refs #469 |
| 5 | `feat(docx): modern comment metadata` | `commentsIds` and `commentsExtensible` entries for new comments and replies (parts made on the first one, `dateUtc` = the comment's instant); `w15:done="0"` written, `done` set on the whole thread (W10); a reply to a reply linked to the root (W11); ids within MS-DOCX bounds and unique per part; `people.xml` per Q8; `w16du:dateUtc` on revisions, as Word writes it beside its local `w:date`; `Comment.durable_id` and `Comment.date_utc` read (Rust, Python, `comment list --json`); compare writes an added or removed comment as its reference run in `w:ins` or `w:del`, with no whole-paragraph replacement, and keeps its private record only for what no revision can hold (Q6) | ~650 | Closes #469 |

- **Order.** 1, 2, 3, 4. PR 5 is independent of the edit path and can run beside PR 2 to PR 4, but after PR 4
  where both touch `comparison.rs`.
- **Each pull request:** a spec amendment if a Word check changes a rule, `docs/ARCHITECTURE.md` (the revision
  writer), the README feature rows, and the skills' docx reference after the release.
- **Word checks:** done (see Word checks: results); a PR whose rule they changed says so.

## Amendment: PR 1 in two (2026-11-27, during PR 1)

The maintainer accepted the spec and all recommendations Q1-Q11. PR 1 passes 900 product lines, so it lands in
three, with the orchestrator's agreement:

- **PR 1a** `feat(docx): tracked paragraph edits in the rust facade` (branch `feat/tracked-paragraph-edits`): the
  session (`TrackedChanges`, `Document::track_changes`, `TrackedDocument`), `RevisionDate` with `CommentDate` as its
  alias, the revision id pool, the redliner with its owner-scoped check, and the paragraph routes.
- **PR 1b** `feat(docx): tracked replacements in the rust facade and the cli` (branch `feat/tracked-replacements`,
  stacked on 1a): the replace family, notes and headers, and `replace --track-author` in the CLI.
- **PR 1c** `feat(docx): tracked text edits in python`: `Document.track_changes`, the class table with its
  completeness test, the guard (a gate in the handle counter's publication, with an allow-list; accepted), the
  fast-route predicates (their effect is only measurable through the Python scaling entries), and the
  `"<Call> (tracked)"` `TIMED` entries. It starts after the binding changes of #660 land.

What PR 1a settles:

- **Alignment key.** A word is the same before and after an edit when its text and its holders (links, controls)
  are. Until PR 2b the key also held its run properties, so that a word the edit kept but gave other formatting was
  written as deleted and inserted; since 2b it takes a property change, as compare writes it.
- **Session date.** `RevisionDate::Now` is the time the session starts: every revision of a session takes it.
- **Split insertions.** Text inserted inside another author's insertion splits it; each later part takes an id of its
  own, as Word writes them (W1, W2): one id per revision element.
- **Refused in PR 1a**, each with the plain message "<what> is not recorded as a tracked change yet":
  - a replacement in a text box, and in a header or footer table or control;
  - `try_replace_text_at` on a table or block control (a paragraph is tracked);
  - an edit that adds, removes or reorders a bookmark or comment mark, or changes paragraph properties;
  - the removal or insertion of a drawing, a field or a reference, and text put in a new link or control.
- **Notes** are tracked through their part's XML, paragraph by paragraph, as replacement edits them.
- **Replacements** (W12): each whole match is deleted and the whole value inserted, read on the paragraph's text
  character by character. A match the replacement could not reach stays as it was, and so does one whose value is its
  own text. Where the matches found there do not explain the paragraph after the replacement, the paragraph is
  aligned word by word instead, as other edits are.

## Amendment: PR 1c in Python (2026-11-27, during PR 1c)

- **Classes.** `test_docx_tracked_changes.py` holds the table: the calls a session tracks (`Run.text`, `Run.remove`,
  `Run.add_tab`, `Run.add_break`, `Paragraph.text`, `Paragraph.add_run`, `Paragraph.replace_text`,
  `Paragraph.replace_xml`, `Cell.text`, `Cell.replace_text`, `Document.set_story_text`, `Document.replace_text_at`
  and the four whole-document replace calls) and those it runs untracked (section 2). Every other mutating call of
  the scaling registry is refused, so a call added to the API is refused until the table names it. The test runs
  each call under a session: a tracked one is checked against its untracked twin, accepted, and against the
  document, rejected.
- **The guard.** The gate in the handle counter's publication refuses any publication its allow-list does not name.
  A route that publishes nothing refuses itself under a session: the table, row and cell property routes, the
  section property routes, the story part routes in place (headers, footers, watermarks, checked first), and the
  document-level calls of section 2's refused list (content controls, form fill and reset, pictures, page colour,
  numbering restarts, chart legends, link targets, field, TOC and bibliography updates, compare).
- **Routes.** Under a session the facade's own routes write tracked changes: `set_story_text` and
  `try_replace_text_at` (their staged and owned variants included) take the paragraph route; `try_set_cell_text`
  writes the cell's one paragraph as tracked changes and refuses a cell of more paragraphs, whose others it would
  remove (PR 3); a whole-cell replacement replaces each paragraph and refuses a nested table; a table outside the
  body (a header, footer or note table) refuses the `Cell` calls, whose route edits the whole table, while its
  paragraphs' own calls are tracked; the replace calls refuse a match in a header or footer table and track one in a
  note's table; `replace_paragraph_xml` writes the new paragraph as tracked changes of the old one.
- **Content-free edits.** An edit that changes no character, its formatting or its holders, such as a run split, is
  kept as it is: Word records nothing for it.
- **`revision_ids`** in Python reads the document's parts: the session's ids the document holds. An edit refused after
  its revisions took ids, on a dropped copy or in place before the paragraph was put back, leaves none.

## Amendment: PR 2 in three (2026-11-27, before PR 2)

PR 2 passes 900 product lines, so it lands in three: **2a** reads and resolves every property change, **2b** writes
run and paragraph property changes, **2c** table and section property changes and compare's (F4).

What PR 2a settles:

- **Kinds.** `RevisionKind` is `#[non_exhaustive]` and gains `RowPropertyChange` (`w:trPrChange`),
  `CellPropertyChange` (`w:tcPrChange`), `TablePropertyExceptionChange` (`w:tblPrExChange`), `TableGridChange`
  (`w:tblGridChange`), `NumberingChange` (`w:numberingChange`), `CellInsertion` (`w:cellIns`) and `CellDeletion`
  (`w:cellDel`), named `row_property_change` ... `cell_deletion` in the CLI and Python.
- **Resolution.** A property change of a row, a cell, a row's exceptions or a grid resolves as the other property
  changes: accepted, it goes; rejected, its old value replaces its owner. A cell insertion rejected, or a cell deletion
  accepted, removes the cell, and the row with its only cell; the other way, only the mark goes. A numbering change goes both ways: its old value is
  the number as it read (`w:original`), not a numbering to restore. `w:cellMerge` stays unread.
- **Grid changes with an id alone** (Google Docs, in every table; Word too) read with no author and resolve with their
  table's property change selected by its id, its author or its date, without counting as a change of their own; by
  their own id; or with every change. Not with the ids comparison names, which are exactly the changes it means.
  One that records no old grid keeps the grid. Compare leaves them as it leaves other markup: they hold no one's
  change.

What PR 2b settles:

- **Alignment** reads a word's text and its holders, not its run properties: a word kept with other properties stays
  where it is, in a copy of its run with the new properties and a `w:rPrChange` of the old ones, one per run element
  written, inside an insertion too (W3). The old properties are those an earlier change holds, if any, else the run's
  before the edit; the change takes the session's author and date (W4).
- **Paragraph properties**: one `w:pPrChange`, last in `w:pPr`, old value as for runs; a style or numbering change is
  one too (W5). A change of the paragraph mark's run properties or of its section is refused.

What PR 2c settles:

- **Tables.** A table, row or cell property edit takes its route in place and writes a `w:tblPrChange`, a
  `w:tblGridChange` (an id alone, as Word writes it) and a `w:tcPrChange` for each owner that changed. A row's own
  change takes a `w:trPrChange`, which Word reads and a rejection undoes, as Word's Reject All brings the row back
  (W8), beside the markers Word writes for it: its table, grid and each of its cells take a change holding what they
  hold now. A row or cell edit is redlined on its row alone, so its cost does not grow with the table. Cell merges
  and column insertions or deletions stay refused (W8). A table outside the body, or one a route in place does not
  take, is refused.
- **Sections.** A section edit writes one `w:sectPrChange`, last, whose old value leaves out the header and footer
  references; rejecting it keeps them, which the resolver did not do before.
- **Compare (F4).** A cell's changed properties are written as a `w:tcPrChange`, a row's as a `w:trPrChange`. A row
  that holds a revision of its own (an insertion, a deletion, a move or an earlier change) keeps its properties with
  a diagnostic.

## Open questions

### Product (maintainer)

- **Q1. Session or twins.** Recommendation: the session (section 1).
- **Q2. Granularity of a text redline.** Word, fixed, as compare's `word`, for paragraph text edits. Decided after
  W12: the replace family deletes each whole match and inserts the whole value, as Word's tracked Replace All.
- **Q3. `w:trackRevisions`.** Recommendation: independent, as decided above. A caller who wants Word to keep
  tracking sets `settings.track_revisions` too.
- **Q4. What Word never tracks runs untracked in a session.** The alternative is refusing every call the session
  does not track. Recommendation: run them untracked, as Word does.
- **Q5. Date type.** Recommendation: `RevisionDate` with the three variants of `CommentDate`, and `CommentDate` kept
  as a type alias of it, so nothing breaks.
- **Q6. Compare's comment record.** (A) Keep it as is. (B) Write only real marks: an added or removed comment as its
  reference run in `w:ins` or `w:del`; reply, text and resolution changes carried from the edited side, with nothing
  to undo. (C) B, plus the private record only for what B cannot undo. Recommendation: C. It is right in Word, and
  oxoffice's reject still restores everything (W9).
- **Q7. `RevisionKind`.** Recommendation: new variants plus `#[non_exhaustive]` in PR 2, one breaking change, then
  only additions.
- **Q8. `people.xml`.** Recommendation: never create it. When it exists, add a `w15:person` for a new author with
  `w15:presenceInfo w15:providerId="None" w15:userId="{author}"` (W10). It is optional, and no corpus file has one.
- **Q9. Resolving a thread.** Decided after W10: `done` on every comment of the thread, as Word saves it (PR 5).
- **Q10. Replying to a reply.** Decided after W11: the reply links to the thread root, as Word rewrites it (PR 5).
- **Q11. Edits under a session that the redliner cannot mark** (section 3, refused by effect). Recommendation: plain
  refusals. A real need becomes an issue.
- **Q12. Where a range mark goes** (found in the #696 review). `Paragraph::set_text` from `<bs/>old<be/> tail` to
  `new tail` wraps all of "new tail"; the redline keeps the end mark after "new", where it stood. A start mark between
  two deleted words would go into the `w:del` and Accept would drop it: that edit is refused, with a plain message.
  Recommendation: the redline's placement, the mark stays by the words it marked. Open: where Word puts the mark
  when the text around it is retyped.
- **Q13. Text box copies.** A text box edit is written in the `mc:Choice` and its `mc:Fallback` copy, the revisions
  of both with the same ids, as the copy repeats the bookmarks. Word reads the Choice. Open: whether Word renumbers
  the copy on save.

### Word checks: results

Run on Word for Mac 16.115.2 on 2026-11-27: orphan branch `probe/rev-word-checks` of oxoffice-dev/oxoffice (head
13c5e59f), `RESULTS.md` and one folder per check, the files Word saved byte for byte.

- **W1.** Typing inside the author's insertion of an earlier session splits it with a new insertion; deleting text
  the author inserted earlier drops it, no `w:del` (R1, R3).
- **W2.** Inside another author's insertion: split in three; a deletion nests, `w:ins` (other) > `w:del` (R2, R4).
- **W3.** Bolding the author's own inserted text writes an `rPrChange` inside the `w:ins` (R6).
- **W4.** Two formatting changes: one `rPrChange`, author and date of the last editor, old properties from before the
  first (R7).
- **W5.** Numbering and style: `pPrChange` only, no `numberingChange`.
- **W6.** Our 0.8.0 compare moves (no range marks, ids all 0) read as deletion plus insertion; a move needs range
  marks that also cover the `moveTo` paragraph mark (PR 4).
- **W7.** Ranges start inside the first moved paragraph after `w:pPr`, end at body level after the last moved
  paragraph; unique ids, `w:name` pairs (PR 4).
- **W8.** Row height: no `trPrChange` (`tblPrChange`, `tblGridChange`, `tcPrChange` instead); cell shading:
  `tcPrChange`; column insertion half tracked (Reject All keeps it), column deletion and cell merge not tracked; no
  `cellIns`, `cellDel` or `cellMerge` ever (PR 2, PR 3: refused under a session).
- **W9.** A comment's reference run in `w:ins`: Reject All removes the comment; in `w:del`: Accept All does (Q6).
- **W10.** `commentsIds` maps `paraId` to `durableId`; `commentsExtensible` keyed by `durableId`, `dateUtc` true
  UTC; `people.xml` with provider "Windows Live"; `done` on the root is saved on the root and the replies (PR 5).
- **W11.** A reply to a reply is rewritten to the root's `paraId`: threads are flat (PR 5).
- **W12.** Tracked Replace All deletes the whole match and inserts the whole replacement; the final paragraph mark
  is never marked (PR 1b of the cut below, PR 3).
- **W13.** A TOC update is tracked whole: the new entries inserted, the old deleted (stays refused in this series).
- **Dates.** Word writes `w:date` as local time with a `Z` suffix, seconds zeroed, and the true UTC in
  `w16du:dateUtc` on revisions and `w16cex:dateUtc` on comments. oxoffice writes the caller's date as given; the
  `dateUtc` attributes come with PR 5.

## Found on the way, outside this spec

- **A package with no styles part.** `comment add --anchor` on such a package panics at save in a debug build: "relationship rId4 of /word/document.xml is saved but not reserved", `canonicalize.rs:756`.
  The run-range `comment add` on the same file succeeds. Compare of such a package against an edited copy with a
  comment fails: "styles relationship allocation failed: duplicate relationship id rId2". Hand-made file, written by
  no corpus producer, so a plain refusal would do, but a panic is not one.

## Links

- #469, #477 (theme `REV`), #443, #448, #460, #541 (Word check of tracked links), #6 and #466 (`PERF`), #660 (`WRD`).
- Specs: [A whole paragraph's simple field is tracked as Word tracks it](2026-11-21-simple-field-retired-with-its-paragraph.md),
  [A link in a paragraph added or removed whole is tracked as a HYPERLINK field](2026-11-23-whole-paragraph-hyperlink-as-field.md),
  [Resolving a removed last mark](2026-11-26-resolving-a-removed-last-mark.md),
  [Edit cost: one body paragraph edited in place](2026-11-24-one-body-paragraph-edited-in-place.md),
  [Body handles follow their element](2026-11-24-body-handles-follow-their-element.md).
