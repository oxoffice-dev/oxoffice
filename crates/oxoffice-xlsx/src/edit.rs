//! What an edit changes and how a save writes it.
//!
//! An edit never rewrites a part in the package. It changes a model and
//! leaves the part as it was read:
//!
//! - the **workbook part** is a typed model ([`WorkbookXml`]) whose appended
//!   records and replaced elements are spliced into its source bytes;
//! - a **worksheet** keeps the rows and the children of the root an edit
//!   changed ([`SheetEdits`]), as they now are, beside the sheet's index:
//!   the save splices them into the source bytes, so an edit costs its rows
//!   and children and a sheet nobody edited stays bytes;
//! - the **package** takes at once what is not worth a model: the part, the
//!   relationship and the content type of a new sheet, the removal of the
//!   calculation chain.
//!
//! The first change of a workbook marks it edited ([`Workbook::mark_edited`]),
//! and a workbook that holds a formula then asks Excel to calculate it when it
//! opens it (`fullCalcOnLoad`) and writes no calculation chain, since any edit
//! may change what a formula reads; a workbook with no formula has no cached
//! value to go stale and is left alone, so Excel does not ask to save a file
//! it had no need to calculate. An edit that writes a formula asks for the
//! calculation whatever the sheets held ([`Workbook::require_calculation`]).
//! Every edit goes through these, and a call that changes nothing (the same
//! width again, panes removed from a sheet that has none) marks nothing.
//!
//! A save writes what edits changed in [`Workbook::write_staged_parts`], on a
//! copy of the package: a part nobody changed is not written again, and a
//! workbook nobody edited is saved as the package it was read from. A part
//! that edits come to change (shared strings, styles) is written there too.

use std::collections::BTreeMap;

use oxoffice_opc::OpcPackage;
use oxoffice_xlsx_oxml::namespaces::{rel_types, transitional_relationship_type};
use oxoffice_xlsx_oxml::worksheet::{
    CT_Row, CellChild, RowChild, SheetEdits, holds_formula, splice,
};
use oxoffice_xlsx_oxml::{Element, OxmlError};

use crate::open::is_external;
use crate::sheet::Worksheet;
use crate::{Error, Result, SheetKind, Workbook};

/// A sheet of a workbook that can be edited, by its position.
///
/// Holds the workbook mutably, so nothing else reads or edits it while the
/// sheet is being edited; the changes are the workbook's, and stay in it
/// until it is saved.
#[derive(Debug)]
pub struct WorksheetMut<'a> {
    pub(crate) workbook: &'a mut Workbook,
    pub(crate) position: usize,
}

