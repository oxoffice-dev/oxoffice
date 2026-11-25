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
- **Body blocks are copied run by run** (amended 2026-11-25). An edit may
  name `EditUnit::BodyBlocks` several times, disjoint and in slot order,
  for blocks far apart (a comment range's start and end, a heading and its
  link paragraph): each run is copied alone, so the edit does not copy the
  blocks between them. Every unit but the last keeps its number of blocks,
  and the last takes the body's change of length; the debug undo oracle
  refuses an edit that breaks this. The report gives each changed unit
  (`EditReport::body_blocks`).
- The debug oracle of each converted facade call runs the staged route on
  a copy (`assert_in_place_matches_staged`) and applies the undo to a
  copy of the edited document (`assert_undo_restores`).

### S1b as built (amended 2026-11-24)

- **No note model for tables.** The notes parts stay the authority: the
  typed footnotes model keeps paragraphs only and is never written over
  an existing part, so `insert_content` at a destination in the
  footnotes or endnotes part splices the block into the part's bytes on
  the live document (`EditUnit::Footnotes` and a new `EditUnit::Endnotes`,
  whose part has no model: the part journal undoes it, a copy of its
  bytes answers the no-op). A fragment holding a note reference keeps the
  staged route (copying a shown note reads the main part).
- **A related story is read from its part.** `footnote_story`,
  `endnote_story` and `story_items` of a header, footer, note or comment
  story read that part alone; they serialized the main part before.
