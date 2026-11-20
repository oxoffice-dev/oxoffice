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

## Links

- [ROADMAP.md](../ROADMAP.md), how the library is meant to be used.
- [2026-11-20-known-limitations-in-the-cli.md](2026-11-20-known-limitations-in-the-cli.md)
