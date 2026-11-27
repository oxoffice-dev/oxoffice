//! Reading cells: values, formulas, displayed texts and styles; the used
//! range, merges and comments of a sheet.
//!
//! A cell is read from its row, which is read when a cell of it is; the
//! shared strings, the styles (with the theme) and the persons of threaded
//! comments are read once per workbook, the first time a cell needs them.

use std::collections::HashSet;
use std::sync::Arc;

use oxoffice_numfmt::{DateKind, DateTime, FormatOptions};
use oxoffice_opc::OpcPackage;
use oxoffice_xlsx_oxml::comments::{Person, read_notes, read_persons, read_threaded_comments};
use oxoffice_xlsx_oxml::namespaces::{rel_types, transitional_relationship_type};
use oxoffice_xlsx_oxml::reference::{MAX_COLUMNS, MAX_ROWS};
use oxoffice_xlsx_oxml::shared_strings::SharedStrings;
use oxoffice_xlsx_oxml::styles::StylesXml;
use oxoffice_xlsx_oxml::worksheet::{CT_Cell, CT_Row, SheetIndex};
use oxoffice_xlsx_oxml::{CellRange, CellRef, Element, parse_f64, xstring};

use crate::cell::{Cell, CellError, FormulaKind, Reference, Value};
use crate::comments::{Comment, merge};
use crate::open::is_external;
use crate::ranges::{RangeEntry, RangeLookup};
use crate::sheet::SheetKind;
use crate::style::{CellFormat, StyleTable, theme_colors};
use crate::{DateSystem, Error, Lazy, Result, Workbook, Worksheet};

/// The most cells a bulk read of values returns: past it, a read asks for
/// a smaller range rather than filling memory with empty cells.
pub const MAX_BULK_CELLS: u64 = 10_000_000;

/// The parts of a workbook its cells read, each read once.
#[derive(Clone, Debug, Default)]
pub(crate) struct ReadParts {
    shared_strings: Lazy<Option<SharedStrings>>,
    styles: Lazy<Arc<StyleTable>>,
    persons: Lazy<Vec<Person>>,
}

/// A lazily read value, or the error its read gave, as the facade's error.
fn lazy<'a, T>(
    cell: &'a Lazy<T>,
    part: &str,
    read: impl FnOnce() -> std::result::Result<T, String>,
) -> Result<&'a T> {
    cell.get_or_init(read)
        .as_ref()
        .map_err(|message| Error::MalformedPart {
            part: part.to_owned(),
            message: message.clone(),
        })
}

impl Workbook {
    /// Whether the cached values of formulas may be out of date: the file
    /// asks for a recalculation on open (`calcPr fullCalcOnLoad`, as
    /// XlsxWriter writes it over placeholder results), or calculation is
    /// manual and was not run at the last save.
    pub fn recalculation_needed(&self) -> bool {
        self.workbook.calc_pr().is_some_and(|calc| {
            calc.full_calc_on_load() || (calc.calc_mode() == "manual" && !calc.calc_on_save())
        })
    }

    /// The part the first workbook relationship of `rel_type` names.
    fn related_part(&self, rel_type: &str) -> Option<String> {
        related_part(&self.package, &self.workbook_part, rel_type)
    }

    fn part_bytes(&self, part: &str) -> Result<&[u8]> {
        self.package
            .get_part_shared(part)
            .map(|bytes| bytes.as_slice())
            .ok_or_else(|| Error::MissingPart {
                part: part.to_owned(),
            })
    }

    /// The shared strings, `None` for a workbook without them.
    fn shared_strings(&self) -> Result<Option<&SharedStrings>> {
        let part = self.related_part(rel_types::SHARED_STRINGS);
        let name = part.clone().unwrap_or_default();
        lazy(&self.read.shared_strings, &name, || {
            let Some(part) = part else {
                return Ok(None);
            };
            let bytes = self
                .package
                .get_part_shared(&part)
                .ok_or_else(|| format!("package part is missing: {part}"))?;
            SharedStrings::parse(Arc::clone(bytes))
                .map(Some)
                .map_err(|error| error.to_string())
        })
        .map(Option::as_ref)
    }

