//! The `oxoffice xlsx` command tree: command-line access to the public
//! `oxoffice-xlsx` facade. Sheets, rows and columns are numbered from 1,
//! like every number of the command line; cells are A1 references.

mod json;

mod edit;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use oxoffice_cli_support::{
    StagedOutputSet, ensure_new_output_allowed, ensure_output_paths_allowed, json_envelope,
};
use oxoffice_xlsx::{
    CellInput, CellRange, CsvImport, CsvOptions, DateSystem, FormulaOptions, SaveOptions,
    SheetKind, SheetState, Spill, Value, Workbook, WorkbookClass, Worksheet,
};
use serde_json::json;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(version, about = "CLI tool for XLSX files")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the sheets of a workbook, their used ranges, the date system and class
    Inspect {
        file: PathBuf,
        /// Output the workbook as schema-1 JSON, each sheet's page setup included
        #[arg(long)]
        json: bool,
    },
    /// Print the cells of each sheet as Excel displays them
    Text {
        file: PathBuf,
        /// Output schema-1 JSON: values, formulas, merges and comments
        #[arg(long, conflicts_with = "markdown")]
        json: bool,
        /// Output a Markdown table per sheet, headed by the column letters
        #[arg(long)]
        markdown: bool,
        /// A sheet to print, by name or number from 1 (repeatable; every
        /// sheet by default)
        #[arg(long)]
        sheet: Vec<String>,
    },
    /// Write one sheet as CSV: displayed texts from A1 to the end of the used range
    Csv {
        file: PathBuf,
        /// The sheet, by name or number from 1 (the first visible worksheet by default)
        #[arg(long)]
        sheet: Option<String>,
        /// Stored values rather than displayed texts (numbers, dates as serials)
        #[arg(long)]
        raw: bool,
        /// The field separator: one character, or `tab`
        #[arg(long, default_value = ",")]
        delimiter: String,
        /// Start with a UTF-8 byte order mark
        #[arg(long)]
        bom: bool,
        /// Write to this file rather than the standard output
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        /// Replace an existing output file
        #[arg(long)]
        force: bool,
    },
    /// Print the cells of a range: values, formulas, number formats and styles
    Cells {
        file: PathBuf,
        /// A1 reference: B3, A1:C10, A:C or 2:5, with a sheet prefix or not ('Q1 data'!B3)
        range: String,
        /// The sheet, by name or number from 1 (the first visible worksheet by default)
        #[arg(long)]
        sheet: Option<String>,
        /// Output schema-1 JSON, each cell with its stored formula and resolved style
        #[arg(long)]
        json: bool,
    },
    /// Print the notes and comment threads of each sheet
    Comments {
        file: PathBuf,
        /// A sheet, by name or number from 1 (repeatable; every sheet by default)
        #[arg(long)]
        sheet: Vec<String>,
        /// Output schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Make a workbook with one empty sheet, or one for each --sheet
    ///
    /// Calibri 11 under the Office 2013-2022 theme. The same command gives
    /// the same bytes. A .xltx, .xlsm or .xltm extension makes a template or
    /// a macro-enabled workbook.
    New {
        /// The name of a sheet, once for each sheet; `Sheet1` when none
        #[arg(long = "sheet", value_name = "NAME")]
        sheets: Vec<String>,
        /// The file to write, never replaced unless --force
        #[arg(short = 'o', long)]
        output: PathBuf,
        /// Replace the output when it exists
        #[arg(long)]
        force: bool,
        /// Output the result as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Edit a workbook from a JSON map of the edits, in order
    ///
    /// The map is an array of objects, applied in order, all or nothing; a
    /// refusal names the entry, counted from 0. Each entry does one thing:
    /// {"cell": "B2", "value": 3} writes a number, a string, true, false or
    /// null (the content cleared); {"cell": "D2", "formula": "B2*C2"} a
    /// formula, with "stored": true for the stored form and "spill": true for
    /// a spilling one; {"cell": "E2", "date": "2026-01-05"} a date, a date and
    /// time or a time; {"range": "A1:F1", "style": {"bold": true, "fill":
    /// "FFFF00"}} changes parts of the style (bold, italic, underline,
    /// strike, font_name, font_size, font_color, fill, border, border_color,
    /// horizontal, vertical, wrap_text, number_format); {"range": "C2:C8",
    /// "number_format": "0.00"} sets the number format; {"range": "A20:B30",
    /// "clear": "contents"} clears contents, formats or all. A style of a
    /// range of cells writes at most 1,000,000 of them, of whole rows at most
    /// 100,000 rows; a clear of formats writes at most 1,000,000 cells under a
    /// styled row or column.
    /// {"add_sheet": "Summary"} adds a sheet after the others and makes it
    /// the sheet of the entries after it; {"columns": "A:C", "width": 20}
    /// sets column widths, in characters; {"rows": "2:5", "height": 30} sets
    /// row heights, in points; {"freeze": "B2"} freezes the rows above and
    /// the columns left of a cell, {"freeze": null} removes the frozen
    /// panes. An entry names its sheet with "sheet" (a name, or a number
    /// from 1); without it, it takes the sheet of the entry before it, and
    /// the first entry the first visible worksheet.
    Edit {
        file: PathBuf,
        /// The JSON file of the edits
        #[arg(long, value_name = "EDITS.json")]
        map: PathBuf,
        /// The file to write, never the input and never replaced unless --force
        #[arg(short = 'o', long)]
        output: PathBuf,
        /// Replace the output when it exists
        #[arg(long)]
        force: bool,
        /// Drop every formula's cached value, so that LibreOffice computes them on open
        #[arg(long)]
        drop_cached_values: bool,
        /// Output the result as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Write a value or a formula into a cell
    ///
    /// VALUE is read as a CSV import reads a field: a number (no leading
    /// zero, at most 15 significant digits), TRUE or FALSE, a date
    /// YYYY-MM-DD with a time or not, an empty text clearing the cell, any
    /// other text as it is. --text keeps it text, --formula makes it a formula.
    /// Options go before or after CELL and VALUE; a VALUE that starts with `-`
    /// and is not a number goes last, after `--`.
    Set {
        file: PathBuf,
        /// A1 reference of the cell, with a sheet prefix or not ('Q1 data'!B3)
        cell: String,
        /// The value, or the formula with --formula. A negative number is a
        /// value; any other text that starts with `-` goes after `--`, the
        /// options before it (`set FILE A1 --formula -o OUT -- -A1*2`)
        #[arg(allow_negative_numbers = true)]
        value: String,
        /// The sheet, by name or number from 1 (the first visible worksheet by default)
        #[arg(long)]
        sheet: Option<String>,
        /// Write VALUE as text, whatever it looks like
        #[arg(long, conflicts_with = "formula")]
        text: bool,
        /// VALUE is a formula, as Excel displays it (an `=` is accepted)
        #[arg(long)]
        formula: bool,
        /// The formula is in the form the file stores (`_xlfn.XLOOKUP(...)`)
        #[arg(long, requires = "formula")]
        stored: bool,
        /// A dynamic array formula that spills from the cell (Excel 365), over
        /// the range Excel finds; Google Sheets shows its first value alone
        #[arg(long, requires = "formula")]
        spill: bool,
        /// A dynamic array formula that spills over this range, from the cell
        /// (`J2:J5`): Google Sheets spills it too
        #[arg(
            long,
            requires = "formula",
            conflicts_with = "spill",
            value_name = "RANGE"
        )]
        spill_range: Option<String>,
        /// The file to write, never the input and never replaced unless --force
        #[arg(short = 'o', long)]
        output: PathBuf,
        /// Replace the output when it exists
        #[arg(long)]
        force: bool,
        /// Drop every formula's cached value, so that LibreOffice computes them on open
        #[arg(long)]
        drop_cached_values: bool,
        /// Output the result as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Write a CSV file into a sheet, from a cell
    ///
    /// The rectangle the CSV covers is written, an empty field clearing its
    /// cell. UTF-8, a byte order mark skipped. Fields are typed strictly: a
    /// number (no leading zero, at most 15 significant digits), TRUE or
    /// FALSE, a date YYYY-MM-DD with a time or not, the rest text (a leading
    /// = included: a CSV holds no formula).
    ImportCsv {
        file: PathBuf,
        /// The CSV file
        csv: PathBuf,
        /// The sheet, by name or number from 1 (the first visible worksheet by default)
        #[arg(long)]
        sheet: Option<String>,
        /// The cell of the first field
        #[arg(long, default_value = "A1")]
        at: String,
        /// Keep every field text
        #[arg(long)]
        text: bool,
        /// The file to write, never the input and never replaced unless --force
        #[arg(short = 'o', long)]
        output: PathBuf,
        /// Replace the output when it exists
        #[arg(long)]
        force: bool,
        /// Drop every formula's cached value, so that LibreOffice computes them on open
        #[arg(long)]
        drop_cached_values: bool,
        /// Output the result as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
}

/// Runs a parsed `oxoffice xlsx` command. A failure prints its error and
/// ends the process with status 1.
pub fn run(cli: Cli) {
    let result = match cli.command {
        Command::Inspect { file, json } => inspect(&file, json),
        Command::New {
            sheets,
            output,
            force,
            json,
        } => new(&sheets, &output, force, json),
        Command::Edit {
            file,
            map,
            output,
            force,
            drop_cached_values,
            json,
        } => edit_workbook(
            &file,
            &map,
            &Output {
                path: output,
                force,
                drop_cached_values,
                json,
            },
        ),
        Command::Set {
            file,
            cell,
            value,
            sheet,
            text,
            formula,
            stored,
            spill,
            spill_range,
            output,
            force,
            drop_cached_values,
            json,
        } => (|| -> Result<()> {
            let input = if formula {
                let spill = match spill_range {
                    Some(range) => Some(spill_option(&range)?),
                    None => spill.then_some(Spill::Anchor),
                };
                CellInput::Formula(value, FormulaOptions { stored, spill })
            } else if text {
                CellInput::Value(if value.is_empty() {
                    Value::Empty
                } else {
                    Value::Text(value)
                })
            } else {
                CellInput::infer(&value)
            };
            set(
                &file,
                &cell,
                sheet.as_deref(),
                input,
                &Output {
                    path: output,
                    force,
                    drop_cached_values,
                    json,
                },
            )
        })(),
        Command::ImportCsv {
            file,
            csv,
            sheet,
            at,
            text,
            output,
            force,
            drop_cached_values,
            json,
        } => import_csv(
            &file,
            &csv,
            sheet.as_deref(),
            &at,
            !text,
            &Output {
                path: output,
                force,
                drop_cached_values,
                json,
            },
        ),
        Command::Text {
            file,
            json,
            markdown,
            sheet,
        } => text(&file, json, markdown, &sheet),
        Command::Csv {
            file,
            sheet,
            raw,
            delimiter,
            bom,
            output,
            force,
        } => csv(
            &file,
            sheet.as_deref(),
            raw,
            &delimiter,
            bom,
            output.as_deref(),
            force,
        ),
        Command::Cells {
            file,
            range,
            sheet,
            json,
        } => cells(&file, &range, sheet.as_deref(), json),
        Command::Comments { file, sheet, json } => comments(&file, &sheet, json),
    };
    let flushed = io::stdout().flush();
    if let Err(error) = result.and(flushed.map_err(Into::into)) {
        eprintln!("Error: {error}");
        process::exit(1);
    }
}

fn class_name(class: WorkbookClass) -> &'static str {
    match class {
        WorkbookClass::Workbook => "workbook",
        WorkbookClass::MacroEnabledWorkbook => "macro-enabled workbook",
        WorkbookClass::Template => "template",
        WorkbookClass::MacroEnabledTemplate => "macro-enabled template",
    }
}

/// The sheet `spec` names: a name (exact, then in any case), else a number
/// from 1.
fn sheet<'a>(workbook: &'a Workbook, spec: &str) -> Result<Worksheet<'a>> {
    workbook
        .sheet(spec)
        .or_else(|| {
            spec.parse::<usize>()
                .ok()
                .and_then(|number| number.checked_sub(1))
                .and_then(|position| workbook.sheet_at(position))
        })
        .ok_or_else(|| format!("no sheet is named or numbered {spec}").into())
}

