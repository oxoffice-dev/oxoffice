---
date: 2026-11-20
status: accepted
scope: docx, pptx, cli, python
---

# Skill helpers move into the library

## Context

The Word and PowerPoint skills (oxoffice-skills) ship two helper scripts,
`docx_ops.py` (26 KB) and `pptx_ops.py` (9 KB). Agents call them for the
operations the library does not offer in one safe call: saving without
overwriting the input, counted replacements, comments anchored on a text,
reading every story with its part and style. That code lives outside the
library, so it is not tested by its CI and not available from Rust or the
CLI.

The skills should be documentation and examples only. Whatever an agent needs
belongs in the library, Rust first, then the CLI and Python.

## Decision

Each helper gets one of three fates. The skills wait for this work before
their first release, and then drop both scripts.

| Helper | Becomes |
| --- | --- |
| `save_atomic(doc, out, src)` (docx, pptx) | `save` writes atomically (temporary file, sync, rename, mode kept) and refuses to overwrite the file the document was opened from unless asked. Rust, Python. |
| `replace_batch(src, out, edits)` (docx) | `Document::replace_all(edits)` with an expected count per pair, all or nothing, and a check that no replaced text is left. Rust, Python. The CLI's `replace --map` uses it. |
| `replace_batch` (pptx) | Same call on `Presentation`, the CLI already has `replace --map --expect`. |
| `comment_on_text`, `commented` (docx) | `add_comment_on_text` gains `occurrence`, `in_tables` and a default date of now, and checks the anchor before it commits. CLI `docx comment add --text`. |
| `story_paragraphs`, `all_text` (docx) | `Document::story_paragraphs()` returns story kind, part name, style and text for every paragraph of every story. CLI `docx text --json` gains part and style. |
| `count(source, text)` (docx) | `Document::count_text(text)`, per story kind. CLI `docx count`. |
| `locate`, `isolate` (docx) | `Paragraph::find_text(text, occurrence)` returns a span, and `Document::isolate_span(span)` splits the runs so the span is whole runs that can be formatted. |
| `strip_generated_bookmarks` (docx) | Not an API: the comparison learns to ignore renumbered `_Toc` bookmarks, which closes the matching limitation. |
| `walk(shapes)` (pptx) | `Shapes::walk()` (recursive, groups included). |
| `overflowing(path)` (pptx) | Nothing new: `pptx fit` already covers it, the skills document it. |
| `textmap(path)` (docx) | Nothing new: `docx text --json` is the call, the skills document it. |
| `pages`, `rebuild_toc`, `move_thread_to_text`, `anchored_text`, `fix_template_content_type` | Dropped: the library already has `page_count`, `rebuild_toc`, `move_comment_to_text`, `Comment.anchor_text`, and saving fixes the template content type. |
| `now`, `solid_png`, `image_size`, `cli`, `EditError` | Dropped: short snippets in the recipes where an example needs them. |

## Plan

One pull request per row group, each with tests on the corpus:

1. Atomic save that refuses its input, Word and PowerPoint.
2. Counted replacements, Word and PowerPoint, and the CLI `--map` on top.
3. Comments on a text, with the CLI.
4. Story paragraphs with part and style, and counts, with the CLI.
5. Find and isolate a text span.
6. `Shapes::walk()`.
7. The skills drop both scripts and cite the new calls.

## Outcome

