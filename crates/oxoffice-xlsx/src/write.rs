//! Writing cells: values, formulas, clears and bulk writes.
//!
//! Every write goes through one path ([`WorksheetMut::apply`]): it reads the
//! rows it writes, refuses what Excel would repair or what this version does
//! not write (part of an array formula, a value under a merged range, a
//! table header renamed, a pivot table's cells) before it changes anything,
//! then dissolves the shared formula groups it writes into (each other
//! member becomes a plain formula with its own text), writes the cells and
//! stages their rows. Text goes to the shared strings, a date's format and a
//! style edit to the styles, a spilling formula's block to `metadata.xml`,
//! each created when the workbook lacks it. An edit costs its rows, and the
//! rows of a shared group it dissolves, once.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use oxoffice_numfmt::DateTime;
use oxoffice_xlsx_formula::{TokenKind, check_stored, result_extent, to_stored, tokenize};
use oxoffice_xlsx_oxml::metadata::MetadataXml;
use oxoffice_xlsx_oxml::namespaces::{MAIN, STRICT_MAIN, content_types, rel_types, strict};
use oxoffice_xlsx_oxml::reference::{MAX_COLUMNS, MAX_ROWS};
use oxoffice_xlsx_oxml::worksheet::{
    CT_Cell, CT_CellFormula, CT_Row, CellChild, RangeFormula, RangeFormulaKind, RowChild,
    TextElement,
};
use oxoffice_xlsx_oxml::{CellRange, CellRef, Element, Tag, format_f64, qualified, xstring};

use crate::cell::{CellError, Reference, Value};
use crate::format::{Restyle, StyleIndex, StylePatch};
use crate::read::{columns, holds_content, related_parts};
use crate::{DateSystem, Error, Result, Workbook, WorksheetMut};

/// The most characters a cell holds.
const MAX_TEXT: usize = 32_767;

/// How a formula is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FormulaOptions {
    /// The text is the form the file stores (`_xlfn.XLOOKUP(...)`,
    /// `_xlpm.` parameters), written as it is once checked, rather than the
    /// form Excel displays.
    pub stored: bool,
    /// A dynamic array formula that spills from its cell, as Excel 365
    /// writes `SORT`, `FILTER` or `UNIQUE`: an array formula marked dynamic
    /// in `metadata.xml`, over the range [`Spill`] says. `None` for a formula
    /// of one cell.
    pub spill: Option<Spill>,
}

/// Where a dynamic array formula spills.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Spill {
    /// From its cell over its result, written as its `ref` so that Google
    /// Sheets, which takes the range from the file, spills it too: the
    /// result's range when the formula's text decides it (a range,
    /// `SORT(A2:A10)`, `A2:A10*2`, `SEQUENCE(5)`), or the most it can fill
    /// when the data decide how much (`UNIQUE(A2:A10)`, `FILTER`: the range
    /// they read), which Excel and Google Sheets fill as far as the result
    /// goes, leaving the rest empty; see
    /// [`oxoffice_xlsx_formula::result_extent`]. Its other cells hold
    /// nothing. When neither tells it (`SEQUENCE(B1)`, `GROUPBY`), the
    /// formula is written on its cell alone, as XlsxWriter writes it: Excel
    /// spills it on open, Google Sheets shows the first value alone.
    Anchor,
    /// Over this range, which starts at the formula's cell and whose other
    /// cells hold nothing: written as its `ref`, so Google Sheets spills it
    /// too. A range wider than the result was not checked in Google.
    Range(CellRange),
}

/// A date, a time of day, both, or a duration: written as the serial
/// number of the workbook's date system, with a format that shows it when
/// the cell's format is General.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DateValue {
    /// A day, shown `yyyy-mm-dd`.
    Date { year: i32, month: u32, day: u32 },
    /// A time of day, shown `hh:mm:ss`.
    Time {
        hour: u32,
        minute: u32,
        second: u32,
        millisecond: u32,
    },
    /// A day and a time, shown `yyyy-mm-dd hh:mm:ss`.
    DateTime(DateTime),
    /// A duration in seconds, 0 or more, shown `[h]:mm:ss`.
    Duration(f64),
}

impl DateValue {
    /// The format code a cell shown as General takes: locale-independent,
    /// unlike Excel's built-in short date.
    pub fn number_format(&self) -> &'static str {
        match self {
            DateValue::Date { .. } => "yyyy-mm-dd",
            DateValue::Time { .. } => "hh:mm:ss",
            DateValue::DateTime(_) => "yyyy-mm-dd hh:mm:ss",
            DateValue::Duration(_) => "[h]:mm:ss",
        }
    }

    /// The serial of the value in `system`.
    fn serial(&self, system: DateSystem) -> Result<f64> {
        let system = match system {
            DateSystem::Date1900 => oxoffice_numfmt::DateSystem::Date1900,
            DateSystem::Date1904 => oxoffice_numfmt::DateSystem::Date1904,
        };
        let invalid = |what: String| Error::InvalidArgument(what);
        match *self {
            DateValue::Date { year, month, day } => system
                .to_serial(DateTime {
                    year,
                    month,
                    day,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    millisecond: 0,
                })
                .ok_or_else(|| {
                    invalid(format!(
                        "{year:04}-{month:02}-{day:02} is not a date the workbook can hold"
                    ))
                }),
            DateValue::Time {
                hour,
                minute,
                second,
                millisecond,
            } => {
                if hour > 23 || minute > 59 || second > 59 || millisecond > 999 {
                    return Err(invalid(format!(
                        "{hour:02}:{minute:02}:{second:02} is not a time of day"
                    )));
                }
                let milliseconds = ((hour * 60 + minute) * 60 + second) * 1000 + millisecond;
                Ok(f64::from(milliseconds) / 86_400_000.0)
            }
            DateValue::DateTime(at) => system.to_serial(at).ok_or_else(|| {
                invalid(format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02} is not a date and time the workbook can hold",
                    at.year, at.month, at.day, at.hour, at.minute, at.second
                ))
            }),
            DateValue::Duration(seconds) => {
                if !seconds.is_finite() || seconds < 0.0 {
                    return Err(invalid(format!(
                        "a duration is a number of seconds from 0, not {seconds}"
                    )));
                }
                Ok(seconds / 86_400.0)
            }
        }
    }
}

/// What a write puts in a cell: a value, a date, or a formula. Text is
/// never read as a formula; a formula is always this explicit variant.
#[derive(Clone, Debug, PartialEq)]
pub enum CellInput {
    /// A number, a text, a boolean, an error, or nothing (the content
    /// cleared). An empty text clears the content too.
    Value(Value),
    Date(DateValue),
    /// The formula text, with or without its `=`, and how it is written.
    Formula(String, FormulaOptions),
}

impl CellInput {
    /// The input a text stands for, read strictly as a CSV import reads a
    /// field: `-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?` with no leading zero
    /// but `0` itself and at most 15 significant digits is a number (`007`
    /// and a 16-digit card number stay text), `TRUE` or `FALSE` in any case
    /// a boolean, `YYYY-MM-DD` with ` HH:MM[:SS]` or `THH:MM[:SS]` or not a
    /// date, an empty text nothing, anything else text, a leading `=`
    /// included.
    pub fn infer(text: &str) -> CellInput {
        if text.is_empty() {
            return CellInput::Value(Value::Empty);
        }
        if let Some(number) = strict_number(text) {
            return CellInput::Value(Value::Number(number));
        }
        if text.eq_ignore_ascii_case("true") {
            return CellInput::Value(Value::Boolean(true));
        }
        if text.eq_ignore_ascii_case("false") {
            return CellInput::Value(Value::Boolean(false));
        }
        if let Some(date) = iso_date(text, false) {
            return CellInput::Date(date);
        }
        CellInput::Value(Value::Text(text.to_owned()))
    }
}

impl From<Value> for CellInput {
    fn from(value: Value) -> Self {
        CellInput::Value(value)
    }
}

impl From<f64> for CellInput {
    fn from(number: f64) -> Self {
        CellInput::Value(Value::Number(number))
    }
}

impl From<i32> for CellInput {
    fn from(number: i32) -> Self {
        CellInput::Value(Value::Number(f64::from(number)))
    }
}

impl From<&str> for CellInput {
    fn from(text: &str) -> Self {
        CellInput::Value(Value::Text(text.to_owned()))
    }
}

impl From<String> for CellInput {
    fn from(text: String) -> Self {
        CellInput::Value(Value::Text(text))
    }
}

impl From<bool> for CellInput {
    fn from(boolean: bool) -> Self {
        CellInput::Value(Value::Boolean(boolean))
    }
}

impl From<CellError> for CellInput {
    fn from(error: CellError) -> Self {
        CellInput::Value(Value::Error(error))
    }
}

