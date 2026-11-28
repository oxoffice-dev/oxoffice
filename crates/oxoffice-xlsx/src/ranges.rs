//! The array formulas and data tables of a sheet, found by the cell: what
//! gives a cell of their range its kind, its range and its staleness.

use std::collections::{BTreeMap, HashMap};

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

/// The height classes of the ranges: class `k` holds the ranges `2^k` to
/// `2^(k+1) - 1` rows tall, and a sheet has `2^20` rows.
const CLASSES: usize = 21;

/// The ranges of a sheet, each in the class of its height, by first row and
/// column. A range of class `k` over row `r` starts at most `2^(k+1) - 2`
/// rows above it, so the ranges over a row are found by one ordered scan per
/// class, in a time that follows their number and the log of the sheet's,
/// whatever their heights; an edit adds or removes one in the log of their
/// number, so a write never rebuilds them.
#[derive(Clone, Debug)]
pub(crate) struct RangeLookup {
    classes: Vec<BTreeMap<(u32, u32), RangeEntry>>,
    /// Where each range is, by the cell that holds its formula.
    anchors: HashMap<CellRef, (usize, (u32, u32))>,
}

impl Default for RangeLookup {
    fn default() -> Self {
        RangeLookup {
            classes: vec![BTreeMap::new(); CLASSES],
            anchors: HashMap::new(),
        }
    }
}

/// The class of a range's height.
fn class_of(range: &CellRange) -> usize {
    let height = range.last.row - range.first.row + 1;
    ((u32::BITS - 1 - height.leading_zeros()) as usize).min(CLASSES - 1)
}

/// The first row a range of `class` over row `row` may start at.
fn lowest_start(class: usize, row: u32) -> u32 {
    row.saturating_sub((1u32 << (class + 1)).saturating_sub(2))
}

impl RangeLookup {
    pub(crate) fn new(formulas: &[RangeFormula]) -> Self {
        let mut lookup = RangeLookup::default();
        for formula in formulas {
            lookup.insert(formula);
        }
        lookup
    }

    /// Adds the range of `formula`; of two ranges at the same place, the
    /// first one stays.
    pub(crate) fn insert(&mut self, formula: &RangeFormula) {
        let range = formula.range;
        let class = class_of(&range);
        let key = (range.first.row, range.first.column);
        let entry = RangeEntry {
            range,
            kind: match formula.kind {
                RangeFormulaKind::Array => FormulaKind::Array,
                RangeFormulaKind::DataTable => FormulaKind::DataTable,
            },
            volatile: formula.volatile,
        };
        if let std::collections::btree_map::Entry::Vacant(vacant) = self.classes[class].entry(key) {
            vacant.insert(entry);
            self.anchors.insert(formula.cell, (class, key));
        }
    }

    /// Removes the range whose formula `anchor` holds, if any.
    pub(crate) fn remove(&mut self, anchor: CellRef) {
        if let Some((class, key)) = self.anchors.remove(&anchor) {
            self.classes[class].remove(&key);
        }
    }

    /// The ranges that may reach rows `first..=last`, class by class.
    fn candidates(&self, first: u32, last: u32) -> impl Iterator<Item = &RangeEntry> {
        self.classes
            .iter()
            .enumerate()
            .filter(|(_, ranges)| !ranges.is_empty())
            .flat_map(move |(class, ranges)| {
                ranges
                    .range((lowest_start(class, first), 0)..=(last, u32::MAX))
                    .map(|(_, entry)| entry)
            })
    }

    /// The ranges over `row`, by first column.
    pub(crate) fn in_row(&self, row: u32) -> RowRanges<'_> {
        let mut found: Vec<&RangeEntry> = self
            .candidates(row, row)
            .filter(|entry| entry.range.last.row >= row)
            .collect();
        found.sort_by_key(|entry| entry.range.first.column);
        RowRanges(found)
    }

    /// The range holding `cell`.
    pub(crate) fn find(&self, cell: CellRef) -> Option<RangeEntry> {
        self.in_row(cell.row).find(cell.column)
    }

    /// The ranges that share a cell with `range`.
    pub(crate) fn meeting(&self, range: CellRange) -> Vec<RangeEntry> {
        self.candidates(range.first.row, range.last.row)
            .filter(|entry| entry.range.intersects(&range))
            .copied()
            .collect()
    }
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
        // An edit adds and removes a range in place.
        let mut lookup = lookup.clone();
        lookup.remove(CellRef::parse("A1").unwrap());
        lookup.insert(&formula("G3:H900", RangeFormulaKind::Array, false));
        assert_eq!(lookup.find(CellRef::parse("A500").unwrap()), None);
        assert_eq!(
            lookup
                .find(CellRef::parse("H700").unwrap())
                .map(|entry| entry.range.to_string())
                .as_deref(),
            Some("G3:H900")
        );
        assert_eq!(meeting("C2:E9"), ["B2:C3", "E2"]);
        assert!(meeting("F1:G9").is_empty());
    }
}
