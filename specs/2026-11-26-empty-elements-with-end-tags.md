---
date: 2026-11-26
status: accepted
scope: core, docx, pptx, tests
---

# An element with nothing inside reads the same in both writings

## Context

An element with nothing inside is written `<w:b/>` by most producers and
`<w:b></w:b>` by others (LibreOffice's `w:fldChar`, #413; any generator that
serializes empty elements with an end tag). quick-xml delivers the first as
one `Event::Empty`, the second as an `Event::Start` and an `Event::End`. The
readers model leaves on `Event::Empty` alone (1,335 `Event::Empty` matches in
the seven crates of section 1, at 9997d2e7), so the second writing is kept
raw or skipped: a tab or a break leaves the text (`oxoffice-docx`
`document/story_paragraphs.rs:332-347`), bold and italic are lost
(`oxoffice-docx-oxml` `run_properties.rs`: its `Start` arm models only `rtl`
and `lang`). The `empty-with-end-tag` writing of #658 finds 143 differences
on 45 inputs, listed as eight `*.docx` entries of
`tests/corpus/writings-known.toml`, checked by the full checks only. The
opposite writing, `empty-self-closed`, has no known entry: the `Empty` arms
read the corpus's own `<x></x>` elements, self-closed, as the `Start` arms
read them, so the switch should leave the originals' results unchanged.

A prototype (`wip/658b-collapse-empty-elements`, 6d04a8bd) wraps quick-xml's
readers in `oxoffice_core::xml_reader::{Reader, NsReader}`, which deliver
`<x></x>` as one `Event::Empty`, the reader after the end tag, and points 115
files of six crates at them. It costs nothing measurable and breaks 134
tests, for two reasons:

1. Source surgery assumes an empty element's bytes end with `/>`: the
   content fragment closure (`docx-oxml` `document/source/fragments.rs`)
   inserts declarations at `after - 2` and writes `</w: xmlns:...p>`.
   Measured at 9997d2e7: 96 lines in 51 files (appendix).
2. Writers lose the writing. `raw_xml::capture_empty_element(e)` writes
   `<x/>` from the start tag alone, so an unmodelled `<x></x>` comes back
   `<x/>`; and the `Start` arms that keep a writing on purpose go dead: the
   carriers of `<w:rtl></w:rtl>`, `<w:lang></w:lang>` and the paragraph
   toggles (`run_properties.rs`, `paragraph_properties.rs`), the empty
   `w:t` that holds an empty note mark (`text/run.rs`), the raw
   `<w:lastRenderedPageBreak></w:lastRenderedPageBreak>`.

Also measured: 276 `capture_empty_element` calls in those crates, 271 with
the reader in scope (the other five in `drawing` `text/paragraph.rs`); 119
files there name quick-xml's `Reader` or `NsReader`.

## Decision

### 1. The reader collapses, in one place