impl From<DateValue> for CellInput {
    fn from(date: DateValue) -> Self {
        CellInput::Date(date)
    }
}

/// What a clear removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Clear {
    /// Values and formulas; the formats stay.
    Contents,
    /// The formats: the cells take the workbook's default format.
    Formats,
    /// Both.
    All,
}

/// `-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?`, no leading zero but `0` itself,
/// at most 15 significant digits.
fn strict_number(text: &str) -> Option<f64> {
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    if let Some(exponent) = exponent {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if !digits(exponent) {
            return None;
        }
    }
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (mantissa, None),
    };
    if !digits(whole) || fraction.is_some_and(|fraction| !digits(fraction)) {
        return None;
    }
    if whole.len() > 1 && whole.starts_with('0') {
        return None;
    }
    let significant = whole
        .bytes()
        .chain(fraction.unwrap_or("").bytes())
        .skip_while(|byte| *byte == b'0')
        .count();
    if significant > 15 {
        return None;
    }
    text.parse::<f64>().ok().filter(|number| number.is_finite())
}

impl DateValue {
    /// The date, date and time, or time of day `text` writes:
    /// `YYYY-MM-DD`, with ` HH:MM[:SS]` or `THH:MM[:SS]` or not, or
    /// `HH:MM[:SS]` alone. A date the calendar lacks is none.
    pub fn parse(text: &str) -> Option<DateValue> {
        iso_date(text, true)
    }
}

/// `YYYY-MM-DD`, with ` HH:MM[:SS]` or `THH:MM[:SS]` or not; with
/// `time_alone`, `HH:MM[:SS]` alone too, a time of day. A date the calendar
/// lacks is none: checked in the 1900 system, which holds the 1904 one's
/// dates, the workbook's system deciding only the serial.
fn iso_date(text: &str, time_alone: bool) -> Option<DateValue> {
    fn number(part: &str, length: usize) -> Option<u32> {
        (part.len() == length && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse().ok())?
    }
    fn time(text: &str) -> Option<(u32, u32, u32)> {
        let mut parts = text.split(':');
        let hour = number(parts.next()?, 2)?;
        let minute = number(parts.next()?, 2)?;
        let second = parts.next().map_or(Some(0), |part| number(part, 2))?;
        (parts.next().is_none() && hour <= 23 && minute <= 59 && second <= 59)
            .then_some((hour, minute, second))
    }
    if text.len() <= 8 && text.contains(':') {
        if !time_alone {
            return None;
        }
        let (hour, minute, second) = time(text)?;
        return Some(DateValue::Time {
            hour,
            minute,
            second,
            millisecond: 0,
        });
    }
    let (date, clock) = match text.get(10..11) {
        Some(" " | "T") => (&text[..10], Some(&text[11..])),
        None if text.len() == 10 => (text, None),
        _ => return None,
    };
    let mut parts = date.split('-');
    let year = number(parts.next()?, 4)? as i32;
    let month = number(parts.next()?, 2)?;
    let day = number(parts.next()?, 2)?;
    if parts.next().is_some() {
        return None;
    }
    let value = match clock {
        Some(clock) => {
            let (hour, minute, second) = time(clock)?;
            DateValue::DateTime(DateTime {
                year,
                month,
                day,
                hour,
                minute,
                second,
                millisecond: 0,
            })
        }
        None => DateValue::Date { year, month, day },
    };
    // A date the calendar lacks (a 30 February) is text.
    value.serial(DateSystem::Date1900).ok()?;
    Some(value)
}

/// The content a write gives a cell, checked.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Content {
    Empty,
    Number(f64),
    Text(String),
    Boolean(bool),
    /// An error value's code.
    Error(&'static str),
    /// A formula as stored, and the range a dynamic array formula spills
    /// over.
    Formula {
        stored: String,
        spill: Option<SpillRange>,
    },
}

/// The range a dynamic array formula spills over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpillRange {
    pub range: CellRange,
    pub origin: SpillOrigin,
}

/// Where the range of a dynamic array formula comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpillOrigin {
    /// The call gave it ([`Spill::Range`]).
    Given,
    /// The result's, which the formula's text decides, or the formula's cell
    /// alone when nothing tells it.
    Result,
    /// The most the result can fill: the data decide how much of it.
    Bound,
}

impl SpillOrigin {
    /// How the formula spills over its range.
    fn spills(self) -> &'static str {
        match self {
            SpillOrigin::Bound => "can spill",
            _ => "spills",
        }
    }

    /// What to do about a range that meets something: `actions`, and a
    /// range that stops before it for a range given, or the range the result
    /// fills for one the data decide.
    fn way_out(self, actions: &[&str]) -> String {
        let mut actions = actions.to_vec();
        match self {
            SpillOrigin::Given => actions.push("give a range that stops before it"),
            SpillOrigin::Result => {}
            SpillOrigin::Bound => actions.push("give the range its result fills"),
        }
        match actions.split_last() {
            Some((last, [])) => (*last).to_owned(),
            Some((last, rest)) => format!("{}, or {last}", rest.join(", ")),
            None => String::new(),
        }
    }
}

impl Content {
    fn is_formula(&self) -> bool {
        matches!(self, Content::Formula { .. })
    }
}

/// What a write does to the style of a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StyleChange {
    Keep,
    /// The default format (`s` removed).
    Remove,
    /// The patch of the call.
    Patch,
}

/// What a write does to one cell.
#[derive(Clone, Debug)]
pub(crate) struct CellWrite {
    pub at: CellRef,
    /// The new content, or `None` to keep it.
    pub content: Option<Content>,
    /// The number format the cell takes when its format is General: a
    /// date's.
    pub date_code: Option<&'static str>,
    pub style: StyleChange,
}

/// The cells a call writes, for the array formula check: each cell given
/// (a whole array is written when each of its cells is), or a rectangle (a
/// clear, which reaches cells the sheet does not write).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Region {
    Cells,
    Rectangle(CellRange),
}

/// What a write of a cell's content may not change, read once per sheet.
#[derive(Clone, Debug, Default)]
pub(crate) struct Guards {
    merges: Vec<CellRange>,
    headers: Vec<TableHeader>,
    pivots: Vec<(String, CellRange)>,
}

/// The header row of a table: its row, its first column and the names of
/// its columns.
#[derive(Clone, Debug)]
struct TableHeader {
    table: String,
    row: u32,
    first: u32,
    names: Vec<String>,
}

/// The parts edits write and the indexes that find what they hold, made at
/// the first edit that needs each.
#[derive(Clone, Debug, Default)]
pub(crate) struct EditParts {
    /// The plain shared strings by text, the first of each.
    strings: Option<HashMap<String, usize>>,
    pub(crate) styles: Option<StyleIndex>,
    pub(crate) metadata: Option<MetadataXml>,
}

impl Content {
    /// The content `input` gives the cell `at` in a workbook of `system`,
    /// checked, with the number format a date shows with.
    fn of(
        at: CellRef,
        input: CellInput,
        system: DateSystem,
    ) -> Result<(Content, Option<&'static str>)> {
        Ok(match input {
            CellInput::Value(value) => (Content::of_value(value)?, None),
            CellInput::Date(date) => (
                Content::Number(date.serial(system)?),
                Some(date.number_format()),
            ),
            CellInput::Formula(text, options) => {
                let spill = match options.spill {
                    None => None,
                    // Widened to the result's range by the sheet's write.
                    Some(Spill::Anchor) => Some(SpillRange {
                        range: CellRange::new(at, at),
                        origin: SpillOrigin::Result,
                    }),
                    Some(Spill::Range(range)) if range.first == at => Some(SpillRange {
                        range,
                        origin: SpillOrigin::Given,
                    }),
                    Some(Spill::Range(range)) => {
                        return Err(Error::InvalidArgument(format!(
                            "a formula spills from its cell: the range {range} starts at {}, not at {at}",
                            range.first
                        )));
                    }
                };
                (
                    Content::Formula {
                        stored: stored_formula(&text, options)?,
                        spill,
                    },
                    None,
                )
            }
        })
    }

