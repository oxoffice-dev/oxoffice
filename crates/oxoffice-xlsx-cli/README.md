# oxoffice-xlsx-cli

This crate is the `oxoffice xlsx` command tree of the `oxoffice` binary,
built by the `oxoffice-cli` crate.

## Capabilities

- `oxoffice xlsx inspect FILE [--json]`: the sheets of a workbook (position,
  name, state, kind), its date system and class. A file that is not a
  workbook is refused with a plain message naming the way out.
- `oxoffice xlsx new -o FILE [--sheet NAME]... [--force] [--json]`: a new
  workbook, Calibri 11 under the Office 2013-2022 theme, the same bytes every
  time. A file that exists is replaced only with `--force`.
- `oxoffice xlsx edit FILE --map EDITS.json -o OUT [--force] [--json]`: a
  JSON array of edits applied in order, all or nothing, a refusal naming the
  entry (from 0): `{"add_sheet": "Summary"}`, `{"columns": "A:C", "width": 20}`
  (characters), `{"rows": "2:5", "height": 30}` (points), `{"freeze": "B2"}`
  or `{"freeze": null}`. An entry names its sheet with `"sheet"` (a name, or
  a number from 1) and otherwise takes the sheet of the entry before it.
