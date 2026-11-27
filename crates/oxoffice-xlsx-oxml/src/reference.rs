//! Cell references in the A1 form the parts write: `B3`, `$B$3`, and ranges
//! `A1:C10`. Rows and columns are 0-based here; the A1 text is 1-based.

use std::fmt;

/// The rows of a worksheet: 1,048,576.
pub const MAX_ROWS: u32 = 1_048_576;
/// The columns of a worksheet: 16,384, `A` to `XFD`.
pub const MAX_COLUMNS: u32 = 16_384;

/// A cell by its 0-based row and column. Ordered row-major.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellRef {
    pub row: u32,
    pub column: u32,
}

impl CellRef {
    pub fn new(row: u32, column: u32) -> Self {
        CellRef { row, column }
    }

    /// `B3` or `$B$3`, letters in any case, within the grid.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('$').unwrap_or(text);
        let letters = text.bytes().take_while(u8::is_ascii_alphabetic).count();
        let (letters, digits) = text.split_at(letters);
        let digits = digits.strip_prefix('$').unwrap_or(digits);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let row: u32 = digits.parse().ok()?;
        if row == 0 || row > MAX_ROWS {
            return None;
        }
        Some(CellRef {
            row: row - 1,
            column: parse_column(letters)?,
        })
    }
}

impl fmt::Display for CellRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", column_letters(self.column), self.row + 1)
    }
}

/// The letters of a 0-based column: `A` for 0, `XFD` for 16,383.
pub fn column_letters(column: u32) -> String {
    let mut letters = Vec::new();
    let mut rest = column + 1;
    while rest > 0 {
        let digit = (rest - 1) % 26;
        letters.push(b'A' + digit as u8);
        rest = (rest - 1) / 26;
    }
    letters.reverse();
    String::from_utf8(letters).expect("ASCII letters")
}

/// The 0-based column of `letters`, in any case, within the grid.
pub fn parse_column(letters: &str) -> Option<u32> {
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let mut column = 0u32;
    for byte in letters.bytes() {
        if !byte.is_ascii_alphabetic() {
            return None;
        }
        column = column * 26 + u32::from(byte.to_ascii_uppercase() - b'A') + 1;
    }
    (column <= MAX_COLUMNS).then(|| column - 1)
}

/// The columns a span of whole columns names, 0-based, both ends included:
/// `B`, `A:C` or `$A:$C`, letters in any case, the ends in either order.
pub fn parse_column_span(text: &str) -> Option<(u32, u32)> {
    let (a, b) = text.split_once(':').unwrap_or((text, text));
    let column = |end: &str| parse_column(end.strip_prefix('$').unwrap_or(end));
    let (a, b) = (column(a)?, column(b)?);
    Some((a.min(b), a.max(b)))
}

/// The rows a span of whole rows names, 0-based, both ends included: `3`,
/// `2:5` or `$2:$5`, the ends in either order.
pub fn parse_row_span(text: &str) -> Option<(u32, u32)> {
    let (a, b) = text.split_once(':').unwrap_or((text, text));
    let row = |end: &str| {
        let digits = end.strip_prefix('$').unwrap_or(end);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let row: u32 = digits.parse().ok()?;
        (1..=MAX_ROWS).contains(&row).then(|| row - 1)
    };
    let (a, b) = (row(a)?, row(b)?);
    Some((a.min(b), a.max(b)))
}

/// A rectangle of cells, its corners ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRange {
    pub first: CellRef,
    pub last: CellRef,
}

impl CellRange {
    /// The rectangle with these two corners, in either order.
    pub fn new(a: CellRef, b: CellRef) -> Self {
        CellRange {
            first: CellRef::new(a.row.min(b.row), a.column.min(b.column)),
            last: CellRef::new(a.row.max(b.row), a.column.max(b.column)),
        }
    }

    /// `A1:C10`, or one cell `A1`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.split_once(':') {
            Some((a, b)) => Some(CellRange::new(CellRef::parse(a)?, CellRef::parse(b)?)),
            None => CellRef::parse(text).map(|cell| CellRange::new(cell, cell)),
        }
    }

    pub fn contains(&self, cell: CellRef) -> bool {
        (self.first.row..=self.last.row).contains(&cell.row)
            && (self.first.column..=self.last.column).contains(&cell.column)
    }
}

impl fmt::Display for CellRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.first == self.last {
            write!(f, "{}", self.first)
        } else {
            write!(f, "{}:{}", self.first, self.last)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a1_references_read_and_write() {
        for (text, row, column, written) in [
            ("A1", 0, 0, "A1"),
            ("$B$3", 2, 1, "B3"),
            ("b$3", 2, 1, "B3"),
            ("XFD1048576", 1_048_575, 16_383, "XFD1048576"),
            ("AA10", 9, 26, "AA10"),
        ] {
            let cell = CellRef::parse(text).expect(text);
            assert_eq!((cell.row, cell.column), (row, column), "{text}");
            assert_eq!(cell.to_string(), written);
        }
        for text in ["", "A", "1", "A0", "XFE1", "A1048577", "A1B", "$$A1"] {
            assert_eq!(CellRef::parse(text), None, "{text}");
        }
        let range = CellRange::parse("C4:A2").unwrap();
        assert_eq!(range.to_string(), "A2:C4");
        assert!(range.contains(CellRef::new(2, 1)) && !range.contains(CellRef::new(4, 0)));
    }

    #[test]
    fn whole_columns_and_rows_read_as_spans() {
        for (text, columns, rows) in [
            ("B", Some((1, 1)), None),
            ("a:c", Some((0, 2)), None),
            ("$C:$A", Some((0, 2)), None),
            ("XFD", Some((16_383, 16_383)), None),
            ("3", None, Some((2, 2))),
            ("2:5", None, Some((1, 4))),
            ("$5:$2", None, Some((1, 4))),
            ("1048576", None, Some((1_048_575, 1_048_575))),
            ("", None, None),
            ("A:", None, None),
            ("XFE", None, None),
            ("0:3", None, None),
            ("1048577", None, None),
            ("A1", None, None),
            ("A:3", None, None),
        ] {
            assert_eq!(parse_column_span(text), columns, "columns {text:?}");
            assert_eq!(parse_row_span(text), rows, "rows {text:?}");
        }
    }
}
