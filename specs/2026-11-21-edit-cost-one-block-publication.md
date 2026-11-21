---
date: 2026-11-21
status: accepted
scope: perf, docx, python
---

# Edit cost: publishing the one block an edit touched

Step 1b-iii of [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md):
the paragraph and run edits, and the cell insertions, on documents that Word
or Google Docs saved.

## Context

Nine calls clone the whole document, edit the clone, then rebuild the whole
owner inventory (`prepare_word_revision`):

| Calls | Publication today |
| --- | --- |
| `Paragraph.text` | `ParagraphText` |
| `Paragraph.add_run`, `add_equation` | `ParagraphAppend` |
| `Run.remove`, `Run.add_field`, `Run.add_legacy_form_field`, `Document.split_run` | `Conservative` |
| `Cell.add_paragraph`, `Cell.add_table` | `Conservative` |

`Paragraph.add_hyperlink` publishes as `ParagraphAppend` too, but its facade
half stages the relationship in a copy of the document: it leaves `GROWING`
with PR 3c of the plan, not here.

On a saved body of 400 paragraphs (`scaling` profile, steady state),
`Paragraph.text` costs about 30 ms and `Paragraph.add_run` 32 ms, linear in
the body. The body splice that made the appends flat (#127) accepts only
plain paragraphs, and no paragraph Word writes is plain: its identifiers are
raw root attributes, which make no owner, no story item and no story. The
binding already reads one block alone (`word_block_owners`).

Which handles an edit retires is not local today. On the whole route:

- `ParagraphText` and `ParagraphAppend` keep the paragraph's owner and
  properties states and give birth to its content and runs; they publish in
  the global revision mode, so the document epoch retires and every `Run`
  handle of the document goes stale. `ParagraphText` also retires every
  hyperlink of the document, and both retire every owner of a comment or
  text-box story.
- `Conservative` does not match the inventory: every handle goes stale.

The local cell route (`apply_local_cell_edit`) retires only the cell's
owners, but in the global mode, so `Run` handles go stale there too. Only the
plain paragraph XML route publishes in the local mode.

Those rules are safe because they retire the owners whose routes an edit can
shift: a text-box or table-cell story is addressed by its ordinal in its
part, and a hyperlink by its story, the smallest story item that holds it
and its index among that item's links (a paragraph inside a block content
control is not an item: its links count with the control's). Removing a text
box, a cell story, a story item or a link renumbers the ones after it, and the
matcher pairs old and new owners by equal route.

## Decision

1. **An edit of one block retires the handles whose element it changes or
   whose route it shifts, and no others.** For the nine calls:
   - The edited paragraph (for a run edit, the run's paragraph) follows the
     `ParagraphText` slot rule: its owner and properties states stay, its
     content state, its runs, its hyperlinks and every story inside it (text
     boxes, with their tables, cells, paragraphs, runs and links) are born. A
     text-box paragraph keeps today's rule (all its states born).
   - A cell insertion retires the cell's content and its descendants; the
     cell keeps its owner state and the table its owner and grid states.
   - Owners whose route holds an ordinal the edit shifts retire, in every
     story:
     - when the edit changes how many story items its story holds (fields,
       inline content controls, drawings), the hyperlinks of the later items
       of that story;
     - when it adds or removes a link inside a story item that holds more
       than the edited paragraph (a block content control), the later links
       of that item;
     - when it changes how many text-box or table-cell stories its part
       holds, every owner of a later story of that kind in that part.
     Paragraphs and tables keep their states: the body's are matched by their
     block slot, the other stories' by their ordinal among the story's
     paragraphs or tables, which an edit of one paragraph does not move.
   - The snapshot scope retires, as for every publication.
   - Nothing else retires. The publication uses the local revision mode, so
     the document epoch stays and the `Run` handles of other paragraphs keep
     working; so do other paragraphs, tables, cells, sections, headers,
     footers, notes, comments, and the hyperlinks of other paragraphs.
   On every route, an edit that leaves the edited paragraph (or cell) equal
   to what it was is a no-op: it publishes nothing and keeps every handle.
   The nine calls get block publications of their own. `ParagraphText`,
   `ParagraphAppend` and `Conservative` keep their rules for the other calls
   that use them (`Paragraph.add_internal_hyperlink`, `Document.add_note`).
   The whole route, the local cell route and the block route of decision 2
   apply this same rule, so a handle's survival does not depend on the route.
2. **The binding publishes the block it re-reads.** The facade edits a copy
   of the one direct body block (a paragraph, or the table that holds the
   cell) and swaps it in when the edit succeeds, as table property edits do
   (1b-i). Otherwise the binding re-reads
   the block (`word_block_owners`), places its owners at the block's slot,
   and replaces the block's old owners: the body splice, generalized from
   plain paragraphs to one block replaced, with a range per owner family,
   the change in body story items and body link ordinals, and the shift of
   the cell stories after a table. The pieces take the new block.
3. **The whole route stays** for blocks outside the body (headers, footers,
   notes, comments, text boxes), blocks that hold a text box before or after
   the edit, a cell whose body block is a block content control, paragraphs
   inside a block content control, edits that move a section break, and
   unaligned pieces or inventories. 1b-iii-b covers direct body paragraphs;
   paragraphs and runs inside body-table cells take the table as their block
   in 1b-iii-c.
4. **A debug oracle proves every kept handle, without the matcher.** After
   every publication of the nine calls, on every route, debug builds check
   each owner whose state is kept. The owners whose element the edit rewrites
   (the edited paragraph; for a cell edit, the cell and its table) keep their
   route. Every other kept owner, inside the edited block or table as well as
   outside it, has the same element (its bytes, serialized alone) at its old
   route moved by the edit's shift. The shift is known from the edit and
   defined for every story: the item indexes of the edited story after the
   edit, by the change in its item count; the link indexes of the item that
   holds the edit, by the change in its link count; the text-box and
   table-cell story ordinals of the part after the edited block, by the
   change in their count; and, for the body, the block slots, which an edit
   in place does not move.
   They also rebuild the inventory and its indexes from the whole document
   and compare (`assert_word_inventory_rebuilds`, extended to the indexes by
   route, generation and parent), and compare the block's piece with a fresh
   read. 1b-iii-a puts this check on the whole route before any local route
   exists.

What stays guaranteed: a handle either finds its element or raises, never
another element; an edit that fails, or a no-op, leaves the document and every
handle as they were; reads after an edit see what a save writes.

What changes for a user: after these nine calls, handles outside the edited
block keep working, where most of them raised a stale-handle error. Python
tests pin which handles are kept and which retire, including the cases the
ordinal rule protects: a text box removed before another one, a table added
in a cell before cells that hold links, the first of two links of a paragraph
removed, a field removed before paragraphs with links in a footer and in a
table cell, and a link removed from the first paragraph of a block content
control whose later paragraphs hold links.

### What we do not do

- No new plain route: the root attributes stay raw, the block is re-read.
- No change to the other publications.
- No incremental update of the inventory indexes in this step. The splice
  still walks every owner and reindexes, and renumbers later body items,
  link ordinals and cell stories: integer work linear in the owners, with no
  document read. `Document.insert_paragraph` takes the same splice at the
  tenth block, so with that renumbering of almost every owner, and passes the
  scaling gate. An incremental index is a later step of the plan.

## Plan

| PR | Change | Leave `GROWING` |
| --- | --- | --- |
| 1b-iii-a | Decision 1 on the whole route and the local cell route, with the identity oracle of decision 4; Python tests of kept and retired handles | none (behaviour only) |
| 1b-iii-b | Decision 2 for direct body paragraphs: `Paragraph.text`, `add_run`, `add_equation`, the four run edits; the generalized splice | up to 7, measured |
| 1b-iii-c | Decision 2 for body tables: `Cell.add_paragraph`, `Cell.add_table`, cell paragraph and run edits, and `Cell.text` on saved bodies if it follows | up to 3, measured |

Each PR runs the full scaling set locally, the debug oracle over the Rust and
Python suites, and gives the steady cost at 25, 100 and 400 paragraphs.

## Links

- [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md), decisions 1,
  4 and 5; issues #6, #234.
- #127 (the body splice), #233 (table property edits in place).