`oxoffice_core::xml_reader` (the prototype's module) holds `Reader` and
`NsReader`: quick-xml's readers, but an element whose start tag is followed
at once by its own end tag (`</name`, optional whitespace, `>`) is delivered
as one `Event::Empty`, the reader positioned after the end tag. Nothing else
changes. An element holding whitespace, a comment, a processing instruction,
a reference or CDATA is not collapsed (whitespace is content in `w:t` and
`a:t`, and no writing of the harness puts it in an empty element).
`expand_empty_elements` turns the collapse off. The end tag is read through
quick-xml, its name checks apply, and the `NsReader` closes the element's
namespace scope at the next read, as for an empty-element tag. Each derefs to
its quick-xml reader.

Every reader of a document part goes through it, in the seven crates that
read them: `oxoffice-chart`, `-docx`, `-docx-layout` (it reads the model's
raw bookmarks), `-docx-oxml`, `-drawing`, `-pptx`, `-pptx-oxml`. Typed models
and the facades' scanners alike: the scanners classify leaves too
(`story_paragraphs`), and fixing 1,335 arms one by one does not fix the
class.

Reading verbatim stays, and is named: `raw_xml::capture_element` (an
unmodelled subtree as written), `xml_element` (where tags are),
`Reader::verbatim()` (quick-xml's reader, for a loop that copies events and
models none). Not wrapped: `oxoffice-opc` and `oxoffice-sml` (package parts
and spreadsheets, outside the harness), the document properties in
`oxoffice-core`, `oxoffice-xml-test` (its canonical form stays independent
of the library).

### 2. The writing travels with the reader

`xml_reader::EmptyWriting { SelfClosed, EndTag }`, and `empty_writing()` on
both readers: how the last `Event::Empty` read was written, valid until the
next read, `SelfClosed` after any other event. quick-xml's `BytesStart`
cannot carry it, and the code that needs it holds the reader (271 of 276
capture calls).

In `oxoffice_core::raw_xml`:

- `capture_empty(e: &BytesStart, writing: EmptyWriting) -> Result<Vec<u8>>`
  writes `<x .../>` or `<x ...></x>`. It replaces `capture_empty_element`,
  which is removed so that the compiler lists every caller; a caller that
  builds its own element passes `SelfClosed`.
- `write_empty(writer, e, writing)`, for a loop that writes back an
  `Event::Empty` it read.

### 3. Positions come from `xml_element`

Source surgery takes where a tag ends from the helper of #658, never from
`/>`:

- `xml_element::start_tag(xml) -> Result<StartTag>` reads the start tag (or
  empty-element tag) `xml` starts with, one event: `name`, `range`,
  `attributes_end` (before its `/>` or its `>`: where an attribute or a
  namespace declaration goes) and `self_closing`. The only code that knows
  "two bytes or one".
- `ElementSpan::attributes_end()`, the same on a span.
- `start_tag`, `element_span`, `find_element`, `root_element_span`,
  `replace_content` and `append_content` take bytes as well as text
  (`&(impl AsRef<[u8]> + ?Sized)`, the last two returning the same kind they
  are given): most sites hold bytes (the chart's `authoring.rs` opens an
  element for content on bytes), `&str` callers are unchanged.

The 96 lines are of four kinds:

| Kind | Today | Becomes |
| --- | --- | --- |
| Has it content? | `xml[..after].ends_with(b"/>")`, `get(end - 2) == Some(&b'/')` | the event kind (`Event::Empty`); `start_tag(..).self_closing` for the writing |
| Insert in the tag | `after - if empty { 2 } else { 1 }`, `end - 2` | `start_tag(..).attributes_end` |
| Open it for content | `xml[..end - 2]` + `>` + content + `</x>` | `replace_content`, `append_content` |
| Rewrite the start tag | new head + `if empty { "/>" } else { ">" }` + the bytes after the event | new head + `&xml[tag.attributes_end..]`: the tag's own close, content and end tag as written |

### 4. The writers' rule

- **Unmodelled XML keeps its writing.** `<x></x>` captured with
  `capture_empty(e, reader.empty_writing())` is written back `<x></x>`; a
  subtree captured by `capture_element` is read verbatim already.
- **A modelled element is written as the model writes it**, whatever its
  input writing, as prefixes, quotes and attribute order already are (fixed
  on write): `<w:b></w:b>` reads as bold and is written `<w:b/>` when its run
  is written again. A part a save keeps byte for byte is not touched; edited
  parts are held to the canonical form, which does not see the difference.
- **What the model keeps on purpose today stays.** An `Empty` arm takes over
  what its element's `Start` arm did for the element with nothing inside,
  with the writing from the reader: the rtl, lang and paragraph-toggle
  carriers record `<x></x>` when `EndTag`; `<w:t></w:t>` and
  `<w:delText></w:delText>` read as an empty text, an empty note mark
  included, and so does `<w:t/>` (raw until now);
  `<w:lastRenderedPageBreak></w:lastRenderedPageBreak>` stays raw in its
  writing. The model's own end-tag forms (an empty `w:t`, the plain-text
  control shell) are still written so.
- **Source surgery copies source bytes**; a start tag it rewrites keeps the
  element's close as written (section 3).
- **A loop that writes back an `Event::Empty` it read keeps the writing**
  (`write_empty`, or `capture_empty` where it used `capture_event`), or
  reads verbatim when it models nothing.

A test that asserts the writing of unmodelled XML passes unchanged. A test
that asserts `<x></x>` reads otherwise than `<x/>` asserts the bug: it
changes, named in the pull request.

### 5. Two checks

In `crates/oxoffice-core/tests/xml_element.rs`, beside the end-tag search
check of #658 and the raw attribute check of #668, built the same way: code
outside `#[cfg(test)]` items, an exact count per file with a reason, a
self-test on sample code.

- **No `/>` assumption** in `crates/*/src`: a two-character literal `"/>"`
  or `b"/>"`; a byte compared with `b'/'` at an offset (`tag[end - 1] ==
  b'/'`, `get(end - 2) == Some(&b'/')`); `if empty { 2 } else { 1 }` or
  `Event::Empty(_) => 2`; two taken from a tag's end (`after - 2`,
  `head_end - 2`, `buffer_position() as usize - 2`, their `saturating_sub(2)`
  or `checked_sub(2)`, `truncate(x.len() - 2)`, `[..x.len() - 2]`).
  `SELF_CLOSING: [(file, count, reason)]` allows the helper and the verbatim
  capture (`xml_element.rs`, `raw_xml.rs`), writers of elements they build
  (`writes`), HTML, EPUB, SVG and RTF (`not a document part`), stack
  arithmetic (`not a tag`), and the crates outside section 1 (`verbatim
  reader`). These patterns find 129 lines in 64 files at 9997d2e7: the 96
  to move and 33 to allow.