    /// The cell formats, resolved once.
    pub(crate) fn style_table(&self) -> Result<&StyleTable> {
        let part = self.related_part(rel_types::STYLES);
        let name = part.clone().unwrap_or_default();
        lazy(&self.read.styles, &name, || {
            let styles = match &part {
                Some(part) => {
                    let bytes = self
                        .package
                        .get_part_shared(part)
                        .ok_or_else(|| format!("package part is missing: {part}"))?;
                    Some(StylesXml::parse(Arc::clone(bytes)).map_err(|error| error.to_string())?)
                }
                None => None,
            };
            let theme = match self.related_part(rel_types::THEME) {
                Some(theme) => match self.package.get_part_shared(&theme) {
                    Some(bytes) => {
                        theme_colors(bytes).map_err(|error| format!("{theme}: {error}"))?
                    }
                    None => Default::default(),
                },
                None => Default::default(),
            };
            Ok(Arc::new(StyleTable::build(styles.as_ref(), &theme)))
        })
        .map(Arc::as_ref)
    }

    /// The people of threaded comments.
    fn persons(&self) -> Result<&[Person]> {
        let part = self.related_part(rel_types::PERSON);
        let name = part.clone().unwrap_or_default();
        lazy(&self.read.persons, &name, || match &part {
            Some(part) => match self.package.get_part_shared(part) {
                Some(bytes) => read_persons(bytes).map_err(|error| error.to_string()),
                None => Ok(Vec::new()),
            },
            None => Ok(Vec::new()),
        })
        .map(Vec::as_slice)
    }

    fn format_options(&self) -> FormatOptions {
        FormatOptions::default().with_date_system(self.numfmt_date_system())
    }

    fn numfmt_date_system(&self) -> oxoffice_numfmt::DateSystem {
        match self.date_system() {
            DateSystem::Date1900 => oxoffice_numfmt::DateSystem::Date1900,
            DateSystem::Date1904 => oxoffice_numfmt::DateSystem::Date1904,
        }
    }
}

/// The part the first relationship of `rel_type` from `source` names,
/// Strict types read as their Transitional twins.
fn related_part(package: &OpcPackage, source: &str, rel_type: &str) -> Option<String> {
    package
        .get_part_rels(source)?
        .items
        .iter()
        .find(|relationship| {
            transitional_relationship_type(&relationship.rel_type) == rel_type
                && !is_external(relationship)
        })
        .map(|relationship| OpcPackage::resolve_rel_target(source, &relationship.target))
}

/// The cells of a row with their 0-based columns: `r`, or the column after
/// the cell before. A column written twice keeps its first cell. Cells
/// written in order, as every producer writes them, cost one comparison
/// each; the columns seen are kept in a set from the first cell out of
/// order.
fn columns(row: &CT_Row) -> Vec<(u32, &CT_Cell)> {
    let mut cells: Vec<(u32, &CT_Cell)> = Vec::new();
    let mut seen: Option<HashSet<u32>> = None;
    let mut highest: Option<u32> = None;
    let mut next = 0;
    for cell in row.cells() {
        let column = cell.r().map_or(next, |r| r.column);
        next = column + 1;
        let new = if let Some(seen) = seen.as_mut() {
            seen.insert(column)
        } else if highest.is_none_or(|highest| column > highest) {
            true
        } else {
            let mut known: HashSet<u32> = cells.iter().map(|(known, _)| *known).collect();
            let new = known.insert(column);
            seen = Some(known);
            new
        };
        if new {
            highest = Some(highest.map_or(column, |highest| highest.max(column)));
            cells.push((column, cell));
        }
    }
    cells
}

/// Whether a cell holds a value or a formula.
fn holds_content(cell: &CT_Cell) -> bool {
    cell.formula().is_some()
        || cell.inline_string().is_some()
        || cell.value().is_some_and(|value| !value.is_empty())
}

