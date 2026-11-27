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
# Ok::<(), oxoffice_xlsx::Error>(())
```
