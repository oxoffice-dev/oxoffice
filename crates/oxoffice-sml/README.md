# oxoffice-sml

`oxoffice-sml` builds deterministic one-sheet XLSX workbooks for editable OOXML
chart data.

## Capabilities

- Text and numeric columns with headers and shared strings.
- Optional number formats for numeric columns.
- Spreadsheet limits and finite-number validation.
- Formula ranges for chart series and deterministic XLSX package writing.
- Shared-string identity, quoted formulas, number formats, and bounded row
  counts produce one editable chart workbook without a spreadsheet facade.

## Use it when

Use this crate when an OOXML chart needs an editable `.xlsx` data workbook.
Use a general spreadsheet library for standalone workbook applications.

## Relationship

`oxoffice-chart` uses this crate for chart data. It builds the workbook package on
the shared `oxoffice-opc` layer and does not depend on presentation code.

## Example

```rust,no_run
use oxoffice_sml::{Column, Workbook};

let workbook = Workbook::new(
    "Sales",
    vec![Column::Number {
        header: "Revenue".into(),
        values: vec![120.0, 150.0],
        number_format: Some("$0.00".into()),
    }],
)?;
let bytes = workbook.to_xlsx_bytes()?;
assert!(!bytes.is_empty());
# Ok::<(), oxoffice_sml::Error>(())
```

This crate is not a general spreadsheet library. It does not read workbooks or
provide formulas, multiple worksheets, charts, or arbitrary cell styling.
