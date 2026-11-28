# oxoffice-xlsx-py

The Rust bindings behind `oxoffice.xlsx`, the Excel API of the `oxoffice`
Python package. The `oxoffice-py` crate builds the wheel.

`oxoffice.xlsx` opens Excel-compatible workbooks from Excel, Google Sheets or
LibreOffice, reads their cells (values, formulas, displayed texts, styles),
comments and page setups, writes values, dates, formulas, styles, number
formats and CSV imports, and saves them without loss, password-protected or
not. It makes a new workbook, adds sheets, sets column widths, row heights
and frozen panes. It works directly with OOXML packages and needs neither Excel nor
LibreOffice.