    fn of_value(value: Value) -> Result<Content> {
        Ok(match value {
            Value::Empty => Content::Empty,
            Value::Number(number) if number.is_finite() => {
                // Excel has no negative zero.
                Content::Number(if number == 0.0 { 0.0 } else { number })
            }
            Value::Number(number) => {
                return Err(Error::InvalidArgument(format!(
                    "a cell holds a finite number, not {number}"
                )));
            }
            Value::Text(text) if text.is_empty() => Content::Empty,
            Value::Text(text) => {
                let length = text.encode_utf16().count();
                if length > MAX_TEXT {
                    return Err(Error::InvalidArgument(format!(
                        "a cell holds at most 32,767 characters, and this text has {length}"
                    )));
                }
                Content::Text(text)
            }
            Value::Boolean(boolean) => Content::Boolean(boolean),
            Value::Error(error) => Content::Error(match error {
                CellError::Null => "#NULL!",
                CellError::DivisionByZero => "#DIV/0!",
                CellError::Value => "#VALUE!",
                CellError::Reference => "#REF!",
                CellError::Name => "#NAME?",
                CellError::Number => "#NUM!",
                CellError::NotAvailable => "#N/A",
                CellError::GettingData => "#GETTING_DATA",
                CellError::Other(code) => {
                    return Err(Error::InvalidArgument(format!(
                        "{code} is not an error value a cell stores: write #NULL!, #DIV/0!, #VALUE!, #REF!, #NAME?, #NUM!, #N/A or #GETTING_DATA"
                    )));
                }
            }),
        })
    }
}

/// The stored form of a formula written with `options`, checked. A plain
/// cell's formula that is `@` before one reference or defined name is
/// stored without its `@`, as Excel stores it (the implicit intersection of
/// a formula without arrays); `@` before anything else stays
/// `_xlfn.SINGLE`.
fn stored_formula(text: &str, options: FormulaOptions) -> Result<String> {
    let refused = |error: oxoffice_xlsx_formula::SyntaxError| {
        Error::InvalidArgument(format!(
            "`{text}` is not a formula Excel reads as written: {error}"
        ))
    };
    let stored = if options.stored {
        let text = text.strip_prefix('=').unwrap_or(text);
        check_stored(text).map_err(refused)?;
        text.to_owned()
    } else {
        to_stored(text).map_err(refused)?
    };
    if options.spill.is_none()
        && let Some(inner) = stored
            .strip_prefix("_xlfn.SINGLE(")
            .and_then(|rest| rest.strip_suffix(')'))
        && tokenize(inner).is_ok_and(|tokens| {
            tokens.len() == 1 && matches!(tokens[0].kind, TokenKind::Reference | TokenKind::Name)
        })
    {
        return Ok(inner.to_owned());
    }
    Ok(stored)
}

/// The cell `reference` names: `B3` or `$B$3`.
fn checked_cell(reference: &str) -> Result<CellRef> {
    match Reference::parse(reference)? {
        Reference::Cell(cell) => Ok(cell),
        _ => Err(Error::NotACell {
            reference: reference.to_owned(),
        }),
    }
}

/// What the cells a call writes were.
#[derive(Debug, Default)]
struct OldCells {
    styles: Vec<u32>,
    absent: Vec<u32>,
    strings: Vec<bool>,
}

/// The cells of a row by column: the child index of the first cell of each
/// column, and whether the cells are written in ascending order, as every
/// producer writes them.
#[derive(Debug)]
pub(crate) struct RowIndex {
    pub(crate) columns: HashMap<u32, usize>,
    pub(crate) ascending: bool,
}

impl Default for RowIndex {
    fn default() -> Self {
        RowIndex {
            columns: HashMap::new(),
            ascending: true,
        }
    }
}

impl RowIndex {
    pub(crate) fn new(row: &CT_Row) -> Self {
        let mut index = RowIndex::default();
        let mut next = 0;
        let mut last: Option<u32> = None;
        for (position, child) in row.children.iter().enumerate() {
            if let RowChild::Cell(cell) = child {
                let column = cell.r().map_or(next, |r| r.column);
                next = column + 1;
                index.ascending &= last.is_none_or(|last| column > last);
                last = Some(column);
                index.columns.entry(column).or_insert(position);
            }
        }
        index
    }

    fn cell<'r>(&self, row: &'r CT_Row, column: u32) -> Option<&'r CT_Cell> {
        match row.children.get(*self.columns.get(&column)?)? {
            RowChild::Cell(cell) => Some(cell),
            RowChild::Other(_) => None,
        }
    }
}

/// `row` with the cells of `removed` (child indexes) gone and the cells of
/// `added` (by column, in order) at their place; every cell takes its `r`,
/// so that no cell written without one moves.
pub(crate) fn rebuild_row(
    row: &mut CT_Row,
    number: u32,
    added: Vec<(u32, CT_Cell)>,
    removed: &[usize],
    ascending: bool,
) {
    let mut next = 0;
    for child in &mut row.children {
        if let RowChild::Cell(cell) = child {
            let column = cell.r().map_or(next, |r| r.column);
            next = column + 1;
            if cell.tag.get("r").is_none() {
                cell.tag
                    .set("r", Some(&CellRef::new(number, column).to_string()));
            }
        }
    }
    let removed: BTreeSet<usize> = removed.iter().copied().collect();
    if !ascending {
        for &child in removed.iter().rev() {
            row.children.remove(child);
        }
        for (column, cell) in added {
            let at = insertion_index(row, column);
            row.children.insert(at, RowChild::Cell(cell));
        }
        return;
    }
    let last_cell = row
        .children
        .iter()
        .rposition(|child| matches!(child, RowChild::Cell(_)));
    let mut added = added.into_iter().peekable();
    let mut children = Vec::with_capacity(row.children.len() + added.len());
    for (position, child) in std::mem::take(&mut row.children).into_iter().enumerate() {
        if let RowChild::Cell(cell) = &child {
            let column = cell.r().map_or(0, |r| r.column);
            while let Some((_, new)) = added.next_if(|(at, _)| *at < column) {
                children.push(RowChild::Cell(new));
            }
        }
        if !removed.contains(&position) {
            children.push(child);
        }
        if Some(position) == last_cell {
            children.extend(added.by_ref().map(|(_, cell)| RowChild::Cell(cell)));
        }
    }
    // A row with no cell yet: its new cells before what else it holds.
    let rest: Vec<RowChild> = added.map(|(_, cell)| RowChild::Cell(cell)).collect();
    if !rest.is_empty() {
        children.splice(0..0, rest);
    }
    row.children = children;
}

/// Where a new cell of `column` goes in `row`: before the first cell after
/// it, else after the last cell.
fn insertion_index(row: &CT_Row, column: u32) -> usize {
    let mut next = 0;
    let mut after_last = 0;
    for (index, child) in row.children.iter().enumerate() {
        if let RowChild::Cell(cell) = child {
            let at = cell.r().map_or(next, |r| r.column);
            if at > column {
                return index;
            }
            next = at + 1;
            after_last = index + 1;
        }
    }
    after_last
}

/// The cell of `row` at `column`, read: one pass over the row.
fn cell_at(row: Option<&CT_Row>, column: u32) -> Option<&CT_Cell> {
    columns(row?)
        .into_iter()
        .find(|(at, _)| *at == column)
        .map(|(_, cell)| cell)
}

/// Whether a cell holds an array formula or a data table.
fn holds_range_formula(cell: &CT_Cell) -> bool {
    cell.formula()
        .is_some_and(|formula| matches!(formula.t().as_str(), "array" | "dataTable"))
}

/// A cell's content as written: the shared string index of a text, the
/// metadata block of a spilling formula.
enum Written<'a> {
    Empty,
    Number(f64),
    SharedString(usize),
    Boolean(bool),
    Error(&'static str),
    /// A formula, and for a dynamic array its block and range.
    Formula {
        stored: &'a str,
        spill: Option<(u32, CellRange)>,
    },
}

/// Writes `content` into `cell`, at `at`: its value, formula and type go,
/// with the metadata that described them (`cm`, `vm`); its style, its
/// other attributes and the children the model does not know stay.
fn write_content(cell: &mut CT_Cell, at: CellRef, content: &Written<'_>, prefix: &str) {
    for attribute in ["t", "cm", "vm"] {
        cell.tag.set(attribute, None);
    }
    cell.children
        .retain(|child| matches!(child, CellChild::Other(_)));
    let value = |text: String| {
        CellChild::Value(TextElement {
            tag: Tag::new(&qualified(prefix, "v")),
            text,
        })
    };
    let mut children = Vec::new();
    match content {
        Written::Empty => {}
        Written::Number(number) => children.push(value(format_f64(*number))),
        Written::SharedString(index) => {
            cell.tag.set("t", Some("s"));
            children.push(value(index.to_string()));
        }
        Written::Boolean(boolean) => {
            cell.tag.set("t", Some("b"));
            children.push(value(if *boolean { "1" } else { "0" }.to_owned()));
        }
        Written::Error(code) => {
            cell.tag.set("t", Some("e"));
            children.push(value((*code).to_owned()));
        }
        Written::Formula { stored, spill } => {
            let mut tag = Tag::new(&qualified(prefix, "f"));
            if let Some((cm, range)) = spill {
                let reference = if range.first == range.last {
                    at.to_string()
                } else {
                    range.to_string()
                };
                tag.set("t", Some("array"));
                tag.set("ref", Some(&reference));
                cell.tag.set("cm", Some(&cm.to_string()));
            }
            children.push(CellChild::Formula(CT_CellFormula {
                tag,
                text: (*stored).to_owned(),
            }));
        }
    }
    children.append(&mut cell.children);
    cell.children = children;
}