impl Workbook {
    /// The sheet named `name` to edit: the exact name first, then the name
    /// in any case.
    pub fn sheet_mut(&mut self, name: &str) -> Option<WorksheetMut<'_>> {
        let position = self.sheet(name)?.position();
        Some(WorksheetMut {
            workbook: self,
            position,
        })
    }

    /// The sheet at a 0-based `position` to edit.
    pub fn sheet_at_mut(&mut self, position: usize) -> Option<WorksheetMut<'_>> {
        (position < self.sheets.len()).then_some(WorksheetMut {
            workbook: self,
            position,
        })
    }

    /// Notes that an edit changed the workbook. The save then writes what
    /// edits staged, and, when a sheet holds a formula, asks Excel to
    /// calculate the workbook on open ([`Workbook::require_calculation`]).
    pub(crate) fn mark_edited(&mut self) {
        self.edited = true;
        if self.holds_formula() {
            self.require_calculation();
        }
    }

    /// Whether any sheet holds a formula, as the sheets were read: one scan
    /// of their bytes, kept. An edit that writes a formula does not wait for
    /// it to be found there, it calls [`Workbook::require_calculation`].
    pub(crate) fn holds_formula(&self) -> bool {
        *self.formulas.get_or_init(|| {
            self.sheets
                .iter()
                .filter(|sheet| sheet.kind != SheetKind::Chartsheet)
                .filter_map(|sheet| self.package.get_part(&sheet.part))
                .any(holds_formula)
        })
    }

    /// Asks Excel to calculate the workbook on open and drops the
    /// calculation chain, once.
    pub(crate) fn require_calculation(&mut self) {
        if std::mem::replace(&mut self.calculates, true) {
            return;
        }
        self.workbook.set_full_calc_on_load();
        remove_calculation_chain(&mut self.package, &self.workbook_part);
    }

    /// Writes into `package`, a copy of the workbook's own, every part an
    /// edit changed: the sheets with their staged rows and children
    /// spliced in, the workbook part with its new records and `calcPr`, the
    /// shared strings, styles and metadata edits wrote into. With
    /// `drop_cached_values`, every formula's cached value goes too, and the
    /// workbook asks to be calculated on open.
    pub(crate) fn write_staged_parts(
        &self,
        package: &mut OpcPackage,
        drop_cached_values: bool,
    ) -> Result<()> {
        for position in 0..self.sheets.len() {
            let entry = &self.sheets[position];
            let sheet = Worksheet {
                workbook: self,
                position,
            };
            let dropped;
            let edits = if drop_cached_values && entry.kind == SheetKind::Worksheet {
                dropped = sheet.without_cached_values()?;
                &dropped
            } else {
                &entry.edits
            };
            if edits.is_empty() {
                continue;
            }
            let (xml, index) = sheet.index()?;
            let spliced = splice(xml, index, edits).map_err(|error| Error::MalformedPart {
                part: entry.part.clone(),
                message: error.to_string(),
            })?;
            package.set_part(&entry.part, spliced);
        }
        if drop_cached_values && !self.calculates {
            let mut workbook = self.workbook.clone();
            workbook.set_full_calc_on_load();
            remove_calculation_chain(package, &self.workbook_part);
            if workbook.part().is_changed() {
                package.set_part(&self.workbook_part, workbook.to_xml());
            }
        } else if self.workbook.part().is_changed() {
            package.set_part(&self.workbook_part, self.workbook.to_xml());
        }
        let mut write = |rel_type: &str, bytes: Vec<u8>| {
            if let Some(part) = self.related_part(rel_type) {
                package.set_part(&part, bytes);
            }
        };
        if let Some(Ok(Some(strings))) = self.read.shared_strings.get()
            && strings.is_changed()
        {
            write(rel_types::SHARED_STRINGS, strings.to_xml());
        }
        if let Some(Ok(Some(styles))) = self.read.styles_xml.get()
            && styles.part().is_changed()
        {
            write(rel_types::STYLES, styles.to_xml());
        }
        if let Some(metadata) = &self.edit.metadata
            && metadata.is_changed()
        {
            write(rel_types::SHEET_METADATA, metadata.to_xml());
        }
        Ok(())
    }
}

/// Removes the calculation chain of the workbook part from `package`: its
/// relationship, its part and its content type.
fn remove_calculation_chain(package: &mut OpcPackage, workbook_part: &str) {
    let chain = |relationship: &oxoffice_opc::Relationship| {
        transitional_relationship_type(&relationship.rel_type) == rel_types::CALC_CHAIN
            && !is_external(relationship)
    };
    let removed = package.remove_relationships_where(workbook_part, chain);
    for relationship in removed {
        let part = OpcPackage::resolve_rel_target(workbook_part, &relationship.target);
        package.remove_part(&part);
        package.remove_content_type_override(&part);
    }
}

impl Worksheet<'_> {
    /// The sheet's edits with every formula's cached value dropped, and those
    /// of the cells of array formulas' and data tables' ranges: the rows
    /// that hold one, as edits staged them or as written, without their `v`,
    /// `is`, `t` and `vm`.
    fn without_cached_values(&self) -> Result<SheetEdits> {
        let entry = self.entry();
        let mut edits = entry.edits.clone();
        let (xml, index) = self.index()?;
        let lookup = self.range_lookup()?;
        let mut numbers: std::collections::BTreeSet<u32> = index
            .rows
            .iter()
            .filter(|span| holds_formula(&xml[span.range.clone()]))
            .map(|span| span.row)
            .collect();
        for formula in index
            .range_formulas
            .iter()
            .chain(entry.array_edits.values().flatten())
        {
            numbers.extend(formula.range.first.row..=formula.range.last.row);
        }
        numbers.extend(entry.edits.rows.keys());
        for number in numbers {
            let Some(mut row) = self.rows_in(number, number)?.remove(&number) else {
                continue;
            };
            let ranges = lookup.in_row(number);
            let mut changed = false;
            let cells: Vec<u32> = crate::read::columns(&row)
                .into_iter()
                .map(|(column, _)| column)
                .collect();
            let mut at = 0;
            for child in &mut row.children {
                let RowChild::Cell(cell) = child else {
                    continue;
                };
                let column = cells.get(at).copied().unwrap_or(u32::MAX);
                at += 1;
                if cell.formula().is_none() && ranges.find(column).is_none() {
                    continue;
                }
                let before = cell.clone();
                cell.children.retain(|child| {
                    !matches!(child, CellChild::Value(_) | CellChild::InlineString(_))
                });
                cell.tag.set("t", None);
                cell.tag.set("vm", None);
                changed |= *cell != before;
            }
            if changed {
                edits.rows.insert(number, Some(row));
            }
        }
        Ok(edits)
    }
}

