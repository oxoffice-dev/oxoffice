//! A cell as read: its value, its formula, its displayed text and its
//! style, a snapshot taken by address. And the A1 references the reads take.

use std::fmt;
use std::sync::Arc;

use oxoffice_numfmt::DateTime;
use oxoffice_xlsx_oxml::reference::parse_column;
use oxoffice_xlsx_oxml::{CellRange, CellRef};

use crate::style::ResolvedStyle;
use crate::{Error, Result};

/// The value of a cell; for a formula, the value its producer computed at
/// its last save.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// No value: an empty cell, or a formula without a cached value.
    Empty,
    /// A number. Dates and times are numbers too, serial days in the
    /// workbook's date system (see [`Cell::datetime`]).
    Number(f64),
    Text(String),
    Boolean(bool),
    Error(CellError),
}

impl Value {
    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }
}

/// An error value: what a formula gives when it cannot compute, or a
/// constant written so.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CellError {
    /// `#NULL!`
    Null,
    /// `#DIV/0!`
    DivisionByZero,
    /// `#VALUE!`
    Value,
    /// `#REF!`
    Reference,
    /// `#NAME?`
    Name,
    /// `#NUM!`
    Number,
    /// `#N/A`
    NotAvailable,
    /// `#GETTING_DATA`
    GettingData,
    /// A code of a later Excel (`#SPILL!`, `#CALC!`...), as stored.
    Other(String),
}

impl CellError {
    /// The error a code names.
    pub fn from_code(code: &str) -> Self {
        match code {
            "#NULL!" => CellError::Null,
            "#DIV/0!" => CellError::DivisionByZero,
            "#VALUE!" => CellError::Value,
            "#REF!" => CellError::Reference,
            "#NAME?" => CellError::Name,
            "#NUM!" => CellError::Number,
            "#N/A" => CellError::NotAvailable,
            "#GETTING_DATA" => CellError::GettingData,
            other => CellError::Other(other.to_owned()),
        }
    }

    /// The code, as a cell shows it.
    pub fn code(&self) -> &str {
        match self {
            CellError::Null => "#NULL!",
            CellError::DivisionByZero => "#DIV/0!",
            CellError::Value => "#VALUE!",
            CellError::Reference => "#REF!",
            CellError::Name => "#NAME?",
            CellError::Number => "#NUM!",
            CellError::NotAvailable => "#N/A",
            CellError::GettingData => "#GETTING_DATA",
            CellError::Other(code) => code,
        }
    }
}

impl fmt::Display for CellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// The kind of a formula other than a plain one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FormulaKind {
    /// A member or the master of a shared formula: one formula filled over
    /// a range, each cell's references moved.
    Shared,
    /// An array formula over [`Cell::array_range`], dynamic (spilling) or
    /// not.
    Array,
    /// A what-if data table over [`Cell::array_range`].
    DataTable,
}

impl FormulaKind {
    /// `shared`, `array` or `data_table`.
    pub fn as_str(self) -> &'static str {
        match self {
            FormulaKind::Shared => "shared",
            FormulaKind::Array => "array",
            FormulaKind::DataTable => "data_table",
        }
    }
}

/// A cell, read by its address: a snapshot, which an edit of the workbook
/// does not change.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub address: CellRef,
    /// For a formula, the value its producer computed at its last save;
    /// [`Cell::stale`] says whether it may be out of date.
    pub value: Value,
    /// The formula as Excel displays it after its `=`: future functions
    /// without `_xlfn.`, a shared formula member's references moved from
    /// its master.
    pub formula: Option<String>,
    /// The formula as the file stores it.
    pub stored_formula: Option<String>,
    /// The kind of the formula the cell holds, or of the array formula or
    /// data table whose range holds it; `None` for a plain formula or none.
    pub formula_kind: Option<FormulaKind>,
    /// The array formula's or data table's range the cell is in, the cell
    /// that holds the formula or another one.
    pub array_range: Option<CellRange>,
    /// The cached value of the formula, or of the array formula whose range
    /// holds the cell, may be out of date: the workbook asks for a
    /// recalculation ([`crate::Workbook::recalculation_needed`]) or the
    /// formula is one Excel computes again on every open (`ca`).
    pub stale: bool,
    /// The number format code, a built-in id resolved; `General` when the
    /// style names none.
    pub number_format: String,
    /// The value as Excel displays it in a column wide enough for it,
    /// en-US.
    pub text: String,
    /// The number as a date and time, when the number format shows a date,
    /// a time or both (not a duration such as `[h]:mm`).
    pub datetime: Option<DateTime>,
    pub style: Arc<ResolvedStyle>,
}

