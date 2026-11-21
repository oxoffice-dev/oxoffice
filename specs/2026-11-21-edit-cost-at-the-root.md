---
date: 2026-11-21
status: draft
scope: perf, docx, python, tests
---

# Edit cost at the root

ROADMAP item 3. An edit should cost its own element, as in python-docx, not
the whole document. Complexity is the criterion, not wall time.

## Context

### What an edit costs today

Released 0.1.0 wheel (PyPI, release profile), Linux, steady state: the median
of 5 calls on fresh targets in one open document, after a first call. "Saved"
is the scaling test's document (6 runs a paragraph, a table and a picture
every 10th) with the identifiers Word writes on every paragraph, run and row
(`w14:paraId`, `w14:textId`, `w:rsid*`); the Google Docs variant is within
30 %. Milliseconds.

| Call | plain 25 / 100 / 400 ¶ | saved 25 / 100 / 400 ¶ | python-docx, saved 400 ¶ |
| --- | --- | --- | --- |
| `Run.text` | 1.9 / 3.9 / 14.2 | 10.5 / 24.4 / 95.6 | 0.02 |
| `Paragraph.text` | 0.7 / 1.7 / 7.7 | 0.8 / 1.8 / 8.8 | 0.03 |
| `Cell.text` | 9.5 / 19.6 / 72.4 | 9.4 / 20.5 / 75.8 | |
| `Table.add_row` | 20.0 / 43.5 / 82.8 | 10.2 / 21.3 / 73.4 | 0.08 |
| `Document.add_paragraph` | 1.0 / 1.9 / 6.7 | 1.0 / 1.9 / 5.9 | 0.02 |
| `Document.remove_content` | 0.9 / 2.1 / 6.3 | 10.7 / 27.9 / 163.7 | |
| `Document.add_footnote` | 11.9 / 24.4 / 127.9 | 13.0 / 25.7 / 91.6 | |
| `Run.bold` | 0.0 / 0.0 / 0.0 | 0.0 / 0.0 / 0.0 | |

Every call but the in-place property setters grows linearly with the body.
The first call after opening costs 2 to 30 times the steady one (lazy scans
of the main part). On the corpus `report.docx` (68 paragraphs), `Run.text`
costs 3 ms as authored and 62 to 70 ms once saved by Word or Google
(`scaling` profile).

### Where the time goes

Callgrind on the `scaling` profile, one steady call on the saved
100-paragraph body (instruction counts, inclusive; read the shares, not the
sizes, since the workspace crates are not optimised in that profile):

| Phase | `Run.text` (1.18 G) | `Paragraph.text` (77 M) | `Table.add_row` (0.75 G) |
| --- | --- | --- | --- |
| staged save to prove the edit | 85 % (`saves_as`) | 0 | 82 % (`add_empty_table_row`, staged commit) |
| of which drawing and relationship id canonicalization | 60 % | 0 | 48 % |
| whole-model resource guard (`admit_word_model`, overlaps the above) | 61 %, 143 passes | 0 | 50 % |
| handle inventory (`prepare_word_revision`) | 6 % | 85 % | 16 % |
| whole `Document` clone (`clone_for_staging`) | 1.5 %, 3 clones | 8 % | 0.8 % |

One `Run.text` on 100 paragraphs rebuilds the canonical main part 11 times
(five consumers of the staged save, each for both documents, and the
unsavable-body check), writes the document part 45 times, validates resource
limits over the whole model 62 times, rereads the package twice, compares
body blocks 448 times and clones paragraphs 904 times, for a body of 169
paragraphs and 13 tables.

### Which route each call takes

The 257 timed calls of `test_docx_scaling.py`, one call each on the
25-paragraph document, routes read from the work counters of a debug build:

| Route today | Calls | In `GROWING` |
| --- | --- | --- |
| in place (property setters) | 94 | 0 |
| whole clone, then handle inventory | 62 | 4 |
| whole clone, body append pieces (`add_paragraph`, `add_table`...) | 7 | 0 |
| whole clone, then staged save comparison (`saves_as`) | 13, and `Run.text` on saved files | 6 |
| whole clone, staged save and reopen (facade staged commit) | 78, and `remove_content(_range)` on saved files | 71 |

### Three causes, one shape