/// The sheets `specs` name, or every sheet when they name none.
fn sheets<'a>(workbook: &'a Workbook, specs: &[String]) -> Result<Vec<Worksheet<'a>>> {
    if specs.is_empty() {
        return Ok(workbook.sheets().collect());
    }
    specs.iter().map(|spec| sheet(workbook, spec)).collect()
}

/// The sheet `spec` names, or the first visible worksheet.
fn sheet_or_first<'a>(workbook: &'a Workbook, spec: Option<&str>) -> Result<Worksheet<'a>> {
    match spec {
        Some(spec) => sheet(workbook, spec),
        None => workbook
            .sheets()
            .find(|sheet| {
                sheet.kind() == SheetKind::Worksheet && sheet.state() == SheetState::Visible
            })
            .ok_or_else(|| "the workbook has no visible worksheet: name one with --sheet".into()),
    }
}

fn inspect(file: &Path, as_json: bool) -> Result<()> {
    let workbook = Workbook::open(file)?;
    let date1904 = workbook.date_system() == DateSystem::Date1904;
    let mut out = io::stdout().lock();
    if as_json {
        let mut sheets = Vec::new();
        for sheet in workbook.sheets() {
            let worksheet = sheet.kind() == SheetKind::Worksheet;
            let page_setup = if worksheet {
                json::page_setup(&sheet.page_setup()?)
            } else {
                serde_json::Value::Null
            };
            let used_range = sheet.used_range()?;
            sheets.push(json!({
                "sheet": sheet.position() + 1,
                "name": sheet.name(),
                "state": sheet.state().as_str(),
                "kind": sheet.kind().as_str(),
                "used_range": used_range.map(|range| range.to_string()),
                "page_setup": page_setup,
            }));
        }
        let record = json_envelope(json!({
            "file": file.display().to_string(),
            "class": class_name(workbook.class()),
            "date1904": date1904,
            "recalculation_needed": workbook.recalculation_needed(),
            "sheets": sheets,
        }))?;
        writeln!(out, "{}", serde_json::to_string_pretty(&record)?)?;
        return Ok(());
    }
    writeln!(out, "File: {}", file.display())?;
    writeln!(out, "Class: {}", class_name(workbook.class()))?;
    writeln!(
        out,
        "Date system: {}",
        if date1904 { "1904" } else { "1900" }
    )?;
    if workbook.recalculation_needed() {
        writeln!(
            out,
            "Recalculation: asked on open, cached values may be stale"
        )?;
    }
    writeln!(out, "Sheets: {}", workbook.sheets().len())?;
    for sheet in workbook.sheets() {
        let mut details = vec![sheet.kind().as_str().to_owned()];
        if sheet.state() != SheetState::Visible {
            details.push(sheet.state().as_str().to_owned());
        }
        if let Some(used) = sheet.used_range()? {
            details.push(used.to_string());
        }
        writeln!(
            out,
            "  {}. {} ({})",
            sheet.position() + 1,
            sheet.name(),
            details.join(", ")
        )?;
    }
    Ok(())
}

