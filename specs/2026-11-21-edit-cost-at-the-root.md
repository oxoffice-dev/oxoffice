---
date: 2026-11-21
status: accepted
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

The 257 timed calls of `test_docx_scaling.py`, one call each on the saved
25-paragraph document, routes read from the work counters of a debug build.
`GROWING` is today's set (N and 4N, plain bodies); the last column is a
prototype of the scaling test on saved bodies at N and 16N (25 and 400
paragraphs):

| Route on the saved body | Calls | In `GROWING` | Grow at 16N |
| --- | --- | --- | --- |
| in place (property setters) | 94 | 0 | 0 |
| whole clone, then handle inventory | 62 | 4 | 48 |
| whole clone, body append pieces (`add_paragraph`, `add_table`...) | 7 | 0 | 7 |
| whole clone, then staged save comparison (`saves_as`) | 14 | 6 | 14 |
| whole clone, staged save and reopen (facade staged commit) | 80 | 71 | 80 |

On the plain body three calls take a cheaper route: `Run.text` the handle
inventory, `remove_content` and `remove_content_range` the append pieces.

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
(`paragraph_is_plain`: no raw XML on the paragraph or its runs, no bookmark,
hyperlink, comment range or content control). The parser keeps the root
attributes of a paragraph and of each run as raw records (docx-oxml
`text.rs`, `ROOT_ATTRIBUTES_POSITION`, `RAW_ROOT_ATTRIBUTES_FLAG`), so no
paragraph Word or Google Docs writes is plain: `Run.text` costs 6.7 times more on the saved body, `remove_content`
26 times more.