- **No quick-xml reader** in the seven crates of section 1: no
  `quick_xml::Reader`, `quick_xml::NsReader`, `quick_xml::reader::` path or
  `Reader`/`NsReader` in a `use quick_xml::{..}`, so that a new reader cannot
  miss the collapse. Verbatim reads name `Reader::verbatim` or
  `capture_element`. The allow-list is expected empty; an entry says why its
  code must see `Start` and `End` for an empty element.

### 6. Not done

Whitespace-only content (section 1); the input writing of a modelled element
through an edit (fixed on write); package parts, spreadsheets and document
properties (no writing of the harness reaches them); a check over echo loops
(a read `Event::Empty` cannot be told from a built one by its text: the
review and the round-trip tests cover them).

## Plan

**PR 1, the helpers and the sites, no behaviour change.**

1. `oxoffice-core`: `xml_reader` (the prototype, plus `EmptyWriting` and
   `empty_writing`), `raw_xml::capture_empty` and `write_empty` (beside
   `capture_empty_element`, still used), `xml_element::start_tag`,
   `attributes_end` and byte inputs; the `/>` check, every line of the
   appendix in its allow-list.
2. One commit per crate, in dependency order (`docx-oxml`, `drawing`,
   `chart`, `pptx-oxml`, `docx`, `docx-layout`, `pptx`): its sites on
   `xml_element`, its entries out of the allow-list.

Checked by the helpers' tests and every existing test, unchanged: no reader
collapses yet.

**PR 2, the switch.**

1. The seven crates read through `xml_reader` (the prototype's import
   change, plus `docx-layout`); the reader check.
2. `capture_empty_element` removed, its 276-277 callers on
   `capture_empty(e, reader.empty_writing())` (the five of
   `drawing/text/paragraph.rs` take the writing as a parameter); echo loops
   and `capture_event` of a read `Event::Empty` per section 4.
3. The `Start` arms of section 4 folded into their `Empty` arms, found by
   grep (`*_is_explicitly_empty`, `read_text` of an element that can be
   empty, `*_has_payload`) and by the prototype's 134 failures.
4. The behaviour tests, the eight known entries removed, the perf check.
5. `docs/ARCHITECTURE.md`, model invariants: every reader of a document
   part goes through `oxoffice_core::xml_reader`, and `raw_xml::capture_empty`
   carries the writing of an empty element, beside `capture_element`.

One commit per crate for 1 and 2 (mechanical), one per family for 3.

### Tests

- Core: `<x></x>` and `<p:x a="1"></p:x >` read as one `Event::Empty` with
  `EndTag`, the reader after the end tag, by `read_event` and
  `read_event_into`; `<x/>` with `SelfClosed`; not collapsed: `<x> </x>`,
  `<x><!--c--></x>`, `<x>&amp;</x>`, `<x><![CDATA[]]></x>`, `<x><y/></x>`,
  anything under `expand_empty_elements`; `<a:x xmlns:a="urn:x"></a:x><a:y/>`
  resolves `x`, not `y`; `capture_empty`, `write_empty` give back each
  writing; `start_tag` of `<x/>`, `<x />`, `<x a='1'\n/>`, `<x>`,
  `<x a="1" >`, `<x></x>`, from text and bytes, after a byte order mark; the
  two checks and their self-tests.
- Word, each in both writings with the same result: run toggles (`w:b`,
  `w:i`, `w:strike`, `w:vanish`, `w:rtl`: the formatting read, the
  Markdown); `w:tab`, `w:br`, `w:cr`, `w:noBreakHyphen`, `w:ptab` in the
  text of the body, a header and a footnote (paragraph text, story
  paragraphs, Markdown); `<w:pStyle w:val="Heading1"></w:pStyle>`,
  `w:keepNext`, `w:pageBreakBefore`, `w:bidi`, a list item's `w:ilvl` and
  `w:numId`; `<w:t></w:t>` and `<w:t/>` after a custom-mark footnote
  reference and in a plain run; an empty `<w:p></w:p>` and
  `<w:sdtContent></w:sdtContent>` inserted, copied and moved where they need
  declarations (the result parses, declarations in the start tag).
- Word, the writing kept: `<x:k xmlns:x="urn:oxoffice:test"></x:k>` as a run
  child, in `w:rPr`, in `w:pPr` and between blocks, and
  `<w:lastRenderedPageBreak></w:lastRenderedPageBreak>`: byte for byte by a
  save without an edit and by an edit of another paragraph, in its writing
  when its own paragraph's text is replaced.