fn new(sheets: &[String], output: &Path, force: bool, as_json: bool) -> Result<()> {
    ensure_new_output_allowed(output, force)?;
    let workbook = if sheets.is_empty() {
        Workbook::new()
    } else {
        Workbook::with_sheet_names(sheets)?
    };
    workbook.save(output)?;
    written(
        "new",
        json!({"sheets": workbook.sheets().map(|sheet| sheet.name().to_owned()).collect::<Vec<_>>()}),
        output,
        as_json,
    )
}

/// The range `--spill-range` gives a spilling formula.
fn spill_option(range: &str) -> Result<Spill> {
    CellRange::parse(range)
        .map(Spill::Range)
        .ok_or_else(|| format!("--spill-range takes a range such as J2:J5, not {range:?}").into())
}

/// Where an edited workbook is written, and how.
struct Output {
    path: PathBuf,
    force: bool,
    drop_cached_values: bool,
    json: bool,
}

impl Output {
    /// Saves `workbook` with the options asked, and reports it.
    fn save(&self, workbook: &Workbook, operation: &str, detail: serde_json::Value) -> Result<()> {
        let options = SaveOptions {
            drop_cached_values: self.drop_cached_values,
            ..SaveOptions::default()
        };
        workbook.save_with_options(&self.path, options)?;
        written(operation, detail, &self.path, self.json)
    }
}