**3. The gate is blind to both.** `test_docx_scaling.py` builds its
documents with python-docx, without these identifiers, so it times the plain
routes that no saved file takes. Its criterion (4N under 2.5 times N plus
5 ms, N = 25) lets a linear cost with a fixed part pass: `Paragraph.text`,
`add_paragraph` and plain `Run.text` are linear and pass. Even on saved
bodies, `Run.text` and `remove_content` pass (measured: the same 81 calls
xfail, the other 178 timed calls and reads pass). The 81 calls of `GROWING` (#6) are the ones whose
slope is steep enough to show. At N and 16N on saved bodies, 149 calls grow,
none faster than linear.

What the code already does right: property setters edit in place
(`Run.bold`: 0.0 ms), and `Document::edit_properties` edits without staging,
puts the package graph and the identifier registry back on refusal, and in
debug builds runs the staged edit on a clone and compares. That is the shape
this spec generalizes.

## Decision

An edit is a local, in-place change of the elements it touches. The whole
document is checked when it is saved, and in debug builds and tests on every
edit.

1. **Edits work in place, with a journal.** The facade edits the live model.
   Each edit records, in a journal on the document, the elements and registry
   entries it changes with their previous state, never a copy of a whole
   registry or package graph. A failure, or a panic, replays the journal
   backwards from a drop guard, so the document is left as it was. No
   whole-`Document` clone per edit.
2. **No save at edit time.** An edit runs the checks of its own element: the
   element's resource limits, characters XML 1.0 can carry, the relationship
   ids and parts it references, admission of raw XML input as today. The
   staged save (canonicalization, resource guard, serialization, reread) runs
   in `save` and the outputs (`to_bytes`, render, convert). Its error is as
   useful as the save can make it: it names the part and, where the save can
   tell, the element and the journal operation that introduced it. A form
   operation checks the controls it changes; its whole-document validation
   moves to the save too (#7). Some opened bodies cannot be saved once a
   typed edit touches them (retained nested namespace owners that cannot be
   replayed, a shadowed writer prefix: `unsavable_body_edit`). Today four
   routes refuse such an edit up front (`refuse_unsavable_body_edit`), the
   staged routes refuse it through their staged save, and the inventory
   routes leave it to the save, as `Paragraph.text` documents. After, the
   four up-front refusals stay, on their cached answer, and every other edit
   leaves it to the save, as `Paragraph.text` does today.
3. **Identifiers are allocated at insert and kept by the save.** Today the
   save keeps the ids the source had, repeated ones included, and gives the
   ids edits added their values in document order, each by its own rule: the
   highest drawing id in use plus one, the next relationship id above the
   preserved ones, the lowest free bookmark id. Reads match the file only
   because staged commits reopen after each edit. After, an insert takes its
   value by its insert rule (see *The rule* below) when it is made, and the
   save keeps it, as it keeps the source's. One change to save output: ids
   added by edits follow the order of the edits, not of the document, and
   keep their value after earlier content is removed. The pull requests that
   change it say so in their body, and the changelog carries it. In detail
   (amended 2026-11-21, before lot 3a-0):
   - **Scope**: every id the save renumbers today: bookmark ids, the
     bookmark ids of a rebuilt table of contents, numbering instance and
     definition ids, relationship ids of the main part and of the story
     parts (headers, footers, notes, comments), the relationship ids of the
     parts a document gains (styles, settings, numbering, footnotes,
     comments, extended comments), drawing ids, and the names of media parts
     (`/word/media/imageN`). A gained endnotes or glossary part keeps its
     provisional relationship id (`rdocxDeferredEndnotes`,
     `rdocxDeferredGlossary`) in the saved file today; it takes `rIdN` too,
     a second change to save output that the same pull request announces.
   - **Removed content**: the save does not compact. An id keeps its value
     after earlier content is removed. An id a removal frees is taken again
     only if that removal released it from the registry; otherwise it stays
     taken until the document is reopened.
   - **The rule**: where the insert's rule and the save's differed (bookmarks
     took the lowest free id among the source's only, numbering and
     relationship ids were numbered from the source's highest), the insert's
     rule, which is what reads already show after an in-place edit. A part a
     document gains takes the relationship rule (`rIdN`) when it is
     reserved, instead of a provisional name.
   - **Ids a save creates** on the copy it writes (a part a model needs and
     the package lacks) are allocated at that save, by the same rule. The
     live document does not hold them, as today, except after `sign` and a
     save that normalizes a field, which install the copy they wrote.
4. **Edits say what they touched.** The binding drains the same journal,
   through a `#[doc(hidden)]` facade call as it reaches
   `direct_body_paragraph_facts` today, and updates its handle inventory from
   it: the story, the owners changed, the block slots inserted or removed. A
   no-op is decided on the touched element (model before equals model after).
   The journal holds one call: a facade edit, or the several facade edits
   one binding call makes, which the binding opens and closes as one, so a
   failure in a later step undoes the earlier ones. The next call starts it
   empty, so a Rust or CLI caller that never reads it keeps one call's worth.
5. **The whole-document routes become the oracle.** In debug builds every
   edit also runs today's route and asserts agreement: the journal equals the
   inventory diff, the no-op answer equals `saves_as`, and the document saves
   and reopens to the model the reads see. These are today's debug cross-check
   sites, kept and widened; release builds never run them. The oracle only
   checks what the tests call, and today's Python tests call few edits on a
   debug build (the timed calls run on the `scaling` profile). So PR 0 adds a
   debug-build test that runs every timed call once, untimed, on a small
   saved document, and each pull request adds Python behaviour tests for the
   calls it changes: handles kept or retired, no-op, refusals.

What stays guaranteed: an edit that fails, or panics, leaves the document
unchanged; a no-op keeps every handle; a handle either finds its element or
raises, never another element; a save writes what the reads showed.

What moves, and is accepted:

- An edit that passes its element checks but leaves the document unsavable
  is reported by `save`, not by the edit, and the edit is already in place.
  The save error names the part, and the element and journal operation where
  it can (decision 2). The oracle makes every such case a test failure in
  debug builds, so the gap is a bug in the element checks, not a behaviour.
- On a body that cannot be saved after a typed edit, the staged routes no
  longer refuse the edit; the save does, as it already does after
  `Paragraph.text`. The four routes that refuse up front keep doing so.
- Rust callers meet that error at save too. A CLI command saves within the
  same command, so a CLI user sees it at the same step as today.

### What we do not do

- **No new fast routes.** Widening `paragraph_is_plain` to accept root
  attributes would make `Run.text` and `remove_content` 7 to 26 times faster
  on saved files and leave every call linear, with one more route to keep in
  step. The decision above removes the routes instead.
- **No other change to save.** Saving stays linear in the document, as it
  must, and keeps its checks; decision 3 is its only output change (the
  order of added ids, their value after a removal). The guard
  run once per part write (62 passes per staged save) is worth a later look
  at save time, outside this item.
- **No PowerPoint work here.** pptx calls pass the scaling gate, but many
  also clone the presentation and compare saves (pptx-py `shape.rs`,
  `presentation.rs`). PR 0 runs the pptx scaling test once at 16N as a
  measurement; any growth it shows becomes its own item.
- **No Rust signature change.** The behaviour changes are the ones above:
  saveability errors at save, and the ids of added content in saved files
  (in edit order, kept after a removal).
- **No caching layer, no parallelism.** They would hide the slope, not
  remove it.

## Plan

Each pull request is checked by the full scaling set (run locally on the
`scaling` profile; CI runs the QUICK set on the pull request and the full set
on three OS once merged), the debug oracle over the whole Python and Rust
suites, and the steady-state table above at 25, 100 and 400 paragraphs in its
description. A call leaves `GROWING` in the pull request that makes it flat.

Every call outside the in-place setters pays the binding's whole clone and
handle inventory, and the calls of the save comparison and the staged commit
pay them on top of their staging
(the plain `Run.text` route, which already skips the save comparison, still
grows at 16N). So the shared costs go first: a call can only leave `GROWING`
once both its own proof and the shared costs are gone. PR 1 brings the
journal and converts the calls whose only whole-document costs are those
two; PR 2 and 3a-3d convert their own calls the same way, in the binding as
well as in the facade, when they remove their proof.

| PR | Change | Area | Calls | Leave `GROWING` |
| --- | --- | --- | --- | --- |
| 0 | **A gate that sees what users hit.** `builders.edit_cost_docx` writes Word's identifiers on every paragraph, run and row; the docx documents go from N and 4N to N and 16N (25 and 400 paragraphs, `LINEAR` 24): on the `scaling` profile a call passes only if its cost at 400 paragraphs stays under 2.5 times its cost at 25 plus 5 ms. `GROWING` re-measured (149 calls on the prototype). A test runs every timed call once, untimed, on a small saved document, so that a debug build puts each of them under the oracle. `docs/DEVELOPMENT.md`, the test and `conftest.py` docstrings, the scaling job's comment in `ci.yml` and the test-suite spec say 16N for docx (pptx stays at 4N). The pptx scaling test is measured once at 16N and the result reported. The full docx scaling set takes about 19 minutes locally instead of 5. | tests | all | none; it joins 68 |
| 1 | **The journal.** Edits that clone and then match the inventory edit the live model with the journal (decision 1), and the binding updates its handles from it (decision 4); the whole clone and the inventory diff run as debug oracles. | docx facade, docx-py | 62 and the 7 appends | 55 |
| 2 | **No save comparison.** `publish_owner_candidate` / `publish_block_candidate` decide the no-op on the touched owner and run the element checks; `saves_as` and the block routes run as debug oracles; the calls use the journal. A test document whose body cannot be saved after a typed edit checks the refusals and the save error. | docx-py | 14 (fonts, protection, custom XML, numbering restart, page border, `move_content_control`, `Run.text`) | 14 |
| 3a | **Structural edits commit in place: body.** First the save keeps the ids edits added (decision 3), with the reopen oracle (lot 3a-0a), then the registry and the package graph journal the entries an edit reserves instead of being copied (lot 3a-0b, #275). Then staged commits edit the live model with the journal, allocate their ids, and stop staging and reopening. Body blocks, tables, sections, replacements, revisions, fragments. | docx facade, docx-py | 30 | 30 |
| 3b | Same, notes and comments. | docx facade, docx-py | 10 | 10 |
| 3c | Same, pictures, charts, hyperlinks, page colour. | docx facade, docx-py | 12 | 12 |
| 3d | Same, content controls, legacy forms, permissions, mail merge, headers, footers, watermarks, section stories. Form operations check only the controls they change, measured at 25 and 1,000 paragraphs. | docx facade, docx-py | 28 | 28 |

The expected counts come from the routes above; the scaling run of each pull
request is the judge, and a call that stays linear for another reason gets a
line in that pull request. The pull request that empties `GROWING` removes
the limitation of #6 from `limitations.json`, 3d the one of #7, and the
first after which the Python replacement costs what the CLI does (measured
from PR 1 on) the one of #5 (the limitations page is built at release since
2026-11-21: nothing to regenerate, see
[the limitations page is built at release](2026-11-21-limitations-page-built-at-release.md)).

Order with the other work of ROADMAP items 1b and 2:

- PR 0 first: it changes only the tests, and fixes what the gate
  measures before anything is judged by it.
- Splits of item 2, as agreed with that work and the orchestration on
  2026-11-21, which changes the order of
  [the code inspection spec](2026-11-21-code-inspection-and-refactoring.md)
  for its steps 16 and 17 only: the facade and binding `document.rs` splits are cut by item names and re-cut
  on `main` after each merge of this plan, so these pull requests never wait
  for them. The docx-oxml `text.rs` and `document.rs` splits land first: no
  pull request of this plan edits those files before they are in. The
  publication code then sits in `text/work.rs`, `text/emission.rs` and
  `document/source.rs`.
- The helpers of item 1b are additive. A helper that mutates uses the
  publication of the time; the next PR of this plan converts it with the
  others.
- The debug cross-checks that item 2 keeps are the oracle of decision 5.
  They stay on in every debug build, with no switch, so every edit a CI test
  makes is checked. The debug work counters item 2 deletes are not brought
  back: the scaling run is the gate, the oracle checks agreement.

## Links

- Issues: #5 (Python replace slower than the CLI), #6 (edits cost the whole
  body), #7 (form operations cost seconds).
- ROADMAP item 3; item 2 inspection notes on the publication machinery.
- [A test suite from scratch](2026-11-20-test-suite-from-scratch.md): the
  scaling and budget gates.
- Upstream history of the same family: tensorbee/rdocx#403, #407, #455.
