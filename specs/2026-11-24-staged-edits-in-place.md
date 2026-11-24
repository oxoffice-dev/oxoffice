---
date: 2026-11-24
status: accepted
scope: perf, docx, python
---

# Edit cost: staged edits run in place

Lots 3a to 3d of [edit cost at the root](2026-11-21-edit-cost-at-the-root.md)
(decisions 1, 3, 4 and 5), on the reservation journal of #600 and the ids
allocated at insert of lot 3a-0a (#423). It generalizes two shapes main
already has: `Document::edit_properties` (#600) and
`Document::edit_body_paragraph_in_place`
([one body paragraph edited in place](2026-11-24-one-body-paragraph-edited-in-place.md)).

## Context

About 90 of the 125 calls of `GROWING` (main 8f05bfd8) pay a staged commit in
the facade. Their routes, read from a debug build with backtrace traces at
each staging entry point (`handoff/perfrun/routes.md`):

| Route | Calls |
| --- | --- |
| facade clone, edit the clone, `prepare_and_reopen_staged` (save, reread), commit | 85 |
| of which the binding also proves the edit by a staged save comparison (`saves_as`) | 33 |
| facade clone, edit, `commit_staged_mutation` without reopen | 22 |
| binding routes only (block edits, inventory, `saves_as`) | 18 |

The notes and comments of S1 below, `scaling` wheel of main 8f05bfd8 on
Linux, the body of `test_docx_scaling.py`, fastest of the steady calls
(ms):

| Call | 25 ¶ | 100 ¶ | 400 ¶ |
| --- | --- | --- | --- |
| `Document.add_footnote` | 37.7 | 83.8 | 276.8 |
| `Document.remove_footnote` | 36.2 | 76.9 | 242.8 |
| `FootnoteEndnote.add_paragraph` | 22.4 | 50.1 | 164.4 |
| `FootnoteEndnote.delete` | 27.2 | 60.7 | 210.0 |
| `Document.add_comment` | 5.5 | 8.5 | 22.9 |
| `Document.reply_to` | 94.9 | 245.1 | 859.9 |
| `Document.remove_comment` | 46.9 | 131.6 | 478.4 |
| `Document.move_comment` | 473.9 | 1239.5 | 4289.6 |

Every one of these calls, and every staged call but six, then rebuilds the binding's whole handle
inventory and matches it with the old one (`prepare_word_revision`), on a
second whole clone the binding made first.

The staging answers three questions at edit time (root spec, cause 1): did
the edit change anything, can the document still be saved, and do the reads
see what a reopened save shows. Since 3a-0a, the third holds without a
reopen for an edit that reserves at insert every id it writes, and the save
keeps them. Since #600, a reservation in the registry and the package graph
is journaled and undone without a copy. What is left is the model: the
staged routes edit a copy because nothing else puts the model back on a
refusal, and nothing tells the binding what changed.

Two costs that are not the stage also grow with the body of the scaling
document, whose media parts grow with it (a picture every 10th paragraph):
`observe_package_graph`, which every part reservation runs, scans every part
and relationship (#600's stated residual), and some routes rebuild every
related story cache after an edit (`refresh_related_story_caches`).

## Decision

An edit names the units it may change, runs on the live model, and keeps a
copy of those units only, which puts them back on any exit that is not a
success.

1. **Units.** `Document::edit_in_place(units, edit)` (hidden) takes the
   list of units `edit` may change, each snapshotted before it runs, at the
   cost of the unit:
   - `BodyBlocks { slot, count }`: the `count` direct body blocks from
     `slot` (0 for an insertion), cloned; the undo puts them back in place of
     whatever the edit left between `slot` and the block that followed them
     (found by the body length delta). `edit_body_paragraph_in_place` is the
     case `count = 1` and keeps its signature.
   - `Note { kind, id }`, `Comment { id }`: the entry and its index in its
     model (none for a new one), with the model's dirty flag and part name;
     the extended comment records of that comment.
   - `Part(name)`: a story part held as package bytes (header, footer,
     chart, media, custom XML): its bytes or its absence, its relationships
     and its content type override. It is what undoes a removal, which the
     reservation journal cannot (`removed_from_package`).
   - `Model(kind)`: one document-level model the edit rewrites (styles,
     settings, font table, the section properties of one section), cloned
     whole. These models do not grow with the body.
   - `Numbering { abstract_ids, instance_ids }`: the numbering definitions
     and instances the edit changes or adds, by id, not the whole model,
     whose instances can grow with the lists of the body.
   The scalar state every edit touches (dirty flags, the layout caches, the
   carried block records and main body scans of the changed slots) is saved
   with the units. A unit's no-op answer compares the unit before and after,
   so it costs the unit too.
2. **One journal per call, one guard.** `edit_in_place` opens the #600
   reservation journal and holds the units and the journal in an `EditUndo`
   drop guard while `edit` runs: an error drops it, and so does the unwind of
   a panic, which restores the units backwards and undoes the reservations
   (no `catch_unwind`, as #627's guard). Only a commit disarms it. A facade call made inside an open
   call (the `add_style` of a missing note reference or hyperlink style, a
   staged helper a route reuses) joins it: its units are added to the open
   call's, and the outer call undoes or keeps everything at once. On success
   the call keeps an `EditUndo` (the units, the journal) until the caller
   commits it. A Rust or CLI caller commits at once (the public facade
   methods do); the binding takes it (`take_edit_undo`, hidden) and holds it
   in its own drop guard until its publication commits, as #627's guard holds
   the old paragraph: a refused publication or a panic in the binding restores
   the document. That guard and #627's are one: a binding call holds one
   `EditUndo`, which a body paragraph edit fills with one `BodyBlocks` unit.
3. **No stage, no reopen.** A converted route reserves at insert every id it
   writes (decision 3 of the root spec: relationship, drawing, bookmark,
   comment, note and numbering ids, part names), and copied XML takes fresh
   ids through `fresh_drawing_id_edits` and its bookmark and `w:sdt/w:id`
   siblings. It refreshes the caches of its units only. Saveability: the four
   up-front refusals stay (`refuse_unsavable_body_edit`); every other route
   leaves the rest to the save, as `Paragraph.text` does (decision 2 of the
   root spec). A reservation observes the parts it adds, not the whole
   package graph: `observe_package_graph` becomes a check of the parts and
   relationships the journal added, and the whole scan runs at open and in
   the debug coverage oracle (`assert_covers`).
4. **The edit says what it touched.** `EditUndo` gives the binding an
   `EditReport`: each unit and whether it changed, and for `BodyBlocks` the
   blocks removed and inserted at the slot. The binding publishes from it,
   without a clone of its own and without the inventory diff:
   - body units: the block replacement and body splices of the BLK lane
     (`ParagraphEdit`, `BodySplice`, a table as one block);
   - note, comment and part units: a story publication that retires the
     owners of that story and reads the owners of that story alone (one
     note, one comment thread, one header or footer part), every other owner
     kept; `HeaderFooterAppend` is today's narrow case of it;
   - `Model` units: the property publications that exist (styles, settings);
   - nothing changed: no publication, every handle kept.
5. **Oracles (decision 5).** Debug builds keep each staged route as it is
   today, renamed `*_staged`, and run it on a `clone_for_staging` copy taken
   before the edit:
   - same `Ok` value or same error text;
   - after a success: the live document writes the bytes the staged and
     reopened copy writes (`to_bytes`), and the registry covers the live
     package (`assert_covers`);
   - **an edit that writes outside its units panics**: the debug build
     applies the `EditUndo` to a copy of the live document after the edit
     and asserts that the copy equals the document before (package graph,
     registry, models, body), so a write outside the declared units, or a
     reservation the journal missed, fails at the edit that made it;
   - after a refusal: the live document equals the copy taken before;
   - the binding: `assert_word_inventory_rebuilds` and
     `assert_kept_owners_name_their_elements` after each story publication,
     as after each splice.

What stays guaranteed: an edit that fails, or panics, leaves the document
and every handle unchanged; a no-op keeps every handle; a handle either
finds its element or raises, never another element; a save writes what the
reads showed.

What changes for a user: the changes of decisions 2 and 3 of the root spec,
for the calls converted: a body that cannot be saved after a typed edit is
refused by the save instead of by the edit (the four up-front refusals
stay), and handles outside the story an edit touched keep working where all
of them were retired.

### S1 in three parts, as built (amended 2026-11-24)

S1 lands in parts. **S1a** builds the mechanism and converts
`resolve_comment`, the one call that changes a comments story and no
element a handle names. **S1b** converts the calls that add blocks to a
note story (`FootnoteEndnote.add_paragraph`, `FootnoteEndnote.add_table`)
with the story publication of decision 4: today they go through
`insert_content`, which serializes the whole document and rewrites the
note part, and the typed note model holds paragraphs only, so a table in
a note needs a block of the note model first. **S1c** converts the calls
that also edit a body paragraph (a note reference, the range marks of a
comment): `add_footnote`, `add_endnote`, `remove_footnote`,
`remove_endnote`, `FootnoteEndnote.delete`, `add_comment`, `reply_to`,
`remove_comment`, `move_comment`.

What S1a builds differs from decision 1 in how the units are kept:

- **Parts go through a part journal, not a `Part(name)` copy.** The
  package records, at the first write of each part in a call
  (`set_part`, `remove_part`), its old bytes or its absence
  (`PartJournal`, `oxoffice-opc`). The undo writes them back. It costs
  the parts the call writes. It puts a removed part's bytes back, but not
  the relationships, content type overrides and registry entries the
  removal retired: S1c, the first part with a removal, journals those
  too. Until then the undo of a removal keeps the reservation journal's
  debug refusal.
- **The registry and the package graph are never copied.** They grow
  with the body. The #600 reservation journal undoes what the call
  reserved.
- **The notes, comments and styles models are copied whole**
  (`EditUnit::{Footnotes, Comments, Styles}`). They grow with their own
  entries, not with the body: the scaling document carries three
  comments whatever its length. `resolve_comment` costs 3.2, 6.1 and
  20.4 ms at 25, 100 and 400 paragraphs on `main`, 0.33, 0.33 and
  0.47 ms in place.
- **An edit that changes no element a handle names publishes nothing**
  (the rule of PR 2 of the root spec). `resolve_comment` writes the
  `done` flag of the comments extension part and the paragraph id kept
  beside a comment paragraph: no owner changes, every handle stays. A
  comment without a paragraph gains one; it keeps the staged route.
- `observe_package_graph` still scans the whole package graph in S1a; it
  is part of the 0.47 ms above. It becomes incremental (decision 3) with
  the first call whose cost it dominates.
- **A nested call stays within the open call's units.** It joins the
  open call, whose copies and journals already cover it, and adds no unit
  of its own: a route names, at its outer call, every unit its helpers
  change (the styles of a nested `add_style`). Debug builds assert that
  the nested call's units are among the open call's, and the open call's
  undo oracle proves nothing else was written.
- The debug oracle of each converted facade call runs the staged route on
  a copy (`assert_in_place_matches_staged`) and applies the undo to a
  copy of the edited document (`assert_undo_restores`).

### S4: content controls, forms, mail merge (amended 2026-11-25)

S4 lands in three pull requests: settings-and-parts calls with the content
control property setters, then the calls that change body blocks, then the
form operations (#7).

- **Removals are journaled, not refused.** A call that removes a part,
  relationships or a content type override (a mail-merge package) needs an
  undo the reservation journal does not have. The part journal records,
  besides the bytes of each part, the relationships of an owner before an
  edit changes them in place (`record_part_rels`, called before a
  `get_part_rels_mut` edit; `remove_part_rels` records itself) and the
  overrides it removes (`remove_content_type_override`). It costs the
  relationships of the owners an edit changes: the settings part's, or
  the custom XML item's, never the main part's for these calls. The
  registry journals what it retires (`retire_authored_part`,
  `retire_authored_story_relationships`) and the story relationship ids
  it authors, and the undo holds them again.
- **`Settings` and `Parts` units.** `EditUnit::Settings` copies the
  settings model and its part name, which do not grow with the body. A
  settings edit made inside an open call (`set_document_variable`,
  `set_mail_merge_settings`) joins it and keeps no journal of its own.
  `EditUnit::Parts` names package parts an edit writes outside any model
  (custom XML items, recipient data): the part journal is their copy, and
  `part_journal_changed` their no-op answer.
- **Mail merge publishes nothing.** `set_mail_merge_package` and
  `remove_mail_merge_package` write the settings, the merge store, the
  recipient part and their relationships: no handle names any of them, so
  every handle stays. `protect_form` writes the settings in place after
  its validation, as `set_document_protection` does.
- **Which controls bind a store is read from the typed controls.** Before
  it overwrites or releases its store, a mail-merge call asks whether
  another control binds it. That check serialized the whole body; it reads
  the body's typed controls (`content_controls`) and scans the other XML
  parts, and serializes the body only when it holds a block kept as raw
  XML. Debug builds compare it with the serialized scan.
  `set_mail_merge_package` costs 11.2, 30.2 and 103.9 ms at 25, 100 and
  400 paragraphs on `main`, 2.4, 2.5 and 2.6 ms in place;
  `remove_mail_merge_package` 10.0, 28.7 and 103.2 ms, 1.4, 1.5 and
  2.9 ms.

- **A content control property is edited in its block.** The setters of
  a control's tag, alias, lock, appearance, placeholder flag, data binding
  and type write the one direct body block that holds the control (the
  control itself, or the paragraph, table or control around it), edit the
  control's properties in those bytes with today's editors, read the block
  back (it must rewrite to the same bytes) and swap it in under a
  `BodyBlocks { slot, count: 1 }` unit. The binding finds the block from
  its pieces' item counts, without reading the stories, re-reads that
  block's owners and keeps every handle: the control and every element
  around it stay the same elements. Controls outside the body, a type
  change that needs a fresh identifier or a relationship check (a picture,
  a repeating section that wraps its content), a body already known not to
  save, and a block that does not read back the same keep today's route.
  The seven setters cost 3.1 to 4.1 ms at 25, 100 and 400 paragraphs.

### What we do not do

- No change to which handles of the touched story retire.
- No incremental story inventory inside a story: a story publication reads
  its whole story (one note, one header), which does not grow with the body.
  The notes and comments parts do grow with their own entries; that is the
  cost of their part, not of the body.
- No Rust signature change: `edit_in_place`, `take_edit_undo` and
  `EditReport` are hidden.

## Plan

Each pull request: the calls it converts leave `GROWING` (the strict xfail's
XPASS), the scaling run of its calls and the QUICK set on the `scaling`
profile, the debug oracle over the Python debug suite and
`cargo test -p oxoffice-docx`, the steady table at 25/100/400 paragraphs
(main -> branch), Python behaviour tests per call (handles kept or retired,
no-op, refusal leaves the document unchanged), a mutation check per guard.

| PR | Calls | Leave `GROWING` |
| --- | --- | --- |
| S1 | `edit_in_place`, `EditUndo`, `EditReport`, story publication, incremental `observe_package_graph`; notes and comments: `add_footnote`, `add_endnote`, `remove_footnote`, `remove_endnote`, `FootnoteEndnote.add_paragraph` / `add_table` / `delete`, `add_comment`, `reply_to`, `resolve_comment`, `remove_comment`, `move_comment` | 12 |
| S2 | body: `insert_content`, `clone_content`, `move_content`, `import_fragment`, `insert_document`, `insert_html`, `insert_bibliography`, `insert_citation`, `add_bibliography_source`, `replace_bibliography_source`, `accept_revision_id`, `reject_revision_id`, `insert_section`, `remove_section`, `isolate_span`; the facade of the table edits and of the scoped replacements (published by the BLK lane) | 15 + shared |
| S3 | pictures, charts, hyperlinks, page colour, headers, footers, watermarks, section stories | 28 |
| S4 | content controls, legacy forms, permissions, mail merge, `set_story_text`, `move_content_control`; form operations check the controls they change (#7) | 20 |

S3 and S4 touch disjoint files and can run in parallel once S1 is merged.

## Links

- Issues: #466 (edit cost), #6, #7.
- [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md),
  [one body paragraph edited in place](2026-11-24-one-body-paragraph-edited-in-place.md),
  [a body splice updates the owner index in place](2026-11-24-word-owner-index-updated-in-place.md).
