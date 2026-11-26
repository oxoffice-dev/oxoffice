---
date: 2026-11-26
status: accepted
scope: docx, python, cli
---

# A table written in a block content control has a location

## Context

#657 gave the block operations one resolver (`document/block_locations.rs`) and four location shapes, the same in
every story: `[i]`, story item `i`; `[c, k]`, paragraph `k` of block control `c`; `end(story)`;
`control_end(story, c)`. A table written directly in the `w:sdtContent` of a block control has none of them (#660,
limitation `docx-block-control-table-location`).

**Google Docs writes this shape on ordinary files**: a whole table alone in a block control, between two body
paragraphs (`<w:sdt><w:sdtPr><w:lock w:val="contentLocked"/><w:id .../><w:tag w:val="goog_rdk_N"/></w:sdtPr>
<w:sdtContent><w:tbl>...</w:tbl></w:sdtContent></w:sdt>`), and in footers too (reported by other projects). No
corpus file holds one (every `w:tbl` of the 52 Word files checked). Measured on 0.7.0 (Python) with that shape:

- **`doc.paragraphs` lists the cells' paragraphs** (`Document::paragraphs`: the model walk enters a table written
  in a block control, while it skips a direct body table, against its own doc: "skips body tables"). Those entries
  have no body story location: `Paragraph.xml` raises `paragraph XML owner route is unsupported`, `replace_text`
  raises `IndexError: paragraph index out of range` (an internal message), so a `for p in doc.paragraphs:
  p.replace_text(...)` loop stops at the first cell. The body story's paragraphs (`story_paragraphs`) do not list
  them, and the CLI `diff` reads them in its body stream on purpose ("left to the body paragraphs").
- **Through the table they work**: `doc.tables[t].cell(r, c).paragraphs[k]` reads and edits (`xml`,
  `replace_text`, `insert_paragraph_before`, text, runs; checked on merged cells, a nested table, two control tables,
  a direct table before or after, bookmarks around). Except in a cell that itself holds a table in a block control:
  its `paragraphs` list the inner table's too, and `xml` raises on every paragraph of that cell.
- **The table handle**: refused as a location argument; `Table.xml`, `Table.replace_xml`, `Table.add_row` and
  `Cell.xml` raise (`... owner route is unsupported`, `table has no checked direct physical owner`), for want of a
  location (`TableLocation::Body(index).native` asks for a direct body slot).
- **In a footer** the table is in neither `footer.tables` nor `footer.paragraphs` (both list the story's direct
  items); its cells are reached only as cell stories of `doc.story_paragraphs`.
- **No location**: the block operations refuse `[c, k]` of kind table with the limitation's message; the element
  operations that take a table location (`table_element_xml`, `replace_table_xml`, `read_story_table`,
  `edit_story_table`, `story_table_cell`, `cell_element_xml`, `replace_cell_xml`, the row and column calls,
  `measure_content`) read it with `story_item_span`, which takes `[i]` only; `try_replace_text_at` reads `[c, k]` as
  a paragraph whatever its kind (a table is refused only because `story_range_paragraph_source` refuses it);
  `insert_content_control` with placement `row` takes `[t]` only. The workaround (`from_range` between the blocks
  around it) fails for a table alone or first in its control: Google's case.

A table inside a control is no story item (`scan_story_items` lists the owner's direct children and, below them,
controls, simple fields and drawings), and giving it an `[i]` would shift every later story item.

## Decision

A table written in a block control is a table like any other: it has a location, it is listed where tables are
listed, and its cells are no paragraphs of the story around it.

### The location: `[c, k]` of kind table

`ContentLocation::new(story, StoryItemKind::Table, vec![c, k])` names table `k` among the tables written directly in
the content of block control `c`, the innermost control, as for a paragraph. The kind says what `k` counts:
paragraphs for kind paragraph (unchanged), tables for kind table. Same in every story; no new constructor, field or
Python property. `[c, n]` past the control's `n` tables names nothing. Python spells it
`ContentLocation(story, "table", (c, k))`, and a `Table` handle converts to it.

