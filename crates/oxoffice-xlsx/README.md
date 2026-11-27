# oxoffice-xlsx

`oxoffice-xlsx` opens and saves Excel-compatible workbooks (`.xlsx`, `.xlsm`,
`.xltx`, `.xltm`) without loss, from Excel, Google Sheets or LibreOffice, in
Transitional or Strict Open XML.

## Capabilities

- Opening from a path or bytes, a password-protected workbook with its
  password; a legacy `.xls`, a binary `.xlsb`, an OpenDocument spreadsheet,
  a Word or PowerPoint file refused by their content, with a plain message.
- The sheets: name, position, state (visible, hidden, very hidden) and kind;
  the date system; each sheet's page setup.
- Saving without an edit writes every part back byte for byte. A save is
  atomic and never replaces the file the workbook was opened from unless
  asked; the extension selects the workbook, template or macro-enabled
  class.
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
use oxoffice_xlsx::Workbook;

let workbook = Workbook::open("budget.xlsx")?;
for sheet in workbook.sheets() {
    println!("{} {:?}", sheet.name(), sheet.state());
}
workbook.save("copy.xlsx")?;

let mut new = Workbook::new();
let mut data = new.add_sheet("Data")?;
data.set_column_width("A:C", 20.0)?; // characters
data.set_row_height("1", 30.0)?; // points
data.freeze_panes(Some("A2"))?;
new.save("new.xlsx")?;
# Ok::<(), oxoffice_xlsx::Error>(())
```
