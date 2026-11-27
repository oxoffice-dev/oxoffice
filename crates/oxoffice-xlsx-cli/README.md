# oxoffice-xlsx-cli

This crate is the `oxoffice xlsx` command tree of the `oxoffice` binary,
built by the `oxoffice-cli` crate.

## Capabilities

- `oxoffice xlsx inspect FILE [--json]`: the sheets of a workbook (position,
  name, state, kind), its date system and class. A file that is not a
  workbook is refused with a plain message naming the way out.