- **Story splice in the binding.** The publication retires the owners of
  the touched note, reads the owners of its part alone
  (`WordPieces::read_part`), gives the touched note's owners the keys of
  the retired ones (then new keys), and keeps every other owner and the
  document epoch, as a paragraph edit does. When the part's other stories
  read otherwise than before (a table or text box that renumbers the
  part's cell or text-box stories), it publishes the whole inventory with
  the same rule (`prepare_story_revision`). The Python API reaches that
  case only through a note other than the newest, which no handle names
  today. Debug builds check both against the whole inventory
  (`assert_word_inventory_rebuilds`, `assert_kept_owners_name_their_elements`).
- What stays linear: finding the touched note's owners walks the owner
  list (a compare per owner), under a millisecond at 400 paragraphs.

### S4: content controls, forms, mail merge (amended 2026-11-25)

S4 lands in three pull requests: settings-and-parts calls with the content
control property setters, then the calls that change body blocks, then the
form operations (#7).

- **Removals are journaled, not refused.** A call that removes a part,
  relationships or a content type override (a mail-merge package) needs an
  undo the reservation journal does not have. The part journal records,
  besides the bytes of each part, the relationships it removes one by one
  (`remove_relationships_where`: the owner, the position and the item,
  which the undo inserts back, latest first), the relationships of an owner
  it removes whole (`remove_part_rels`, at the cost of that owner's, a
  custom XML item's or a recipient part's) and the overrides it removes
  (`remove_content_type_override`). It costs the items an edit removes,
  never a copy of the main part's relationships, which grow with the body:
  the edge to a released store or to an emptied settings part is removed
  item by item, and an external relationship a call adds (a recipient
  source) goes through the reservation journal, as an internal one does.
  `record_part_rels` stays for an edit that changes an owner's
  relationships in place otherwise; these calls no longer use it. The
  registry journals what it retires (`retire_authored_part`,
  `retire_authored_story_relationships`) and the story relationship ids it
  authors, and the undo holds them again.
- **The part journal is undone before the reservation journal.** The
  reservation undo removes by id or by name, so it must not remove what the
  part journal just put back. A relationship added to an owner whose
  relationships the part journal recorded whole
  (`part_journal_records_rels`) is not recorded as a reservation: the copy
  puts the owner back as it was. An add of an id the part journal removed
  one by one first records the owner whole, so the same holds. An edit
  never adds again, through the reservation journal, an override it removed
  through the part journal in the same call; debug builds check it.
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
  its validation, as `set_document_protection` does. These calls leave the
  body alone, so after a typed edit of a body that cannot be saved after
  one they are made and the save reports the body, as after a section
  edit; the staged route failed at its own save. The debug oracles accept
  that difference only when the document already failed to save before the
  call, with the same error.
- **Which controls bind a store is read from the typed controls.** Before
  it overwrites or releases its store, a mail-merge call asks whether
  another control binds it. That check serialized the whole body; it reads
  the body's typed controls (`content_controls`) and scans the other XML
  parts. The typed controls do not include those inside a piece kept as raw
  XML (a text box in a run, a drawing kept as source, an unknown child, a
  raw block or control content): when no typed control binds the store and
  such a piece names a `dataBinding` (a byte search over the raw pieces),
  the body is serialized and scanned. A picture names none. Debug builds
  compare the typed answer with the serialized scan.
  `set_mail_merge_package` costs 11.2, 30.2 and 103.9 ms at 25, 100 and
  400 paragraphs on `main`, 2.5, 2.8 and 3.9 ms in place;
  `remove_mail_merge_package` 10.0, 28.7 and 103.2 ms, 1.5, 1.8 and
  2.8 ms. The walk of the raw pieces costs about 1 ms at 400 paragraphs in
  the unoptimized scaling build.
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
  a repeating section that wraps its content), a body that cannot be saved
  after a typed edit, and a block that does not read back the same keep
  today's route. That answer is asked once after each invalidation; a
  property edit that adds no namespace declaration but the standard `w14`
  or `w15` binding keeps it across the swap, and a settings or package part
  edit (a store, a mail-merge package) keeps it too.
  The seven setters cost 3.5 to 4.4 ms at 25, 100 and 400 paragraphs.
- **Control and permission ids come from the registry.** A new content
  control takes the smallest free `w:id` of an `sdt`, a new permission
  range the smallest free `w:id` of a `permStart`, as today; today both
  read every story to find it. The identifier registry holds the `sdt` ids
  and the permission ids of every story, read at open with the other ids,
  and reserves a new one through the reservation journal (#600): a refusal
  gives it back. A control or a range an in-place edit removes keeps its
  id held until the next reopen, as a removed bookmark does, so a later
  call can take a larger id than the staged route would: an id is unique,
  never meaningful. The coverage oracle (`assert_covers`) checks both sets:
  an id the reopened document holds and the registry does not is a missing
  reservation. The staged route, kept as the debug oracle, is given the id
  the registry reserved.
- **Structural control calls edit their blocks.** `insert_content_control`,
  `remove_content_control`, `unwrap_content_control`,
  `move_content_control`, `clone_content_control` and
  `repeat_content_control` on a control of the body edit the direct body
  blocks they change under a `BodyBlocks { slot, count }` unit: an insert
  between blocks is 0 -> 1, inside a paragraph or a cell 1 -> 1, a removal
  1 -> 0, an unwrap 1 -> N, a move the span from the first to the last
  slot it changes (its cost grows with the distance moved, not with the
  body), a clone 0 -> 1, a repeat 1 -> 1 (the block that holds the repeating
  section). The binding publishes the change with the block splice of the
  BLK lane (`publish_body_blocks_replaced`, N -> M with control blocks),
  never a second splice: the owners of the new blocks are born, those of
  the removed ones retire, every other handle stays. A clone or a repeat
  freshens the ids of what it copies from the registry, not from a scan of
  the document. A removal takes the comment threads and the notes only the
  removed blocks reference: their references are read from those blocks,
  and the notes and comments parts are edited under their units. A control
  outside the body, a shape the splice answers `WholeRoute` for, and a
  block that does not read back the same keep today's route.
- **Paragraph calls edit their paragraph.** `set_story_text` and
  `insert_legacy_form_field` on a direct body paragraph, and
  `add_story_permission_range` whose two ends sit in one direct body
  paragraph, edit that paragraph and publish it with the same splice, as
  the one-paragraph publication main already has (B2): the paragraph's
  handle stays and its runs retire. The location of the item is checked
  without listing the stories (`Document::stories` reads the whole main
  part for the fingerprints the staged route checks). A permission range
  reads and writes the markers of its paragraph alone, a pair of a fresh
  id: a document whose markers do not pair elsewhere keeps them, where the
  staged route refuses it. A range across two paragraphs, a paragraph of a
  cell, a text box or another story keep today's route until the splice
  takes their shapes.
- **Form operations read the controls they change (#7).** `fill_form` and
  `reset_form` resolve their targets from the typed controls of the body
  (a tag) and the order of the legacy fields of a story (a part and an
  ordinal), without serializing a story; they read the state of the
  targeted controls only, plan as today, and write each changed control in
  its direct body block, one `BodyBlocks` unit per block, inside one edit;
  a bound control also writes its custom XML item under the `Parts` unit.
  The checks that need every control (a target named twice, two targets
  that nest) look at the targets only. A target inside a raw piece (a text
  box kept as source, a raw block) or in another story keeps today's
  route; a header or footer control is published by the story splice of
  #634 (one header part), so the call does not read the body. The binding
  publishes the changed blocks as above. Measured at 25 and 1,000
  paragraphs, in the body and in a header; the limitation
  `docx-form-cost` goes with the fix.
- **Form operations as built (amended 2026-11-25).** The blocks that
  hold a target are found by the walk of what the body writes (#637), a
  query by tag (`body_slots_with_control_tags`: a typed control's `tag`, or a
  `tag` element in a raw fragment, whatever its prefix; debug builds check
  it misses no block of the serialized part). Each such block is written
  alone between the head and the tail of the main part
  (`body_block_part`) and read by today's scan, so the controls a call sees
  are the staged route's, at the cost of those blocks; a header or footer
  part that names a target is read whole, as today, at its own cost.
  The checks that need every control holding a tag see every block that
  holds it. The plan is today's; each changed block is swapped under its
  own `BodyBlocks` unit, a header or footer part and the custom XML items
  are written under `Parts`, all in one edit. The binding keeps every
  handle of a changed block whose stories, items, paragraphs, tables and
  links are those it held (the paragraphs it holds read their new text),
  and reads a changed header or footer part again alone (#634's story
  splice); a block of another shape undoes the edit and takes today's
  route. A call that changes a body block and a header or footer part at
  once keeps today's route: a story publication keeps the owners of every
  other story, which a body block edit of the same call would contradict. A tag a note or a comment names (parts the document models) and a
  legacy field target keep today's route: a legacy field is addressed by
  its ordinal in its part, which counts every field of the part, and
  `set_legacy_form_field_value` stages the document. The staged route
  patches the producer's bytes of a main part no edit wrote yet, where the
  in-place route writes it from the model, as every body edit does: the
  debug oracle compares the main parts read and written again. At 25 and
  1,000 paragraphs (scaling build), a fill of a body control costs 3.7 and
  7.6 ms, of a header control 4.5 and 9.0 ms (27.5 s on `main`); a fill of
  a legacy field still costs 0.5 and 28.7 s, so `docx-form-cost` narrows
  to legacy fields and #7 stays open for them.

### S3: headers, footers, section stories, watermarks, page colour (amended 2026-11-25)

S3 lands in two pull requests: the story parts and document-level
properties (S3a), then pictures, charts and hyperlinks (S3b), whose body
paragraphs publish through the BLK lane's block replacement.

- **Two more units.** `EditUnit::SectionProperties { index }` copies the
  `w:sectPr` of one section only (the one inside a body paragraph's
  properties, or the final one), never the paragraph that holds it: a
  section break is not a body owner, so a changed section reference
  publishes no body block. Finding the section compares the body blocks
  and copies none. `EditUnit::Background` copies the document background.
  `EditUnit::FontTable` copies the font table model and its part name.
  Header and footer parts are package bytes: the part journal is their
  copy (`EditUnit::Parts`), with the relationships and content types the
  edit records (the image default of a new watermark picture through
  `add_content_type_default_journaled`).
- **A relationship set is recorded once.** When the part journal already
  holds an owner's relationships (a prune that recorded the header's), a
  relationship added to that owner is not also reserved: the undo would
  remove what the part journal had just put back
  (`OpcPackage::part_journal_records_rels`).
- **Header text, section stories, watermarks.** `try_set_header`,
  `try_set_footer` and the first-page setters, `insert_content` into a
  header or footer, `create_section_story`, `link_section_story`,
  `inherit_section_story`, `unlink_section_story`, `remove_section_story`,
  the VML and DrawingML watermarks and `remove_drawingml_watermark_for`
  write the header and footer parts they name, the section references and
  the media of a watermark picture in place, reserving every part name,
  relationship and drawing id at insert. The written part is checked for
  characters XML 1.0 cannot carry (`reject_part_xml_characters`), the
  refusal the reopen gave. The calls that replace or drop a story keep
  the staged route in a document whose header or footer parts hold
  comment markers, since only the staged reconcile removes the comments
  such a story anchored: the header and footer text setters (`try_set_header`,
  `try_set_footer` and the first-page setters), `create_section_story`,
  `link_section_story`, `inherit_section_story`, `unlink_section_story`
  (through `replace_section_story`), `remove_section_story`,
  `set_text_watermark_for`, `set_drawingml_text_watermark_for` and
  `set_drawingml_image_watermark_for`. `set_text_watermark`,
  `set_image_watermark`, `remove_drawingml_watermark_for` and
  `insert_content` into a header or footer replace no story and need no
  fallback. Whatever the route, the binding publishes the whole inventory
  for such a document (`header_footer_edits_are_staged`).
- **The font table joins the mechanism.** `edit_font_table` runs inside
  `edit_in_place` with the `FontTable`, `Settings` and `Parts` units: a
  panic after the font table part is written is undone like any other.
- **`set_page_color`** writes the background and the settings in place
  and publishes nothing.
- **Publication.** The binding runs these calls on its live
  document and publishes from the story parts (`edit_story_parts`): a
  header or footer part whose bytes changed has its owners retired and
  read again alone (`WordPieces::read_part`), an added part is read
  alone, a part no section shows any more is retired, and every other
  owner and the document epoch are kept, as the note splice of S1b does.
  A section whose stories changed renews the content view of its
  `HeaderFooter` handles. A section whose `w:sectPr` changed renews its
  states. The body paragraph holding that `w:sectPr` is read again too,
  because its bytes changed. `set_page_color` and a no-op publish nothing.
  A document whose header or footer parts hold comment markers, pieces
  that do not match the story parts, or a part that cannot be read alone
  publish the whole inventory, as before. Debug builds check each
  publication against the whole inventory.

### S2c: bibliography sources (amended 2026-11-25)

- `add_bibliography_source` and `replace_bibliography_source` edit the
  source collection's custom XML part on the live document under
  `EditUnit::Parts`: the collection is read from the package (no body
  serialization), created through the journaled custom XML store helpers
  when absent, and the checks of the staged publication (the collection
  reads back with unique identities) run on the live part. A replacement
  that changes nothing writes nothing.
- The binding publishes nothing: no body content changes, every handle
  stays bound (as `add_custom_xml_store`). The staged route stays the
  debug oracle.
- `remove_bibliography_source` keeps its staged route: it reads every
  citation field of the document to refuse removing a cited source.

### S1c-1: notes and comments with a body reference (amended 2026-11-25)

`add_footnote`, `add_endnote` and `add_comment` on one direct body
paragraph edit in place over `BodyBlocks { slot, count: 1 }`, the notes or
comments model, `Styles` (a missing reference style joins the open edit,
no nested commit) and `Parts`. The facade inserts the typed reference run
in the paragraph (`create_note_in_body_slot`; `add_comment_with_date` over
its start..end slots) and appends the note or comment last in its part;
the comment id is reserved in the journal. Both note routes refuse text
XML 1.0 cannot carry up front (the staged route refused it at reopen).

The binding publishes both halves in one step
(`publish_body_paragraph_and_story`): the paragraph's block replacement
and the story splice of the new entry are both checked before either
commits, since each half alone sees the other's change unpublished; the
debug inventory check runs once after both. The edited paragraph keeps its
owner, its run handles go (the paragraph retention); every other handle is
kept. A first note or comment (its part is born), a table cell, block
control or text box location, a range over two paragraphs, a non-body
story (`StoryRunRange`) and any half answering the whole route undo the
edit and keep today's route. The debug oracle compares saved parts in a
normal form (`oracle_normal_form`): namespace declarations repeating the
root's (same prefix and URI) are dropped, as the staged byte splice
re-declares them.
### S2b: isolate_span and insert_citation (amended 2026-11-25)

Both calls change one direct body paragraph and nothing else (a citation
writes its field runs only: no source, style or setting). On a body
paragraph they run inside `edit_body_paragraph_in_place` and publish the
block replacement (#636): the paragraph keeps its owner, its run handles
go, every other handle is kept. The citation's typed insertion
(`Paragraph::insert_field_at`, `insert_accepted_run`) replaces the byte
splice of `insert_story_run_staged`; the staged route stays as the debug
oracle. A listed story item is validated before its body index is
trusted. A table cell, a header or footer, a block content control, a
text box, a hyperlink or a story item the caller built keep today's
route; `insert_citation_after_text` (a text search) too.

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