fn edit_workbook(file: &Path, map: &Path, output: &Output) -> Result<()> {
    ensure_output_paths_allowed(std::slice::from_ref(&output.path), file, output.force)?;
    let edits = edit::parse(&std::fs::read_to_string(map)?)?;
    let mut workbook = Workbook::open(file)?;
    edit::apply(&mut workbook, &edits)?;
    output.save(&workbook, "edit", json!({"edits": edits.len()}))
}

/// The position of the sheet a cell's prefix or `--sheet` names, or of the
/// first visible worksheet, and the reference without its prefix.
fn sheet_position<'r>(
    workbook: &Workbook,
    reference: &'r str,
    spec: Option<&str>,
) -> Result<(usize, &'r str)> {
    let (prefix, reference) = split_sheet(reference);
    let position = match (prefix, spec) {
        (Some(_), Some(_)) => {
            return Err("the cell names its sheet: drop --sheet, or the sheet prefix".into());
        }
        (Some(name), None) => workbook
            .sheet(&name)
            .ok_or_else(|| format!("no sheet is named {name}"))?
            .position(),
        (None, spec) => sheet_or_first(workbook, spec)?.position(),
    };
    Ok((position, reference))
}

fn set(
    file: &Path,
    cell: &str,
    spec: Option<&str>,
    input: CellInput,
    output: &Output,
) -> Result<()> {
    ensure_output_paths_allowed(std::slice::from_ref(&output.path), file, output.force)?;
    let mut workbook = Workbook::open(file)?;
    let (position, reference) = sheet_position(&workbook, cell, spec)?;
    let mut sheet = workbook
        .sheet_at_mut(position)
        .expect("a position just found");
    sheet.set_value(reference, input)?;
    let name = sheet.name().to_owned();
    output.save(
        &workbook,
        "set",
        json!({"sheet": position + 1, "name": name, "cell": reference}),
    )
}

