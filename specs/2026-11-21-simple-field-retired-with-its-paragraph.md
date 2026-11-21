---
date: 2026-11-21
status: proposed
scope: docx
---

# A whole paragraph's simple field is tracked as Word tracks it

> Amended on 2026-11-21 after the Word check (issue #66). The first version retired the `w:fldSimple` shell in the revision engine. Word does not do that, so the rule is dropped: the comparison writes the field in the shape Word writes itself.

## Context

`compare` refuses to insert or delete a whole paragraph that holds a simple field (`w:fldSimple`). The message is "cannot track a whole paragraph's simple field outside a field it replaces" (issue #66).

The refusal is deliberate (`refuse_uncarried_field_owners` in `comparison.rs`):

- A `w:fldSimple` cannot carry an insertion or deletion mark of its own; only the runs inside it can.
- So accepting the deletion of such a paragraph would leave an empty `<w:fldSimple w:instr="PAGE"/>` shell merged into the next paragraph. Word evaluates that shell again, and its value comes back.

`insert_toc` writes its page numbers as simple `PAGEREF` fields. So adding or removing a heading under such a table of contents cannot be compared either.

## What Word does

Checked on the Mac with Word 16.115 ([comment on #66](https://github.com/oxoffice-dev/oxoffice/issues/66#issuecomment-7063001702)):

- **Word never writes `w:fldSimple`.** Any save turns it into a complex field: `fldChar` begin, `instrText`, separate, the result, `fldChar` end.
- **Given the `fldSimple` shape**, with the runs inside it marked and the paragraph mark marked, Word keeps the shell. After accepting the deletion (or rejecting the insertion), an empty `PAGE` field stays, merged into the next paragraph, and Word prints it again. This is what oxoffice's engine does today.
- **When Word makes the change itself**, the field is complex and every run of it, `fldChar` and `instrText` included (`delInstrText` for a deletion), sits inside the paragraph's one `w:del` or `w:ins`. Accepting or rejecting then removes the field with its paragraph, and nothing is left.

## Decision

**The comparison writes a whole inserted, deleted or moved paragraph's simple field as a complex field**, every run of it inside the paragraph's `w:ins`, `w:del`, `w:moveFrom` or `w:moveTo`: the shape Word writes itself. Accepting or rejecting removes it with the paragraph, in oxoffice's engine as in Word, with no new rule in the engine.

- **The field**: `fldChar` begin (with the `w:fldLock` and `w:dirty` of the `w:fldSimple`), the instruction in `w:instrText` (`w:delInstrText` in a deletion), `fldChar` separate, the result runs, `fldChar` end. The `fldChar` and instruction runs take the properties of the first result run, as Word gives them.
- **Reuse**: the comparison already writes this shape when a field changes form between the two versions (`clean_simple_complex_source`, `qualified_complete_field_payload`), and it records a `FieldConversionWitness` so that its own checks accept a simple field on one side and a complex one on the other. A whole paragraph's simple field takes the same path.
- **The kept side**: the side that keeps the paragraph gets the complex field, not the simple one. The two forms are the same field to Word and to oxoffice's field readers.
- **`refuse_uncarried_field_owners`** stops refusing these paragraphs. It still refuses a simple field it cannot rewrite (a `w:fldSimple` that holds anything but runs, such as a nested field or a hyperlink), with the same message.

**What stays as today:**

- **The revision engine.** No retirement rule. A file that already holds the `fldSimple` shape (marked runs inside a `w:fldSimple` of a paragraph removed whole, written by another producer) resolves as today, leaving the shell, as Word does.
- **Content removed in part, a kept paragraph, a field carried by a complex field**: unchanged.
- **Other stories.** The same rewriting applies in headers, footers, notes and comments, where a paragraph can be inserted or deleted whole.

## Plan

One pull request, tests first, in `tests/docx/revisions_and_compare.rs`:

- the #66 reproduction: a paragraph with a `PAGE` simple field inserted, and one deleted; accept and reject give the edited and the original text, with no field left on the side that drops the paragraph;
- the redline holds the field as complex, every run of it inside the paragraph's `w:ins` or `w:del` (`delInstrText` in the deletion);
- a moved paragraph with a simple field, both ends;
- a simple field with `w:fldLock` keeps it on the `fldChar` begin;
- a simple field holding a hyperlink keeps the refusal;
- a heading added under an `insert_toc` table of contents, then `rebuild_toc`;
- the same in a header.

Then the rewriting in `comparison.rs`, the admission in `refuse_uncarried_field_owners`, and the `docx-compare-simple-field-paragraph` limitation file removed.

## Links

- Issue #66, and the Word check on it.
- `refuse_uncarried_field_owners`, `clean_simple_complex_source`, `qualified_complete_field_payload` and `FieldConversionWitness` (`comparison.rs`).