| Option | Verdict |
| --- | --- |
| A. The table becomes story item `[i]` | Ruled out. Every later item of the story shifts: a location or a Python `StoryItem` taken before, a comment anchor, a scoped replacement selector, the CLI's JSON `index_path` would name another item, silently. Paragraphs inside controls are no items either. |
| B. `[c, j]` counting every block of the control | Ruled out: it changes what `[c, k]` names for every control holding a table or a nested control before a paragraph. With a flag instead, a paragraph gets two numberings. |
| C. `[c, k]` of kind table, `k` counting tables | **Chosen.** The kind is part of every location and checked by every resolver (`KindMismatch`); nothing was named by it (refused naming #660); Python and the #657 matrix already spell it (`t:box/0`). It mirrors a container's separate `paragraphs` and `tables`. |
| D. A new constructor or flag (`control_table(story, c, k)`, a third path step, a marker step) | Ruled out: a field every location match, hash and Python object carries for what the kind already says. |
| E. Relative to the paragraph before (`[c, k, 1]`) | Ruled out: nothing for a table alone or first in its control. |

**The shape is shared once.** `[t, r]` of kind table already names row `r` of table `[t]` for
`insert_content_control` with placement `cell` (undocumented). The two never meet: story item `t` is a table there
and a block control here. A block or element operation given `[t, r]` with `t` a table refuses it, naming `[t]`.

### One count, read everywhere

- **Block side.** `resolve_block_location` reads `[c, k]` of kind table as it reads kind paragraph: the container is
  the control's content, the block its `k`-th table. Every operation of the #657 table then takes it.
- **Element side.** `story_item_span` (behind `story_item_source`) reads `[c, k]` of kind paragraph and of kind
  table, and `story_range_paragraph_source` becomes it, so every element operation reads both. `measure_content`,
  which refuses a control paragraph today (`invalid story item path`), takes it with the rest.
- **One scan.** `scan_story_control_paragraphs` becomes `scan_story_control_blocks(xml, control, kind)`, with the
  main-part layout fast path of `story_control_paragraphs`. The resolver, the element reader and the scoped
  replacement read it; the scoped replacement's own `[c, ordinal]` read (`replace.rs`) counts by the location's kind:
  today it counts paragraphs, so a table location there would edit paragraph `k`.
- **Placements.** `insert_content_control` with placement `row` takes any table location (`[i]`, `[c, k]`); placement
  `cell` takes a row as its table's path followed by the row index: `[t, r]` (unchanged) or `[c, k, r]`.

### Listings: a table in a block control is a table

- **Paragraph walks skip it**, as they skip a direct table: the model's body and cell walks (`CT_Body::paragraphs`,
  `paragraph_count_before`, `CT_Sdt::collect_paragraphs` for a block owner, the facade's `nth_paragraph_in_control`)
  no longer enter a table written in a block control. `Document::paragraphs`, `paragraph`, `paragraph_mut`,
  `paragraph_count` and Python `doc.paragraphs` then list the body story's paragraphs (the list `story_paragraphs`
  gives), and a cell's paragraphs (`Cell.paragraphs`) the cell story's. A table's cells are reached through the table
  (`doc.tables`, its cells), as a direct table's. Row and cell controls inside a table keep their walk.
  **Behaviour change**: for a document holding such a table, `doc.paragraphs` loses those entries and the later
  indexes move; every user of the walk is checked, and one that wants the table's text reads the table, as for
  a direct one.
- Rejected: routing those `doc.paragraphs` entries to their cell story. Each cell paragraph would be listed twice
  (`doc.paragraphs` and `doc.tables`), which the usual loop over paragraphs, then over tables, processes twice; and a
  body paragraph handle would point into a cell story.