fn import_csv(
    file: &Path,
    csv: &Path,
    spec: Option<&str>,
    at: &str,
    infer: bool,
    output: &Output,
) -> Result<()> {
    ensure_output_paths_allowed(std::slice::from_ref(&output.path), file, output.force)?;
    let text = std::fs::read(csv)?;
    let mut workbook = Workbook::open(file)?;
    let (position, reference) = sheet_position(&workbook, at, spec)?;
    let mut sheet = workbook
        .sheet_at_mut(position)
        .expect("a position just found");
    sheet.import_csv(reference, &text, CsvImport { infer })?;
    let name = sheet.name().to_owned();
    output.save(
        &workbook,
        "import-csv",
        json!({"sheet": position + 1, "name": name, "at": reference}),
    )
}

/// Reports a file written: a schema-1 record, or a line.
fn written(operation: &str, detail: serde_json::Value, output: &Path, as_json: bool) -> Result<()> {
    let mut out = io::stdout().lock();
    if as_json {
        let mut payload = json!({
            "operation": operation,
            "output": output.display().to_string(),
        });
        let object = payload.as_object_mut().expect("literal is an object");
        if let serde_json::Value::Object(detail) = detail {
            object.extend(detail);
        }
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&json_envelope(payload)?)?
        )?;
    } else {
        writeln!(out, "Written to {}", output.display())?;
    }
    Ok(())
}