**1. The unit of an edit is the document.** A mutating call clones the whole
`Document`, edits the clone, proves the clone by staging it (the binding's
`saves_as` saves both documents and compares the packages; the facade's
staged commits stage and reopen), then rediscovers what changed by comparing
every body block and every handle owner with the previous inventory
(`WordPieces::assemble`, `indexed_inventory_matches`). Only the edit itself
is local. The staging exists to answer three questions at edit time: did the
edit change anything (so live handles survive a no-op), can the document
still be saved (so a bad edit fails at the call), and do reads after the edit
see what a reopened save would show (renumbered identifiers). The inventory
diff exists because no edit says what it touched.

**2. The local routes skip saved files.** Narrow routes avoid the staged save
for one paragraph (`direct_paragraph_saves_as`,
`direct_body_paragraph_facts`), but they accept only "plain" paragraphs
(`paragraph_is_plain`: empty `extra_xml`). The parser keeps a paragraph's
root attributes as a raw record in `extra_xml` (docx-oxml `text.rs`,
`ROOT_ATTRIBUTES_POSITION`), so no paragraph Word or Google Docs writes is
plain: `Run.text` costs 6.7 times more on the saved body, `remove_content`
26 times more.

**3. The gate is blind to both.** `test_docx_scaling.py` builds its
documents with python-docx, without these identifiers, so it times the plain
routes that no saved file takes. Its criterion (4N under 2.5 times N plus
5 ms, N = 25) lets a linear cost with a fixed part pass: `Paragraph.text`,
`add_paragraph` and plain `Run.text` are linear and pass. The 81 calls of
`GROWING` (#6) are the ones whose slope is steep enough to show.

What the code already does right: property setters edit in place
(`Run.bold`: 0.0 ms), and `Document::edit_properties` edits without staging,
puts the package graph and the identifier registry back on refusal, and in
debug builds runs the staged edit on a clone and compares. That is the shape
this spec generalizes. PowerPoint follows it already: every pptx call is flat.

## Decision

An edit is a local, in-place change of the elements it touches. The whole
document is checked when it is saved, and in debug builds and tests on every
edit.

1. **Edits work in place.** The facade edits the live model. Atomicity comes
   from the edit itself: it validates its input and builds what it inserts
   before it writes, and keeps an undo of the elements and registries it
   changes for the failures that remain, as `edit_properties` does. No
   whole-`Document` clone per edit.
2. **No save at edit time.** An edit runs the checks of its own element: the
   element's resource limits, characters XML 1.0 can carry, the relationship
   ids and parts it references, admission of raw XML input as today. The
   staged save (canonicalization, resource guard, serialization, reread) runs
   in `save` and the outputs (`to_bytes`, render, convert). A document that
   cannot be saved fails there, with today's error.
3. **Identifiers are allocated, not renumbered.** An insert takes its drawing
   id, relationship id or paragraph id from allocators the document keeps
   (highest in use plus one), so reads after an edit already show the saved
   value. The save keeps its canonicalization for what it repairs in foreign
   files (duplicate ids).
4. **Edits say what they touched.** A facade edit returns a receipt: the
   story, the owners it changed, the block slots it inserted or removed. The
   binding updates its handle inventory from the receipt. A no-op is decided
   on the touched element (model before equals model after).
5. **The whole-document routes become the oracle.** In debug builds every
   edit also runs today's route and asserts agreement: the receipt equals the
   inventory diff, the no-op answer equals `saves_as`, and the document still
   saves. These are today's debug oracle sites, kept and widened. Release
   builds never run them. CI runs the Python and Rust suites on the `ci`
   profile, so every test exercises the oracle on every edit.

What stays guaranteed: an edit that fails leaves the document unchanged; a
no-op keeps every handle; a handle either finds its element or raises, never
another element; a save writes what the reads showed.

What moves: an edit that passes its element checks but leaves the document
unsavable is reported by `save`, not by the edit. The oracle makes every such
case a test failure in debug builds, so the gap is a bug to close in the
element checks, not a behaviour.

### What we do not do

- **No new fast routes.** Widening `paragraph_is_plain` to accept root
  attributes would make `Run.text` and `remove_content` 7 to 26 times faster
  on saved files and leave every call linear, with one more route to keep in
  step. The decision above removes the routes instead.
- **No change to save.** Saving stays linear in the document, as it must.
  The guard run once per part write (62 passes per staged save) is worth a
  later look at save time, outside this item.
- **No PowerPoint work.** pptx edits are flat already.
- **No Rust API break.** Receipts are internal to the facade and the
  bindings; Rust callers see the same methods, faster.
- **No caching layer, no parallelism.** They would hide the slope, not
  remove it.

## Plan

Each pull request is checked by the full scaling set (run locally on the
`scaling` profile; CI runs the QUICK set on the pull request and the full set
on three OS once merged), the debug oracle over the whole Python and Rust
suites, and the steady-state table above at 25, 100 and 400 paragraphs in its
description. A call leaves `GROWING` in the pull request that makes it flat.

| PR | Change | Area | Calls | Leave `GROWING` |
| --- | --- | --- | --- | --- |
| 0 | **The gate sees saved files.** `builders.edit_cost_docx` writes Word's identifiers on every paragraph, run and row. `GROWING` re-measured: the calls that grow only on saved bodies join it (expected: `Run.text`, `remove_content`, `remove_content_range`). | tests | all | none; no library change |
| 1 | **No save comparison.** The binding's `publish_owner_candidate` / `publish_block_candidate` decide the no-op on the touched owner and run the element checks; `saves_as` and the block routes stay as debug oracles. | docx-py | 13 (fonts, protection, custom XML, numbering restart, page border, `move_content_control`) and `Run.text` | 6, and `Run.text` |
| 2a | **Structural edits commit in place: body.** Staged commits edit the live model with an undo, allocate their identifiers, and stop staging and reopening. Body blocks, tables, sections, replacements, revisions, fragments. | docx facade | 28 and `remove_content(_range)` | 26, and those two |
| 2b | Same, notes and comments. | docx facade | 10 | 10 |
| 2c | Same, pictures, charts, hyperlinks, page colour. | docx facade | 12 | 12 |
| 2d | Same, content controls, legacy forms, permissions, mail merge, headers, footers, watermarks, section stories. Form operations measured at 25 and 1,000 paragraphs (#7). | docx facade | 28 | 23 |
| 3 | **Handles from receipts.** Facade edits return receipts; `prepare_word_revision` applies them; the block diff and inventory match run as debug oracles. Python `replace_all` measured against the CLI (#5). | docx facade, docx-py | 62 and the 7 appends | 4 (`Cell.add_table`, list items, `add_chart`) |
| 4 | **No whole-document clone.** The last `clone_for_staging` on edit paths replaced by in-place edits with undo. | docx facade, docx-py | every edit | none; steady-state table flat at 400 ¶ |
| 5 | **A gate that sees slopes.** The docx scaling documents go from N and 4N to N and 16N (25 and 400 paragraphs), so that a linear residual cannot hide behind the floor. `GROWING` is empty. | tests | all | the set is gone |

The expected counts come from the routes above; the scaling run of each pull
request is the judge, and a call that stays linear for another reason gets a
line in that pull request.

Order with the other work of ROADMAP items 1b and 2:

- PR 0 first: it changes only the test documents, and fixes what the gate
  measures before anything is judged by it. It goes on top of #22, which
  changes the same test.
- A split of item 2 never lands under a pull request of this plan that is
  open on the same file. Splits name items, not lines, and are re-cut on a
  fresh `main`; a hand-written change cannot be. So a perf pull request
  starts after the split of its file when that split is ready, and
  otherwise the split waits for it. In practice: the facade `document.rs`
  split before PR 2a, the binding `document.rs` split before PR 1 or after
  PR 3.
- The helpers of item 1b are additive. A helper that mutates uses the
  publication of the time; the next PR of this plan converts it with the
  others.
- The debug cross-checks that item 2 keeps are the oracle of decision 5.
  They stay on in every debug build, with no switch, so every CI test run
  checks every edit. The debug work counters item 2 deletes are not brought
  back: the scaling run is the gate, the oracle checks agreement.

## Links

- Issues: #5 (Python replace slower than the CLI), #6 (edits cost the whole
  body), #7 (form operations cost seconds).
- ROADMAP item 3; item 2 inspection notes on the publication machinery.
- [A test suite from scratch](2026-11-20-test-suite-from-scratch.md): the
  scaling and budget gates.
- Upstream history of the same family: tensorbee/rdocx#403, #407, #455.
