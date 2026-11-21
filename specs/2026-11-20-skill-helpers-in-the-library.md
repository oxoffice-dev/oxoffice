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
  alternate-content block is listed but not entered: its fallback members
  cannot be edited, and everything the walk gives can be, as with the skills'
  helper, which entered groups only. Python has `ShapeCollection.walk()`, a
  list of `(depth, shape)` with live handles, on slides, groups, layouts and
  masters. The CLI gains `oxoffice pptx shapes [--slide N] [--json]`, the flat
  listing the skills' `pptx_ops.py shapes` printed, with paths in JSON.

## Links

- [ROADMAP.md](../ROADMAP.md), how the library is meant to be used.
- [2026-11-20-known-limitations-in-the-cli.md](2026-11-20-known-limitations-in-the-cli.md)
