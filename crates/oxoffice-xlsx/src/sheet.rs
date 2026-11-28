//! The sheets of a workbook, in the order of `sheets`: their names, states
//! and kinds, the part each is read from, and the index one pass over a
//! sheet builds the first time it is read.

use std::collections::BTreeMap;
use std::sync::Arc;

use oxoffice_opc::OpcPackage;
use oxoffice_xlsx_oxml::namespaces::{rel_types, transitional_relationship_type};
use oxoffice_xlsx_oxml::workbook::WorkbookXml;
use oxoffice_xlsx_oxml::worksheet::{CT_Row, PageSetupElements, SheetEdits, SheetIndex};

use crate::open::is_external;
use crate::page_setup::PageSetup;
use crate::{Error, Lazy, Result, Workbook};

/// Whether a sheet shows in Excel's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SheetState {
    Visible,
    /// Hidden, listed by Excel's Unhide.
    Hidden,
    /// Hidden, and not listed by Excel's Unhide.
    VeryHidden,
}

impl SheetState {
    fn parse(state: &str) -> Self {
        match state {
            "hidden" => SheetState::Hidden,
            "veryHidden" => SheetState::VeryHidden,
            _ => SheetState::Visible,
        }
    }

    /// The name SpreadsheetML gives it: `visible`, `hidden`, `veryHidden`.
    pub fn as_str(self) -> &'static str {
        match self {
            SheetState::Visible => "visible",
            SheetState::Hidden => "hidden",
            SheetState::VeryHidden => "veryHidden",
        }
    }
}

/// What a sheet holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SheetKind {
    /// Cells.
    Worksheet,
    /// One chart and no cells.
    Chartsheet,
    /// An Excel 5 dialog.
    Dialogsheet,
    /// Excel 4 macros.
    Macrosheet,
}

impl SheetKind {
    fn of_relationship(rel_type: &str) -> Option<Self> {
        Some(match transitional_relationship_type(rel_type) {
            rel_types::WORKSHEET => SheetKind::Worksheet,
            rel_types::CHARTSHEET => SheetKind::Chartsheet,
            rel_types::DIALOGSHEET => SheetKind::Dialogsheet,
            rel_types::MACROSHEET | rel_types::INTERNATIONAL_MACROSHEET => SheetKind::Macrosheet,
            _ => return None,
        })
    }

    /// `worksheet`, `chartsheet`, `dialogsheet` or `macrosheet`.
    pub fn as_str(self) -> &'static str {
        match self {
            SheetKind::Worksheet => "worksheet",
            SheetKind::Chartsheet => "chartsheet",
            SheetKind::Dialogsheet => "dialogsheet",
            SheetKind::Macrosheet => "macrosheet",
        }
    }
}

/// A sheet of the workbook and its part.
#[derive(Clone, Debug)]
pub(crate) struct SheetEntry {
    pub(crate) name: String,
    pub(crate) state: SheetState,
    pub(crate) kind: SheetKind,
    pub(crate) relationship_id: String,
    pub(crate) part: String,
    pub(crate) index: Lazy<SheetIndex>,
    /// What edits changed in the part, written by the next save: its rows
    /// and children as they now are, the rest of the part kept as read.
    pub(crate) edits: SheetEdits,
    /// The array formulas and data tables, found by the cell; read from the
    /// index the first time a cell needs them.
    pub(crate) ranges: std::sync::OnceLock<crate::ranges::RangeLookup>,
    /// The used range, read the first time it is asked for. An edit of
    /// the sheet's cells resets it.
    pub(crate) used: Lazy<Option<oxoffice_xlsx_oxml::CellRange>>,
}

impl SheetEntry {
    /// A worksheet the workbook has just been given: its part is in the
    /// package.
    pub(crate) fn added(name: &str, relationship_id: String, part: String) -> Self {
        SheetEntry {
            name: name.to_owned(),
            state: SheetState::Visible,
            kind: SheetKind::Worksheet,
            relationship_id,
            part,
            index: Lazy::new(),
            edits: SheetEdits::default(),
            ranges: Default::default(),
            used: Lazy::new(),
        }
    }
}

impl SheetEntry {
    /// The sheets `workbook` lists, each with the part its relationship
    /// names.
    pub(crate) fn resolve(
        package: &OpcPackage,
        workbook_part: &str,
        workbook: &WorkbookXml,
    ) -> Result<Vec<Self>> {
        let relationships = package.get_part_rels(workbook_part);
        let mut sheets = Vec::new();
        for record in workbook.sheets() {
            let name = record.name();
            let relationship_id = record
                .relationship_id()
                .ok_or_else(|| Error::MalformedPart {
                    part: workbook_part.to_owned(),
                    message: format!("sheet {name} names no relationship"),
                })?;
            let relationship = relationships
                .and_then(|relationships| relationships.get_by_id(&relationship_id))
                .filter(|relationship| !is_external(relationship))
                .ok_or_else(|| Error::MissingRelationship {
                    source_part: workbook_part.to_owned(),
                    relationship_id: relationship_id.clone(),
                })?;
            let kind = SheetKind::of_relationship(&relationship.rel_type).ok_or_else(|| {
                Error::MalformedPart {
                    part: workbook_part.to_owned(),
                    message: format!(
                        "sheet {name}: relationship {relationship_id} is a {}, not a sheet",
                        relationship.rel_type
                    ),
                }
            })?;
            let part = OpcPackage::resolve_rel_target(workbook_part, &relationship.target);
            if !package.contains_part(&part) {
                return Err(Error::MissingPart { part });
            }
            sheets.push(SheetEntry {
                name,
                state: SheetState::parse(&record.state()),
                kind,
                relationship_id,
                part,
                index: Lazy::new(),
                edits: SheetEdits::default(),
                ranges: Default::default(),
                used: Lazy::new(),
            });
        }
        Ok(sheets)
    }
}

