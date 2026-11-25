---
date: 2026-11-25
status: accepted
scope: docx, python
---

# One block location resolver: paragraphs and ends inside block content controls

## Context

A block content control (`w:sdt` around paragraphs) is where Word and Google Docs write a table of contents, and
where forms and templates keep their rich-text blocks. The block operations of the facade were built around the
direct children of a story owner, and each container was added operation by operation (#433 for the body's end
after a final control, #622 for run edits in a nested control). #657 is the same family:

- `Paragraph.insert_paragraph_before` on a paragraph inside a block control raises `NotImplementedError: use
  document.insert_paragraph with a direct body content index`, which cannot reach inside either. It refuses a cell
  paragraph and a header, footer or note paragraph too (`supports direct body owners`).
- `clone_content`, `move_content`, `remove_content_at` (Python `pop_content`) and `insert_content` take direct
  children only: `[1, 1]` fails with `story location error: invalid story item path [1, 1]` in Rust, and Python
  `find_content_index` refuses the handle (`content handle is not a direct body child`).
- `DocumentFragment::from_range` and `import_fragment` accept a paragraph of a block control, `(control, k)`, at any
  depth of nesting (checked on the issue's body and on a control nested in another), but nothing names the end of
  a control's content: `(control, n)` is out of bounds, and a range ending at `(control + 1,)` "splits block
  ownership". The last paragraph of a control cannot be copied, and nothing can be appended after it.
- No refusal names a route that works.

Five private checks decide what a location means, each for some operations: `validated_direct_content_item`,
`validated_content_boundary`, `validated_insertion_boundary` (direct children and the story end),
`fragment_content_boundary`, `fragment_insertion_boundary` (the same, plus `(control, k)`); content-control
placement has its own (`require_block_parent`), and `insert_picture_to_story` reuses the first. A shape one of them
knows is refused by the others with another message.

What the paths already mean, and keep meaning: `[i]` is story item `i` of a story owner (every item, nested
fields, drawings and controls included, in document order); `[c, k]` is the `k`-th of the paragraphs written
directly in the content of block control `c`, the route `Document::paragraph_story_location` gives a body
paragraph at any depth (innermost control), and the one comments, scoped replacement and run edits (#622) take.

The eight TOC controls of the corpus (`thesis.docx` and `record.docx` from Word, LibreOffice and oxoffice,
`google/thesis.docx`, `word-google/record.docx`) all end their TOC field in a paragraph of its own after the last
entry, and begin it in the first entry; besides, Word's entries each hold a whole `PAGEREF` field, Google's none.

## Decision

### Locations

Four shapes, the same in every story (body, cell, header, footer, footnote, endnote, comment, text box):

| Shape | Constructor | Names |
| --- | --- | --- |
| `[i]` | `ContentLocation::new(story, kind, vec![i])` | the block that is story item `i`: a direct child of the story owner, or a block control written directly in a block control's content |
| `[c, k]` | `ContentLocation::new(story, Paragraph, vec![c, k])` | paragraph `k` of block control `c` (unchanged meaning) |
| story end | `ContentLocation::end(story)` | after the story's last block, before the body's `w:sectPr` (unchanged) |
| control end | `ContentLocation::control_end(story, c)` **new** | after the last child of control `c`'s content, before `</w:sdtContent>` |

- `control_end` is a location with `is_end` set and the path `[c]`, of kind `ContentControl`. A new getter
  `ContentLocation::is_end()` tells it from the location of the control itself, which has the same kind and path.
  Python gets `ContentLocation.control_end(story, control_index)` and the property `ContentLocation.is_end`.
- **`(c, n)` keeps refusing.** `k` counts the control's paragraphs only, so `(c, n)` would mean "after the last
  paragraph", which is not the end of the content when the control ends with a table, a nested control or
  bookmark ends; the story end is a constructor, not `[n]`, already; and an index one past the last stays an error
  everywhere, so an off-by-one never appends silently. The refusal names `control_end`.
- The end of a story that ends with a block control is after the control (the #433 shape); the control's end is
  inside it. Both stay before the body's `w:sectPr`.

### One resolver

`resolve_block_location(xml, story_owner, location) -> BlockSlot`, in a new module
`crates/oxoffice-docx/src/document/block_locations.rs`, replaces the five checks and `require_block_parent`. Every
location-taking block operation calls it and nothing else to read a location:

- `BlockSlot` holds the **container** (the story owner, or the `w:sdtContent` of the block control the location is
  in, with that control's story item index), the container's direct children (bookmark markers and preserved
  nodes included, the body's `w:sectPr` excluded), the index of the named block among them (their count at an
  end) and its byte boundary. `slot.block()` gives the named block and refuses an end; `slot.insertion()` gives the
  boundary before the bookmark starts that lead the block (the rule of the architecture doc, applied among the
  container's children); `slot.inserted_paragraph_location()` gives the location a paragraph inserted there takes
  (`[j]` in a story, `[c, k]` in a control), counted from that boundary.
- A shape the resolver knows works in every operation below; a shape it refuses is refused by all of them with the
  same message.

Rules the resolver applies, for every operation:

- **Block controls only.** A control in a paragraph, around rows or around cells names no block container. A
  repeating section's content holds only its items, and an item stays directly in a repeating section's content:
  a copy, move or import of an item anywhere else is refused (as `clone_content_control` refuses it), a block that
  is no item goes into an item (`control_end(story, item)`), an item is added with `repeat_content_control`.
- **A container keeps a block.** Removing or moving out the only paragraph, table or control of a block control is
  refused: the route is to remove the control itself (`remove_content_control`, or `remove_content_at` on the
  control). oxoffice opens an empty `w:sdtContent`; whether Word does is not checked, so we do not write one. The
  same holds for the only block of a header, footer, note, comment or text box, whose schema asks for one block
  (a comment refused it already; the others were left empty): the route is to insert the new content first, or
  remove the note or comment, or unwrap a control (`unwrap_content_control`). A table cell keeps a
  paragraph, directly or in another of its blocks (a control's paragraph counts, as `remove_content_control`
  counts it), for a removal or a move out of it, the removal of a control holding its only paragraph included. The body may be empty. **Behaviour change**: `remove_content_control` of a
  direct block control that is the only block of a header, footer, note, comment or text box is refused too (it
  left the story empty); `unwrap_content_control` keeps its content.
- **A typed control keeps one block.** A plain text, date, drop-down list, combo box, picture or check box control
  that holds a block takes no other (decision 3 below).
- **A block cannot move into itself** (a control into its own content).
- **A range stays in one container**: `from_range` takes a start and an end of the same container; a range ending
  after a control's last paragraph ends at `control_end`.
- Unchanged: the body's `w:sectPr` is not a block, a cell keeps a paragraph, a paragraph owning a section break is
  never copied, moved or removed by these operations, a move stays in its story, a fragment with the final section
  properties goes only to a direct body location or the body's end (today `import_fragment` checks `is_end` alone,
  which `control_end` would pass: the resolver checks the container).

### Operations

| Operation | Takes | Today | With the resolver |
| --- | --- | --- | --- |
| `insert_content` (in place for notes, headers, footers; staged otherwise) | destination | `[i]`, end | every shape |
| `import_fragment` | destination | `[i]`, end, `[c, k]` | every shape |
| `insert_html_fragment` | destination (body, cell, header, footer) | `[i]`, end | every shape; in a control the serialized route, and `direct_range` counts the control's children; in a cell the modeled route no longer counts the cell's `w:tcPr`, which put the HTML after the paragraph named |
| `clone_content` | source, destination | `[i]`; `[i]`, end | every block; every shape |
| `move_content` | source, destination (same story) | `[i]`; `[i]`, end | every block; every shape, across containers of the story |
| `remove_content_at` | location | `[i]` | every block |
| `DocumentFragment::from_range` | start, end | `[i]`, `[c, k]`; `[i]`, end, `[c, k]` | every block; every shape |
| `insert_content_control` (block, paragraph placement) | destination | `[i]`, end | every shape |
| `move_content_control`, `clone_content_control` of a block control | source, destination | a direct one through `move_content`, `clone_content` | a block control at any depth through them; every shape |
| `insert_picture_to_story` | `after` | `[i]` | every block |
| `insert_caption`, `insert_index`, `insert_table_of_figures`, `insert_table_of_authorities` | destination, returned location | `[i]`, end; `[i]` | every shape; the inserted paragraph's location from the resolver |
| `insert_bibliography`, `insert_building_block` | destination | through `insert_content`, `import_fragment` | inherit |

In a control, the rules of the story level hold among the control's children: a paragraph owns the bookmark starts
written just before it whose bookmarks end in it or in the ends just after it, and takes them when it is copied,
moved or removed; an insertion goes before the starts that lead a block, and at `control_end` after the control's
last child, trailing bookmark ends included. A copy drops `w14:paraId` and `w14:textId`, a note it references is
copied, its identities are made unique, as at story level. A paragraph inserted at the end of a control whose last
paragraph owns a section break lands in the next section, as after such a paragraph in the body.

Element operations on one paragraph (`set_story_text`, run edits, `replace_paragraph_xml`, comments) already take
`[c, k]` through `story_range_paragraph_source`, and `remove_content_control` and `unwrap_content_control` take a
control at any depth: unchanged. The resolver reads `[c, k]` with the same paragraph scan, so both families name
the same paragraph.

### Copies keep complex fields whole (#661)

A copy that would leave a complex field unbalanced is refused: `clone_content` and `move_content` of a block, and
`DocumentFragment::from_range` (so `import_fragment` of its range), when the block or the range holds a
`w:fldChar` begin without its end or an end without its begin (counted in document order, nested fields, hyperlinks,
controls and tables included). Today the copy is accepted and the field gets two begins or two ends (#661): cloning
the first entry of a table of contents, which begins the TOC field, or the paragraph after the last one, which ends
it. The message names the route: copy a block that holds whole fields, or a range from the field's begin to its
end. A block or range holding whole fields, inside another field's result or not (a TOC entry with its own
`PAGEREF`), is copied as today. A removal (`remove_content_at`, Python `pop_content`) is not a copy and is
unchanged: its fragment put back where it was restores the field. This makes the copies stricter than today: a
behaviour change, said in the pull request.

### Python

- **One argument converter** for every location argument: `ContentLocation`; `StoryItem` (its location);
  `Paragraph` or `Table` handle (the location of that block, in any story, at any depth); `int` (a direct body
  index, the body's item count being its end, as today); `Story` (its end). Used by `insert_content`,
  `pop_content`, `clone_content` and `move_content` (source and destination), `import_fragment` and
  `DocumentFragment.from_range`. Today these take one or two of the forms each.
- **`Paragraph.insert_paragraph_before` works on every paragraph**: a body paragraph at any depth of block
  controls, a cell paragraph, a header, footer, note, comment or text box paragraph. A direct body paragraph keeps
  its body splice (and its handles); the others insert through `insert_content` at the paragraph's location and
  publish as `insert_content` does (handles taken before go stale and are re-fetched). It returns the new
  paragraph, as `doc.paragraphs`, a cell or the story would give it.
- **`find_content_index`** keeps returning the body index of a direct body block. For any other block its
  `ValueError` names the route: pass the paragraph or table itself to `clone_content`, `move_content`,
  `pop_content` or `insert_content`.
- `insert_paragraph`, `insert_table`, `insert_toc`, `remove_content`, `remove_content_range` and `insert_document`
  stay direct body index calls, with their fast paths: the location calls above cover the rest.

### CLI

No change. No `oxoffice docx` command copies, moves or inserts a block at a location: the edit commands work on the
whole document (replace, fill, fields, TOC rebuild, revisions) or a body paragraph index (comments,
`bibliography insert --index`). A future insert command would parse the same four shapes.

### Refusals

Every refusal of the resolver and of these operations about a location or its container names the route that works,
in words Rust and Python share (`control_end(story, 1)`, `remove_content_control`). They are `Error::Other` with
that message, as the other refusals of these operations are; `StoryError` keeps its variants (adding one would
break an exhaustive match).

| Case | Message names |
| --- | --- |
| `(c, n)` past the control's paragraphs | `control_end(story, c)` |
| `[i]` past the story's items | `end(story)` |
| `[i]` a field, drawing or control inside a paragraph | the paragraph that holds it |
| `[c, k]` where `c` is not a block control (a paragraph, a control in a paragraph, around rows or cells) | the paragraph `[c]`, or the cell story |
| an end as a source or a range start | name the block |
| a block into a repeating section's own content | `control_end` of an item, `repeat_content_control` |
| the only block of a control removed or moved out | `remove_content_control` |
| the only block of a header, footer, note, comment or text box removed | remove that header, note or comment |
| the only paragraph of a cell removed | clear its text, or remove its row |
| a second block into a typed control | set the control's value, or remove or unwrap the control |
| a copy holding part of a complex field (`clone_content`, `move_content`, `from_range`) | a block with whole fields, or a range from the field's begin to its end |
| a range across containers | `control_end(story, c)` or the control `[c]` |
| a move to another story | `clone_content` then `remove_content_at` |
| HTML into a note, comment or text box | `from_html`, then `from_range` and `import_fragment` |
| a section-inclusive fragment elsewhere than the body | a direct body location or `end(body)` |
| Python `find_content_index` of a block outside the body's direct children | the handle itself to the location calls |
| a table written directly in a block control, as a location | the limitation `docx-block-control-table-location` (#660) |

The kind check of `[i]` (`KindMismatch`) and the refusals about the content itself (comment anchors, relationships,
a section-owning paragraph) are not about the location and keep their messages.

**Limitation** `crates/oxoffice-cli/limitations/docx-block-control-table-location.json`: a table written directly
in a block content control has no location, so it cannot be copied, moved or removed alone; workaround: copy, move
or remove it with its control, or within a fragment range between the blocks around it (`from_range` from the
paragraph before it to the next block or `control_end`, then `import_fragment`). Its issue is #660.

### Acceptance: a table of contents entry added by hand

On `word/thesis.docx` and `google/thesis.docx` of the corpus, add an entry after the last one by cloning it: Rust
`clone_content([c, n - 2], [c, n - 1])` (before the paragraph that ends the field), then set the copy's text;
Python `doc.clone_content(last_entry, field_end_paragraph)`, and `field_end_paragraph.insert_paragraph_before(...)`.
The control holds one more paragraph; the TOC field still begins and ends once; the copy has no `w14:paraId` and
every `w14:paraId` stays unique; save and reopen give the same story paragraphs. On the issue's body, a paragraph
cloned to `control_end` lands after the control's last paragraph and before `After`, and a fragment of `Inside 2`
ends at `control_end`.

### Deliberately not

- A location for a table (or another non-paragraph block) written directly in a block control: the limitation
  above. `[c, k]` of kind table would need the element operations on tables to take it too.
- Location forms for the direct body index calls (`insert_paragraph`, `insert_table`...), and a CLI command.
- Keeping Python handles across an insertion inside a control (the body splice handles only direct body blocks).
- Blocks inside `w:customXml`: the facade does not list them as paragraphs.

## Plan

One pull request, `feat(docx): one resolver for block locations, inside block content controls too`, closing #657
and #661, in two commits:

1. **Rust facade.** The resolver and `control_end` / `is_end`; every Rust operation of the table through it; the
   wrappers' returned locations from `inserted_paragraph_location`; the refusals; whole fields in copies; the
   limitation file; `docs/ARCHITECTURE.md` (a model convention: the four shapes and the resolver). Tests in a new
   area module `tests/docx/block_locations.rs`:
   - `every_block_operation_takes_every_location_shape_or_names_the_working_route`: a table-driven matrix,
     operation (insert, import, HTML insert, caption and picture inserts with their returned locations, clone
     source and destination, move source and destination, remove, range start and end) by shape (`[i]`, `[c, k]`
     and the end of controls one, two and three levels deep; the body's end; `(c, n)`; a cell paragraph, a control
     in a cell, a control holding its cell's only paragraph; a control that is its header's only block; a
     footnote, a text box and a comment paragraph; a plain text control; a repeating section, its items and its
     own `[c, 0]`; an empty control; an inline control; a table in a control). Each cell either works (the story's
     paragraphs as expected, the document saved, reopened with the same story paragraphs, and saved again to the
     same bytes) or is refused with a message containing its route; at a block control, the content control call
     of the same operation agrees. The refused cells are listed, so a gap shows in the test.
   - Short tests where a cell cannot say it: ranges and their routes in the issue's body, the XML order at a
     control's end, bookmark markers, every typed control, the TOC acceptance, whole fields, the order of a table
     of authorities' categories, the limitation's workaround.
2. **Python.** The converter, `ContentLocation.control_end` and `is_end`, `insert_paragraph_before` on every
   paragraph, the `find_content_index` refusal, stubs and docstrings. Tests in
   `crates/oxoffice-py/tests/test_docx_block_locations.py`, what the binding owns (the Rust matrix checks the
   semantics): each argument form once, the refusals and their exceptions, `insert_paragraph_before` in every
   story, the issue's six lines, the TOC acceptance. Python lists no Paragraph handle for a paragraph of a control
   in a header, footer, note or comment (`HeaderFooter.paragraphs` lists the story's own paragraphs, as
   python-docx does): the location calls reach it.

Checks: `cargo fmt`, clippy on `oxoffice-docx` and `oxoffice-docx-py` (all targets, all features, warnings denied),
`cargo test -p oxoffice-docx --test docx block_locations` and the modules the resolver's callers touch
(`fragment_import`, `content_controls`, `fields_and_toc`, `pictures`, `story_paragraphs`), the Python tests above
and the fragment, story and content control tests after `maturin develop`, and the issue's repro.

## Decisions of the review (2026-11-25)

1. One pull request for the Rust facade and Python (two commits), closing #657 and #661 (maintainer, replacing
   the two stacked pull requests first agreed).
2. The limitation `docx-block-control-table-location` has its own issue, #660: a table inside a control is not a
   story item (`story_scan.rs`), and giving it an `[i]` would shift the later indices.
3. **Typed controls refuse a second block.** An insertion into a plain text, date, drop-down list, combo box,
   picture or check box block control that already holds a block is refused, for the reason the empty
   `w:sdtContent` is: Word is not checked, so we never write what it may reject. The message names the route (set
   the control's value, or remove or unwrap the control). This makes the fragment route stricter than today
   (`import_fragment` took it): a behaviour change, said in the pull request. Rich text, unconstrained, gallery
   (TOC, bibliography), group and repeating section item controls are unchanged.
4. The copy of a block holding part of a complex field (#661) is folded in: refused, with the route (see "Copies
   keep complex fields whole").
5. The matrix has a text box paragraph row (`w:txbxContent` is its own nested owner in `scan_story_items`) and a
   comment paragraph row.
6. Independent review (2026-11-25): an item stays in its section; a move destination column; the cell keeps a
   paragraph for `remove_content_at` too; a move into an empty control writes it open; refusals name routes that
   work (a section's `[c, 0]`, a story range ending in a control, the limitation's workaround). The maintainer
   asked for fewer test lines: one Rust matrix with the cases as rows and columns, and Python tests of the
   binding only.

## Links

- Issues #657 and #661; related #433, #622, #466; limitation #660.
- [Compare adds, removes and moves block content controls whole](2026-11-23-compare-block-content-controls.md).
- [Cut and pasted content keeps its bookmarks whole and its ids unique](2026-11-22-cut-content-keeps-its-bookmarks-and-ids.md).
