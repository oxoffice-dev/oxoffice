---
date: 2026-11-24
status: accepted
scope: perf, docx, python
---

# Edit cost: one body paragraph edited in place

Step 1b-iii-b of [publishing the one block an edit touched](2026-11-21-edit-cost-one-block-publication.md)
(decision 2), on the inventory that
[a body splice updates in place](2026-11-24-word-owner-index-updated-in-place.md).

## Context

The paragraph and run edits of a direct body paragraph pay the body twice.
`scaling` wheel on Linux, the body of `test_docx_scaling.py` (paragraphs
Word saved), fastest of 9 calls, main at e473ce26:

| Call | 25 ¶ | 100 ¶ | 400 ¶ |
| --- | --- | --- | --- |
| `Paragraph.text` | 4.26 ms | 8.97 ms | 28.95 ms |
| `Paragraph.add_run` | 4.25 ms | 9.03 ms | 29.55 ms |
| `Paragraph.add_equation` | 4.59 ms | 9.28 ms | 29.65 ms |
| `Run.remove` | 4.38 ms | 9.20 ms | 29.22 ms |
| `Run.add_field` | 5.57 ms | 10.84 ms | 32.10 ms |
| `Run.add_legacy_form_field` | 7.18 ms | 13.04 ms | 34.40 ms |
| `Document.split_run` | 4.52 ms | 9.33 ms | 29.54 ms |

- **The facade** edits a copy of the whole document
  (`clone_for_staging`), so that a failed edit leaves the document as it
  was. Each of these edits changes one `w:p` and nothing else: it runs on a
  `Paragraph`, which holds the paragraph alone, and reserves no identifier,
  relationship or part.