impl<'a> WorksheetMut<'a> {
    /// The name, as its tab shows it.
    pub fn name(&self) -> &str {
        self.read().name()
    }

    /// The 0-based position among the workbook's tabs.
    pub fn position(&self) -> usize {
        self.position
    }

    /// The id of the relationship from the workbook part to this sheet's
    /// part: what names the sheet whatever its position or name.
    pub fn relationship_id(&self) -> &str {
        self.read().relationship_id()
    }

    pub(crate) fn read(&self) -> Worksheet<'_> {
        Worksheet {
            workbook: self.workbook,
            position: self.position,
        }
    }

    /// Refuses a sheet that holds no cells, naming what it is.
    pub(crate) fn require_worksheet(&self, what: &str) -> Result<()> {
        match self.read().kind() {
            SheetKind::Worksheet => Ok(()),
            kind => Err(Error::Refused(format!(
                "{:?} is a {}, which has no {what}",
                self.name(),
                kind.as_str()
            ))),
        }
    }

    fn malformed(&self, error: OxmlError) -> Error {
        Error::MalformedPart {
            part: self.read().entry().part.clone(),
            message: error.to_string(),
        }
    }

    /// The prefix the elements this sheet's edits write take: the one the
    /// root binds to SpreadsheetML, none for a default namespace.
    pub(crate) fn prefix(&self) -> Result<String> {
        let (_, index) = self.read().index()?;
        Ok(index.prefix.clone().unwrap_or_default())
    }

    /// The child of the root named `name` as it stands: as an edit staged
    /// it, else as written.
    pub(crate) fn child(&self, name: &str) -> Result<Option<Element>> {
        let sheet = self.read();
        if let Some(staged) = sheet.entry().edits.children.get(name) {
            return staged
                .as_deref()
                .map(Element::parse)
                .transpose()
                .map_err(|error| self.malformed(error));
        }
        let (xml, index) = sheet.index()?;
        index
            .child(name)
            .map(|child| Element::parse(&xml[child.range.clone()]))
            .transpose()
            .map_err(|error| self.malformed(error))
    }

    /// Stages the child `name` as `after`, which was `before`: nothing when
    /// they are the same, else the change, which marks the workbook edited.
    pub(crate) fn stage_child(
        &mut self,
        name: &str,
        before: &Option<Element>,
        after: Option<Element>,
    ) {
        if *before == after {
            return;
        }
        let bytes = after.map(|element| element.to_xml());
        self.workbook.sheets[self.position]
            .edits
            .children
            .insert(name.to_owned(), bytes);
        self.workbook.mark_edited();
    }

    /// The rows `first..=last` (0-based) the sheet has, as an edit staged
    /// them or as written, by row number.
    pub(crate) fn rows_in(&self, first: u32, last: u32) -> Result<BTreeMap<u32, CT_Row>> {
        self.read().rows_in(first, last)
    }

    /// Stages the rows given (number, row as it was, row as it is now). A row
    /// that is as it was is not staged; a change marks the workbook edited.
    /// Gives whether a row changed.
    pub(crate) fn stage_rows(&mut self, rows: Vec<(u32, Option<CT_Row>, CT_Row)>) -> bool {
        let mut changed = false;
        for (number, before, after) in rows {
            if before.as_ref() == Some(&after) {
                continue;
            }
            self.workbook.sheets[self.position]
                .edits
                .rows
                .insert(number, Some(after));
            changed = true;
        }
        if changed {
            // The cells may have changed: the used range is read again.
            self.workbook.sheets[self.position].used = Default::default();
            self.workbook.mark_edited();
        }
        changed
    }
}