/// The serial of an ISO 8601 date, time or both (`t="d"`): `2026-09-01`,
/// `2026-09-01T08:30:00.250`, `08:30`; a zone (`Z`, `+02:00`) is ignored, the
/// wall clock kept.
fn iso_serial(text: &str, system: oxoffice_numfmt::DateSystem) -> Option<f64> {
    let number = |part: &str, digits: usize| -> Option<u32> {
        (part.len() == digits && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse().ok())?
    };
    let (date, time) = match text.split_once('T') {
        Some((date, time)) => (Some(date), Some(time)),
        None if text.contains(':') => (None, Some(text)),
        None => (Some(text), None),
    };
    let (year, month, day) = match date {
        Some(date) => {
            let mut parts = date.split('-');
            let year = number(parts.next()?, 4)?;
            let month = number(parts.next()?, 2)?;
            let day = number(parts.next()?, 2)?;
            if parts.next().is_some() {
                return None;
            }
            (year as i32, month, day)
        }
        None => match system {
            oxoffice_numfmt::DateSystem::Date1900 => (1899, 12, 31),
            oxoffice_numfmt::DateSystem::Date1904 => (1904, 1, 1),
        },
    };
    let (mut hour, mut minute, mut second, mut millisecond) = (0, 0, 0, 0);
    if let Some(time) = time {
        let time = time
            .strip_suffix('Z')
            .or_else(|| {
                let at = time.rfind(['+', '-'])?;
                Some(&time[..at])
            })
            .unwrap_or(time);
        let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
        let mut parts = clock.split(':');
        hour = number(parts.next()?, 2)?;
        minute = number(parts.next()?, 2)?;
        second = parts.next().map_or(Some(0), |part| number(part, 2))?;
        if parts.next().is_some() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let digits: String = fraction.chars().chain("000".chars()).take(3).collect();
        millisecond = digits.parse().ok()?;
    }
    system.to_serial(DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        millisecond,
    })
}

/// What reads the cells of one worksheet.
struct SheetReader<'a> {
    sheet: Worksheet<'a>,
    xml: &'a [u8],
    index: &'a SheetIndex,
    ranges: &'a RangeLookup,
    styles: &'a StyleTable,
    options: FormatOptions,
    recalculation: bool,
}

impl<'a> SheetReader<'a> {
    fn part(&self) -> &str {
        &self.sheet.entry().part
    }

    fn malformed(&self, message: String) -> Error {
        Error::MalformedPart {
            part: self.part().to_owned(),
            message,
        }
    }

    /// The rows from `first` to `last`, read, with their 0-based numbers,
    /// as edits staged them or as written; a row written twice keeps its
    /// first writing.
    fn rows(&self, first: u32, last: u32) -> Result<Vec<(u32, CT_Row)>> {
        Ok(self.sheet.rows_in(first, last)?.into_iter().collect())
    }