/// A sheet as tab-separated displayed texts: `# <name>`, then each row that
/// holds a value, from the first column of the used range; a sheet that
/// holds no cells is `# <name> (chartsheet)`, its kind named.
fn plain_text(sheet: &Worksheet<'_>) -> Result<String> {
    let mut out = match sheet.kind() {
        SheetKind::Worksheet => format!("# {}\n", sheet.name()),
        kind => format!("# {} ({})\n", sheet.name(), kind.as_str()),
    };
    let Some(used) = sheet.used_range()? else {
        return Ok(out);
    };
    let cells = sheet.cells(&used.to_string())?;
    let mut at = 0;
    while at < cells.len() {
        let row = cells[at].row();
        let end = at
            + cells[at..]
                .iter()
                .take_while(|cell| cell.row() == row)
                .count();
        let last = cells[end - 1].column();
        let mut fields = vec![""; (last - used.first.column + 1) as usize];
        for cell in &cells[at..end] {
            fields[(cell.column() - used.first.column) as usize] = &cell.text;
        }
        out.push_str(&fields.join("\t"));
        out.push('\n');
        at = end;
    }
    Ok(out)
}

fn text(file: &Path, as_json: bool, markdown: bool, specs: &[String]) -> Result<()> {
    let workbook = Workbook::open(file)?;
    let sheets = sheets(&workbook, specs)?;
    let mut out = io::stdout().lock();
    if as_json {
        let mut records = Vec::new();
        for sheet in &sheets {
            let used = sheet.used_range()?;
            let cells = match used {
                Some(used) => sheet.cells(&used.to_string())?,
                None => Vec::new(),
            };
            records.push(json!({
                "sheet": sheet.position() + 1,
                "name": sheet.name(),
                "state": sheet.state().as_str(),
                "kind": sheet.kind().as_str(),
                "used_range": used.map(|range| range.to_string()),
                "cells": cells.iter().map(json::text_cell).collect::<Vec<_>>(),
                "merged": sheet.merged_ranges()?.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "comments": sheet.comments()?.iter().map(json::comment).collect::<Vec<_>>(),
            }));
        }
        let record = json_envelope(json!({
            "date1904": workbook.date_system() == DateSystem::Date1904,
            "recalculation_needed": workbook.recalculation_needed(),
            "sheets": records,
        }))?;
        writeln!(out, "{}", serde_json::to_string_pretty(&record)?)?;
        return Ok(());
    }
    let sections = sheets
        .iter()
        .map(|sheet| {
            if markdown {
                Ok(sheet.to_markdown()?)
            } else {
                plain_text(sheet)
            }
        })
        .collect::<Result<Vec<_>>>()?;
    write!(out, "{}", sections.join("\n"))?;
    Ok(())
}

fn csv(
    file: &Path,
    spec: Option<&str>,
    raw: bool,
    delimiter: &str,
    bom: bool,
    output: Option<&Path>,
    force: bool,
) -> Result<()> {
    let delimiter = match delimiter {
        "tab" => '\t',
        other => {
            let mut characters = other.chars();
            match (characters.next(), characters.next()) {
                (Some(character), None) if !matches!(character, '"' | '\r' | '\n') => character,
                _ => {
                    return Err(format!(
                        "--delimiter takes one character other than a quote or a line break, or `tab`, not {other:?}"
                    )
                    .into());
                }
            }
        }
    };
    let workbook = Workbook::open(file)?;
    let sheet = sheet_or_first(&workbook, spec)?;
    let text = sheet.to_csv(&CsvOptions {
        delimiter,
        raw,
        bom,
    })?;
    match output {
        Some(output) => {
            ensure_output_paths_allowed(&[output.to_path_buf()], file, force)?;
            let mut staged = StagedOutputSet::with_replace_existing(force);
            staged.stage_bytes(output, text.as_bytes())?;
            staged.publish()?;
        }
        None => io::stdout().lock().write_all(text.as_bytes())?,
    }
    Ok(())
}

