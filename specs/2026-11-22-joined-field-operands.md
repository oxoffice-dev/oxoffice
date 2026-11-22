---
date: 2026-11-22
status: accepted
scope: docx
---

# A field operand that joins text and nested fields

## Context

Word reads a field instruction as whitespace-separated tokens, a quoted token
being one operand. A nested field is updated first and its result takes its
place in the instruction. So `{ IF "Mr { MERGEFIELD Name }" = "Mr Smith"
"Known" "Unknown" }` compares `Mr Smith` with `Mr Smith`, and
`"{ MERGEFIELD First } { MERGEFIELD Last }"` is one operand, the two results
with a space between them. Mail-merge templates write both shapes.

oxoffice has no such operand (issue #50). `FieldArgument` (oxoffice-docx-oxml)
is `Text(String)` or `Nested(Box<Field>)`. The lexer (`end_token` in
`field_instruction.rs`) splits a token made of several pieces into one token
per piece and marks each ill-formed. The field then gets too many operands:
`update_fields` keeps the stored result with "field IF requires 5 positional
operands", a message that hides the cause. A quoted operand that is one
nested field and nothing else works (#31).

The same split happens without quotes, when nothing separates the pieces:
`{ = { MERGEFIELD Qty }*2 }` or `{ IF { MERGEFIELD A }{ MERGEFIELD B } = "xy"
... }`.

## Decision

**A new operand shape, `FieldArgument::Joined(Vec<FieldArgument>)`: the pieces
of one token in source order, each `Text` or `Nested`.** Its value is the
pieces' values joined, the way Word reads it.

- **Reading**: `end_token` makes one `Joined` operand of a token with several
  pieces, quoted or not, and keeps it well formed. A `Joined` always holds at
  least one `Nested` piece and at least two pieces; adjacent text is one piece.
  A token with a single piece keeps today's `Text` or `Nested`, so every
  instruction that reads today reads the same.
- **A name written against a nested field**: `{ ={ MERGEFIELD Qty }*2 }`, as
  Word's Ctrl+F9 leaves it, reads the leading text as the field's name (`=`)
  and the rest of the token as its first operand, as today's split did.
- **Where a nested field sits**: `NestedFieldPosition::Argument(i)` and
  `Switch(i)` keep their meaning, "in operand `i`". A `Joined` operand with
  two nested pieces is listed twice in the nested order, the n-th listing being
  its n-th nested piece. No new position type.
- **Evaluation** (`field/evaluate.rs`, and the layout's field substitution,
  which reads operands the same way): a `Joined` operand's value is each
  piece's value in order, a nested piece giving its evaluated result as a
  `Nested` operand gives it today. When one nested piece has no result, the
  operand has none: the field keeps its stored result with a diagnostic naming
  that piece's field, as for a `Nested` operand.
- **Bookmark names and other single-source readers** (`bookmark_reference_span`,
  TOC and index switches, the import renaming of #244) treat a `Joined`
  operand as they treat a `Nested` one: not a plain name, left as is.
- **Writing**: an instruction read from a file keeps its source bytes, as every
  parsed instruction does. `FieldInstruction::new` accepts a `Joined` operand
  and writes it quoted, its text escaped and its nested fields in place, so a
  field built with one reads back the same.
- **Equality and identity** (`instruction_structure_eq`,
  `instruction_source_identity_eq`, the comparison's field facts): piece by
  piece, as for the other two shapes.
- **Surfaces**: the operand model is Rust only (the facade does not re-export
  `FieldArgument`; the CLI and Python show an instruction as text). The
  behaviour reaches all three through `update_fields`, `evaluate_fields`,
  mail merge and rendering, with no new API. `FieldArgument` is not marked
  `#[non_exhaustive]`: the workspace's own crates match on it.

Deliberately not done: no evaluation of a nested field inside the operand of
a switch that takes a format picture (`\@`, `\#`) beyond what `Nested` does
today; it gets the same `Joined` reading and value, nothing more.

## Plan

One pull request, `feat(docx): a field operand joins text and nested fields`:

1. Model and lexer in oxoffice-docx-oxml: the variant, `end_token`, the nested
   order, `FieldInstruction::new` and the canonical writer, equality and
   identity, owner and work checks. Unit tests on the lexer: quoted text and
   field, two fields with a space, unquoted adjacency, one-piece tokens
   unchanged.
2. Every `match` on `FieldArgument` in the facade, the layout and the
   comparison gets its `Joined` arm (17 files today), evaluation first.
3. Tests in `crates/oxoffice-docx/tests/docx/fields_and_toc.rs`: the strict
   known failure of #50 passes and loses its `should_panic`; an IF on
   `"{ MERGEFIELD First } { MERGEFIELD Last }"`; an `=` formula on
   `{ MERGEFIELD Qty }*2`; a missing nested result keeps the stored result
   with the piece's diagnostic; a document holding these saves unchanged
   and reopens with the same operands.

Checked by the crate suites of oxoffice-docx-oxml, oxoffice-docx and
oxoffice-docx-layout, and the corpus round trips.

## Links

- Issue #50; #31 (a quoted operand that is one nested field).
