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
   changes before the commit, which cannot fail. An
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
   replacement splice publishes it:
   - the paragraph's owner keeps its key, its owner and properties states,
     and takes the new block's facts; its descendants (runs, hyperlinks,
     inline content) retire; the owners of the new block below the
     paragraph are born and appended, as a splice appends;
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
   order from it. The paragraph and table lists, ordinals and walks read
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
   `Document.tables`, a cell's paragraphs, the runs of a paragraph,
   `iter_inner_content`, texts and order equal to a reopened copy. Every splice that removes owners uses it, so
   `Document.remove_content`, `remove_content_range` and `pop_content` can
   become local in a later pull request, which lets the removal splice take
   paragraphs Word saved.
4. **What keeps today's route**: the cases of decision 3 of the one-block
   spec (blocks outside the body, blocks holding a text box before or after
   the edit, paragraphs in block content controls, section breaks, unaligned
   pieces), plus an inventory whose body item records are not anchored
   (#617), and the hyperlink case of point 2.
5. **Oracles.** Debug builds keep a copy of the document from before the
   edit, and after each block replacement check every kept owner
   (`assert_kept_owners_name_their_elements`), rebuild the inventory from the
   whole document and compare (`assert_word_inventory_rebuilds`, unordered),
   and compare the block's read with the whole-part read (as 1b-iii-a does).

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
| B4 | Body removal through the swap: `remove_content`, `remove_content_range`, `pop_content` on paragraphs Word saved | up to 3, measured |
| B5 | 1b-iii-c, the table as block | up to 3, measured |

Each PR proves itself as the one-block spec's plan says: the full scaling
set on the `scaling` profile, the debug oracles over the Python suite and
`cargo test -p oxoffice-docx`, the steady cost at 25, 100 and 400
paragraphs, Python tests of kept and retired handles, refusals and failures after the
swap that leave the document bytes and every handle unchanged (on a body
Word saved), reads in document order after a removal that swapped, and a
mutation check of each guard.

## Links

- #466 (edit cost at the root), #6, #622
- [Publishing the one block an edit touched](2026-11-21-edit-cost-one-block-publication.md)
- [A body splice updates the owner index in place](2026-11-24-word-owner-index-updated-in-place.md)
- [Body handles follow their element](2026-11-24-body-handles-follow-their-element.md)