/// A range with its sheet prefix, if any: `'Q1 data'!B3` gives `Q1 data`
/// and `B3`.
fn split_sheet(range: &str) -> (Option<String>, &str) {
    let Some((prefix, reference)) = range.rsplit_once('!') else {
        return (None, range);
    };
    let name = match prefix
        .strip_prefix('\'')
        .and_then(|quoted| quoted.strip_suffix('\''))
    {
        Some(quoted) => quoted.replace("''", "'"),
        None => prefix.to_owned(),
    };
    (Some(name), reference)
}

fn cells(file: &Path, range: &str, spec: Option<&str>, as_json: bool) -> Result<()> {
    let workbook = Workbook::open(file)?;
    let (prefix, reference) = split_sheet(range);
    let sheet = match (prefix, spec) {
        (Some(_), Some(_)) => {
            return Err("the range names its sheet: drop --sheet, or the sheet prefix".into());
        }
        (Some(name), None) => workbook
            .sheet(&name)
            .ok_or_else(|| format!("no sheet is named {name}"))?,
        (None, spec) => sheet_or_first(&workbook, spec)?,
    };
    let cells = sheet.cells(reference)?;
    let mut out = io::stdout().lock();
    if as_json {
        let rectangle = sheet.range(reference)?;
        let merged: Vec<String> = sheet
            .merged_ranges()?
            .iter()
            .filter(|merge| rectangle.is_some_and(|range| range.intersects(merge)))
            .map(ToString::to_string)
            .collect();
        let record = json_envelope(json!({
            "sheet": sheet.position() + 1,
            "name": sheet.name(),
            "range": reference,
            "cells": cells.iter().map(json::full_cell).collect::<Vec<_>>(),
            "merged": merged,
        }))?;
        writeln!(out, "{}", serde_json::to_string_pretty(&record)?)?;
        return Ok(());
    }
    for cell in &cells {
        match &cell.formula {
            Some(formula) => writeln!(out, "{}\t{}\t={formula}", cell.address, cell.text)?,
            None => writeln!(out, "{}\t{}", cell.address, cell.text)?,
        }
    }
    Ok(())
}

fn comments(file: &Path, specs: &[String], as_json: bool) -> Result<()> {
    let workbook = Workbook::open(file)?;
    let sheets = sheets(&workbook, specs)?;
    let mut out = io::stdout().lock();
    if as_json {
        let mut records = Vec::new();
        for sheet in &sheets {
            records.push(json!({
                "sheet": sheet.position() + 1,
                "name": sheet.name(),
                "comments": sheet.comments()?.iter().map(json::comment).collect::<Vec<_>>(),
            }));
        }
        let record = json_envelope(json!({ "sheets": records }))?;
        writeln!(out, "{}", serde_json::to_string_pretty(&record)?)?;
        return Ok(());
    }
    for sheet in &sheets {
        for comment in sheet.comments()? {
            let resolved = if comment.resolved { ", resolved" } else { "" };
            writeln!(
                out,
                "{}!{} ({}{resolved}) {}: {}",
                sheet.name(),
                comment.address,
                comment.kind.as_str(),
                comment.author.as_deref().unwrap_or("unknown"),
                comment.text
            )?;
            for reply in &comment.replies {
                writeln!(
                    out,
                    "  reply {}: {}",
                    reply.author.as_deref().unwrap_or("unknown"),
                    reply.text
                )?;
            }
        }
    }
    Ok(())
}
