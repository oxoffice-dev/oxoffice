# oxoffice-xlsx-formula

`oxoffice-xlsx-formula` reads spreadsheet formulas as a workbook stores them
and as Excel displays them.

## Capabilities

- Tokens of a formula as stored (ECMA-376 Part 1, 18.17; MS-XLSX 2.2.2): A1
  references, ranges, whole columns and rows, sheet prefixes quoted or not,
  3D (`Sheet1:Sheet3!A1`) and external (`[1]Sheet1!A1`) references, defined
  names, table references (`Expenses[[#This Row],[Quantity]]`), texts,
  numbers, booleans, error values, array constants, operators and calls.
  Joined, the tokens give the formula back.
- The displayed form and back: future functions lose and take `_xlfn.`
  (`_xlfn._xlws.` for `FILTER`, `SORT` and `PY`), LET and LAMBDA parameters
  `_xlpm.` in their scope (an optional `[y]` is `_xlop.y`), a function passed
  as a value `_xleta.` (`GROUPBY(A2:A9,B2:B9,SUM)`), `_xlfn.SINGLE(x)` is `@x`
  and `_xlfn.ANCHORARRAY(x)` is `x#`. A stored formula comes back to its
  exact text.
- A shared formula's member: the master's formula with its relative
  references moved; a reference leaving the grid becomes `#REF!`.
- The syntax check a written formula passes: operands, operators, calls,
  array constants, `,` between arguments, each built-in function's argument
  count, at most 255 arguments, 64 nested functions and 8,192 characters,
  with a plain message.
- How large a formula's result is, from its parse tree: exactly when its
  text decides it (a range, `A2:A10*2`, `SORT`, `SEQUENCE(5)`, `TAKE` with
  constants...), at most when the data decide how much of the range it reads
  it fills (`FILTER`, `UNIQUE`), unknown otherwise: what a dynamic array
  formula spills over, without an evaluator.

## Use it when

Use this crate to show, write or check a workbook formula, to read a
shared formula's members, or to know the range a dynamic array formula
spills over. It depends on no oxoffice crate; the xlsx facade
uses it, and the evaluator will build on it.

## Example

```rust
use oxoffice_xlsx_formula::{move_references, to_displayed, to_stored};

assert_eq!(to_displayed("_xlfn._xlws.SORT(B2:B7,,-1)"), "SORT(B2:B7,,-1)");
assert_eq!(to_stored("LET(x,A1,x*2)")?, "_xlfn.LET(_xlpm.x,A1,_xlpm.x*2)");
assert_eq!(move_references("D2*$E$1", 3, 0)?, "D5*$E$1");
assert!(to_stored("IF(1,2,3,4)").is_err());
# Ok::<(), oxoffice_xlsx_formula::SyntaxError>(())
```

The function table (`src/functions/table.rs`) is written by
`tools/gen-xlsx-functions/generate.py` from Microsoft's MS-XLSX and MS-XLS
specifications. See `specs/2026-11-27-xlsx-open-read-edit.md`.
