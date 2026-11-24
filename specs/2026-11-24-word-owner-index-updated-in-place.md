---
date: 2026-11-24
status: accepted
scope: perf, python, docx
---

# Word binding: a body splice updates the owner index in place

## Context

Since [body handles follow their element](2026-11-24-body-handles-follow-their-element.md),
a paragraph or table inserted inside the body keeps every other handle, and
`Document.insert_paragraph` costs what `Document.add_paragraph` costs. Both
are still linear. `scaling` wheel on macOS, the body of
`test_docx_scaling.py`, fastest of 9 calls on 3 documents:

| Call | 25 ¶ | 100 ¶ | 400 ¶ |
| --- | --- | --- | --- |
| `Document.insert_paragraph(10)` | 0.59 ms | 1.08 ms | 3.07 ms |
| `Paragraph.insert_paragraph_before` | 0.58 ms | 1.07 ms | 3.00 ms |
| `Document.insert_table(10, 2, 2)` | 3.94 ms | 4.50 ms | 6.71 ms |
| `Document.add_paragraph` | 0.72 ms | 1.20 ms | 3.11 ms |

A profile of steady calls at 400 paragraphs (macOS `sample`) puts 85 to 90 %
of the three paragraph calls in the owner inventory of the binding:

| Share of the call | `insert_paragraph` | `insert_paragraph_before` | `add_paragraph` |
| --- | --- | --- | --- |
| `commit_body_splice` | 51 % | 49 % | 46 % |
| of which `index_owners_again` | 36 % | 34 % | 32 % |
| `prepare_body_splice` | 30 % | 32 % | 29 % |
| `plan_body_splice` | 8 % | 7 % | 13 % |

What is left (the inserted paragraph's own facts, the native insertion, the
Python handle) does not grow. `insert_table` pays the same linear part (about
2.8 ms at 400 paragraphs) on top of a fixed 3.9 ms that reads the detached
table (`detached_table_owner_snapshots`, `spliced_table_blocks`).

The inventory is linear because an owner is known by its position and holds
absolute numbers:

- **Position.** `PyDocument::owners` is a vector in walk order. `parent`, the
  values of `by_generation`, the buckets of `by_route`, the keys and values
  of the `*_by_parent` maps and the values of `direct_body_paragraphs` are
  positions in it. A splice inserts the new owners at their walk position,
  so every later position moves, and the commit indexes every owner again.
