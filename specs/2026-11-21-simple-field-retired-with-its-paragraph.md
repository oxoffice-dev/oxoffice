---
date: 2026-11-21
status: accepted
scope: docx
---

# A whole paragraph's simple field is tracked as Word tracks it

> Amended on 2026-11-21 after the Word check (issue #66). The first version retired the `w:fldSimple` shell in the revision engine. Word does not do that, so the rule is dropped: the comparison writes the field in the shape Word writes itself.

## Context

`compare` refuses to insert or delete a whole paragraph that holds a simple field (`w:fldSimple`). The message is "cannot track a whole paragraph's simple field outside a field it replaces" (issue #66).

The refusal is deliberate (`refuse_uncarried_field_owners` in `comparison.rs`):

- A `w:fldSimple` cannot carry an insertion or deletion mark of its own; only the runs inside it can.
- So accepting the deletion of such a paragraph would leave an empty `<w:fldSimple w:instr="PAGE"/>` shell merged into the next paragraph. Word evaluates that shell again, and its value comes back.

`rebuild_toc` writes the page numbers of a table of contents as simple `PAGEREF` fields. So adding or removing a heading under a rebuilt table of contents cannot be compared either.

## What Word does

Checked on the Mac with Word 16.115 ([comment on #66](https://github.com/oxoffice-dev/oxoffice/issues/66#issuecomment-7063001702)):

- **Word never writes `w:fldSimple`.** Any save turns it into a complex field: `fldChar` begin, `instrText`, separate, the result, `fldChar` end.
- **Given the `fldSimple` shape**, with the runs inside it marked and the paragraph mark marked, Word keeps the shell. After accepting the deletion (or rejecting the insertion), an empty `PAGE` field stays, merged into the next paragraph, and Word prints it again. This is what oxoffice's engine does today.
- **A deletion made in Word** puts every run of the complex field, `fldChar` and `delInstrText` included, inside the paragraph's one `w:del`; accepting removes the field with its paragraph. **An insertion made in Word** marked the text, begin, instruction and end runs (in two `w:ins`), and rejecting removed the whole field with its paragraph. Nothing was left in either case.

## Decision

**The comparison writes a whole inserted or deleted paragraph's simple field as a complex field, all of it inside one `w:ins` or `w:del`**, as Word's deletion does. Accepting or rejecting removes it with the paragraph, in oxoffice's engine as in Word, with no new rule in the engine.

- **The field**: `fldChar` begin (carrying the `w:fldLock`, `w:dirty` and `w:fldData` of the `w:fldSimple`), the instruction in `w:instrText` (`w:delInstrText` in a deletion), `fldChar` separate, the result runs, `fldChar` end. The `fldChar` and instruction runs are bare, as the existing conversion (`clean_simple_complex_source`) writes them. An empty result (`<w:fldSimple w:instr=" PAGE " w:dirty="1"/>`) gives begin, instruction, separate and end.
- **One wrapper**: the whole field, from begin to end, goes in one `w:ins` or `w:del` under one revision id (`qualified_complete_field_payload` writes that span), not one wrapper per run as `tracked_paragraph_children` does for other runs. The wrapper holds the field's runs only: a bookmark, comment range or proofing mark next to the field stays outside it, in the wrapper of its own neighbours, so that begin and end are its first and last children. Resolving one id at a time cannot then split the field.
- **Where**: the rewriting sits where every whole-paragraph emitter writes a paragraph's children (`paragraph_children_xml` and `tracked_paragraph_children`), so that it covers all four of them: the inserted and deleted paragraphs of a body, header, footer, note, comment, cell or content control (`inserted_paragraph`, `deleted_paragraph`); the last paragraph of a story, whose mark stays and whose children are marked (`final_paragraph`); and a paired paragraph that `compare_paragraph` writes as a deletion and an insertion because its boundaries differ. A carried paragraph (a table of contents replaced whole, which the alignment also writes through these emitters) is written as today: `CarriedParagraphs` reaches the emitters, and they convert only a field that is not carried.
- **Moves**: a moved paragraph that holds a simple field is written as a deleted paragraph and an inserted one, the shape the Word check covers. A move pair shares one id on both ends, which a field written whole cannot.
- **The comparison's checks**: a field converted this way is on one side only. Under `ignore_fields` no witness is recorded, as `compare_field_xml` records none there, and the guard compares field source facts without one. Its witness is one-sided (`FieldConversionWitness` today pairs an original and an edited occurrence under two ids): it names the occurrence of the side that keeps the paragraph and the one revision id of its wrapper. The acceptance check (for an insertion) or the rejection check (for a deletion) proves that this span holds one complete complex field equal to that occurrence's simple field. The other check sees the paragraph removed.
- **The kept side**: the side that keeps the paragraph gets the complex field, not the simple one. The two forms are the same field to Word and to oxoffice's field readers. A later comparison of that result with the simple form, though, sees a change of form (`compare_field_xml` writes a form change as a deletion and an insertion).
- **What stays refused**: `refuse_uncarried_field_owners` stops refusing what the conversion accepts. A simple field it cannot convert keeps a refusal, with the conversion's own message: a nested `w:fldSimple`, an unconsumed attribute on the `w:fldSimple`. A `w:fldSimple` inside a `w:hyperlink` stays refused with the #66 message: the hyperlink would need a shape of its own. The emitters check it themselves (on the path `paragraph_wraps_simple_field` already finds), so that the paired path, which never reaches `refuse_uncarried_field_owners`, refuses it too.

**What stays as today:**

- **The revision engine.** No retirement rule. A file that already holds the `fldSimple` shape (marked runs inside a `w:fldSimple` of a paragraph removed whole, written by another producer) resolves as today, leaving the shell, as Word does.
- **Content removed in part, a kept paragraph, a field carried by a complex field**: unchanged.
- **`ignore_fields`** keeps its meaning for paired paragraphs. A paragraph inserted or deleted whole is written as above with it on too.

## Plan

One pull request, tests first, in `tests/docx/revisions_and_compare.rs`:

- the #66 reproduction: a paragraph with a `PAGE` simple field inserted, and one deleted; accept and reject give the edited and the original text, with no field left on the side that drops the paragraph;
- the redline holds the field as complex, from begin to end inside one `w:ins` or `w:del` (`delInstrText` in the deletion);
- the same paragraph as the last one of the body, and as the last line of a footer (`Page {PAGE}`);
- the same in a footnote, a comment and a table cell;
- a paired paragraph holding a simple field whose boundaries change (a hyperlink added around its text);
- a moved paragraph with a simple field: written as a deletion and an insertion;
- a simple field with `w:fldLock`, and an empty one with `w:dirty`;
- a simple field inside a hyperlink keeps the refusal, inserted whole and in a paired paragraph whose boundaries change;
- an inserted caption with a bookmark around its `SEQ` simple field: the bookmark stays outside the field's wrapper;
- a table of contents replaced whole (carried) is written as today;
- `ignore_fields` on: the guard passes without a witness;
- a heading added under a table of contents that `rebuild_toc` filled on both versions; then, on the accepted redline, `rebuild_toc` again.

Then the rewriting in `comparison.rs`, the one-sided witness in its checks, the admission in `refuse_uncarried_field_owners`, and the `docx-compare-simple-field-paragraph` limitation file removed.

## Links

- Issue #66, and the Word check on it.
- `refuse_uncarried_field_owners`, `paragraph_children_xml`, `clean_simple_complex_source`, `qualified_complete_field_payload`, `FieldConversionWitness` and `compare_field_xml` (`comparison.rs`); `tracked_paragraph_children` (`revision.rs`).