impl Cell {
    /// The 0-based row.
    pub fn row(&self) -> u32 {
        self.address.row
    }

    /// The 0-based column.
    pub fn column(&self) -> u32 {
        self.address.column
    }
}

/// What an A1 reference names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reference {
    Cell(CellRef),
    Range(CellRange),
    /// Whole columns, 0-based, first to last.
    Columns(u32, u32),
    /// Whole rows, 0-based, first to last.
    Rows(u32, u32),
}

impl Reference {
    /// `B3`, `$B$3`, `A1:C10`, `A:C` or `2:5`, letters in any case.
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let invalid = || Error::InvalidReference {
            reference: text.to_owned(),
        };
        let Some((first, last)) = text.split_once(':') else {
            return CellRef::parse(text)
                .map(Reference::Cell)
                .ok_or_else(invalid);
        };
        if let Some(range) = CellRange::parse(text) {
            return Ok(Reference::Range(range));
        }
        let strip = |part: &'_ str| part.strip_prefix('$').unwrap_or(part).to_owned();
        let (first, last) = (strip(first), strip(last));
        if let (Some(a), Some(b)) = (parse_column(&first), parse_column(&last)) {
            return Ok(Reference::Columns(a.min(b), a.max(b)));
        }
        let row = |part: &str| {
            let number: u32 = part
                .bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| part.parse().ok())??;
            CellRef::parse(&format!("A{number}")).map(|cell| cell.row)
        };
        match (row(&first), row(&last)) {
            (Some(a), Some(b)) => Ok(Reference::Rows(a.min(b), a.max(b))),
            _ => Err(invalid()),
        }
    }

    /// The rectangle the reference names, whole columns and rows clipped to
    /// `used`; `None` when they meet no used cell.
    pub(crate) fn clipped(self, used: Option<CellRange>) -> Option<CellRange> {
        match self {
            Reference::Cell(cell) => Some(CellRange::new(cell, cell)),
            Reference::Range(range) => Some(range),
            Reference::Columns(first, last) => used.map(|used| {
                CellRange::new(
                    CellRef::new(used.first.row, first),
                    CellRef::new(used.last.row, last),
                )
            }),
            Reference::Rows(first, last) => used.map(|used| {
                CellRange::new(
                    CellRef::new(first, used.first.column),
                    CellRef::new(last, used.last.column),
                )
            }),
        }
    }

    /// Whether whole rows or columns are named.
    pub(crate) fn is_whole(self) -> bool {
        matches!(self, Reference::Columns(..) | Reference::Rows(..))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_read_cells_ranges_columns_and_rows() {
        let cell = |text| CellRef::parse(text).unwrap();
        for (text, expected) in [
            ("b3", Reference::Cell(cell("B3"))),
            ("$B$3", Reference::Cell(cell("B3"))),
            (
                "C10:A1",
                Reference::Range(CellRange::new(cell("A1"), cell("C10"))),
            ),
            ("c:$A", Reference::Columns(0, 2)),
            ("5:$2", Reference::Rows(1, 4)),
        ] {
            assert_eq!(Reference::parse(text).unwrap(), expected, "{text}");
        }
        for text in [
            "",
            "A",
            "3",
            "A1:",
            "A:1",
            "0:2",
            "A1:B",
            "XFE:XFE",
            "1:1048577",
        ] {
            assert!(Reference::parse(text).is_err(), "{text}");
        }
    }
}