- Decks: `<a:br></a:br>` (a line break in the text), `<p:ph
  type="title"></p:ph>` (the title placeholder found), `<a:buNone></a:buNone>`
  (no bullet in the Markdown); `<x:k></x:k>` in a shape's `p:nvPr` kept
  across a text replacement in that shape.
- The writings harness: the eight entries removed. An entry that does not
  happen fails, so the full checks (`full-checks.yml` dispatched on the
  branch) prove the class gone; `empty-self-closed` stays without entries.
- Perf: open, text, Markdown and a save without an edit of the eight
  `long-*` copies, the Rust facades built in the `scaling` profile, ten runs
  each, main against the branch: no copy more than 3 % slower by its median,
  the numbers in the pull request. The QUICK scaling set runs as usual.

## Decided with the maintainer

1. `raw_xml::capture_empty_element`, public in the published `oxoffice-core`,
   is removed (not deprecated): the facades are the API and `oxoffice-core`
   their plumbing, the crate is at 0.x, and a deprecated function that loses
   the writing would stay callable. The pull request that removes it is
   marked breaking (`!` and a `BREAKING CHANGE:` footer).

## Links

- #664, #658, #413, #656, #668
- [Equivalent XML writings give the same result](2026-11-25-equivalent-xml-writings.md)
- Prototype: `wip/658b-collapse-empty-elements` (6d04a8bd)

## Appendix: the `/>` sites at 9997d2e7

Lines found by the patterns of the first check, outside `#[cfg(test)]`
items, in the seven crates of section 1, by file (`src/` left out).

- `oxoffice-chart` (2): `authoring.rs` 1385, 1463.
- `oxoffice-docx-oxml` (21): `content_control.rs` 906, 918, 1792, 1794;
  `document/source/fragments.rs` 62; `glossary.rs` 1164, 1350, 1598;
  `header_footer.rs` 575; `settings.rs` 2911, 2912; `text/bindings.rs` 1509,
  2002, 2011, 2123; `text/field_complex.rs` 359, 361, 3231;
  `text/field_legacy_form.rs` 1767; `text/field_rewrite.rs` 1342, 2232.
- `oxoffice-docx` (41): `bibliography.rs` 795, 1190;
  `bibliography/paragraph_properties.rs` 370; `bibliography/source.rs` 24,
  631, 758; `comments.rs` 3161, 4031, 4047, 4084, 4137;
  `comparison/controls.rs` 1216; `comparison/leaf_publications.rs` 657; `comparison/paragraphs.rs` 668;
  `comparison/text_boxes.rs` 573; `comparison/xml.rs` 60;
  `content_control/element_index.rs` 199; `content_control/namespaces.rs`
  270; `document/block_locations.rs` 257; `document/edited_paragraph.rs`
  381; `document/fragment_checks.rs` 84; `document/namespaces.rs` 771;
  `document/story_scopes.rs` 203, 205; `document/story_source_edits.rs` 757;
  `document/story_text.rs` 165, 1344, 1347; `field/bibliography.rs` 152,
  435; `field/merge_package.rs` 1953, 1963; `field/ref_copy.rs` 212;
  `field/scan.rs` 139, 140, 1743, 1744; `field/story_sources.rs` 565;
  `field/toc.rs` 2794; `revision.rs` 4334, 4427.
- `oxoffice-docx-layout` (1): `engine/cache_accounting.rs` 115.
- `oxoffice-drawing` (3): `text/body.rs` 626; `text/paragraph.rs` 4698,
  4708.
- `oxoffice-pptx-oxml` (13): `comments.rs` 1867, 2009; `namespace.rs` 352,
  353, 391; `picture.rs` 1436; `presentation.rs` 770, 2949; `shape_tree.rs`
  3949; `slide_parts.rs` 534, 1177; `timing.rs` 4218, 4227.
- `oxoffice-pptx` (15): `hyperlinks.rs` 731, 894; `import_notes.rs` 692,
  714, 1319; `import_tables.rs` 333, 696, 701, 713; `masters.rs` 99;
  `properties.rs` 499, 1055; `themes.rs` 521, 913, 1240.

Allowed (33): `oxoffice-core` `raw_xml.rs` 172 and `xml_element.rs` 169;
elements built, not read (`writes`): `docx-oxml` `math.rs` 4117, 4256 and
`text/bindings.rs` 884, `docx` `comparison/fields.rs` 935, `pptx-oxml`
`picture.rs` 180 and `comments.rs` 549, 567; `docx` `html.rs` 2082,
`epub.rs` 3263, 3264, `rtf.rs` 2110, `svg.rs` 242, 483 and
`document/story_text.rs` 239 (a stack); `oxoffice-opc` 5, `oxoffice-sml` 6,
`oxoffice-xml-test` 4, `oxoffice-render-test` 1, `oxoffice-media` 1. A line
of the list above found to write an element it builds moves here.
