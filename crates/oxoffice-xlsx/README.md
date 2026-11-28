# oxoffice-xlsx

`oxoffice-xlsx` opens and saves Excel-compatible workbooks (`.xlsx`, `.xlsm`,
`.xltx`, `.xltm`) without loss, from Excel, Google Sheets or LibreOffice, in
Transitional or Strict Open XML.

## Capabilities

- Opening from a path or bytes, a password-protected workbook with its
  password; a legacy `.xls`, a binary `.xlsb`, an OpenDocument spreadsheet,
  a Word or PowerPoint file refused by their content, with a plain message.
- The sheets: name, position, state (visible, hidden, very hidden), kind and
  used range; the date system; each sheet's page setup, merges and comments
  (notes, and threads with their replies).
- Cells as snapshots: the value (for a formula, the cached one, marked stale
  when the file asks for a recalculation), the formula as Excel displays it
  and as stored (a shared formula's members moved from their master), array
  ranges, the number format and the text Excel displays, dates, and the
  resolved style (theme and indexed colours in RGB). Bulk reads of a range,
  a sheet as CSV, the workbook as Markdown.
- Cells written: numbers, texts (to the shared strings, an identical plain
  item reused), booleans, errors, dates with a format that shows them,
  formulas (checked, stored as Excel stores them, spilling ones marked in
  `metadata.xml`), clears, bulk writes, CSV imports with strict typing,
  styles and number formats (identical records reused, the rest appended).
  A write into a shared formula's range turns its other cells into plain
  formulas; what Excel would repair is refused with nothing changed. Reads
  see the edits. `SaveOptions::drop_cached_values` leaves every formula to
  an application that does not calculate on open, as LibreOffice.
- Saving without an edit writes every part back byte for byte, except that
  a package signature that already fails verification gets its mark, as in
  docx and pptx. A save is atomic and never replaces the file the workbook
  was opened from unless asked; the extension selects the workbook, template
  or macro-enabled class.
- A new workbook, `Workbook::new()`: one empty sheet, Calibri 11 under the
  Office 2013-2022 theme, the same bytes every time. `add_sheet` adds a
  sheet after the others under Excel's naming rules, `set_column_width`,
  `set_row_height` and `freeze_panes` set the grid of a sheet. Edits are
  staged and a save writes only the parts they changed, the rest as it was
  read; an edited workbook that holds a formula asks Excel to calculate it on
  open, and a signed one has its package signature marked invalidated.

## Use it when

Use this crate for whole workbooks. `oxoffice-xlsx-oxml` holds the part-level
SpreadsheetML model.

## Example

```rust,no_run
use oxoffice_xlsx::{FormulaOptions, StylePatch, Workbook};

let workbook = Workbook::open("budget.xlsx")?;
for sheet in workbook.sheets() {
    println!("{} {:?} {:?}", sheet.name(), sheet.state(), sheet.used_range()?);
}
let budget = workbook.sheet("Budget").expect("a Budget sheet");
let total = budget.cell("F12")?;
println!("{:?} {}", total.formula, total.text);
workbook.save("copy.xlsx")?;

let mut workbook = Workbook::open("budget.xlsx")?;
let mut budget = workbook.sheet_mut("Budget").expect("a Budget sheet");
budget.set_value("B4", 12_000.0)?;
budget.set_formula("I4", "F4-H4", FormulaOptions::default())?;
budget.set_style("A3:H3", &StylePatch { bold: Some(true), ..StylePatch::default() })?;
workbook.save("edited.xlsx")?;

let mut new = Workbook::new();
let mut data = new.add_sheet("Data")?;
data.set_column_width("A:C", 20.0)?; // characters
data.set_row_height("1", 30.0)?; // points
data.freeze_panes(Some("A2"))?;
new.save("new.xlsx")?;
# Ok::<(), oxoffice_xlsx::Error>(())
```
