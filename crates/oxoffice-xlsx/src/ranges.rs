//! The array formulas and data tables of a sheet, found by the cell: what
//! gives a cell of their range its kind, its range and its staleness.

use oxoffice_xlsx_oxml::worksheet::{RangeFormula, RangeFormulaKind};
use oxoffice_xlsx_oxml::{CellRange, CellRef};

use crate::cell::FormulaKind;

/// An array formula or a data table over a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RangeEntry {
    pub range: CellRange,
    pub kind: FormulaKind,
    /// The anchor's formula is computed again on every open (`ca`).
    pub volatile: bool,
}

/// The ranges of a sheet, sorted by first row, with an implicit balanced
/// tree over them that keeps the last row each subtree reaches: the ranges
/// over a row are found in a time that follows their number and the log of
/// the sheet's, whatever their heights.
#[derive(Clone, Debug, Default)]
pub(crate) struct RangeLookup {
    entries: Vec<RangeEntry>,
    reach: Vec<u32>,
}

impl RangeLookup {
    pub(crate) fn new(formulas: &[RangeFormula]) -> Self {
        let mut entries: Vec<RangeEntry> = formulas
            .iter()
            .map(|formula| RangeEntry {
                range: formula.range,
                kind: match formula.kind {
                    RangeFormulaKind::Array => FormulaKind::Array,
                    RangeFormulaKind::DataTable => FormulaKind::DataTable,
                },
                volatile: formula.volatile,
            })
            .collect();
        entries.sort_by_key(|entry| (entry.range.first.row, entry.range.first.column));
        let mut reach = vec![0; entries.len()];
        fill_reach(&entries, &mut reach, 0, entries.len());
        RangeLookup { entries, reach }
    }

    /// The ranges over `row`, by first column.
    pub(crate) fn in_row(&self, row: u32) -> RowRanges<'_> {
        let mut found = Vec::new();
        self.collect(row, 0, self.entries.len(), &mut found);
        found.sort_by_key(|entry| entry.range.first.column);
        RowRanges(found)
    }

    fn collect<'a>(&'a self, row: u32, low: usize, high: usize, found: &mut Vec<&'a RangeEntry>) {
        if low >= high {
            return;
        }
        let middle = (low + high) / 2;
        if self.reach[middle] < row {
            return;
        }
        self.collect(row, low, middle, found);
        let entry = &self.entries[middle];
        if entry.range.first.row <= row {
            if entry.range.last.row >= row {
                found.push(entry);
            }
            self.collect(row, middle + 1, high, found);
        }
    }

    /// The range holding `cell`.
    pub(crate) fn find(&self, cell: CellRef) -> Option<RangeEntry> {
        self.in_row(cell.row).find(cell.column)
    }

    /// The ranges that share a cell with `range`.
    pub(crate) fn meeting(&self, range: CellRange) -> Vec<RangeEntry> {
        let mut found = Vec::new();
        self.collect_rows(
            range.first.row,
            range.last.row,
            0,
            self.entries.len(),
            &mut found,
        );
        found
            .into_iter()
            .filter(|entry| entry.range.intersects(&range))
            .copied()
            .collect()
    }

    fn collect_rows<'a>(
        &'a self,
        first: u32,
        last: u32,
        low: usize,
        high: usize,
        found: &mut Vec<&'a RangeEntry>,
    ) {
        if low >= high {
            return;
        }
        let middle = (low + high) / 2;
        if self.reach[middle] < first {
            return;
        }
        self.collect_rows(first, last, low, middle, found);
        let entry = &self.entries[middle];
        if entry.range.first.row <= last {
            if entry.range.last.row >= first {
                found.push(entry);
            }
            self.collect_rows(first, last, middle + 1, high, found);
        }
    }
}

/// The last row the ranges of `low..high` reach, kept at each subtree's
/// middle.
fn fill_reach(entries: &[RangeEntry], reach: &mut [u32], low: usize, high: usize) -> u32 {
    if low >= high {
        return 0;
    }
    let middle = (low + high) / 2;
    let left = fill_reach(entries, reach, low, middle);
    let right = fill_reach(entries, reach, middle + 1, high);
    reach[middle] = entries[middle].range.last.row.max(left).max(right);
    reach[middle]
}

/// The ranges over one row, by first column.
pub(crate) struct RowRanges<'a>(Vec<&'a RangeEntry>);

impl RowRanges<'_> {
    /// The range holding `column` of the row.
    pub(crate) fn find(&self, column: u32) -> Option<RangeEntry> {
        let at = self
            .0
            .partition_point(|entry| entry.range.first.column <= column);
        let entry = self.0.get(at.checked_sub(1)?)?;
        (entry.range.last.column >= column).then_some(**entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_range_over_a_cell_is_found_whatever_the_heights() {
        let formula = |range: &str, kind, volatile| RangeFormula {
            cell: CellRange::parse(range).unwrap().first,
            range: CellRange::parse(range).unwrap(),
            kind,
            volatile,
        };
        let lookup = RangeLookup::new(&[
            formula("B2:C3", RangeFormulaKind::Array, false),
            formula("A1:A1000", RangeFormulaKind::Array, true),
            formula("E2", RangeFormulaKind::DataTable, false),
            formula("D1", RangeFormulaKind::Array, false),
            formula("B5", RangeFormulaKind::Array, false),
        ]);
        for (cell, expected) in [
            ("A500", Some("A1:A1000")),
            ("A1000", Some("A1:A1000")),
            ("A1001", None),
            ("C3", Some("B2:C3")),
            ("B4", None),
            ("E2", Some("E2")),
            ("D1", Some("D1")),
            ("F1", None),
            ("B5", Some("B5")),
        ] {
            let found = lookup.find(CellRef::parse(cell).unwrap());
            assert_eq!(
                found.map(|entry| entry.range.to_string()).as_deref(),
                expected,
                "{cell}"
            );
        }
        let tall = lookup.find(CellRef::parse("A2").unwrap()).unwrap();
        assert!(tall.volatile && tall.kind == FormulaKind::Array);
        let table = lookup.find(CellRef::parse("E2").unwrap()).unwrap();
        assert_eq!(table.kind, FormulaKind::DataTable);
        let meeting = |range: &str| -> Vec<String> {
            let mut found: Vec<String> = lookup
                .meeting(CellRange::parse(range).unwrap())
                .iter()
                .map(|entry| entry.range.to_string())
                .collect();
            found.sort();
            found
        };
        assert_eq!(meeting("A999:C1002"), ["A1:A1000"]);
        assert_eq!(meeting("C2:E9"), ["B2:C3", "E2"]);
        assert!(meeting("F1:G9").is_empty());
    }
}
