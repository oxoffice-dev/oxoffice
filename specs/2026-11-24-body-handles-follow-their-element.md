---
date: 2026-11-24
status: accepted
scope: perf, python, docx
---

# Word binding: body handles follow their element across insertions and removals

## Context

`Document.insert_paragraph` fails the nightly scaling gate on Windows (#614):
about 1.5 ms on a body of 25 paragraphs, 8.9 to 9.4 ms on 400, against a
limit of 2.5 times plus 5 ms. It failed on 5 of the last 8 nightly runs,
first on 76c8997, before v0.4.0: not a regression, a cost at the edge.
`scaling` wheel on macOS, fastest of 7 calls, the body of
`test_docx_scaling.py`:

| Paragraphs | `insert_paragraph(10)` | `add_paragraph` |
| --- | --- | --- |
| 25 | 0.72 ms | 0.74 ms |
| 100 | 1.51 ms | 1.24 ms |
| 400 | 5.18 ms | 3.37 ms |

Both calls take the body splice (#127), which relocates every owner after
the slot in place. They differ in their publication. `add_paragraph`
publishes `AppendBody`: every owner keeps its states. A splice inside the
body publishes `Conservative`: every owner's scope states retire and every
owner gets new ones, so every handle of the document goes stale, the
paragraph before the slot included. A profile of `insert_paragraph` at 400
paragraphs puts about a third of the call in that retirement: one state
allocated, hashed and retired per scope of every owner.

A splice inside the body cannot simply keep the states today: a body
handle finds its element by the index it was captured with
(`PathSeg::Para(k)` for a body paragraph, `PathSeg::Body(t)` for a body
table and what it holds). The owner after the slot moves to `k + 1`; a kept
handle would read the paragraph that now sits at `k`. Retiring every state
is what keeps "a handle finds its element or raises, never another
element".

Every such handle already holds its owner's state (`owner`), and the
inventory finds an owner by its state in constant time (`indexed_owner`).
Handles into rich stories (`related`) already re-read their location from
their owner. The splice already moves each kept owner to its new route,
body slot and projection ordinal, and the debug build checks the moved
inventory against a rebuild (`assert_word_inventory_rebuilds`).

## Decision

1. **A body handle finds its element through its owner.** A handle bound
   to an owner (paragraph, table, row, cell, cell paragraph, run, and the
   format views over them: paragraph format, font, colour, tab stops) reads
   the index of its body block from that owner when it is validated: the
   owner's top body ancestor (a body paragraph or a body table) gives its
   projection ordinal, which replaces the captured `Para(k)` of a body
   paragraph path or `Body(t)` of a table path. The rest of the path (row,
   cell, paragraph in the cell, run) is relative to that block, and a splice
   of other blocks does not change it. Handles without an owner (snapshot
   reads, legacy routes) keep their captured index; they are bound to the
   document epoch or the snapshot scope, which every splice retires.
2. **A splice inside the body keeps the handles outside it.**
   `Document.insert_paragraph`, `Paragraph.insert_paragraph_before` on a body
   paragraph, `Document.insert_table`, `Document.remove_content` and
   `Document.remove_content_range` publish as an append does: the document
   epoch and the snapshot retire (so `Run` handles and snapshot reads go
   stale, as after `add_paragraph`), the removed owners retire, the inserted
   ones are born, and every other owner keeps its states and moves to its
   new route. A handle to a block after the slot keeps reading and editing
   that block; a handle to a removed block raises. Removing the last body
   paragraph (`commit_tail_body_removal`) retires its own owners too, not
   the removal epoch that every owner handle held, so the epoch goes: no
   publication retires it any more.
   These calls keep handles where they take the body splice. Every
   insertion does, on any body. A removal does on bodies whose paragraphs
   the splice accepts as plain; a paragraph Word saved is not one today
   (its document blocks plain local edits), so removing it still takes the
   whole route and retires every handle, as before.
3. **A debug oracle proves it.** After every such splice, debug builds
   match the moved inventory against a fresh walk of the document and
   require each kept owner to pair with the owner at its moved index, as
   they already do for appends, and rebuild the inventory and its indexes
   to compare. Python tests take handles before, after and on the slot,
   insert and remove blocks, and check what each handle reads, edits and
   raises.

What stays guaranteed: a handle either finds its element or raises, never
another element; a failed edit leaves every handle as it was.

What changes for a user: after these five calls on the splice route (every
insertion; the removals above), handles to paragraphs and tables outside
the edited range keep working, where all of them raised a stale-handle
error. `Run` handles still go stale, as after an append.

### What we do not do

- **No incremental index here.** The splice still walks every owner and
  indexes the inventory again (`index_owners_again`), as an append does.
  With decision 2, `insert_paragraph` costs about what `add_paragraph` costs
  (3.3 ms at 400 paragraphs on macOS, measured on a prototype): linear in
  the owners, under the gate with a margin on every runner, not flat. An
  index updated in place, so that a splice costs its own block, is the next
  lot of [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md) (#466).
- No change to the other publications. `Document.insert_content`,
  `move_content` and the other structural calls keep `Conservative`; they
  can adopt decision 2 once their route is a splice.
- No wider splice: removing a paragraph Word saved joins it when the
  splice accepts such paragraphs, a step of #466.
- No public API change in Rust or the CLI.

## Plan

One pull request, `perf(python)`, closes #614.

1. Decision 1: one resolver on `PyDocument` (current body ordinal of an
   owner state), called from each handle's validation; the path keeps its
   shape.
2. Decision 2: a keeping publication for splices with an offset or a
   removal (`prepare_body_splice` retires the removed owners' states and
   keeps the others), used by the five calls.
3. Decision 3: the oracle extended to splices inside the body; Python tests
   of kept, moved and retired handles.
4. Check: Rust and Python suites under debug assertions; `scaling` wheel
   with `OXOFFICE_SCALING=full`; the table above measured again. Calls that
   become flat leave `GROWING` (their strict xfail turns into an XPASS).

## Outcome

As planned, with one correction found by the existing handle tests: the
projection ordinal of an owner is not its facade index when an unowned block
comes first (`doc.tables` lists a table in a block content control, which
has no owner). A handle therefore finds its block in the facade projection
(`body_paragraphs`, `body_tables`): at the index it holds while that entry
still names its owner's route, else at the projection ordinal, else by a
scan. `test_docx_body_splice_handles.py` pins that case.

Same machine and body as the table above, `scaling` wheel, fastest of 7
calls:

| Paragraphs | `insert_paragraph(10)` | `insert_paragraph_before` | `add_paragraph` |
| --- | --- | --- | --- |
| 25 | 0.72 -> 0.54 ms | 0.56 ms | 0.74 -> 0.72 ms |
| 100 | 1.51 -> 1.08 ms | 1.09 ms | 1.24 -> 1.20 ms |
| 400 | 5.18 -> 3.06 ms | 3.05 ms | 3.37 -> 3.09 ms |

At 400 paragraphs `insert_paragraph` now sits 3.3 ms under the gate's flat
limit on macOS, against 1.6 ms before (and 0.4 ms over on Windows).
`OXOFFICE_SCALING=full` on `test_docx_scaling.py`: 415 passed, 125 xfailed,
no XPASS; `GROWING` does not change (the removals the scaling body times
are paragraphs Word saved, which keep the whole route). Python suite on a
debug build, every splice under the identity and rebuild oracles: 2197
passed. Without decision 1 (handles reading their captured index), 18 of
the 21 new tests fail.

## Links

- #614 (the Windows nightly), #6 (edit cost), #466 (the remaining lots)
- #127 (the body splice), #426 (handles outside a paragraph edit)
- [Edit cost: publishing the one block an edit touched](2026-11-21-edit-cost-one-block-publication.md)
- [PowerPoint binding: owners that do not depend on their slide's position](2026-11-22-pptx-binding-owners-independent-of-slide-position.md),
  the same principle for slides