/// A sheet of a workbook, by its position.
#[derive(Clone, Copy, Debug)]
pub struct Worksheet<'a> {
    pub(crate) workbook: &'a Workbook,
    pub(crate) position: usize,
}

impl Workbook {
    /// The sheets, in the order of the workbook's tabs.
    pub fn sheets(&self) -> impl ExactSizeIterator<Item = Worksheet<'_>> {
        (0..self.sheets.len()).map(|position| Worksheet {
            workbook: self,
            position,
        })
    }

    /// The sheet at a 0-based `position`.
    pub fn sheet_at(&self, position: usize) -> Option<Worksheet<'_>> {
        (position < self.sheets.len()).then_some(Worksheet {
            workbook: self,
            position,
        })
    }

    /// The sheet named `name`: the exact name first, then the name in any
    /// case, as Excel compares sheet names.
    pub fn sheet(&self, name: &str) -> Option<Worksheet<'_>> {
        let lower = name.to_lowercase();
        let position = self
            .sheets
            .iter()
            .position(|sheet| sheet.name == name)
            .or_else(|| {
                self.sheets
                    .iter()
                    .position(|sheet| sheet.name.to_lowercase() == lower)
            })?;
        self.sheet_at(position)
    }
}

impl<'a> Worksheet<'a> {
    pub(crate) fn entry(&self) -> &'a SheetEntry {
        &self.workbook.sheets[self.position]
    }

    /// The name, as its tab shows it.
    pub fn name(&self) -> &'a str {
        &self.entry().name
    }

    /// The 0-based position among the workbook's tabs.
    pub fn position(&self) -> usize {
        self.position
    }

    pub fn state(&self) -> SheetState {
        self.entry().state
    }

    pub fn kind(&self) -> SheetKind {
        self.entry().kind
    }

    /// The id of the relationship from the workbook part to this sheet's
    /// part: what names the sheet whatever its position or name.
    pub fn relationship_id(&self) -> &'a str {
        &self.entry().relationship_id
    }

    /// The sheet's index, built by one pass the first time.
    pub(crate) fn index(&self) -> Result<(&'a [u8], &'a SheetIndex)> {
        let entry = self.entry();
        let xml = self.part_bytes()?;
        let index = entry
            .index
            .get_or_init(|| SheetIndex::build(xml).map_err(|error| error.to_string()));
        match index {
            Ok(index) => Ok((xml, index)),
            Err(message) => Err(Error::MalformedPart {
                part: entry.part.clone(),
                message: message.clone(),
            }),
        }
    }

    fn part_bytes(&self) -> Result<&'a [u8]> {
        let entry = self.entry();
        self.workbook
            .package
            .get_part_shared(&entry.part)
            .map(|bytes| Arc::as_ref(bytes).as_slice())
            .ok_or_else(|| Error::MissingPart {
                part: entry.part.clone(),
            })
    }

    /// The rows `first..=last` (0-based) the sheet has, as an edit staged
    /// them or as written, by row number; of two rows written with the same
    /// number, the first.
    pub(crate) fn rows_in(&self, first: u32, last: u32) -> Result<BTreeMap<u32, CT_Row>> {
        let (xml, index) = self.index()?;
        let staged = &self.entry().edits.rows;
        let mut rows = BTreeMap::new();
        for span in index.rows_in(first, last).iter() {
            if !rows.contains_key(&span.row) && !staged.contains_key(&span.row) {
                let row = CT_Row::parse(&xml[span.range.clone()]).map_err(|error| {
                    Error::MalformedPart {
                        part: self.entry().part.clone(),
                        message: error.to_string(),
                    }
                })?;
                rows.insert(span.row, row);
            }
        }
        for (&number, row) in staged.range(first..=last) {
            if let Some(row) = row {
                rows.insert(number, row.clone());
            }
        }
        Ok(rows)
    }

    /// The page setup: margins, paper, scale, header and footer, breaks,
    /// print area and titles, schema defaults for what the sheet leaves
    /// out.
    pub fn page_setup(&self) -> Result<PageSetup> {
        let (xml, index) = self.index()?;
        let elements =
            PageSetupElements::read(xml, index).map_err(|error| Error::MalformedPart {
                part: self.entry().part.clone(),
                message: error.to_string(),
            })?;
        Ok(PageSetup::resolve(
            &elements,
            &self.workbook.workbook,
            self.position,
        ))
    }
}
