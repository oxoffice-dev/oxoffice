---
date: 2026-11-21
status: proposed
scope: docx
---

# A simple field goes with the paragraph that is removed whole

## Context

`compare` refuses to insert or delete a whole paragraph that holds a simple field (`w:fldSimple`). The message is "cannot track a whole paragraph's simple field outside a field it replaces" (issue #66).

The refusal is deliberate (`refuse_uncarried_field_owners` in `comparison.rs`):

- A `w:fldSimple` cannot carry an insertion or deletion mark of its own; only the runs inside it can.
- So accepting the deletion of such a paragraph would leave an empty `<w:fldSimple w:instr="PAGE"/>` shell merged into the next paragraph. Word evaluates that shell again, and its value comes back.
- The revision engine (`carried_simple_field_retirements` in `revision.rs`) only retires a shell that a complex field carries. A paragraph removed whole is not such a case.

`insert_toc` writes its page numbers as simple `PAGEREF` fields. So adding or removing a heading under such a table of contents cannot be compared either.

## Decision

**The rule.** When `accept_all`, `reject_all` or a scoped resolution resolves revisions, a `w:fldSimple` is retired, shell included, when both of these hold:

1. **Its content all goes.** Every element inside it is removed by this resolution: deleted or moved-from runs on accept, inserted or moved-to runs on reject. Nothing unmarked is left inside it, and nothing marked with the other kind.
2. **Its paragraph goes whole.** The same resolution removes the mark of the paragraph that holds the field, either directly or inside a `w:hyperlink` (the two paths `paragraph_wraps_simple_field` watches). Everything else in that paragraph goes too, so the paragraph merges into nothing.

The paragraph mark is the proof of the shell's lifetime: the shell lives and dies with the paragraph, as the paragraph's other unmarked wrappers already do.

**The comparison.** `refuse_uncarried_field_owners` stops refusing a whole inserted, deleted or moved paragraph. Its simple field is written as it is today, with the field's runs inside the `w:ins`, `w:del`, `w:moveFrom` or `w:moveTo` of the paragraph. Accepting then gives the edited version and rejecting gives the original, with the simple field still simple on the side that keeps it.

**What stays as today:**

- **Content removed in part.** A run inside the field survives, so the shell stays with its surviving result.
- **Paragraph kept.** The field's runs are deleted but the paragraph mark is not. The shell stays, so the field stays in the kept paragraph and its value comes back. That is today's behaviour, and the comparison never writes this shape for a paragraph it keeps.
- **Field carried by a complex field.** The existing carried rule applies, unchanged.
- **Other stories.** The same rule applies in headers, footers, notes and comments, where a paragraph can be removed whole.

## What changes for files that already have this shape

A Word file can already hold a paragraph removed whole with a simple field inside, for example a redline made by Word itself:

- **Accepting** a deleted one today leaves the empty shell in the next paragraph; after this change the shell goes.
- **Rejecting** an inserted one: the same.

Nothing changes for any other shape.

## Word does the same? Not verified yet

The assumption is that Word, when it accepts the deletion of a whole paragraph holding a simple field (or rejects its insertion), removes the field with the paragraph. Nobody has checked it. OX-M will check it on the Mac with Word if the user opens it for that:

1. Make a paragraph with a `PAGE` simple field.
2. Delete it with track changes on, then accept.
3. Look at `document.xml`.

If Word keeps the shell, this spec is revisited before the code merges.

## Plan

One pull request, tests first, in `tests/docx/revisions_and_compare.rs` and `tests/docx/revisions.rs`:

- accepting a hand-built deleted paragraph with a simple field removes the field;
- rejecting a hand-built inserted one does the same;
- a partly deleted field keeps its shell;
- a kept paragraph keeps its shell;
- a field carried by a complex field is unchanged;
- the #66 reproduction: insert and delete a paragraph with a `PAGE` simple field, accept and reject checked;
- a heading added under an `insert_toc` table of contents, then `rebuild_toc`.

Then the rule in `revision.rs`, the admission in `comparison.rs`, and the `docx-compare-simple-field-paragraph` limitation file removed.

## Links

- Issue #66.
- `refuse_uncarried_field_owners` (`comparison.rs`) and `carried_simple_field_retirements` (`revision.rs`).
