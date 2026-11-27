//! The `oxoffice xlsx` command tree: command-line access to the public
//! `oxoffice-xlsx` facade. Sheets are numbered from 1, like every number of
//! the command line.

mod edit;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use oxoffice_cli_support::{ensure_new_output_allowed, ensure_output_paths_allowed, json_envelope};
use oxoffice_xlsx::{DateSystem, SheetState, Workbook, WorkbookClass};
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
    /// Print the sheets of a workbook, its date system and class
    Inspect {
        file: PathBuf,
        /// Output the workbook as schema-1 JSON
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
    /// {"add_sheet": "Summary"} adds a sheet after the others and makes it
    /// the sheet of the entries after it; {"columns": "A:C", "width": 20}
    /// sets column widths, in characters; {"rows": "2:5", "height": 30} sets
    /// row heights, in points; {"freeze": "B2"} freezes the rows above and
    /// the columns left of a cell, {"freeze": null} removes the frozen
    /// panes. An entry names its sheet with "sheet" (a name, or a number
    /// from 1); without it, it takes the sheet of the entry before it, and
    /// the first entry the first sheet.
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
            json,
        } => edit_workbook(&file, &map, &output, force, json),
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

fn inspect(file: &Path, as_json: bool) -> Result<()> {
    let workbook = Workbook::open(file)?;
    let date1904 = workbook.date_system() == DateSystem::Date1904;
    let mut out = io::stdout().lock();
    if as_json {
        let sheets: Vec<_> = workbook
            .sheets()
            .map(|sheet| {
                json!({
                    "sheet": sheet.position() + 1,
                    "name": sheet.name(),
                    "state": sheet.state().as_str(),
                    "kind": sheet.kind().as_str(),
                })
            })
            .collect();
        let record = json_envelope(json!({
            "file": file.display().to_string(),
            "class": class_name(workbook.class()),
            "date1904": date1904,
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
    writeln!(out, "Sheets: {}", workbook.sheets().len())?;
    for sheet in workbook.sheets() {
        let state = match sheet.state() {
            SheetState::Visible => String::new(),
            state => format!(", {}", state.as_str()),
        };
        writeln!(
            out,
            "  {}. {} ({}{state})",
            sheet.position() + 1,
            sheet.name(),
            sheet.kind().as_str()
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

fn edit_workbook(file: &Path, map: &Path, output: &Path, force: bool, as_json: bool) -> Result<()> {
    ensure_output_paths_allowed(&[output.to_path_buf()], file, force)?;
    let edits = edit::parse(&std::fs::read_to_string(map)?)?;
    let mut workbook = Workbook::open(file)?;
    edit::apply(&mut workbook, &edits)?;
    workbook.save(output)?;
    written("edit", json!({"edits": edits.len()}), output, as_json)
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
