---
date: 2026-11-21
status: draft
scope: perf, docx, python
---

# Edit cost: publishing the one block an edit touched

Step 1b-iii of [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md):
the paragraph and run edits, and the cell insertions, on documents that Word
or Google Docs saved.

## Context

Ten calls clone the whole document, edit the clone, then rebuild and match
the whole owner inventory (`prepare_word_revision`):

| Calls | Publication today |
| --- | --- |
| `Paragraph.text` | `ParagraphText` |
| `Paragraph.add_run`, `add_hyperlink`, `add_equation` | `ParagraphAppend` |
| `Run.remove`, `Run.add_field`, `Run.add_legacy_form_field`, `Document.split_run` | `Conservative` |
| `Cell.add_paragraph`, `Cell.add_table` | `Conservative` |

On a saved body of 400 paragraphs (`scaling` profile, steady state),
`Paragraph.text` costs about 30 ms and `Paragraph.add_run` 32 ms, linear in
the body. The body splice that made the appends flat (#127) accepts only
plain paragraphs, and no paragraph Word writes is plain: its identifiers are
raw root attributes. They make no owner, no story item and no story; the
splice refuses them only because they are raw XML.

Rebuilding only the edited block is enough to find its new owners (the
binding already reads one block alone, `word_block_owners`, and places a
cell's owners this way in `apply_local_cell_edit`). What is not local today
is which handles an edit retires. Measured on `main`, after
`Paragraph.text` or `Paragraph.add_run` on one body paragraph, every `Run`
handle of the document is stale (the document epoch retires), and so is
every hyperlink handle and every owner of a comment or text-box story. After
`Run.remove`, `Run.add_field`, `split_run`, `Cell.add_paragraph` or
`Cell.add_table`, every handle of the document is stale (`Conservative`).
Reproducing those rules exactly costs a walk of every owner at every call.
The two local routes that exist already keep those handles (the local cell
route, the plain paragraph XML route), so a handle's survival depends today
on which route an edit happened to take.

## Decision

1. **An edit of one block retires the handles of that block only.** A
   paragraph text edit or append, and the four run edits on the run's
   paragraph, follow the rule `ParagraphText` applies to the paragraph it
   selects: the paragraph keeps its owner and properties states, its content
   state and its runs are born. A cell insertion follows the `CellText` rule
   on its cell. Nothing outside the block retires: not the document epoch
   (the local revision mode, as `RunText` uses), not other paragraphs' runs,
   hyperlinks, comments, text boxes or sections. The snapshot scope still
   retires, as for every publication. The whole route applies the same rule,
   so a handle's survival no longer depends on the route.
2. **The binding publishes the block it re-reads.** The facade edits a copy
   of the one direct body block (a paragraph, or the table that holds the
   cell) and swaps it in when the edit succeeds, as table property edits do
   (1b-i). The binding re-reads that block (`word_block_owners`), places its
   owners at the block's slot and replaces the block's old owners: the body
   splice, generalized from plain paragraphs to one block replaced, with a
   range per owner family, the change of body story items and of body link
   ordinals, and the shift of the cell stories after a table. The pieces
   take the new block.
3. **The whole route stays** for blocks outside the body (headers, footers,
   notes, comments, text boxes), blocks that hold a text box before or after
   the edit, paragraphs inside a block content control, edits that move a
   section break, and unaligned pieces or inventories. `add_hyperlink` and
   `add_internal_hyperlink` stage their relationship or bookmark in the
   facade; they leave `GROWING` with PR 3c of the plan, not here.
4. **The debug oracle proves locality.** After every local publication,
   debug builds rebuild the inventory and its indexes from the whole
   document and compare (`assert_word_inventory_rebuilds`, extended to the
   indexes by route, generation and parent), compare the block's piece with a
   fresh read, and run the whole route on a staged copy: every state the
   local route keeps belongs to an owner whose route and read facts are the
   same before and after the edit, so a kept handle finds its own element.

What stays guaranteed: a handle either finds its element or raises, never
another element; an edit that fails leaves the document and every handle as
they were; reads after an edit see what a save writes.

What changes for a user: handles outside the edited block keep working
after these edits (they raised a stale-handle error). Python tests pin which
handles are kept and which retire, on a new document and on the Word and
Google reports.

### What we do not do

- No new plain route: the root attributes stay raw, the block is re-read.
- No change to the publications of the other calls.

## Plan

| PR | Change | Leave `GROWING` |
| --- | --- | --- |
| 1b-iii-a | Decision 1 on the whole route: the retirement rule, the local revision mode for these publications, block publications in place of `Conservative` for the six calls; Python tests of kept and retired handles | none (behaviour only) |
| 1b-iii-b | Decision 2 for direct body paragraphs: `Paragraph.text`, `add_run`, `add_equation`, the four run edits; the generalized splice and the oracle of decision 4 | 7, measured |
| 1b-iii-c | Decision 2 for body tables: `Cell.add_paragraph`, `Cell.add_table` (and `Cell.text` on saved bodies, if it follows) | 2 or 3, measured |

Each PR runs the full scaling set locally, the debug oracle over the Rust and
Python suites, and gives the steady cost at 25, 100 and 400 paragraphs.

## Links

- [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md), decisions 1,
  4 and 5; issues #6, #234.
- #127 (the body splice), #233 (table property edits in place).
