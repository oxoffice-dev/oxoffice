---
date: 2026-11-23
status: accepted
scope: docx
---

# A link in a paragraph added or removed whole is tracked as a HYPERLINK field

## Context

`compare` writes a paragraph inserted or deleted whole with its revision wrappers inside each `w:hyperlink`
(`<w:hyperlink><w:del>…</w:del></w:hyperlink>`); the hyperlink element carries no mark (#541). oxoffice's accept and
reject remove the emptied link with its paragraph, but Word does not: after Accept All of the deletion or Reject All
of the insertion it keeps an empty `<w:hyperlink w:anchor="…"/>`, merges it into the next paragraph, and a field
update prints the target there ("_TopAfter."). Any link does it: a table of contents entry, a cross-reference, a URL.

What Word writes itself when such a paragraph is deleted or inserted with tracking on (Word 16.115, checked on a Mac,
`handoff/pcwe/word-checks/hyperlink-tracked.md`): no `w:hyperlink`, but a `HYPERLINK` complex field, `HYPERLINK \l
"Target"` for a link to a bookmark and `HYPERLINK "https://example.com/"` for a URL, its begin, instruction,
separate, result and end inside the paragraph's revision wrapper, the paragraph mark marked. Accept All and Reject All
then leave no shell, and Word saves the surviving link back as `w:hyperlink`.

#437 (#66) already writes a simple field of such a paragraph as a complex field in a wrapper of its own, for the same
reason: a `w:fldSimple` cannot carry a mark.

## Decision

**A `w:hyperlink` of a paragraph inserted or deleted whole is written as one `HYPERLINK` complex field, the whole of
it in an `ins` (or `del`) of its own under one new revision id, as Word writes it.**

- **Instruction**: `HYPERLINK "<target>"` for a link with a relationship (its target, from the part's
  relationships), `HYPERLINK \l "<anchor>"` for a link to a bookmark, both for a link with both; `\o "<tooltip>"` and
  `\t "<target frame>"` when the link has them. Quotes and backslashes in a value are escaped as field arguments.
- **Result**: the link's runs, deleted or inserted as the paragraph's other runs; a simple field inside the link is
  the nested complex field #437 writes.
- **Refused**, not dropped: a link with what no switch carries (`w:docLocation`, an unmodelled attribute or child),
  with a message naming it.
- **Where**: only for a paragraph inserted or deleted whole that is neither moved nor carried inside a complex field
  replaced whole; a link in a paragraph that both versions hold, or that moves, keeps its `w:hyperlink`.
- **Guard**: the comparison checks that the resolution keeping the paragraph reads back the link it came from
  (target, anchor, tooltip, result), as it checks a whole-paragraph simple field.

We deliberately do not change links elsewhere, nor how the resolution writes a surviving field (oxoffice keeps the
field form after resolution, as for #437's fields; Word writes the link back as `w:hyperlink`).

## Plan

One pull request, closing #541: the emitter next to `whole_simple_field`, the guard witness, tests (internal and
external links, inserted and deleted, the final paragraph, a link holding a simple field, a table of contents entry
paragraph), then a Word check (both directions, internal and external) before merge.

## Links

- Issue #541; #437 and #66 (whole-paragraph simple fields, `2026-11-21-simple-field-retired-with-its-paragraph.md`).
