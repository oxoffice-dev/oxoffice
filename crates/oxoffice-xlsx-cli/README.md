# oxoffice-xlsx-cli

This crate is the `oxoffice xlsx` command tree of the `oxoffice` binary,
built by the `oxoffice-cli` crate.

## Capabilities

- `oxoffice xlsx inspect FILE [--json]`: the sheets of a workbook (position,
  name, state, kind, used range), its date system and class; the JSON adds
  each sheet's page setup. A file that is not a workbook is refused with a
  plain message naming the way out.
- `oxoffice xlsx new -o FILE [--sheet NAME]... [--force] [--json]`: a new
  workbook, Calibri 11 under the Office 2013-2022 theme, the same bytes every
  time. A file that exists is replaced only with `--force`.
- `oxoffice xlsx edit FILE --map EDITS.json -o OUT [--force]
  [--drop-cached-values] [--json]`: a JSON array of edits applied in order,
  all or nothing, a refusal naming the entry (from 0): `{"cell": "B2",
  "value": 3}` (a number, a string, a boolean or null), `{"cell": "D2",
  "formula": "B2*C2"}` (with `"stored": true`, `"spill": true` or `"spill":
  "J2:J5"`), `{"cell":
  "E2", "date": "2026-01-05"}`, `{"range": "A1:F1", "style": {"bold": true,
  "fill": "FFFF00"}}`, `{"range": "C2:C8", "number_format": "0.00"}`,
  `{"range": "A20:B30", "clear": "contents"}`, `{"add_sheet": "Summary"}`,
  `{"columns": "A:C", "width": 20}` (characters), `{"rows": "2:5", "height":
  30}` (points), `{"freeze": "B2"}` or `{"freeze": null}`. An entry names its
  sheet with `"sheet"` (a name, or a number from 1) and otherwise takes the
  sheet of the entry before it.
- `oxoffice xlsx set FILE CELL VALUE [--sheet S] [--text | --formula
  [--stored] [--spill | --spill-range J2:J5]] -o OUT`: one cell, `VALUE`
  typed as a CSV field (`007` stays text), as text, or a formula; a spilling
  formula over the range Excel finds, or over the range given, which Google
  Sheets spills too. A negative number is a value; other text that starts
  with `-` goes after `--`.
- `oxoffice xlsx import-csv FILE CSV [--sheet S] [--at A1] [--text] -o OUT`:
  a UTF-8 CSV written from a cell, fields typed strictly or kept as text.
  `--drop-cached-values` and `--force` go with every command that writes.
- `oxoffice xlsx text FILE [--json | --markdown] [--sheet S]...`: the cells of
  each worksheet as Excel displays them, as tab-separated rows, one Markdown
  table per sheet headed by the column letters, or a schema-1 record with
  values, formulas, merges and comments.
- `oxoffice xlsx csv FILE [--sheet S] [--raw] [--delimiter C] [--bom] [-o OUT]`:
  one sheet as CSV, displayed texts or stored values.
- `oxoffice xlsx cells FILE RANGE [--sheet S] [--json]`: the cells of a range
  (`B3`, `A1:C10`, `A:C`, `2:5`, `'Q1 data'!B3`) with their formulas; the JSON
  adds the stored formula, its kind and range, the number format and the
  resolved style.
- `oxoffice xlsx comments FILE [--sheet S]... [--json]`: notes and comment
  threads with their replies.

Sheets are named, or numbered from 1.
