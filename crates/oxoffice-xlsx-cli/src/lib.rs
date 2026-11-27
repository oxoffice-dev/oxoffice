//! The `oxoffice xlsx` command tree: command-line access to the public
//! `oxoffice-xlsx` facade. Sheets are numbered from 1, like every number of
//! the command line.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use oxoffice_cli_support::json_envelope;
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
}

/// Runs a parsed `oxoffice xlsx` command. A failure prints its error and
/// ends the process with status 1.
pub fn run(cli: Cli) {
    let result = match cli.command {
        Command::Inspect { file, json } => inspect(&file, json),
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