    /// The value a cell stores.
    fn value(&self, cell: &CT_Cell, address: CellRef) -> Result<Value> {
        let kind = cell.t();
        if kind == "inlineStr"
            && let Some(text) = cell.inline_string()
        {
            return Ok(Value::Text(text.plain_text()));
        }
        let Some(stored) = cell.value() else {
            return Ok(Value::Empty);
        };
        if stored.is_empty() && !matches!(kind.as_str(), "str" | "inlineStr") {
            return Ok(Value::Empty);
        }
        Ok(match kind.as_str() {
            "s" => {
                let strings = self.sheet.workbook.shared_strings()?;
                let text = stored
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|item| strings?.text(item));
                Value::Text(text.ok_or_else(|| {
                    self.malformed(format!(
                        "cell {address} names shared string {stored}, which the workbook does not have"
                    ))
                })?)
            }
            "str" | "inlineStr" => Value::Text(xstring::decode(stored).into_owned()),
            "b" => match stored.trim() {
                "1" | "true" => Value::Boolean(true),
                "0" | "false" => Value::Boolean(false),
                other => {
                    return Err(
                        self.malformed(format!("cell {address} holds `{other}`, not a boolean"))
                    );
                }
            },
            "e" => Value::Error(CellError::from_code(stored.trim())),
            "d" => Value::Number(
                iso_serial(stored.trim(), self.options.date_system).ok_or_else(|| {
                    self.malformed(format!(
                        "cell {address} holds `{stored}`, not an ISO 8601 date of the workbook"
                    ))
                })?,
            ),
            "n" => Value::Number(parse_f64(stored).ok_or_else(|| {
                self.malformed(format!("cell {address} holds `{stored}`, not a number"))
            })?),
            other => {
                return Err(self.malformed(format!("cell {address} has an unknown type `{other}`")));
            }
        })
    }

    /// The stored formula of a cell, its kind, its range and its `ca`.
    fn formula(
        &self,
        cell: &CT_Cell,
        address: CellRef,
    ) -> Result<(Option<String>, Option<FormulaKind>, bool)> {
        let Some(formula) = cell.formula() else {
            return Ok((None, None, false));
        };
        let own = (!formula.text.is_empty()).then(|| formula.text.clone());
        let (stored, kind) = match formula.t().as_str() {
            "shared" => {
                let stored = match own {
                    Some(text) => text,
                    None => self.shared_member(formula.si(), address)?,
                };
                (Some(stored), Some(FormulaKind::Shared))
            }
            "array" => (own, Some(FormulaKind::Array)),
            "dataTable" => (own, Some(FormulaKind::DataTable)),
            _ => (own, None),
        };
        Ok((stored, kind, formula.ca()))
    }

    /// A shared formula member's formula: its master's, moved to it.
    fn shared_member(&self, si: Option<u32>, address: CellRef) -> Result<String> {
        let master = si
            .and_then(|si| self.index.shared_formulas.get(&si))
            .ok_or_else(|| {
                self.malformed(format!(
                    "cell {address} is a member of a shared formula the sheet does not hold"
                ))
            })?;
        let rows = i64::from(address.row) - i64::from(master.cell.row);
        let columns = i64::from(address.column) - i64::from(master.cell.column);
        oxoffice_xlsx_formula::move_references(&master.text, rows, columns).map_err(|error| {
            self.malformed(format!(
                "the shared formula of {} cannot be read: {error}",
                master.cell
            ))
        })
    }

    /// The style record of a cell the sheet does not write: its row's when
    /// the row has one, else its column's, else record 0.
    fn absent_style(&self, row: Option<&CT_Row>, column: u32) -> Result<u32> {
        if let Some(s) = row.filter(|row| row.custom_format()).and_then(CT_Row::s) {
            return Ok(s);
        }
        // The columns as an edit staged them, else as written.
        let cols = match self.sheet.entry().edits.children.get("cols") {
            Some(Some(staged)) => Element::parse(staged),
            Some(None) => return Ok(0),
            None => match self.index.child("cols") {
                Some(cols) => Element::parse(&self.xml[cols.range.clone()]),
                None => return Ok(0),
            },
        }
        .map_err(|error| self.malformed(error.to_string()))?;
        Ok(cols
            .elements_named("col")
            .find(|col| {
                let min = col.tag.u32("min").unwrap_or(0);
                let max = col.tag.u32("max").unwrap_or(0);
                (min..=max).contains(&(column + 1))
            })
            .and_then(|col| col.tag.u32("style"))
            .unwrap_or(0))
    }

    /// The cell at `address`, written as `cell` in `row` or absent, in the
    /// array formula's or data table's `range` when one holds it. A cell of
    /// such a range without its own formula takes the range's kind, range
    /// and staleness.
    fn cell(
        &self,
        address: CellRef,
        row: Option<&CT_Row>,
        cell: Option<&CT_Cell>,
        range: Option<RangeEntry>,
    ) -> Result<Cell> {
        let s = match cell {
            Some(cell) => cell.s(),
            None => self.absent_style(row, address.column)?,
        };
        let format = self.styles.get(s);
        let (value, (stored, kind, ca)) = match cell {
            Some(cell) => (self.value(cell, address)?, self.formula(cell, address)?),
            None => (Value::Empty, (None, None, false)),
        };
        let (kind, array_range, volatile) = match (kind, range) {
            (Some(own @ (FormulaKind::Array | FormulaKind::DataTable)), range) => {
                let written = cell
                    .and_then(CT_Cell::formula)
                    .and_then(|formula| formula.reference());
                let range = written
                    .or(range.map(|range| range.range))
                    .unwrap_or(CellRange::new(address, address));
                (Some(own), Some(range), ca)
            }
            (None, Some(range)) if stored.is_none() => {
                (Some(range.kind), Some(range.range), range.volatile)
            }
            (kind, range) => (kind, range.map(|range| range.range), ca),
        };
        let computed = stored.is_some() || kind.is_some();
        Ok(Cell {
            address,
            text: self.displayed(&value, format),
            datetime: self.datetime(&value, format),
            formula: stored.as_deref().map(oxoffice_xlsx_formula::to_displayed),
            stored_formula: stored,
            formula_kind: kind,
            array_range,
            stale: computed && (self.recalculation || volatile),
            number_format: format.style.number_format.clone(),
            style: Arc::clone(&format.style),
            value,
        })
    }

    fn displayed(&self, value: &Value, format: &CellFormat) -> String {
        match value {
            Value::Empty => String::new(),
            Value::Number(number) => format.format.format_number(*number, &self.options).text,
            Value::Text(text) => format.format.format_text(text).text,
            Value::Boolean(true) => "TRUE".to_owned(),
            Value::Boolean(false) => "FALSE".to_owned(),
            Value::Error(error) => error.code().to_owned(),
        }
    }

    fn datetime(&self, value: &Value, format: &CellFormat) -> Option<DateTime> {
        let Value::Number(number) = value else {
            return None;
        };
        match format.date_kind? {
            DateKind::Date | DateKind::Time | DateKind::DateTime => {
                self.options.date_system.to_datetime(*number)
            }
            DateKind::Duration => None,
        }
    }

    /// The smallest range holding every cell with a value or a formula,
    /// read once.
    fn used_range(&self) -> Result<Option<CellRange>> {
        let entry = self.sheet.entry();
        lazy(&entry.used, &entry.part, || {
            self.read_used_range().map_err(|error| match error {
                Error::MalformedPart { message, .. } => message,
                error => error.to_string(),
            })
        })
        .copied()
    }

    fn read_used_range(&self) -> Result<Option<CellRange>> {
        let mut used: Option<CellRange> = None;
        for (number, row) in self.rows(0, u32::MAX)? {
            for (column, cell) in columns(&row) {
                if holds_content(cell) {
                    let address = CellRef::new(number, column);
                    used = Some(match used {
                        Some(range) => CellRange::new(
                            CellRef::new(
                                range.first.row.min(number),
                                range.first.column.min(column),
                            ),
                            CellRef::new(range.last.row.max(number), range.last.column.max(column)),
                        ),
                        None => CellRange::new(address, address),
                    });
                }
            }
        }
        Ok(used)
    }

    /// The rectangle a reference names, whole rows and columns clipped to
    /// the used range.
    fn rectangle(&self, reference: &str) -> Result<Option<CellRange>> {
        let reference = Reference::parse(reference)?;
        let used = if reference.is_whole() {
            self.used_range()?
        } else {
            None
        };
        Ok(reference.clipped(used))
    }

    /// The cells of `range` with a value or a formula, row by row.
    fn cells(&self, range: CellRange) -> Result<Vec<Cell>> {
        let mut cells = Vec::new();
        for (number, row) in self.rows(range.first.row, range.last.row)? {
            let mut written = columns(&row);
            written.sort_by_key(|(column, _)| *column);
            let ranges = self.ranges.in_row(number);
            for (column, cell) in written {
                let address = CellRef::new(number, column);
                if range.contains(address) && holds_content(cell) {
                    cells.push(self.cell(address, Some(&row), Some(cell), ranges.find(column))?);
                }
            }
        }
        Ok(cells)
    }
}

