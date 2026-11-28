# oxoffice-xlsx-oxml

`oxoffice-xlsx-oxml` reads and writes the SpreadsheetML parts of a workbook
without loss: what it models is read through typed accessors, everything else
is kept as written.

## Capabilities

- The workbook part: date system, sheets, defined names, calculation
  properties.
- Shared strings: the plain text of each item, its runs and phonetics kept
  as written; new items appended, so existing indexes stay valid.
- Styles: number formats, fonts, fills, borders, cell formats and cell styles,
  appended to and never reordered.
- Worksheets: one pass records each row's place, shared formula groups and
  array ranges; a row becomes typed when read or edited, and an edited sheet
  is written as its source bytes with the edited rows and children spliced in.
- Column widths (the `cols` spans, cut and merged as Excel writes them) and
  frozen panes (`sheetViews`), written at their place in the sheet.
- The sheet lists of `docProps/app.xml`, dropped when the sheet list changes.
- Page setup elements, typed and written as read.
- Comments, read only: notes, threaded comments and the persons who wrote
  them.
- Transitional and Strict Open XML.

## Use it when

Use this crate for part-level SpreadsheetML work. Use the `oxoffice-xlsx`
facade for whole workbooks.