- **The binding** publishes the copy with `publish_block_edit`: it reads
  the shift from the block alone (1b-iii-a), then rebuilds the whole owner
  inventory (`prepare_shifted_revision`) and matches it with the old one.
  The body splice that costs its own block (#617) takes plain paragraphs
  only, and a paragraph Word saved is not plain (its identifiers are raw
  root attributes).
- **Removal is linear**: the splice that removes owners compacts the owner
  vector and indexes it again.

## Decision

An edit of one direct body paragraph costs that paragraph: its copy, its
owners, and the integer work on the list of body items that every splice
pays. The rule of which handles are kept and which retire does not change
(decision 1 of the one-block spec): only its cost does.

1. **The facade edits a copy of the paragraph.**
   `Document::edit_body_paragraph_in_place(index, edit)` (hidden) runs `edit`
   on a copy of paragraph `index` (counted as `paragraph_mut` counts) when it
   is a direct body child, and puts the copy in its place when `edit`
   succeeds; it returns the paragraph it replaced, so that the caller can
   put it back (`restore_body_paragraph`). The binding holds the old
   paragraph in a guard from the swap until its publication commits: any
   exit that is not a success (a returned refusal, an error, a panic) drops
   the guard, which puts the old paragraph back, as the drop guard of the
   root spec's decision 1 replays a journal. Nothing of the inventory
   changes before the revision commit. That commit can still return an
   error, before it changes anything: the guard then puts the paragraph
   back. Once it succeeds, the inventory updates that follow cannot fail,
   their room reserved beforehand. An
   edit that fails or panics leaves the document untouched. A paragraph that
   is not a direct body child (inside a block content control, a cell, a
   header) returns `Ok(None)` before `edit` runs: the caller keeps its
   route. As for table property edits (1b-i), the layout and save caches
   drop their answers (`invalidate_layout`); the carried block records
   compare the block's model, so the save writes the new paragraph.
   Only edits that change the one paragraph take this call; an edit that
   reserves an identifier or adds a relationship (`add_hyperlink`) keeps its
   staged route, and joins the #600 journal when it moves here.
2. **The binding replaces the block's owners.** After the facade swap, the
   binding reads the new block alone (`word_block_owners`), and a block
   replacement splice publishes it (`publish_body_blocks_replaced`, a
   general entry point for direct body blocks replaced in place, which takes
   one paragraph replaced by one in this step and answers "whole route" for
   any other shape):
   - the paragraph's owner keeps its key, its owner and properties states,
     and takes the new block's facts; its runs retire; the owners of the
     new block below the paragraph are born and appended, as a splice
     appends. A paragraph that holds a hyperlink, before or after the edit,
     takes the whole route: a hyperlink has no parent, its projection
     ordinal counts the links of the whole story, and the edits of this
     step rarely meet one;
   - the old owners are found from the paragraph's owner through the
     parent maps, never by a walk of the inventory;
   - the body item records after the block move by the change in body
     story items (a field, an inline content control or a drawing is an
     item); paragraphs, tables and cell stories do not move;
   - when the item count changes, decision 1 retires the hyperlinks of the
     later items of the body. Each record counts its hyperlinks, so the
     splice knows whether any later item holds one; when one does, the
     publication takes the whole route (rare: it needs an edit that adds or
     removes a field, control or drawing before a link);
   - the no-op rule stays: a paragraph equal to what it was publishes
     nothing and keeps every handle;
   - the pieces take the new block.
3. **Removal by swap.** A splice removes an owner by moving the last owner
   of the vector into its key, and rewriting the references to the moved
   owner (its children's parent, `by_generation`, its `by_route` bucket, the
   `*_by_parent` maps, `direct_body_paragraphs`, the projections): work in
   the removed owners and the direct children of the moved ones, never in
   the inventory.
   Order: since #617 the owner vector is not in walk order after a splice
   (a splice appends the owners it inserts), and no read takes document
   order from it, with two exceptions B2 found and fixed: a property edit
   that keeps every owner (`relocated_property_owners`) paired the owners
   keyed alike under one parent (the paragraphs of one cell, text box and
   note paragraphs, which have no parent) by key order, and now pairs them
   in document order (story ordinal, then route); and the removal of the
   last body paragraph took the paragraph's runs as the owners right after
   it, and now checks that they are all of its runs and that its item holds
   no hyperlink. The whole-inventory matcher keeps the smallest key when
   several old owners match one route, which no test meets. The paragraph and table lists, ordinals and walks read
   the story order from the projections (`body_paragraphs`, `body_tables`,
   `cell_paragraphs`, `direct_body_paragraphs`) and the `*_by_parent` lists,
   which hold keys in story order; the pieces are per body block. The swap
   rewrites a moved key in place in each of those lists and in its
   `by_route` bucket, so every list keeps its order; no index is ordered by
   key. The whole-inventory routes, which pair owners by route, and the
   unordered oracle are unchanged; the oracle stays ordered wherever an
   order is meant to hold (the projections, the parent lists). B2 checks
   that no read iterates the owner vector for an order, and tests reads in
   document order after a removal that swapped: `Document.paragraphs`,
   `Document.tables`, a cell's paragraphs, the runs of a paragraph, the
   body story items (the Python API has no `iter_inner_content`), texts and
   order equal to a reopened copy. The commit of a removal splice also
   drops the removed slots from `direct_body_paragraphs` before the swap, as
   the records after them moved. Every splice that removes owners uses it, so
   `Document.remove_content`, `remove_content_range` and `pop_content` can
   become local in a later pull request, which lets the removal splice take
   paragraphs Word saved.
4. **What keeps today's route**: the cases of decision 3 of the one-block
   spec (blocks outside the body, blocks holding a text box before or after
   the edit, paragraphs in block content controls, unaligned pieces), plus
   an inventory whose body item records are not anchored (#617), and the
   hyperlink cases of point 2. A paragraph that ends a section takes the
   in-place route: these edits keep the paragraph's properties, so its
   section break does not move, and its section owners keep their index.
5. **Oracles.** Debug builds keep a copy of the document from before the
   edit, and after each block replacement check every kept owner
   (`assert_kept_owners_name_their_elements`), rebuild the inventory from the
   whole document and compare (`assert_word_inventory_rebuilds`, unordered),
   and compare the block's read with the whole-part read (as 1b-iii-a does);
   they also check that the owners kept and retired are those the whole
   route keeps and retires (`block_retention` over a walk). Debug builds
   can fail or panic the publication after the swap on request
   (`OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE`), so that tests see the old
   paragraph put back.

What stays guaranteed: a handle either finds its element or raises, never
another element; a failed edit, a refused publication or a no-op leaves the
document and every handle as they were; reads after an edit see what a save
writes.

### What we do not do

- No change to which handles retire (decision 1 of the one-block spec).
- `Run.text`, `Paragraph.replace_xml` and `Run.replace_xml` publish through
  the staged save comparison (`publish_block_candidate`,
  `publish_owner_candidate`), which another lot of #466 removes; they take
  the block replacement once it is gone. Replacements (`replace_text`) wait
  for their facade half, which edits story sources.
- Body tables (cell paragraphs and runs, `Cell.add_paragraph`,
  `Cell.add_table`, `Cell.text`) are 1b-iii-c: the same splice with the
  table as block.
- No public API change in Rust, the CLI or Python.

## Plan

| PR | Change | Leave `GROWING` |
| --- | --- | --- |
| B1 | This spec | none |
| B2 | Decisions 1-5: the facade call, the block replacement splice, removal by swap; the seven calls of the table above | the seven calls, measured |
| B3 | Paragraphs of nested block content controls run edits as they do one control deep (#622) | none (behaviour) |
| B4+B5 | One pull request (decisions 6-10): blocks replaced N -> M, body removal through the swap (`remove_content`, `remove_content_range`, `pop_content` on paragraphs Word saved), 1b-iii-c with the table as block | up to 6, measured |

Each PR proves itself as the one-block spec's plan says: the full scaling
set on the `scaling` profile, the debug oracles over the Python suite and
`cargo test -p oxoffice-docx`, the steady cost at 25, 100 and 400
paragraphs, Python tests of kept and retired handles, refusals and failures after the
swap that leave the document bytes and every handle unchanged (on a body
Word saved), reads in document order after a removal that swapped, and a
mutation check of each guard.

## Outcome (B2)

As decided, with the corrections written into the decisions above: a
paragraph holding a hyperlink takes the whole route, a paragraph ending a
section takes the in-place route, and two reads that took an order from
owner keys were fixed. When the splice answers "whole route" after the
edit ran (a field added before a later hyperlink), the old paragraph goes
back and the call takes its staged route, which runs the edit again; the
blocks known to need the whole route are refused before the edit runs.

`scaling` wheels of main (e473ce26) and of B2 on Linux, the body of
`test_docx_scaling.py`, fastest of 9 calls:

| Call | 25 ¶ | 100 ¶ | 400 ¶ |
| --- | --- | --- | --- |
| `Paragraph.text` | 4.26 -> 0.86 ms | 8.97 -> 0.87 ms | 28.95 -> 0.87 ms |
| `Paragraph.add_run` | 4.25 -> 0.91 ms | 9.03 -> 0.91 ms | 29.55 -> 0.91 ms |
| `Paragraph.add_equation` | 4.59 -> 1.10 ms | 9.28 -> 1.11 ms | 29.65 -> 1.11 ms |
| `Run.remove` | 4.38 -> 0.99 ms | 9.20 -> 0.99 ms | 29.22 -> 0.99 ms |
| `Run.add_field` | 5.57 -> 1.70 ms | 10.84 -> 1.71 ms | 32.10 -> 1.70 ms |
| `Run.add_legacy_form_field` | 7.18 -> 3.03 ms | 13.04 -> 3.03 ms | 34.40 -> 3.01 ms |
| `Document.split_run` | 4.52 -> 1.05 ms | 9.33 -> 1.05 ms | 29.54 -> 1.05 ms |

The seven calls leave `GROWING`.

## B4+B5: blocks replaced, removed, and tables

B2 publishes one paragraph replaced by one. The calls that remove body
blocks, edit a body table, or insert blocks (lanes STG: content controls,
charts, fragments) need the same splice for any number of blocks of any
shape. Decisions 6-10 extend it; decisions 1-5 hold.

6. **N blocks replaced by M.** `publish_body_blocks_replaced` takes N
   direct body blocks replaced by M at one slot (N or M may be 0):
   paragraphs, tables, and whole block content controls. A block that holds
   a text box, before or after, keeps today's route. The signature does not
   change.
   - The old owners are found by route: the old pieces' facts are placed at
     the bases of the slot's record (as `WordPieces::inventory` places them)
     and each address is looked up in the index; no walk of the inventory.
     Debug builds check they are the parent-map closure of the blocks' roots.
   - The new blocks are read alone (`word_block_owners`) and placed at the
     same bases, running across the M blocks.
   - The body item records of the old items are replaced by records of the
     new items; the records after them shift once by the change in items,
     slots, paragraphs, tables, cell stories and body links. Items and slots
     shift separately: a paragraph with a field spans several items, a slot
     of bookmark markers holds none.
   - Every owner of a replaced item is rebuilt on its new record, the kept
     ones keeping their key and states; the projections
     (`direct_body_paragraphs`, `body_paragraphs`, `body_tables`,
     `cell_paragraphs`) each take one range splice; the pieces take the M
     new blocks.
   - A change in cell stories needs anchored cell records (#617), else the
     whole route.
7. **Which handles stay does not change.** The splice keeps exactly the
   owners the whole route keeps for the same call (decision 1 of the
   one-block spec, `block_retention`): the caller passes its retention, the
   splice matches the new owners to the old ones inside the blocks as the
   local cell route does, and the debug oracle compares with
   `block_retention` over a walk. Removals and insertions keep the rule of
   "body handles follow their element": the owners of other blocks follow,
   the epoch retires.
8. **Body links.** A body item record gets a link base, shifted like the
   other bases, and a body hyperlink's projection ordinal is stored relative
   to it, so the links after a removal or an insertion keep their handles and
   name their links. An edit that changes the item count still retires the
   later links (decision 1): it takes the whole route when a later item
   holds one, as in B2. Records also count the hyperlinks of their cell
   stories, so that the same rule sees links in later tables.
9. **Body removal is local.** The facade removes direct body blocks in place
   (`remove_body_blocks_in_place`, hidden) and returns them, so that the
   binding puts them back on any exit that is not a commit. It answers
   "staged route" when the blocks hold a comment range or reference, a note
   reference or a paragraph section break, or when the body cannot be edited
   in place: those keep today's route, which reconciles comments, notes and
   sections. The relationships the blocks referenced are freed at the next
   write (`freed_relationships`, as `pop_content` does today) instead of a
   whole-document comparison. The bookmark and drawing identifiers of the
   removed blocks stay reserved until the document is reopened: identifiers
   stay unique, and an identifier given later may differ from the one the
   reopening route gave. `pop_content` builds its fragment from the removed
   block alone; debug builds compare it with the staged fragment.
10. **The table as block (1b-iii-c).** `Document::edit_body_table_in_place`
    (hidden) edits a copy of one direct body table and swaps it in, as
    decision 1 does for a paragraph; a table inside a block content control
    keeps its route. `Cell.add_paragraph`, `Cell.add_table`, `Cell.text` and
    the paragraph and run edits of B2 on a paragraph of a body table cell
    publish the table 1 -> 1 with their retention (`CellInsert`, `CellText`,
    the edited cell paragraph). An edit that changes a comment keeps the
    staged route. The cost is the table's, not the body's.

The amendment to the one-block spec's decision 3: a whole block content
control removed, inserted or replaced is a body block like the others;
paragraphs and tables inside a control, edited alone, keep their route.

## Outcome (B4, phase 1)

Decisions 6, 8 and 9 as decided, with these corrections:

- **Removals and insertions take the splice retention; edits keep B2's.**
  `publish_body_blocks_replaced` takes N blocks replaced by M with a new
  `BlockRetention::Spliced`: the old blocks' owners retire, the new ones
  are born, every other owner keeps its states, the epoch retires. No owner
  inside the replaced blocks is kept, so decision 7's matcher is not needed
  for it; it comes with the table as block (decision 10), whose retentions
  keep owners inside the block. `EditedParagraphKeepsItsOwner` stays B2's
  1 -> 1 paragraph path.
- **Plan, reserve, apply.** The publication is split so that another
  splice can share its revision: `plan_blocks_replaced` (read only; whole
  route, unchanged, or a plan with the scopes it retires and bears),
  `reserve_blocks_replaced`, one `commit_scopes` by the caller,
  `apply_blocks_replaced` (cannot fail), and the debug oracle
  `assert_blocks_replaced` after every application.
- **Paragraph section breaks** are refused by the facade (decision 9) and,
  for insertions, by the splice itself.
- **"A body that cannot be edited in place"** is a body a typed edit
  cannot save (`unsavable_body_edit`), not a package that blocks plain
  local edits: every body Word saved blocks those, and B2 edits such
  bodies in place. A removal keeps the answer that the body saves, since a
  body without some of its blocks holds no namespace owner it did not hold
  (debug builds check a kept answer against a cold one on each read).
- **Old cell records.** The splice takes cell stories only when they are
  anchored to their items (#617); records that cannot place a cell-story
  hyperlink make every later item count as holding one.

`scaling` wheels of main (e473ce26) and of this branch on Linux, the body of
`test_docx_scaling.py`, fastest of 3 calls:

| Call | 25 ¶ | 100 ¶ | 400 ¶ |
| --- | --- | --- | --- |
| `Document.remove_content` | 36.43 -> 0.30 ms | 96.25 -> 0.31 ms | 339.65 -> 0.30 ms |
| `Document.remove_content_range` | 36.00 -> 0.30 ms | 95.82 -> 0.30 ms | 344.11 -> 0.31 ms |
| `Document.pop_content` | 30.05 -> 0.38 ms | 67.97 -> 0.39 ms | 226.76 -> 0.38 ms |

The three calls leave `GROWING` (80 -> 77 on main at 090a022f).

## Correction: an edited block reads as its own bytes (#651)

An edit builds its block in memory, and that model is not always the one the
reader builds from the same bytes: a field added to a run stays in the run,
which the reader splits into the run, the field's runs and its result. The
staged route reopened the document, so its reads matched the saved file; the
in-place route read the edited model, so live reads and the owners read from
it differed from a reopen (#651, `Run.add_field`, `Run.add_legacy_form_field`).

Corrected for the class: before planning, `publish_body_blocks_replaced` and
`publish_body_paragraph_and_story` put in place of each inserted block the
block a reopen reads (`Document::reread_body_blocks_in_place`: the block's
bytes in the main part written alone, read back), at the cost of those
blocks. The model, the owners read from it and every read after the edit are
then those of the saved file, for every call that publishes through a block
replacement. A picture whose drawing id the save still allocates keeps that
mark, which its bytes do not carry. A block that does not read back takes the
whole route. The staged route of a block edit (`publish_block_edit`) no
longer reopens either: it installs the edited copy. It now rereads the direct
body blocks the copy changed the same way before it publishes, which costs no
more than the copy it already made; so an in-place call that falls back to it
(a field added before a later hyperlink) reads as the saved file too. Debug
builds check after every block replacement and every staged block edit that
each new block equals the reread of its own bytes.

## Links

- #466 (edit cost at the root), #6, #622, #651
- [Publishing the one block an edit touched](2026-11-21-edit-cost-one-block-publication.md)
- [A body splice updates the owner index in place](2026-11-24-word-owner-index-updated-in-place.md)
- [Body handles follow their element](2026-11-24-body-handles-follow-their-element.md)