- **New `Document::story_tables(story) -> Result<Vec<ContentLocation>>`**: the tables of a story outside its cells, in
  document order: story items of kind table (`[i]`) and tables of its block controls at any depth (`[c, k]`). For the
  body it lists what `Document::tables` lists (a debug assertion). Python `HeaderFooter.tables`, `Note.tables` and
  `Cell.tables` list through it, as `doc.tables` already lists the body's control tables: a Google footer table gets
  a handle.

### The lock is kept, and does not stop an edit

`contentLocked` (Google writes it on these controls) asks the application not to let its user edit the control's
content; `sdtLocked` not to delete the control. oxoffice ignores `w:lock` in every edit today (filling, replacement,
run edits, the #657 block operations), and does here too:

- Honouring it here only would make one family of operations disagree with the rest; honouring it everywhere is its
  own spec (an opt-in check across every edit, and a reader for the lock, which the API lacks).
- Google locks the controls it wraps tables in: honouring it would refuse every edit of every table of a Google
  file, which is the failure this spec removes. What Google's lock protects in Google Docs is not known here.
- It is kept as written: no operation writes or clears `w:lock` but `set_content_control_lock`; editing, copying or
  moving the table leaves its control and lock in place; a copy of the whole control keeps its lock; a table copied
  out of the control is not locked.

### What each operation does at a table `[c, k]`

| Operation | Does |
| --- | --- |
| `table_element_xml`, `read_story_table`, `story_table_cell`, `cell_element_xml`, `story_table_cell_paragraph_location`, `measure_content` | reads that table, its cells (their stories exist already) |
| `try_replace_text_at` | replaces in that table only |
| `replace_table_xml`, `edit_story_table`, `replace_story_content`, `replace_cell_xml`, `clone_story_table_row`, `remove_story_table_row`, `delete_story_table_column` | rewrites that table in place, in its control |
| `clone_content`, `move_content` (source) | copies or moves the table with the bookmark markers it owns; whole complex fields; a move stays in its story; the only block of its control (Google's case) is not moved: the route is to move the control `[c]` |
| `remove_content_at` (Python `pop_content`) | removes it with its markers and returns it; the only block of its control is refused, naming `remove_content_control` (which removes the table with it) |
| `DocumentFragment::from_range` | from the table, or up to it (excluded), in its control |
| insert before: `insert_content`, `import_fragment`, `insert_html_fragment`, `insert_caption`, `insert_index`, `insert_table_of_figures`, `insert_table_of_authorities`, `clone_content` / `move_content` destination, `insert_content_control` (block, paragraph), `insert_bibliography`, `insert_building_block` | inside the control, before the bookmark starts that lead the table; a returned paragraph location is `[c, k']` |
| insert after | at the next block's location, or `control_end(story, c)` after the control's last child; `insert_picture_to_story(after = [c, k])` writes its paragraph right after the table |
| `insert_content_control` placement `row`, `cell` | a row control in that table; a cell control in its row `r`, `[c, k, r]` |

The #657 rules hold as they are (repeating sections, typed controls, a container keeps a block, a cell keeps a
paragraph). Where a table may be written does not change: a table fragment went to `[c, k]` and `control_end`
already.

### Rust

- `ContentLocation::new`, `control_end` and the type's docs say what `[c, k]` names by kind; `Document::paragraph`'s
  doc ("skips body tables") holds for tables in controls.
- `Document::story_tables(story)` (above), and `Document::table_story_location(table_index)`: entry `table_index` of
  `story_tables(body)`, the index `Document::table` takes; `None` past the tables, or when the model counts another
  number of body tables than the source lists (a debug assertion).

### Python

- The location converter takes a `Table` handle in a block control (`table_story_location`); the
  `TABLE_IN_CONTROL` refusal goes. `find_content_index` of such a table names the route of every block that is no
  direct body child (`... has no body index: pass the paragraph or table itself`).
- `TableLocation::Body(index).native` reads the table's location, and `Cell.add_picture`'s direct lookup reads the
  cell story through it: every `Table` and `Cell` method that needs the table's location works for a table in a
  block control (`xml`, `replace_xml`, `add_row`, the cell's `xml`...). The handle's owner is registered at its
  `[c, k]`, as a paragraph handle's is at its `[c, k]`. The owner index reads the locations of all body tables in
  one `story_tables` call, as it reads `paragraph_story_locations`: no body scan per handle.
- `HeaderFooter.tables`, `Note.tables` and `Cell.tables` list through `story_tables`; their handles are related
  table handles at `[i]` or `[c, k]`.
- `doc.paragraphs` and `Cell.paragraphs` follow the model walk (above). Docstrings and stubs: `ContentLocation` says
  `(c, k)` with item kind `"table"`; `tables` says it lists the tables of block controls too.

### CLI

- No command takes a location; the JSON that prints `item_kind` and `index_path` (`text --json`,
  `comments list --json`) lists no table (one in a control would read `{"item_kind": "table", "index_path": [c, k]}`).
- `diff`: the `tables` stream reads every body table `Document::tables` lists, tables in block controls included,
  and the `body` stream no longer holds their cells (today the reverse). `inspect` counts paragraphs and styles from
  the walk: a document holding such a table counts fewer paragraphs, as with a direct table.
- The limitation leaves `oxoffice limitations`.

### Refusals

| Case | Message names |
| --- | --- |
| `[c, n]` of kind table past the control's tables | `control_end(story, c)` |
| `[t, r]` of kind table in a block or element operation, `t` a table | `[t]`, the table itself: a row is no block |
| `[c, k]` of another kind than paragraph or table | the item itself: a nested control is `[i]` |
| `[s, k]` of kind table, `s` a repeating section | a table of an item, `[item, k]`, or `control_end(story, item)` |
| placement `cell` at `[c, k]` (no row) | `[c, k, r]` |
| the only block of a control moved or removed (#657 rule, Google's table) | `remove_content_control`, or the control `[c]` |

The limitation file `docx-block-control-table-location.json` and the messages naming #660 (resolver, Python) go.

### Deliberately not

- A story item for the table: `story_items` stays as it is.
- Honouring `w:lock`, and a reader for it (above).
- Tables inside `w:customXml`, as #657 leaves its paragraphs; tables of comment and text box stories get locations but
  no Python listing (those stories list no tables today).

## Plan

One pull request, `feat(docx): a table written in a block content control has a location`, closing #660, in three
commits after this spec:

1. **Locations (Rust).** `scan_story_control_blocks`; the resolver, `story_item_span` and the scoped replacement on
   it; the placements; the refusals and docs; `docs/ARCHITECTURE.md` (`[c, k]` by kind); the limitation file
   removed.
2. **Listings (Rust, CLI).** The paragraph walks; `story_tables` and `table_story_location`; the CLI `diff` streams.
3. **Python.** The converter, `TableLocation::Body.native`, `Cell.add_picture`, the owner index, the `tables`
   listings, the `find_content_index` route, stubs and docstrings.

Tests, lean, on the shape Google writes (`contentLocked`, `goog_rdk` tag, a table alone in the control):

- **The #657 matrix** (`tests/docx/block_locations.rs`). The fixture gains a Google control holding a table `Boxed`
  at the body's end. The story reader lists a table outside cells as one entry among the paragraphs, so a copy, move
  or removal of a table shows: the six body rows after the fixture's four cell tables move by four, and the body's
  end gets its new neighbour. Row `t:g/0` (a table alone in a locked control): every operation works, but a move or
  removal, refused with the only-block route. Row `t:box/0` becomes "`[c, n]` of kind table past the control's
  tables", refused naming `control_end(story, `. The limitation's workaround test goes.
- **One table-driven test of the element operations** on a control holding a paragraph `Under` and a table `Boxed`,
  at table `[c, 0]`, where paragraph `[c, 0]` exists, so a reader counting paragraphs fails it: each call reads or
  changes `Boxed` and leaves `Under`, and the document saves and reopens; with `measure_content` of paragraph
  `[c, 0]` and the `row` and `cell` placements.
- **One test of the listings**, the Google shape in the body, in a body table's cell and in a footer:
  `Document::paragraphs` and a cell's paragraphs equal their story's paragraphs; `story_tables` of the body, the cell
  and the footer, and `table_story_location`; a scoped replacement in each table; the control keeps its lock
  through an edit and a save.
- **Python** (`test_docx_block_locations.py`): the two #660 rows of `test_a_refusal_names_the_route` go; one test on
  the Google shape of what the binding owns: a `for p in doc.paragraphs: p.replace_text(...)` loop then a loop over
  the tables' cells fill every placeholder once; the table handle as source and destination of `clone_content`,
  `move_content`, `pop_content`, `insert_content`, the same as `ContentLocation(story, "table", (c, 0))`; its `xml`,
  `replace_xml`, `add_row` and a cell's `xml`; `footer.tables` and an edit through it.

Checks: `cargo fmt`; clippy on `oxoffice-docx-oxml`, `oxoffice-docx`, `oxoffice-docx-cli` and `oxoffice-docx-py` (all
targets, all features, warnings denied, and with `--profile scaling`); `cargo test` of `oxoffice-docx-oxml`,
`oxoffice-docx` and `oxoffice-docx-cli` (the walk change reaches every module that lists paragraphs); the writings
sample (`--test docx equivalent_writings`), since readers change; `maturin develop` and `pytest -n 8` on the docx
Python tests (paragraphs, tables, block locations, content controls).

## Open questions

1. **The shape**, `[c, k]` of kind table (option C). Recommended over a new constructor: the kind already
   disambiguates, and Python spells it today.
2. **`[t, r]` of the cell placement** keeps meaning row `r` of table `[t]`; `[c, k, r]` is added for a row of a table
   in a control. Recommended: one rule (a row is its table's path and the row index), told apart from `[c, k]` by the
   kind of item `[0]`.
3. **One element reader for `[c, k]` of both kinds.** Recommended: the table shape has to be read there anyway, and it
   brings `measure_content` to control paragraphs, a refusal of the same class today.
4. **Cell paragraphs leave `doc.paragraphs`** (a behaviour change, indexes move) rather than being routed to their
   cell story. Recommended: it is the documented rule, the body story's list, and python-docx's (a table's cells are
   no body paragraphs); routing lists them twice. No `!` in the title (no signature changes), but said in the pull
   request and the release highlights.
5. **The lock is ignored and kept.** Recommended (above); honouring locks would be its own spec, opt-in.
6. **The Google table alone in its control cannot be moved or removed by its own location** (#657: a control keeps a
   block; the route is the control `[c]` or `remove_content_control`). Recommended: keep; an empty `w:sdtContent` is
   still not written. Python `pop_content(table)` on a Google table raises with that route.
7. **The matrix reader lists every table outside cells**, renumbering six rows. Recommended over listing marked
   tables only: it reads a story's blocks as they are.
8. **Samples.** No corpus file holds the shape, and the real files are not ours to commit. Recommended: the tests
   write the shape as Google does; a Google Docs file made for the corpus later.
9. **Severity.** #660 is labelled S3 (rare); Google writes the shape on ordinary files and a paragraph loop stops on
   it. Recommended: S2.

## Outcome

Shipped as planned in Rust and the CLI. Python differs in three points, left in #660:

- A handle of a table in a block control keeps no owner record: the owner inventory lists story items only. It reads
  its location at each call (`table_story_location`, one body scan), and its edits publish as `Conservative`,
  retiring the other handles as `Document.insert_content` does. No batched `story_tables` read in the owner index.
- `table_story_location` returns `None` when the model and the source count other numbers of body tables, without a
  debug assertion.
- `Cell.tables` is unchanged: its owner inventory cache lists story items, so a table in a block control of a cell is
  reached through `story_tables` or its location.

## Links

- Issue #660; #657 and its spec
  [One block location resolver](2026-11-25-block-locations-inside-content-controls.md), which added the limitation.
- [Word binding: body handles follow their element](2026-11-24-body-handles-follow-their-element.md).