- **1. Atomic save** (#10): `save` on `Document` and `Presentation`
  remembers the file a document was opened from (`open`, `open_encrypted`,
  `open_flat_opc`) and refuses it with `Error::OverwritesSource`, compared by
  device and inode on Unix and by canonical path elsewhere, so a link, a `..`
  spelling or a hard link is refused too. `save_with_options(path,
  SaveOptions { overwrite_source: true })` replaces it; Python has
  `save(path, *, overwrite_source=False)`. Writing was already atomic with the
  mode kept. Only `save` refuses: `save_encrypted`, `save_as_package_class`
  and the conversions do not. The path is carried by staged clones and every
  reopen, checked after a mix of edits in the Rust and Python tests.
- Unoptimised builds overflow the 2 MiB stack of a test thread on some
  PowerPoint edits (already on `main`): `.cargo/config.toml` gives test
  threads 8 MiB, the size the CLI uses.
- **6. `Shapes::walk()`**: `SlideRef::shapes()` and `ShapeRef::children()`
  now return a named `Shapes` iterator (still an `ExactSizeIterator` of
  `ShapeRef`), and `Presentation::part_shapes(owner)` gives the same for a
  layout or master. `Shapes::walk()` visits the shapes left in it and those
  inside groups, depth first in z-order, each with its index path from where
  the walk started (`path.len() - 1` is the depth). From a slide's, layout's
  or master's own shapes, `part_shape_at` and `shape_mut_at` find every path
  again. It keeps its own stack, so nesting cannot overflow the thread's. An
  alternate-content block is listed, read-only as when iterating, but not
  entered: its fallback members cannot be edited at all (an edit failed with
  an index error), and the skills' helper entered groups only too. Python has `ShapeCollection.walk()`, a
  list of `(depth, shape)` with live handles, on slides, groups, layouts and
  masters. The CLI gains `oxoffice pptx shapes [--slide N] [--json]`, the flat
  listing the skills' `pptx_ops.py shapes` printed, with paths in JSON.
- **4. Story paragraphs and counts** (#18): `Document::story_paragraphs()` gives a
  `StoryParagraph` for every paragraph of every story: the body with its
  table cells, text boxes and block content controls, each header and footer
  part, the notes (separators left out) and the comments. Each one carries
  the kind of its part's story (`Body`, `Header`, `Footer`, `Footnote`,
  `Endnote`, `Comment`: a table cell or a text box takes the kind of the story
  around it), its location (owner story, part name, and the item path of
  `story_items`, which counts every item, not paragraphs), its direct style
  id and the text a reader sees in the accepted view. Parts come in
  `stories()` order, paragraphs in document order within a part.
  `Document::count_text(text)` counts by story kind, in a fixed order,
  without overlaps or crossing a paragraph end, and refuses an empty text.
  CLI `docx count FILE TEXT [--json]`, with the total and the total outside
  comments; `docx text --json` story items gain `style` (the part name and
  the body styles were already there). Python `Document.story_paragraphs`,
  `StoryParagraph`, `Document.count_text(text)` returning `{kind: count}`.
- Design moves: comments are listed (the helper left them out), as kind
  `comment`, so a caller that wants the replacement's reach filters them
  (`docx count` gives `outside comments`). The story item inventory lists a
  block content control, a table of contents for one, as one item with its
  paragraphs' texts joined; story paragraphs split it with the paragraph
  paths that comment ranges already use, so a TOC entry keeps its `TOC1`
  style. Style ids are the document's own, localised by Word (`Titre1`,
  `En-tte`). Part names are OPC names (`/word/header1.xml`), where the
  helper gave ZIP names.
- The text is read by a pass of its own over each paragraph, not by the
  story item reader: that reader keeps `w:smartTag` and `w:customXml` opaque
  and skips their text, which `Paragraph::text` and replacement read (found
  by the independent review). The pass reads every `w:t` but deleted and
  moved-away text, properties, the phonetic guide of a ruby (`w:rt`),
  `mc:Fallback` copies, text boxes and field codes (nested fields included),
  and matches `Paragraph::text` on every corpus file. It also reads what no
  other call reaches: a smart tag inside a tracked insertion, the base of a
  ruby Word writes in a run (which `Paragraph::text` leaves out), and the
  paragraphs of a block `w:customXml`, text boxes inside it included, which
  are listed with the element's path then their index. Counts see that
  text, so item 2 can refuse to leave it behind. Known gap: a text box in a
  run inside an inline smart tag or custom XML element is not listed (#78).
  The cost is linear in the document (measured ×3.8 per 4× paragraphs), and
  the scaling tests now time `story_paragraphs` and `count_text` against
  the linear bound.
- **5. Find and isolate a span**: `ParagraphRef::find_text(text,
  occurrence)` (and the same on `Paragraph`) returns a `TextSpan { start,
  end }` in the paragraph's literal run text, the text `split_run` offsets
  count: case-sensitive, without overlaps, in the accepted view, inline
  controls, hyperlinks and tracked insertions included. What
  `Paragraph::text` shows besides (a tab, a break, a field result, simple or
  complex, a drawing, a symbol) is not searched and a match never crosses
  it, nor a non-breaking hyphen or a picture, shape or object kept as raw
  XML; each stretch between two of these is searched on its own, so a
  refused match never hides an overlapping one. Smart tags and custom XML
  are not searched either; reference marks are not barriers.
  `Paragraph::isolate_span(span)` splits runs so that the span is whole runs
  and returns their index range, as `Paragraph::run` counts runs; each run
  keeps its formatting, its insertion or its control, the text does not
  change, and on error the paragraph is unchanged. A tab, a break or another
  text-free item at either edge of the span stays out of those runs, even
  when it starts or ends the run that holds the span's text (Word often
  starts a run with the tab before its text), so bolding the span or
  setting its text leaves it alone; `add_comment_on_text` shares the split
  and keeps such an item out of the comment range. Python:
  `Paragraph.find_text(text, occurrence=0)` returns a `TextSpan` (paragraph,
  start, end), and `Document.isolate_span(span)` returns `(first, last)` for
  a paragraph of the body, a table cell, a header, a footer, a note or a
  comment; a change stales the document's earlier handles, the paragraphs
  of other spans included, as every structural edit does.
- Design move: the plan had `Document::isolate_span(span)`. A Rust paragraph
  handle holds no place in the document, so a span is paragraph-relative and
  isolating it is a paragraph method (`doc.paragraph_mut(i)` or a cell's
  `paragraph_mut`); in Python, where edits go through the document's
  revision tracking, it stays `Document.isolate_span`, the span carrying its
  paragraph. No CLI: the CLI has no run formatting to apply to the runs.

## Links

- [ROADMAP.md](../ROADMAP.md), how the library is meant to be used.
- [2026-11-20-known-limitations-in-the-cli.md](2026-11-20-known-limitations-in-the-cli.md)