/// Whether a cell says nothing but where it is: no style, no content.
fn is_bare(cell: &CT_Cell) -> bool {
    cell.children.is_empty() && cell.tag.attributes_except(&["r"]).is_empty()
}

/// Widens a row's `spans`, when it has one, to the cells it holds.
pub(crate) fn widen_spans(row: &mut CT_Row) {
    let Some(spans) = row.tag.get("spans") else {
        return;
    };
    let written: Vec<u32> = columns(row).iter().map(|(column, _)| column + 1).collect();
    let (Some(&low), Some(&high)) = (written.iter().min(), written.iter().max()) else {
        return;
    };
    let ranges: Vec<(u32, u32)> = spans
        .split_whitespace()
        .filter_map(|span| {
            let (a, b) = span.split_once(':')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .collect();
    if written
        .iter()
        .all(|column| ranges.iter().any(|(a, b)| (a..=b).contains(&column)))
    {
        return;
    }
    let low = ranges
        .iter()
        .map(|(a, _)| *a)
        .chain([low])
        .min()
        .unwrap_or(low);
    let high = ranges
        .iter()
        .map(|(_, b)| *b)
        .chain([high])
        .max()
        .unwrap_or(high);
    row.tag.set("spans", Some(&format!("{low}:{high}")));
}

/// The smallest range holding both.
pub(crate) fn union(a: CellRange, b: CellRange) -> CellRange {
    CellRange::new(
        CellRef::new(
            a.first.row.min(b.first.row),
            a.first.column.min(b.first.column),
        ),
        CellRef::new(a.last.row.max(b.last.row), a.last.column.max(b.last.column)),
    )
}

fn area(range: CellRange) -> u64 {
    u64::from(range.last.row - range.first.row + 1)
        * u64::from(range.last.column - range.first.column + 1)
}

/// Refuses, in the contents one call writes, two spill ranges that meet or a
/// value written inside another formula's spill range: the sheet as it was
/// does not show them, and Excel repairs the overlapping `ref`s.
fn check_batch_spills(contents: &[(CellRef, &Content)]) -> Result<()> {
    let origins: HashMap<CellRef, SpillOrigin> = contents
        .iter()
        .filter_map(|(at, content)| match content {
            Content::Formula {
                spill: Some(spill), ..
            } => Some((*at, spill.origin)),
            _ => None,
        })
        .collect();
    let spills: Vec<RangeFormula> = contents
        .iter()
        .filter_map(|(at, content)| match content {
            Content::Formula {
                spill: Some(spill), ..
            } => Some(RangeFormula {
                cell: *at,
                range: spill.range,
                kind: RangeFormulaKind::Array,
                volatile: false,
            }),
            _ => None,
        })
        .collect();
    if spills.is_empty() {
        return Ok(());
    }
    let batch = crate::ranges::RangeLookup::new(&spills);
    for spill in &spills {
        if let Some(other) = batch
            .meeting(spill.range)
            .into_iter()
            .find(|other| other.range.first != spill.cell)
        {
            return Err(Error::Refused(format!(
                "{} meets {}, spilled by the same write: {}",
                other.range,
                spill.range,
                origins[&spill.cell].way_out(&["write the formulas apart"])
            )));
        }
    }
    for (at, content) in contents {
        if **content != Content::Empty
            && let Some(entry) = batch.find(*at)
            && entry.range.first != *at
        {
            let origin = origins[&entry.range.first];
            return Err(Error::Refused(format!(
                "{at} is written inside {}, where the formula of {} {}: {}",
                entry.range,
                entry.range.first,
                origin.spills(),
                origin.way_out(&["write it elsewhere"])
            )));
        }
    }
    Ok(())
}

/// Whether each write is of nothing over a cell that holds nothing: a
/// write that changes nothing.
fn noop_clears(writes: &[CellWrite], rows: &BTreeMap<u32, CT_Row>) -> Vec<bool> {
    let mut held: HashMap<u32, HashMap<u32, bool>> = HashMap::new();
    writes
        .iter()
        .map(|write| {
            write.content == Some(Content::Empty)
                && !held
                    .entry(write.at.row)
                    .or_insert_with(|| {
                        rows.get(&write.at.row)
                            .map(|row| {
                                columns(row)
                                    .into_iter()
                                    .map(|(column, cell)| (column, holds_content(cell)))
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .get(&write.at.column)
                    .copied()
                    .unwrap_or(false)
        })
        .collect()
}

/// Turns each cell of `dissolved` into a plain formula with its text.
fn dissolve(
    rows: &mut BTreeMap<u32, CT_Row>,
    indexes: &HashMap<u32, RowIndex>,
    dissolved: &[(CellRef, String)],
) {
    for (at, text) in dissolved {
        let Some(row) = rows.get_mut(&at.row) else {
            continue;
        };
        let Some(&child) = indexes
            .get(&at.row)
            .and_then(|index| index.columns.get(&at.column))
        else {
            continue;
        };
        if let RowChild::Cell(cell) = &mut row.children[child] {
            for child in &mut cell.children {
                if let CellChild::Formula(formula) = child {
                    for attribute in ["t", "ref", "si"] {
                        formula.tag.set(attribute, None);
                    }
                    formula.text = text.clone();
                }
            }
        }
    }
}

impl Workbook {
    /// The directory of the workbook part, `/xl`, where new parts go.
    fn part_directory(&self) -> &str {
        self.workbook_part
            .rsplit_once('/')
            .map_or("", |(directory, _)| directory)
    }

    /// A new part of the workbook, related to the workbook part: its name
    /// (`/xl/<file>`, or with a number when taken), relationship and content
    /// type, and its bytes for now.
    fn add_workbook_part(
        &mut self,
        file: &str,
        rel_type: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> String {
        let (stem, extension) = file.rsplit_once('.').unwrap_or((file, "xml"));
        let directory = self.part_directory().to_owned();
        let name = (0..)
            .map(|number: u32| match number {
                0 => file.to_owned(),
                n => format!("{stem}{n}.{extension}"),
            })
            .find(|name| !self.package.contains_part(&format!("{directory}/{name}")))
            .expect("a free part name");
        let part = format!("{directory}/{name}");
        let rel_type = if self.workbook.is_strict() {
            strict(rel_type)
        } else {
            rel_type
        };
        self.package
            .get_or_create_part_rels(&self.workbook_part)
            .add(rel_type, &name);
        self.package.set_part(&part, bytes);
        self.package.content_types.add_override(&part, content_type);
        part
    }

    /// The main namespace the workbook's parts are in.
    fn main_namespace(&self) -> &'static str {
        if self.workbook.is_strict() {
            STRICT_MAIN
        } else {
            MAIN
        }
    }

    /// The index of a plain shared string holding `text`: an identical one
    /// the workbook has, never a rich one, else a new one. The shared strings
    /// part is created when the workbook has none.
    fn shared_string(&mut self, text: &str) -> Result<usize> {
        if self.shared_strings()?.is_none() {
            let strings =
                oxoffice_xlsx_oxml::shared_strings::SharedStrings::empty(self.main_namespace());
            self.add_workbook_part(
                "sharedStrings.xml",
                rel_types::SHARED_STRINGS,
                content_types::SHARED_STRINGS,
                strings.to_xml(),
            );
            self.read.shared_strings = OnceLock::from(Ok(Some(strings)));
        }
        let Some(Ok(Some(strings))) = self.read.shared_strings.get_mut() else {
            unreachable!("the shared strings were just read");
        };
        let known = self.edit.strings.get_or_insert_with(|| {
            let mut known = HashMap::new();
            for (index, item) in strings.items().iter().enumerate() {
                if !item.rich {
                    known.entry(item.text.clone()).or_insert(index);
                }
            }
            known
        });
        if let Some(&index) = known.get(text) {
            return Ok(index);
        }
        let index = strings.push_plain(text);
        known.insert(text.to_owned(), index);
        Ok(index)
    }

    fn count_string_references(&mut self, delta: i64) {
        if delta != 0
            && let Some(Ok(Some(strings))) = self.read.shared_strings.get_mut()
        {
            strings.add_references(delta);
        }
    }

    /// Reads the metadata part, when the workbook has one, for an edit.
    fn read_metadata(&mut self) -> Result<()> {
        if self.edit.metadata.is_some() {
            return Ok(());
        }
        if let Some(part) = self.related_part(rel_types::SHEET_METADATA) {
            let bytes = self
                .package
                .get_part_shared(&part)
                .ok_or_else(|| Error::MissingPart { part: part.clone() })?;
            let metadata = MetadataXml::parse(std::sync::Arc::clone(bytes)).map_err(|error| {
                Error::MalformedPart {
                    part,
                    message: error.to_string(),
                }
            })?;
            self.edit.metadata = Some(metadata);
        }
        Ok(())
    }

    /// The `cm` of a dynamic array cell, its blocks found in `metadata.xml`
    /// or appended, the part created when the workbook has none.
    fn dynamic_array_cell(&mut self) -> Result<u32> {
        self.read_metadata()?;
        if self.edit.metadata.is_none() {
            let metadata = MetadataXml::empty(self.main_namespace());
            self.add_workbook_part(
                "metadata.xml",
                rel_types::SHEET_METADATA,
                content_types::SHEET_METADATA,
                metadata.to_xml(),
            );
            self.edit.metadata = Some(metadata);
        }
        Ok(self
            .edit
            .metadata
            .as_mut()
            .expect("the metadata was just read")
            .dynamic_array_cell())
    }
}

impl WorksheetMut<'_> {
    /// Writes `input` into `cell` (`B3`): a number, a text, a boolean, an
    /// error, nothing (the content cleared), a date, or a formula.
    ///
    /// The cell keeps its style; a date in a cell shown as General takes a
    /// format that shows it (`yyyy-mm-dd`, `hh:mm:ss`, `yyyy-mm-dd hh:mm:ss`
    /// or `[h]:mm:ss`). Excel shows such a date as `#####` in a column of
    /// the default width until it is widened ([`WorksheetMut::set_column_width`]):
    /// it widens a column when a date is typed, not when a file holds one. Text goes to the shared strings, reusing an identical
    /// plain item. A write into a shared formula's range turns the group's
    /// other cells into plain formulas with their own text. Refused, with
    /// the way out and nothing changed: a cell of an array formula or data
    /// table written alone, a value under a merged range but its first cell,
    /// a table header that would no longer name its column, a cell of a pivot
    /// table.
    pub fn set_value(&mut self, cell: &str, input: impl Into<CellInput>) -> Result<()> {
        let at = checked_cell(cell)?;
        self.write_inputs(vec![(at, input.into())])
    }

    /// Writes a formula into `cell`, in the form Excel displays it after its
    /// `=` (an `=` is accepted), or the form the file stores it with
    /// [`FormulaOptions::stored`]. It must pass the syntax check: a formula
    /// Excel would repair (`IF(B2>0;1;0)`, `B2*`, `IF(1,2,3,4)`) is refused.
    /// Written without a cached value, the workbook asks Excel to calculate
    /// it on open.
    pub fn set_formula(
        &mut self,
        cell: &str,
        formula: &str,
        options: FormulaOptions,
    ) -> Result<()> {
        self.set_value(cell, CellInput::Formula(formula.to_owned(), options))
    }

    /// Writes `rows` from `top_left` (`B3`) down and right, one list per row;
    /// a row shorter than another leaves the cells after it as they are. Each
    /// cell as [`WorksheetMut::set_value`] writes it.
    pub fn write(&mut self, top_left: &str, rows: Vec<Vec<CellInput>>) -> Result<()> {
        let at = checked_cell(top_left)?;
        let height = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        let width = u32::try_from(width).unwrap_or(u32::MAX);
        if u64::from(at.row) + u64::from(height) > u64::from(MAX_ROWS)
            || u64::from(at.column) + u64::from(width) > u64::from(MAX_COLUMNS)
        {
            return Err(Error::InvalidArgument(format!(
                "{height} rows of {width} cells from {at} go past the last row or column of the sheet"
            )));
        }
        let mut inputs = Vec::new();
        for (row, values) in (at.row..).zip(rows) {
            for (column, input) in (at.column..).zip(values) {
                inputs.push((CellRef::new(row, column), input));
            }
        }
        self.write_inputs(inputs)
    }

    /// Clears `range` (`A1:C10`, `B3`, whole columns `A:C` or rows `2:5`):
    /// its contents, its formats, or both. Contents are cleared in the cells
    /// the sheet holds; formats there too, and a cell the sheet does not write
    /// under a styled row or column is written with the default format (at
    /// most 1,000,000 such cells at once); the formats of whole columns or
    /// rows go from their `col` spans or rows too, as
    /// [`WorksheetMut::set_style`] writes them. A
    /// clear of a whole array formula removes it; part of one is refused.
    pub fn clear(&mut self, range: &str, what: Clear) -> Result<()> {
        self.require_worksheet("cells")?;
        let reference = Reference::parse(range)?;
        if reference.is_whole() && what != Clear::Contents {
            // What could refuse the formats is read before the contents go.
            self.whole_targets(reference)?;
            if what == Clear::All {
                self.clear_cells(range, Clear::Contents)?;
            }
            return self.restyle_whole(reference, Restyle::Clear);
        }
        self.clear_cells(range, what)
    }

    /// Clears the cells `range` names: contents, formats or both. A clear
    /// of formats reaches every cell of the range, those the sheet does not
    /// write too: one under a styled row or column is written with the
    /// default format, so that a value written there later is not styled.
    fn clear_cells(&mut self, range: &str, what: Clear) -> Result<()> {
        let Some(rectangle) = self.read().range(range)? else {
            return Ok(());
        };
        let formats = matches!(what, Clear::Formats | Clear::All);
        let contents = matches!(what, Clear::Contents | Clear::All);
        let rows = self.rows_in(rectangle.first.row, rectangle.last.row)?;
        let style = if formats {
            StyleChange::Remove
        } else {
            StyleChange::Keep
        };
        let mut writes = Vec::new();
        for (&number, row) in &rows {
            for (column, cell) in columns(row) {
                let at = CellRef::new(number, column);
                if !rectangle.contains(at) {
                    continue;
                }
                let content = contents && holds_content(cell);
                if content || (formats && cell.tag.get("s").is_some()) {
                    writes.push(CellWrite {
                        at,
                        content: content.then_some(Content::Empty),
                        date_code: None,
                        style,
                    });
                }
            }
        }
        if formats {
            // The cells the sheet does not write under a styled row or column
            // are written with the default format: at most 1,000,000.
            let styled_columns: Vec<(u32, u32)> = self
                .read()
                .column_styles()?
                .into_iter()
                .filter(|(min, max, style)| {
                    *style != 0 && *max >= rectangle.first.column && *min <= rectangle.last.column
                })
                .map(|(min, max, _)| {
                    (
                        min.max(rectangle.first.column),
                        max.min(rectangle.last.column),
                    )
                })
                .collect();
            let mut covered = Vec::new();
            let mut each_row = |number: u32, row: Option<&CT_Row>| -> Result<()> {
                let styled_row = row
                    .filter(|row| row.custom_format())
                    .and_then(CT_Row::s)
                    .is_some_and(|s| s != 0);
                let written: HashSet<u32> = row
                    .map(|row| columns(row).into_iter().map(|(column, _)| column).collect())
                    .unwrap_or_default();
                let spans: Vec<(u32, u32)> = if styled_row {
                    vec![(rectangle.first.column, rectangle.last.column)]
                } else {
                    styled_columns.clone()
                };
                for (first, last) in spans {
                    for column in first..=last {
                        if !written.contains(&column) {
                            covered.push(CellRef::new(number, column));
                            if covered.len() as u64 > crate::format::MAX_STYLED_CELLS {
                                return Err(Error::Refused(format!(
                                    "clearing the formats of {rectangle} writes more than {} cells under a styled row or column at once: clear a smaller range",
                                    crate::format::MAX_STYLED_CELLS
                                )));
                            }
                        }
                    }
                }
                Ok(())
            };
            if styled_columns.is_empty() {
                // Only the rows the sheet writes can be styled.
                for (&number, row) in &rows {
                    each_row(number, Some(row))?;
                }
            } else {
                for number in rectangle.first.row..=rectangle.last.row {
                    each_row(number, rows.get(&number))?;
                }
            }
            writes.extend(covered.into_iter().map(|at| CellWrite {
                at,
                content: None,
                date_code: None,
                style: StyleChange::Remove,
            }));
        }
        self.apply(writes, Region::Rectangle(rectangle), None)
    }

    /// Writes each `(cell, input)`, checked first.
    pub(crate) fn write_inputs(&mut self, inputs: Vec<(CellRef, CellInput)>) -> Result<()> {
        let system = self.workbook.date_system();
        let mut writes = Vec::with_capacity(inputs.len());
        for (at, input) in inputs {
            let (mut content, date_code) = Content::of(at, input, system)?;
            if let Content::Formula {
                stored,
                spill: Some(spill),
            } = &mut content
                && spill.origin == SpillOrigin::Result
            {
                *spill = self.result_range(at, stored)?;
            }
            writes.push(CellWrite {
                at,
                content: Some(content),
                date_code,
                style: StyleChange::Keep,
            });
        }
        self.apply(writes, Region::Cells, None)
    }

    /// The range a dynamic array formula written at `at` without one spills
    /// over: its result's when the formula decides its shape (its defined
    /// names read from this sheet), the most it can fill when the data
    /// decide how much, cut at the edge of the sheet (Excel shows #SPILL! for
    /// a result that does not fit), else its cell alone. A result that
    /// reaches past the edge of the sheet is refused.
    fn result_range(&self, at: CellRef, stored: &str) -> Result<SpillRange> {
        let extent = result_extent(stored, |sheet, name| self.defined_name(sheet, name));
        let Some(extent) = extent else {
            return Ok(SpillRange {
                range: CellRange::new(at, at),
                origin: SpillOrigin::Result,
            });
        };
        let last_row = at.row + (extent.shape.rows - 1);
        let last_column = at.column + (extent.shape.columns - 1);
        let past = last_row >= MAX_ROWS || last_column >= MAX_COLUMNS;
        if extent.exact && past {
            return Err(Error::Refused(format!(
                "the formula of {at} gives a result of {} by {} cells, which reaches past the edge of the sheet from {at}, where Excel shows #SPILL!: write it where its result fits",
                extent.shape.rows, extent.shape.columns
            )));
        }
        let last = CellRef::new(last_row.min(MAX_ROWS - 1), last_column.min(MAX_COLUMNS - 1));
        Ok(SpillRange {
            range: CellRange::new(at, last),
            origin: if extent.exact {
                SpillOrigin::Result
            } else {
                SpillOrigin::Bound
            },
        })
    }

    /// The formula the defined name `name` refers to in a formula of this
    /// sheet, names compared in any case: written after a sheet
    /// (`Data!Sales`), that sheet's own name; without, this sheet's own,
    /// else the workbook's.
    fn defined_name(&self, sheet: Option<&str>, name: &str) -> Option<String> {
        let position = match sheet {
            Some(sheet) => self.workbook.sheet(sheet)?.position(),
            None => self.position,
        };
        let local = u32::try_from(position).ok();
        let name = name.to_lowercase();
        let named = || {
            self.workbook
                .workbook
                .defined_names()
                .filter(|defined| defined.name().to_lowercase() == name)
        };
        named()
            .find(|defined| defined.local_sheet_id() == local)
            .or_else(|| {
                sheet
                    .is_none()
                    .then(|| named().find(|defined| defined.local_sheet_id().is_none()))
                    .flatten()
            })
            .map(|defined| defined.text())
    }

    /// What a write of content may not change, read once.
    fn guards(&self) -> Result<&Guards> {
        let sheet = self.read();
        let entry = sheet.entry();
        let read = || -> std::result::Result<Guards, String> {
            let merges = sheet.merged_ranges().map_err(|error| error.to_string())?;
            let workbook = &*self.workbook;
            let parts = |rel_type: &str| related_parts(&workbook.package, &entry.part, rel_type);
            let part_element = |part: &str| -> std::result::Result<Element, String> {
                let bytes = workbook
                    .package
                    .get_part(part)
                    .ok_or_else(|| format!("package part is missing: {part}"))?;
                Element::parse(bytes).map_err(|error| format!("{part}: {error}"))
            };
            let mut headers = Vec::new();
            for part in parts(rel_types::TABLE) {
                let table = part_element(&part)?;
                let Some(range) = table.tag.get("ref").as_deref().and_then(CellRange::parse) else {
                    continue;
                };
                if table.tag.u32("headerRowCount") == Some(0) {
                    continue;
                }
                let names = table
                    .child("tableColumns")
                    .map(|columns| {
                        columns
                            .elements_named("tableColumn")
                            .map(|column| {
                                xstring::decode(&column.tag.get("name").unwrap_or_default())
                                    .into_owned()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                headers.push(TableHeader {
                    table: table
                        .tag
                        .get("displayName")
                        .or_else(|| table.tag.get("name"))
                        .unwrap_or_default(),
                    row: range.first.row,
                    first: range.first.column,
                    names,
                });
            }
            let mut pivots = Vec::new();
            for part in parts(rel_types::PIVOT_TABLE) {
                let pivot = part_element(&part)?;
                if let Some(range) = pivot
                    .child("location")
                    .and_then(|location| location.tag.get("ref"))
                    .as_deref()
                    .and_then(CellRange::parse)
                {
                    pivots.push((pivot.tag.get("name").unwrap_or_default(), range));
                }
            }
            Ok(Guards {
                merges,
                headers,
                pivots,
            })
        };
        entry
            .guards
            .get_or_init(read)
            .as_ref()
            .map_err(|message| Error::MalformedPart {
                part: entry.part.clone(),
                message: message.clone(),
            })
    }

    /// Refuses a write of content Excel would repair or this version does not
    /// write. See [`WorksheetMut::set_value`].
    /// A content written as nothing over a cell that holds nothing changes
    /// nothing (`noop`): it counts for an array formula written whole, and
    /// meets no other refusal.
    fn check_contents(&self, writes: &[CellWrite], noop: &[bool], region: Region) -> Result<()> {
        let contents: Vec<(CellRef, &Content)> = writes
            .iter()
            .filter_map(|write| write.content.as_ref().map(|content| (write.at, content)))
            .collect();
        if contents.is_empty() {
            return Ok(());
        }
        // Array formulas and data tables, whole or not at all.
        let lookup = self.read().range_lookup()?;
        let mut reached: BTreeMap<(CellRef, CellRef), (crate::ranges::RangeEntry, u64)> =
            BTreeMap::new();
        for (at, _) in &contents {
            if let Some(entry) = lookup.find(*at) {
                reached
                    .entry((entry.range.first, entry.range.last))
                    .or_insert((entry, 0))
                    .1 += 1;
            }
        }
        let mut addressed: Option<HashSet<CellRef>> = None;
        for (entry, written) in reached.values() {
            let range = entry.range;
            let whole = match region {
                // Every cell of the range that holds something is written: a
                // spill range this library wrote holds nothing but its formula,
                // so its own call rewrites or retargets it; an array saved by
                // Excel, its cells holding cached values, is written whole or
                // not at all.
                Region::Cells => {
                    *written == area(range) || {
                        let addressed = addressed
                            .get_or_insert_with(|| contents.iter().map(|(at, _)| *at).collect());
                        let mut held_written = true;
                        for (number, row) in self.read().rows_in(range.first.row, range.last.row)? {
                            for (column, cell) in columns(&row) {
                                let at = CellRef::new(number, column);
                                if range.contains(at)
                                    && holds_content(cell)
                                    && !addressed.contains(&at)
                                {
                                    held_written = false;
                                }
                            }
                        }
                        held_written
                    }
                }
                Region::Rectangle(rectangle) => {
                    rectangle.contains(range.first) && rectangle.contains(range.last)
                }
            };
            if !whole {
                let what = match entry.kind {
                    crate::FormulaKind::DataTable => "a data table",
                    _ => "an array formula",
                };
                return Err(Error::Refused(format!(
                    "{range} holds {what} whose cells are written together: write or clear all of {range}, or none of it"
                )));
            }
        }
        let contents: Vec<(CellRef, &Content)> = writes
            .iter()
            .zip(noop)
            .filter(|(_, noop)| !**noop)
            .filter_map(|(write, _)| write.content.as_ref().map(|content| (write.at, content)))
            .collect();
        if contents.is_empty() {
            return Ok(());
        }
        let guards = self.guards()?;
        let bounds = contents
            .iter()
            .map(|(at, _)| CellRange::new(*at, *at))
            .reduce(union)
            .expect("a content written");
        let merges: Vec<&CellRange> = guards
            .merges
            .iter()
            .filter(|merge| merge.intersects(&bounds))
            .collect();
        for (at, content) in &contents {
            if **content != Content::Empty
                && let Some(merge) = merges
                    .iter()
                    .find(|merge| merge.contains(*at) && merge.first != *at)
            {
                return Err(Error::Refused(format!(
                    "{at} is covered by the merged range {merge}: its value goes in {}",
                    merge.first
                )));
            }
            for header in &guards.headers {
                let column = at.column.wrapping_sub(header.first) as usize;
                if at.row != header.row || column >= header.names.len() {
                    continue;
                }
                let name = &header.names[column];
                if !matches!(content, Content::Text(text) if text == name) {
                    return Err(Error::Refused(format!(
                        "{at} is the header of column {name:?} of table {}: a new header renames the column, which is not written yet; write {name:?} or leave the cell",
                        header.table
                    )));
                }
            }
            if let Some((name, range)) = guards.pivots.iter().find(|(_, range)| range.contains(*at))
            {
                return Err(Error::Refused(format!(
                    "{at} is in pivot table {name} ({range}), whose cells Excel computes from its source: edit the source data instead"
                )));
            }
            // A spilling formula's range is free: Excel shows #SPILL! over a
            // value, another array formula or a merge.
            if let Content::Formula {
                spill: Some(spill), ..
            } = content
                && spill.range.first != spill.range.last
            {
                self.check_spill_range(*at, *spill, lookup, &guards.merges)?;
            }
        }
        check_batch_spills(&contents)
    }

    /// Refuses a spill range whose cells other than the formula's hold a
    /// value or a formula, or meet another array formula or a merge.
    fn check_spill_range(
        &self,
        at: CellRef,
        spill: SpillRange,
        lookup: &crate::ranges::RangeLookup,
        merges: &[CellRange],
    ) -> Result<()> {
        let range = spill.range;
        let refused = |what: String| {
            let spills = match spill.origin {
                SpillOrigin::Bound => "can spill over",
                _ => "would spill over",
            };
            Err(Error::Refused(format!(
                "the formula of {at} {spills} {range}, where {what}: {}",
                spill
                    .origin
                    .way_out(&["clear it", "write the formula elsewhere"])
            )))
        };
        for (number, row) in self.read().rows_in(range.first.row, range.last.row)? {
            for (column, cell) in columns(&row) {
                let cell_at = CellRef::new(number, column);
                if cell_at != at && range.contains(cell_at) && holds_content(cell) {
                    return refused(format!("{cell_at} holds a value or a formula"));
                }
            }
        }
        if let Some(entry) = lookup
            .meeting(range)
            .into_iter()
            .find(|entry| entry.range.first != at)
        {
            return refused(format!("{} holds an array formula", entry.range));
        }
        if let Some(merge) = merges.iter().find(|merge| merge.intersects(&range)) {
            return refused(format!("{merge} is merged"));
        }
        Ok(())
    }

    /// Makes `writes`: the one path of every write of cells. See the
    /// [module](self).
    pub(crate) fn apply(
        &mut self,
        mut writes: Vec<CellWrite>,
        region: Region,
        patch: Option<&StylePatch>,
    ) -> Result<()> {
        self.require_worksheet("cells")?;
        // The last write of a cell wins.
        writes.reverse();
        writes.sort_by_key(|write| write.at);
        writes.dedup_by_key(|write| write.at);
        let (Some(first), Some(last)) = (writes.first(), writes.last()) else {
            return Ok(());
        };
        let (first, last) = (first.at.row, last.at.row);

        // What the write reads, and what it refuses, before anything changes.
        let mut rows = self.rows_in(first, last)?;
        let noop = noop_clears(&writes, &rows);
        self.check_contents(&writes, &noop, region)?;
        for (write, noop) in writes.iter_mut().zip(&noop) {
            if *noop {
                write.content = None;
            }
        }
        let dissolved = self.dissolved_groups(&writes, &mut rows)?;
        let before = rows.clone();
        let styled = writes
            .iter()
            .any(|write| write.date_code.is_some() || write.style == StyleChange::Patch);
        if styled {
            self.workbook.style_table()?;
            if self.workbook.styles_xml()?.is_none() {
                return Err(Error::Refused(
                    "the workbook has no styles part, where a cell's format is written".to_owned(),
                ));
            }
        }
        if writes
            .iter()
            .any(|write| matches!(write.content, Some(Content::Text(_))))
        {
            self.workbook.shared_strings()?;
        }
        if writes
            .iter()
            .any(|write| matches!(write.content, Some(Content::Formula { spill: Some(_), .. })))
        {
            self.workbook.read_metadata()?;
        }
        // Each row's cells by column, read once.
        let indexes: HashMap<u32, RowIndex> = rows
            .iter()
            .map(|(&number, row)| (number, RowIndex::new(row)))
            .collect();
        let OldCells {
            styles: old_styles,
            absent,
            strings: old_strings,
        } = self.old_cells(&writes, &rows, &indexes)?;

        // The styles, each style the cells had patched once, and the contents
        // as written: texts in the shared strings, spilling formulas' metadata.
        let new_styles = self.new_styles(&writes, &old_styles, patch)?;
        let (strings, spill_cm) = self.written_contents(&writes)?;

        // The rows: the groups dissolved, then each cell written.
        let prefix = self.prefix()?;
        dissolve(&mut rows, &indexes, &dissolved);
        let mut arrays: BTreeMap<CellRef, Option<RangeFormula>> = BTreeMap::new();
        let mut touched: Option<CellRange> = None;
        let mut formula_written = false;
        let mut references: i64 = 0;
        let mut at_write = 0;
        let mut written_rows = BTreeSet::new();
        while at_write < writes.len() {
            let number = writes[at_write].at.row;
            let end = at_write
                + writes[at_write..]
                    .iter()
                    .take_while(|write| write.at.row == number)
                    .count();
            let empty = RowIndex::default();
            let index = indexes.get(&number).unwrap_or(&empty);
            let mut added: Vec<(u32, CT_Cell)> = Vec::new();
            let mut removed: Vec<usize> = Vec::new();
            for position in at_write..end {
                let write = &writes[position];
                let at = write.at;
                let style = new_styles[position];
                let existing = index.columns.get(&at.column).copied();
                // A cell the sheet does not write is written when its style
                // is not the one its row or column gives it.
                let needs_cell = match &write.content {
                    Some(Content::Empty) | None => {
                        existing.is_some() || style.unwrap_or(0) != old_styles[position]
                    }
                    Some(_) => true,
                };
                if !needs_cell {
                    continue;
                }
                let mut new_cell = None;
                let cell = match existing {
                    Some(child) => {
                        let row = rows.get_mut(&number).expect("the row of a cell");
                        match &mut row.children[child] {
                            RowChild::Cell(cell) => cell,
                            RowChild::Other(_) => unreachable!("the index of a cell"),
                        }
                    }
                    None => {
                        let mut tag = Tag::new(&qualified(&prefix, "c"));
                        tag.set("r", Some(&at.to_string()));
                        if old_styles[position] != 0 {
                            tag.set("s", Some(&old_styles[position].to_string()));
                        }
                        new_cell.insert(CT_Cell {
                            tag,
                            children: Vec::new(),
                        })
                    }
                };
                if let Some(content) = &write.content {
                    let had_array = holds_range_formula(cell);
                    if old_strings[position] {
                        references -= 1;
                    }
                    let written = match content {
                        Content::Empty => Written::Empty,
                        Content::Number(number) => Written::Number(*number),
                        Content::Text(_) => {
                            references += 1;
                            Written::SharedString(strings[position].expect("a text's string"))
                        }
                        Content::Boolean(boolean) => Written::Boolean(*boolean),
                        Content::Error(code) => Written::Error(code),
                        Content::Formula { stored, spill } => Written::Formula {
                            stored,
                            spill: spill.map(|spill| {
                                (spill_cm.expect("a spilling formula's block"), spill.range)
                            }),
                        },
                    };
                    write_content(cell, at, &written, &prefix);
                    formula_written |= content.is_formula();
                    let spill = match content {
                        Content::Formula { spill, .. } => spill.map(|spill| spill.range),
                        _ => None,
                    };
                    if had_array || spill.is_some() {
                        arrays.insert(
                            at,
                            spill.map(|range| RangeFormula {
                                cell: at,
                                range,
                                kind: RangeFormulaKind::Array,
                                volatile: false,
                            }),
                        );
                    }
                }
                let current = cell.tag.u32("s").unwrap_or(0);
                match style {
                    None => cell.tag.set("s", None),
                    Some(0) if current != 0 => cell.tag.set("s", None),
                    Some(s) if s != current => cell.tag.set("s", Some(&s.to_string())),
                    _ => {}
                }
                // A cell that says nothing stays only when its row or column
                // would give it a style it does not have.
                let bare = is_bare(cell) && absent[position] == 0;
                if !bare {
                    touched = Some(match touched {
                        Some(range) => union(range, CellRange::new(at, at)),
                        None => CellRange::new(at, at),
                    });
                }
                // A cell that says nothing goes, unless its row or column
                // would give it a style it does not have.
                match (existing, new_cell) {
                    (Some(child), _) if bare => removed.push(child),
                    (None, Some(cell)) if !bare => added.push((at.column, cell)),
                    _ => {}
                }
            }
            if !added.is_empty() || !removed.is_empty() {
                let row = rows.entry(number).or_insert_with(|| {
                    let mut tag = Tag::new(&qualified(&prefix, "row"));
                    tag.set("r", Some(&(number + 1).to_string()));
                    CT_Row {
                        tag,
                        children: Vec::new(),
                    }
                });
                rebuild_row(row, number, added, &removed, index.ascending);
            }
            at_write = end;
        }
        // The rows written, not the others a write read, widen their spans.
        for write in &writes {
            if let Some(row) = rows.get_mut(&write.at.row)
                && written_rows.insert(write.at.row)
            {
                widen_spans(row);
            }
        }

        // Staged: the rows, the array formulas, the dimension.
        let changes: Vec<(u32, Option<CT_Row>, CT_Row)> = rows
            .into_iter()
            .map(|(number, row)| (number, before.get(&number).cloned(), row))
            .collect();
        let changed = self.stage_rows(changes);
        self.workbook.count_string_references(references);
        if !arrays.is_empty() {
            // The lookup follows the edit, in place: rebuilding it would make
            // each write of an array formula cost the sheet's.
            let entry = &mut self.workbook.sheets[self.position];
            if let Some(lookup) = entry.ranges.get_mut() {
                for (anchor, formula) in &arrays {
                    lookup.remove(*anchor);
                    if let Some(formula) = formula {
                        lookup.insert(formula);
                    }
                }
            }
            entry.array_edits.extend(arrays);
        }
        if formula_written && changed {
            self.workbook.require_calculation();
        }
        if let Some(touched) = touched {
            self.widen_dimension(touched)?;
        }
        Ok(())
    }

    /// The style each written cell has, the one it would read without its
    /// element (its row's, else its column's, else 0: what a cell the sheet
    /// does not write takes), and whether it holds a shared string.
    fn old_cells(
        &self,
        writes: &[CellWrite],
        rows: &BTreeMap<u32, CT_Row>,
        indexes: &HashMap<u32, RowIndex>,
    ) -> Result<OldCells> {
        let mut column_styles: Option<Vec<(u32, u32, u32)>> = None;
        let mut old = OldCells::default();
        for write in writes {
            let row = rows.get(&write.at.row);
            let cell = indexes
                .get(&write.at.row)
                .and_then(|index| index.cell(row?, write.at.column));
            old.strings
                .push(cell.is_some_and(|cell| cell.t() == "s" && cell.value().is_some()));
            let absent = match row.filter(|row| row.custom_format()).and_then(CT_Row::s) {
                Some(s) => s,
                None => {
                    if column_styles.is_none() {
                        column_styles = Some(self.read().column_styles()?);
                    }
                    column_styles
                        .iter()
                        .flatten()
                        .find(|(min, max, _)| (*min..=*max).contains(&write.at.column))
                        .map_or(0, |(_, _, style)| *style)
                }
            };
            old.styles.push(cell.map_or(absent, CT_Cell::s));
            old.absent.push(absent);
        }
        Ok(old)
    }

    /// The style each written cell takes (`None`: the default, `s` gone):
    /// the call's patch, or a date's format in a cell shown as General, each
    /// style the cells had patched once.
    fn new_styles(
        &mut self,
        writes: &[CellWrite],
        old_styles: &[u32],
        patch: Option<&StylePatch>,
    ) -> Result<Vec<Option<u32>>> {
        let mut styles = Vec::with_capacity(writes.len());
        let mut patched: HashMap<(u32, Option<&str>), u32> = HashMap::new();
        for (write, &old) in writes.iter().zip(old_styles) {
            let code = match (write.style, write.date_code) {
                (StyleChange::Remove, _) => {
                    styles.push(None);
                    continue;
                }
                (StyleChange::Patch, _) => None,
                (StyleChange::Keep, Some(code))
                    if self.workbook.style_table()?.get(old).style.number_format == "General" =>
                {
                    Some(code)
                }
                (StyleChange::Keep, _) => {
                    styles.push(Some(old));
                    continue;
                }
            };
            let new = match patched.get(&(old, code)) {
                Some(&new) => new,
                None => {
                    let new = match code {
                        Some(code) => {
                            let patch = StylePatch {
                                number_format: Some(code.to_owned()),
                                ..StylePatch::default()
                            };
                            self.workbook.styled(old, &patch)?
                        }
                        None => self
                            .workbook
                            .styled(old, patch.expect("a patch with a patched write"))?,
                    };
                    patched.insert((old, code), new);
                    new
                }
            };
            styles.push(Some(new));
        }
        Ok(styles)
    }

    /// The shared string of each text written, and the `cm` of a spilling
    /// formula when one is written.
    fn written_contents(
        &mut self,
        writes: &[CellWrite],
    ) -> Result<(Vec<Option<usize>>, Option<u32>)> {
        let mut strings = Vec::with_capacity(writes.len());
        let mut spill_cm = None;
        for write in writes {
            strings.push(match &write.content {
                Some(Content::Text(text)) => Some(self.workbook.shared_string(text)?),
                _ => None,
            });
            if matches!(write.content, Some(Content::Formula { spill: Some(_), .. }))
                && spill_cm.is_none()
            {
                spill_cm = Some(self.workbook.dynamic_array_cell()?);
            }
        }
        Ok((strings, spill_cm))
    }

    /// The shared formula groups `writes` write content into, each read
    /// whole (its rows added to `rows`), and the formula each of its cells
    /// keeps once the group is dissolved: the master its own, each member
    /// the master's moved to it, or its own when it has one.
    fn dissolved_groups(
        &self,
        writes: &[CellWrite],
        rows: &mut BTreeMap<u32, CT_Row>,
    ) -> Result<Vec<(CellRef, String)>> {
        let mut groups = BTreeSet::new();
        let mut row_cells: HashMap<u32, HashMap<u32, &CT_Cell>> = HashMap::new();
        for write in writes.iter().filter(|write| write.content.is_some()) {
            let cells = row_cells.entry(write.at.row).or_insert_with(|| {
                let mut cells = HashMap::new();
                if let Some(row) = rows.get(&write.at.row) {
                    for (column, cell) in columns(row) {
                        cells.entry(column).or_insert(cell);
                    }
                }
                cells
            });
            if let Some(formula) = cells
                .get(&write.at.column)
                .copied()
                .and_then(CT_Cell::formula)
                .filter(|formula| formula.t() == "shared")
                && let Some(si) = formula.si()
            {
                groups.insert(si);
            }
        }
        drop(row_cells);
        if groups.is_empty() {
            return Ok(Vec::new());
        }
        let sheet = self.read();
        let (_, index) = sheet.index()?;
        let malformed = |message: String| Error::MalformedPart {
            part: sheet.entry().part.clone(),
            message,
        };
        let mut dissolved = Vec::new();
        for si in groups {
            let master = index.shared_formulas.get(&si);
            let extent = index
                .shared_extents
                .get(&si)
                .copied()
                .into_iter()
                .chain(master.and_then(|master| master.range))
                .reduce(union);
            let Some(extent) = extent else {
                continue;
            };
            for (number, row) in sheet.rows_in(extent.first.row, extent.last.row)? {
                rows.entry(number).or_insert(row);
            }
            for (&number, row) in rows.range(extent.first.row..=extent.last.row) {
                for (column, cell) in columns(row) {
                    let Some(formula) = cell.formula() else {
                        continue;
                    };
                    if formula.t() != "shared" || formula.si() != Some(si) {
                        continue;
                    }
                    let at = CellRef::new(number, column);
                    let text = if !formula.text.is_empty() {
                        formula.text.clone()
                    } else {
                        let master = master.ok_or_else(|| {
                            malformed(format!(
                                "cell {at} is a member of a shared formula the sheet does not hold"
                            ))
                        })?;
                        oxoffice_xlsx_formula::move_references(
                            &master.text,
                            i64::from(at.row) - i64::from(master.cell.row),
                            i64::from(at.column) - i64::from(master.cell.column),
                        )
                        .map_err(|error| {
                            malformed(format!(
                                "the shared formula of {} cannot be read: {error}",
                                master.cell
                            ))
                        })?
                    };
                    dissolved.push((at, text));
                }
            }
        }
        Ok(dissolved)
    }

    /// Widens `dimension`, when the sheet has one, to hold `touched`.
    pub(crate) fn widen_dimension(&mut self, touched: CellRange) -> Result<()> {
        let before = self.child("dimension")?;
        let Some(mut dimension) = before.clone() else {
            return Ok(());
        };
        let Some(current) = dimension
            .tag
            .get("ref")
            .as_deref()
            .and_then(CellRange::parse)
        else {
            return Ok(());
        };
        // `A1` is what an empty sheet says: replaced when A1 holds nothing.
        let widened = if current.first == current.last
            && cell_at(
                self.rows_in(current.first.row, current.first.row)?
                    .get(&current.first.row),
                current.first.column,
            )
            .is_none()
        {
            touched
        } else {
            union(current, touched)
        };
        if widened == current {
            return Ok(());
        }
        let text = if widened.first == widened.last {
            widened.first.to_string()
        } else {
            widened.to_string()
        };
        dimension.tag.set("ref", Some(&text));
        self.stage_child("dimension", &before, Some(dimension));
        Ok(())
    }
}
