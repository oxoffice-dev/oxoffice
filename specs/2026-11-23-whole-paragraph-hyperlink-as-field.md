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

- **Instruction**: `HYPERLINK "<address>"` for a link through an external relationship (its target, from the
  story part's relationships, the original's or those carried from the edited package), `\l "<anchor>"` for a link
  to a bookmark, `\o "<tooltip>"` and `\t "<target frame>"` when the link has them, in the order of its attributes.
  Quotes and backslashes in a value are escaped as field arguments. `w:history` has no switch: Word drops it from
  the field and writes it back when it saves the link, and so does the guard.
- **Result**: the link's content, deleted or inserted as the paragraph's other runs; a complex field inside it (the
  PAGEREF of a contents entry) nests in the link's field, in the same wrapper.
- **Refused**, not dropped: a link with what no switch carries (`w:docLocation`, an unmodelled attribute), or whose
  relationship targets a part of the package, with a message naming it.
- **Kept as today**, edge cases no Word file holds: a link holding a simple field written whole (#437), since that
  field has a wrapper of its own and Word nests no wrapper in another (Word writes no simple field); and a link
  inside an inline content control, a smart tag or an inline custom XML element, since only the paragraph's direct
  links are written as fields. Word may still keep such a link's emptied element.
- **Where**: only for a paragraph inserted or deleted whole that is neither moved nor carried inside a complex field
  replaced whole; a link in a paragraph that both versions hold, or that moves, keeps its `w:hyperlink`. That covers
  the body, cells, block content controls, notes, comments, headers and footers.
- **Guard**: the resolution that keeps the paragraph is read back as Word saves it: each such field becomes a
  `w:hyperlink` again, its address (through a new relationship), anchor, tooltip and frame read from the field's
  instruction alone, its content the field's result left in place; only the attribute order, the namespace
  declarations and `w:history` come from the comparison. The ordinary postcondition then compares it with the input,
  so a field that misstates the link fails the comparison. A story part is resolved once for all of its links (a
  contents rebuilt whole holds one per entry); a note part, which only the whole document resolves, once per link.
- **Links of the resolved document**: oxoffice keeps the field after resolution, and `links()` reports a `HYPERLINK`
  field to a bookmark with its anchor, as it already reported one to an address; before, it left it out.

We deliberately do not change links elsewhere, nor how the resolution writes a surviving field (oxoffice keeps the
field form after resolution, as for #437's fields; Word writes the link back as `w:hyperlink`).

## Plan

One pull request, closing #541: the emitter next to `whole_simple_field`, the guard restoration, `links()` for a
field to a bookmark, tests (internal and external links, inserted and deleted, the final paragraph, a table of
contents entry, notes, comments, cells and controls, the refusal), then a Word check (both directions, internal and
external) before merge.

## Links

- Issue #541; #437 and #66 (whole-paragraph simple fields, `2026-11-21-simple-field-retired-with-its-paragraph.md`).