impl<'a> Worksheet<'a> {
    /// What reads this sheet's cells; a sheet that holds no cells is
    /// refused.
    fn reader(&self) -> Result<SheetReader<'a>> {
        self.require_cells()?;
        let (xml, index) = self.index()?;
        Ok(SheetReader {
            sheet: *self,
            xml,
            index,
            ranges: self
                .entry()
                .ranges
                .get_or_init(|| RangeLookup::new(&index.range_formulas)),
            styles: self.workbook.style_table()?,
            options: self.workbook.format_options(),
            recalculation: self.workbook.recalculation_needed(),
        })
    }

    /// Refuses a sheet that holds no cells: a chartsheet, a dialogsheet, a
    /// macrosheet.
    pub(crate) fn require_cells(&self) -> Result<()> {
        if self.kind() == SheetKind::Worksheet {
            return Ok(());
        }
        Err(Error::NotAWorksheet {
            sheet: self.name().to_owned(),
            kind: self.kind(),
        })
    }

    /// The cell at `reference` (`B3`, `$B$3`): its value, formula,
    /// displayed text and style. A cell the sheet does not write reads as
    /// empty.
    pub fn cell(&self, reference: &str) -> Result<Cell> {
        match Reference::parse(reference)? {
            Reference::Cell(address) => self.cell_at(address.row, address.column),
            _ => Err(Error::NotACell {
                reference: reference.to_owned(),
            }),
        }
    }

    /// The cell at a 0-based row and column.
    pub fn cell_at(&self, row: u32, column: u32) -> Result<Cell> {
        if row >= MAX_ROWS || column >= MAX_COLUMNS {
            return Err(Error::InvalidReference {
                reference: format!("({row}, {column})"),
            });
        }
        let address = CellRef::new(row, column);
        let reader = self.reader()?;
        let rows = reader.rows(row, row)?;
        let written = rows.first().map(|(_, row)| row);
        let cell = written.and_then(|row| {
            columns(row)
                .into_iter()
                .find(|(written, _)| *written == column)
                .map(|(_, cell)| cell)
        });
        reader.cell(address, written, cell, reader.ranges.find(address))
    }

    /// The values of `range` (`A1:C10`, `B3`, whole columns `A:C` or rows
    /// `2:5` clipped to the used range), row by row: one list per row of
    /// the range, one value per column, [`Value::Empty`] where the sheet
    /// has no value.
    pub fn values(&self, range: &str) -> Result<Vec<Vec<Value>>> {
        let reader = self.reader()?;
        let Some(range) = reader.rectangle(range)? else {
            return Ok(Vec::new());
        };
        let height = range.last.row - range.first.row + 1;
        let width = range.last.column - range.first.column + 1;
        if u64::from(height) * u64::from(width) > MAX_BULK_CELLS {
            return Err(Error::RangeTooLarge {
                range: range.to_string(),
            });
        }
        let mut values = vec![vec![Value::Empty; width as usize]; height as usize];
        for (number, row) in reader.rows(range.first.row, range.last.row)? {
            for (column, cell) in columns(&row) {
                if range.contains(CellRef::new(number, column)) {
                    values[(number - range.first.row) as usize]
                        [(column - range.first.column) as usize] =
                        reader.value(cell, CellRef::new(number, column))?;
                }
            }
        }
        Ok(values)
    }

    /// The rectangle `range` names, whole rows and columns clipped to the
    /// used range as [`Worksheet::values`] reads them; `None` when they meet
    /// no used cell.
    pub fn range(&self, range: &str) -> Result<Option<CellRange>> {
        self.reader()?.rectangle(range)
    }

    /// The cells of `range` that hold a value or a formula, row by row; the
    /// range is read as [`Worksheet::values`] reads it.
    pub fn cells(&self, range: &str) -> Result<Vec<Cell>> {
        let reader = self.reader()?;
        match reader.rectangle(range)? {
            Some(range) => reader.cells(range),
            None => Ok(Vec::new()),
        }
    }

    /// The cells of `range` that hold a value or a formula, row by row.
    pub(crate) fn cells_in(&self, range: CellRange) -> Result<Vec<Cell>> {
        self.reader()?.cells(range)
    }

    /// The smallest range holding every cell with a value or a formula,
    /// `None` for a sheet without one; read once. Neither `dimension`
    /// (Google writes none) nor the rows written (Google pads them to 1000)
    /// say it.
    pub fn used_range(&self) -> Result<Option<CellRange>> {
        self.reader()?.used_range()
    }

    /// The merged ranges, in the order written.
    pub fn merged_ranges(&self) -> Result<Vec<CellRange>> {
        let reader = self.reader()?;
        let Some(merges) = reader.index.child("mergeCells") else {
            return Ok(Vec::new());
        };
        let merges = Element::parse(&reader.xml[merges.range.clone()])
            .map_err(|error| reader.malformed(error.to_string()))?;
        merges
            .elements_named("mergeCell")
            .map(|merge| {
                let reference = merge.tag.get("ref").unwrap_or_default();
                CellRange::parse(&reference).ok_or_else(|| {
                    reader.malformed(format!("a merged range is `{reference}`, not a range"))
                })
            })
            .collect()
    }

    /// The notes and comment threads of the sheet, by cell, row by row. See
    /// [`Comment`].
    pub fn comments(&self) -> Result<Vec<Comment>> {
        self.require_cells()?;
        let workbook = self.workbook;
        let part = &self.entry().part;
        let read = |rel_type: &str| -> Result<Option<(String, &[u8])>> {
            match related_part(&workbook.package, part, rel_type) {
                Some(related) => Ok(Some((related.clone(), workbook.part_bytes(&related)?))),
                None => Ok(None),
            }
        };
        let malformed = |part: &str, error: oxoffice_xlsx_oxml::OxmlError| Error::MalformedPart {
            part: part.to_owned(),
            message: error.to_string(),
        };
        let notes = match read(rel_types::COMMENTS)? {
            Some((part, xml)) => {
                let notes = read_notes(xml).map_err(|error| malformed(&part, error))?;
                Some((part, notes))
            }
            None => None,
        };
        let threads = match read(rel_types::THREADED_COMMENT)? {
            Some((part, xml)) => {
                let threads =
                    read_threaded_comments(xml).map_err(|error| malformed(&part, error))?;
                Some((part, threads))
            }
            None => None,
        };
        let persons = if threads.is_some() {
            workbook.persons()?
        } else {
            &[]
        };
        merge(notes, threads, persons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_gives_each_column_once_in_the_order_written() {
        let row = CT_Row::parse(
            br#"<row r="1"><c r="A1"/><c r="C1"/><c r="B1"/><c r="C1"><v>2</v></c><c/></row>"#,
        )
        .unwrap();
        let read: Vec<(u32, bool)> = columns(&row)
            .into_iter()
            .map(|(column, cell)| (column, cell.value().is_some()))
            .collect();
        // C written twice keeps its first cell; the cell without `r` follows
        // the one before it.
        assert_eq!(read, [(0, false), (2, false), (1, false), (3, false)]);
    }

    #[test]
    fn an_iso_date_reads_as_the_serial_of_its_system() {
        let d1900 = oxoffice_numfmt::DateSystem::Date1900;
        let d1904 = oxoffice_numfmt::DateSystem::Date1904;
        for (text, system, serial) in [
            ("2026-09-01", d1900, Some(46266.0)),
            ("2026-09-01", d1904, Some(44804.0)),
            ("2026-09-01T12:00:00", d1900, Some(46266.5)),
            (
                "2026-09-01T06:00:00.250Z",
                d1900,
                Some(46_266.250_002_893_52),
            ),
            ("2026-09-01T18:00+02:00", d1900, Some(46266.75)),
            ("12:00", d1900, Some(0.5)),
            ("2026-9-1", d1900, None),
            ("2026-09-01T25:00", d1900, None),
            ("noon", d1900, None),
        ] {
            let read = iso_serial(text, system);
            assert!(
                match (read, serial) {
                    (Some(read), Some(serial)) => (read - serial).abs() < 1e-9,
                    (read, serial) => read.is_none() && serial.is_none(),
                },
                "{text}: {read:?}"
            );
        }
    }
}
