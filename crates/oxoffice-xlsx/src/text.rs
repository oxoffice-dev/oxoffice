//! A workbook as text: a sheet as CSV, the workbook as Markdown tables.
//! Both are written by hand, shared by the command line and Python.

use oxoffice_xlsx_oxml::reference::column_letters;
use oxoffice_xlsx_oxml::{CellRange, CellRef, format_f64};

use crate::cell::Value;
use crate::sheet::SheetKind;
use crate::{Result, Workbook, Worksheet};

/// How a sheet is written as CSV.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvOptions {
    /// The field separator, `,` by default.
    pub delimiter: char,
    /// Stored values rather than displayed texts: the shortest number that
    /// reads back the same (dates as serials), `TRUE` / `FALSE`, error
    /// codes.
    pub raw: bool,
    /// Start with a UTF-8 byte order mark, which some spreadsheet apps need
    /// to read the file as UTF-8.
    pub bom: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        CsvOptions {
            delimiter: ',',
            raw: false,
            bom: false,
        }
    }
}

/// The stored value of a cell, as `--raw` writes it.
fn raw(value: &Value) -> String {
    match value {
        Value::Empty => String::new(),
        Value::Number(number) => format_f64(*number),
        Value::Text(text) => text.clone(),
        Value::Boolean(true) => "TRUE".to_owned(),
        Value::Boolean(false) => "FALSE".to_owned(),
        Value::Error(error) => error.code().to_owned(),
    }
}

/// A CSV field, quoted as RFC 4180 asks when it holds the delimiter, a
/// quote or a line break.
fn field(text: &str, delimiter: char) -> String {
    if text.contains([delimiter, '"', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// A Markdown table cell: `|` escaped, line breaks as `<br>`.
fn markdown_cell(text: &str) -> String {
    text.replace("\r\n", "<br>")
        .replace(['\r', '\n'], "<br>")
        .replace('|', "\\|")
}

impl Worksheet<'_> {
    /// The sheet as CSV: from A1 to the last row and column of the used
    /// range, so that each field sits at its cell's position; the displayed
    /// texts (what Excel's CSV export writes) or, with
    /// [`CsvOptions::raw`], the stored values. CRLF line ends, fields
    /// quoted as RFC 4180 asks.
    pub fn to_csv(&self, options: &CsvOptions) -> Result<String> {
        self.require_cells()?;
        let mut out = String::new();
        if options.bom {
            out.push('\u{FEFF}');
        }
        let Some(used) = self.used_range()? else {
            return Ok(out);
        };
        let range = CellRange::new(CellRef::new(0, 0), used.last);
        let width = used.last.column as usize + 1;
        let mut cells = self.cells_in(range)?.into_iter().peekable();
        for row in 0..=used.last.row {
            let mut fields = vec![String::new(); width];
            while let Some(cell) = cells.next_if(|cell| cell.row() == row) {
                let column = cell.column() as usize;
                fields[column] = if options.raw {
                    raw(&cell.value)
                } else {
                    cell.text
                };
            }
            let line: Vec<String> = fields
                .iter()
                .map(|text| field(text, options.delimiter))
                .collect();
            out.push_str(&line.join(&options.delimiter.to_string()));
            out.push_str("\r\n");
        }
        Ok(out)
    }

    /// The sheet as Markdown: `## <name>`, then a table of its used rows
    /// headed by the column letters, each row led by its number (no table
    /// for a sheet without a used cell). A sheet that holds no cells is
    /// `## <name> (chartsheet)`, its kind named. See
    /// [`Workbook::to_markdown`].
    pub fn to_markdown(&self) -> Result<String> {
        let mut out = match self.kind() {
            SheetKind::Worksheet => format!("## {}\n", self.name()),
            kind => format!("## {} ({})\n", self.name(), kind.as_str()),
        };
        if let Some(table) = self.markdown_table()? {
            out.push('\n');
            out.push_str(&table);
        }
        Ok(out)
    }

    /// The used rows of the sheet as a Markdown table, `None` for a sheet
    /// without a used cell.
    fn markdown_table(&self) -> Result<Option<String>> {
        let Some(used) = self.used_range()? else {
            return Ok(None);
        };
        let columns = used.first.column..=used.last.column;
        let mut out = String::from("|   |");
        for column in columns.clone() {
            out.push_str(&format!(" {} |", column_letters(column)));
        }
        out.push_str("\n| --- |");
        for _ in columns.clone() {
            out.push_str(" --- |");
        }
        out.push('\n');
        let cells = self.cells_in(used)?;
        let mut at = 0;
        while at < cells.len() {
            let row = cells[at].row();
            let end = at
                + cells[at..]
                    .iter()
                    .take_while(|cell| cell.row() == row)
                    .count();
            // The cells of a row come by column: one cursor walks them.
            let mut in_row = cells[at..end].iter().peekable();
            out.push_str(&format!("| {} |", row + 1));
            for column in columns.clone() {
                let text = in_row
                    .next_if(|cell| cell.column() == column)
                    .map_or(String::new(), |cell| markdown_cell(&cell.text));
                out.push_str(&format!(" {text} |"));
            }
            out.push('\n');
            at = end;
        }
        Ok(Some(out))
    }
}

impl Workbook {
    /// The workbook as Markdown: for each sheet in tab order, hidden ones
    /// included, `## <name>` and a table of its used rows headed by the
    /// column letters, each row led by its number, so that every value
    /// keeps its address; a sheet that holds no cells is listed with its
    /// kind. The texts are the displayed ones; `|` is escaped and line
    /// breaks are written `<br>`.
    pub fn to_markdown(&self) -> Result<String> {
        let sections = self
            .sheets()
            .map(|sheet| sheet.to_markdown())
            .collect::<Result<Vec<_>>>()?;
        Ok(sections.join("\n"))
    }
}