- **Absolute numbers.** Each owner holds its route (the body story item index
  at the head of its index path, the cell story ordinal of a cell's content),
  `body_slot` and `projection_ordinal`. A splice moves them for every owner
  after the slot. `by_route` hashes the absolute route, so a moved owner must
  be hashed again: faster code would not remove this, only another key.
- **Projections.** `body_paragraphs` and `body_tables` hold an absolute route
  per facade index, rewritten after the slot; `cell_paragraphs` is keyed by
  facade table index, rebuilt after an inserted table;
  `direct_body_paragraphs` is keyed by slot.
- **Walks.** `plan_body_splice` checks the walk order of the whole inventory
  twice and `cell_stories_before` walks it again for a table;
  `prepare_body_splice` builds the whole next inventory (`next_owners`) and
  copies every kept owner's scope states, which the commit writes back
  unchanged.

The PowerPoint binding had the same shape for slides and dropped it:
owners hold paths relative to their slide, and a slide's position is read
from a stable key
([owners independent of slide position](2026-11-22-pptx-binding-owners-independent-of-slide-position.md)).

## Decision

A body splice costs its own block, plus integer work on the list of body
items. It never visits the owners of other blocks.

1. **Stable owner keys.** An owner's key is its index in `owners`, and an
   insertion never moves it: a splice appends the owners it inserts, as a
   local cell edit already does. The inventory is no longer in walk order
   after a splice: the whole-inventory routes match owners by route, the
   splice plan no longer reads that order, and the removal of the last body
   paragraph (which wants the paragraph's owners last) takes the whole route
   when owners added later sit after them.
2. **Owners relative to their body item.** Each top-level body story item
   that holds owners gets a shared record (`BodyItem`) with its current item
   index, its direct body slot, and the number of body paragraphs, body
   tables and main-part cell stories before it. An owner of that item holds
   the record and stores, relative to it: the head of its body route, the
   cell story ordinal of its route in the main part (and of a cell's own
   story), and the projection ordinal of a body paragraph or body table. Its
   `body_slot` is the record's slot. Every other owner (headers, footers,
   notes, comments, text boxes, sections) keeps absolute values, which no
   splice changes. An owner's fields become private: readers get the route,
   slot and ordinal through accessors that resolve them, so none can read a
   stale value. A splice updates the records of the items after it, a few
   integers each, and touches no owner.
3. **Cell stories, where their numbering follows the body.** Main-part cell
   story ordinals are relative to their item only when every cell story of
   the main part belongs to a body table owner and they are numbered item by
   item (the condition `cell_stories_before` checks today). Otherwise they
   stay absolute and a table insertion takes the whole route, as today.
4. **Indexes by key and relative route.** `by_route` hashes the stored
   relative route and the item record's key; a lookup by an absolute route
   finds the record of its item first (by item index, or by cell story
   ordinal). `by_generation`, `parent` and the `*_by_parent` maps hold keys.
   `direct_body_paragraphs` becomes a vector by direct body slot.
   `body_paragraphs` and `body_tables` hold routes relative to an item
   record, and `cell_paragraphs` becomes one entry per facade table: an
   inserted block inserts one entry in each vector.
5. **A splice reads what it touches.** The plan finds the slot's item by
   binary search over the records and checks the removed paragraphs and the
   projection entries around the slot; the preparation retires the removed
   owners and allocates states for the inserted ones only; the commit
   appends and indexes the inserted owners, inserts their record and moves
   the records after it.
6. **The oracles stay, against a full walk.** After every splice, debug
   builds compare the resolved inventory with a fresh walk
   (`assert_word_inventory_rebuilds`, unordered, each owner known by its
   route and its parent's route), check that every owner's resolved route
   finds that owner in `by_route`, that the item records equal those derived
   from the walk, and that each kept owner still names its element
   (`assert_kept_owners_name_their_elements`, on a resolved copy of the
   inventory taken before the edit). A debug copy of an owner resolves its
   record, so it does not follow later splices.

What stays guaranteed: a handle either finds its element or raises, never
another element; a failed edit leaves every handle as it was; handles
outside a splice keep working, as since the previous step.

### What we do not do

- **Removal stays linear.** A splice that removes paragraphs compacts the
  owner vector and indexes it again, as today. It is not timed flat: the
  removals of the scaling body are paragraphs Word saved, which take the
  whole route. A free list of keys makes it local in the step that lets the
  splice remove such paragraphs (#466).
- **The body item list is linear.** A splice moves the records after the
  slot and the vectors of the projections and of the pieces (one pointer or
  a few integers per body item), as the native body vector does: the cost
  of the body list itself, as the pptx binding kept for slides.
- No change to the whole-inventory routes, which rebuild the inventory and
  now also derive the item records from it, and no change to the other
  publications.
- No public API change in Rust, the CLI or Python.

## Plan

One pull request, `perf(python)`, a lot of #466.

1. `BodyItem` records and the anchored owner: private fields, resolving
   accessors, construction from walk addresses (records derived from them).
   Move every reader to the accessors (about 190 sites in
   `crates/oxoffice-docx-py/src`).
2. Indexes by key and relative route; projections relative to records;
   `cell_paragraphs` per facade table.
3. Splice plan, preparation and commit as decided; removal compacts.
4. Oracles as decided.
5. A guard: the four calls must cost at 16N at most 1.5 times their cost at
   N plus 1 ms (`test_docx_scaling.py`), which today's cost fails (3.07 ms
   against 0.59 ms) and a flat cost passes with room on every runner.
6. Check: the Python suite on a debug build (every splice under the
   oracles); `OXOFFICE_SCALING=full` on `test_docx_scaling.py` on the
   `scaling` wheel; the table above measured again.

Estimate: a day of work, most of it step 1 and the checks. Risk: medium,
local to the binding. A wrong record would hand a handle another element;
the oracles check every splice of every debug test, and the new handle tests
of the previous step pin kept, moved and retired handles.

## Links

- #466 (the remaining lots of edit cost at the root), #614 (the previous step)
- [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md)
- [Body handles follow their element](2026-11-24-body-handles-follow-their-element.md)
- [PowerPoint binding: owners independent of slide position](2026-11-22-pptx-binding-owners-independent-of-slide-position.md)
