---
date: 2026-11-25
status: accepted
scope: perf, docx
---

# Edit cost: references found by a walk of what the body writes

Part of [staged edits run in place](2026-11-24-staged-edits-in-place.md)
(decision 1 of [edit cost at the root](2026-11-21-edit-cost-at-the-root.md)).
It serves `Document.remove_style` (this spec's pull request) and the S1c
removals `remove_footnote`, `remove_endnote`, `FootnoteEndnote.delete` and
`remove_comment` (S1c-2, built on it).

## Context

Some calls must know whether, or where, the body names an id: a style, a
note, a comment. The typed model holds most references, but not all: the
model keeps what it does not model as raw XML fragments, written verbatim
(an `mc:AlternateContent` text box in a run, a tracked change, a raw block,
an unmodelled child of a paragraph, run, table or content control). Today
these calls serialize the whole main part and scan it:

- `Document::remove_style` (`document_references_style`): a typed walk
  plus `self.document.to_xml()` scanned with an `NsReader`. The
  serialization keeps the call in `GROWING`.
- `remove_note` (`remove_note_references`) and the comment reconcile scan
  the serialized main part too.

No existing walker reaches every raw fragment the writer writes
(`content_numbering_instance_ids`, `visit_body_paragraphs`,
`walk_tracked_paragraph`, `WordOwner::resource_children` each miss some):
see `handoff/perfrun/written-fragment-walk.md`.

## Decision

### 1. One oxml walk of what the writer writes

`oxoffice-docx-oxml` gains a visitor over a body block, hidden from the
public docs:

```rust
pub trait WrittenVisitor {
    /// Bytes the writer writes verbatim (or with rebound prefixes).
    fn raw(&mut self, xml: &[u8]) {}
    fn paragraph(&mut self, paragraph: &CT_P) {}
    fn paragraph_properties(&mut self, properties: &CT_PPr) {}
    fn run(&mut self, run: &CT_R) {}
    fn run_properties(&mut self, properties: &CT_RPr) {}
    fn table_properties(&mut self, properties: &CT_TblPr) {}
}

impl BodyContent { pub fn visit_written(&self, visitor: &mut dyn WrittenVisitor); }
impl CT_SectPr   { pub fn visit_written(&self, visitor: &mut dyn WrittenVisitor); }
impl CT_Document { pub fn visit_written_outside_body(&self, visitor: &mut dyn WrittenVisitor); }
```

- The walk mirrors the writer: a part written from raw bytes is handed to
  `raw` and not walked as typed content (a revision's `retained_xml()`, a
  drawing's `raw_xml`, a field's parsed source), a part written from the
  typed model is walked typed. An `extra_xml` slot that the writer replaces
  by a typed owner (an equation, a revision, a hyperlink kept as one
  record) is not handed to `raw`; its owner is walked instead. Records that
  hold an owner's own attributes (`ROOT_ATTRIBUTES_POSITION`, a run's root
  attribute flag) are skipped.
- It reaches, typed or raw: paragraphs and their properties (the paragraph
  mark's run properties, the numbering, section and revision children),
  runs and their properties, field result runs, ruby text and ruby runs,
  hyperlinks, equations (re-serialized alone: an equation is small),
  drawings and the text boxes of typed drawings, tables, rows, cells and
  their properties, content controls at every depth with their private
  property and root slots (`w:sdtEndPr` among them), and the three lists of
  content controls kept beside paragraphs, tables and rows.
- The caller iterates `body.content` itself: the index of the block is the
  slot S1c-2 swaps under `EditUnit::BodyBlocks`.
- Two owners are written from bytes the model does not store: an equation
  (re-serialized alone, it is small) and a paragraph or an inline content
  control holding a field the writer rewrites (a changed field, a field
  detached from the run it shared with text): the walk serializes that
  owner alone and hands its bytes to `raw`. An unchanged field hands its
  stored source. Otherwise the walk allocates nothing; its cost is a walk
  of the model, linear in the body, with no serialization of the part.

### 2. Prefix-agnostic searches in raw fragments

A raw fragment may use an inherited `w:` prefix or another prefix for the
WordprocessingML namespace, so the searches read local names, not prefixes
(as `raw_numbering_instance_ids` does): an element start whose local name
is the target, then the attribute whose local name is `val` (styles) or
`id` (notes, comments) before the tag's end, its value unescaped. A false
positive is an element of another namespace with that local name, which no
producer writes inside WordprocessingML content; for a style it would only
keep the style.

### 3. The queries

In `oxoffice-docx`, crate-private:

- `body_references_style(&Document, id) -> bool`: typed `pStyle`, `rStyle`,
  `tblStyle` ids, plus the search in raw fragments; over the body blocks,
  the body's section properties and the document's background. It replaces
  the typed walk and the serialization of `document_references_style`.
  The typed ids today's walk misses come with it: the paragraph mark's
  `rStyle`, ruby runs, field result runs, the content controls of rows,
  the run properties of a content control.
- `body_slots_referencing_note(kind, id) -> Vec<SlotHit>` and
  `body_slots_referencing_comments(&HashSet<i32>) -> Vec<SlotHit>`
  (`commentRangeStart`, `commentRangeEnd`, `commentReference`), with
  `SlotHit { slot: usize, raw: bool }` sorted by slot, one per slot, `raw`
  set when a hit sits in a raw fragment (the caller then re-serializes that
  block alone; a typed-only block is edited typed). Body only: the section
  properties and the background hold no note or comment reference.

### 4. Debug oracle

Debug builds check every answer against today's route: the serialization
of the main part scanned with a namespace-aware reader (for the slot
queries: each block serialized alone and scanned). A difference panics.
It runs over the Python debug suite and `cargo test -p oxoffice-docx`,
with fixtures holding a reference in a tracked insertion, in a text box in
a run's `extra_xml`, in a raw body block, in a field's result, in a ruby
run, in a nested content control's run properties and in a table cell.

### 5. `remove_style` runs in place

`Document::remove_style` runs on the live document instead of a staged
copy, the shape of `link_style_to_numbering` (#633): the numbering journal
(`edit_numbering`) undoes a released numbering level and the reservations,
the styles model is put back on a refusal or a panic. The numbering part
is no longer flushed: releasing a level's `w:pStyle` changes no content
reference, so the numbering model check (`validate_numbering_model`)
replaces the whole graph check, and the save writes the model. The debug
oracle runs the staged route on a copy and compares.

## What we do not do

- No index of references kept up to date across edits: the walk stays
  linear in the body, without serialization. An index would have to follow
  every edit route; the walk follows the writer only.
- No walk of headers, footers, notes or comments: those parts are small
  and keep their byte scans.
- No mutable visitor: S1c-2 edits the blocks it is told about.

## Plan

One pull request: the visitor (oxml), the three queries and their oracle
(docx), `remove_style` in place. `Document.remove_style` leaves `GROWING`
(102 -> 101 at the time of writing). The walk stays linear in the body:
the call passes the scaling gate because a walk of the model costs far less
than the gate's floor, not because it is flat. The pull request gives the
steady cost of `remove_style` and of the slot queries at 400 and 4,000
paragraphs, so the slope left (microseconds per paragraph, no
serialization) is visible. Steady table at 25/100/400 paragraphs
(main -> branch), Python behaviour tests (a style named only in a raw
fragment or a tracked change is refused and the document is unchanged; a
linked pair and a released numbering level still go), a mutation check of
the raw search. S1c-2 then uses the slot queries.

## Links

- Issues: #466, #6.
- Triage: `handoff/perfrun/written-fragment-walk.md` (local).
