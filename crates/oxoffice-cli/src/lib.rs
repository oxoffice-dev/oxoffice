//! The `oxoffice` command line. `oxoffice docx`, `oxoffice pptx` and
//! `oxoffice xlsx` carry the command trees of the three document families,
//! and `oxoffice limitations` lists what this build cannot do yet.
//!
//! The native binary and the Python console script both enter through
//! [`main`], so they share parsing, exit statuses and output behaviour.

use std::ffi::OsString;
use std::io::Write;

use clap::{Args, Parser, Subcommand};

/// The known limitations of this build, with their workarounds, as a JSON
/// array. Each entry has `id`, `format` (`docx`, `pptx`, `xlsx` or `all`),
/// `area`, `title`, `symptom`, `workaround` and `issue`, the number of the
/// GitHub issue that tracks it. The source is `limitations/<id>.json` next to
/// this crate's manifest, one file per entry, joined in file name order by
/// the build script.
pub const LIMITATIONS_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/limitations.json"));

#[derive(Parser)]
#[command(
    name = "oxoffice",
    version,
    about = "Inspect, convert, edit and render Word and PowerPoint files, inspect Excel workbooks"
)]
struct Cli {
    #[command(subcommand)]
    family: Family,
}

#[derive(Subcommand)]
enum Family {
    /// Word documents (.docx, .dotx)
    Docx(oxoffice_docx_cli::Cli),
    /// PowerPoint presentations (.pptx, .potx)
    Pptx(oxoffice_pptx_cli::Cli),
    /// Excel workbooks (.xlsx, .xlsm, .xltx, .xltm)
    Xlsx(oxoffice_xlsx_cli::Cli),
    /// List the known limitations of this build, with their workarounds
    Limitations(LimitationsArgs),
}

#[derive(Args)]
struct LimitationsArgs {
    /// Only the limitations of this format
    #[arg(value_parser = ["docx", "pptx", "xlsx"])]
    format: Option<String>,
    /// Output the limitations as schema-1 JSON, under `limitations`
    #[arg(long)]
    json: bool,
}

/// Run the command line on `args`, the program name first.
///
/// A command that succeeds returns. A failure or a verdict ends the process
/// with its exit status, exactly as the native binary does. On Windows the
/// command runs on a thread with an 8 MiB stack, because a full comparison or
/// validation can exceed the one MiB main-thread stack.
pub fn main(args: Vec<OsString>) {
    #[cfg(windows)]
    {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || run(args))
            .expect("start oxoffice CLI thread")
            .join()
            .expect("oxoffice CLI thread panicked");
    }
    #[cfg(not(windows))]
    run(args);
}

fn run(args: Vec<OsString>) {
    match Cli::parse_from(args).family {
        Family::Docx(cli) => oxoffice_docx_cli::run(cli),
        Family::Pptx(cli) => oxoffice_pptx_cli::run(cli),
        Family::Xlsx(cli) => oxoffice_xlsx_cli::run(cli),
        Family::Limitations(args) => limitations(&args),
    }
}

/// Print the limitations of one format, or of all of them. A closed output
/// pipe ends the listing quietly.
fn limitations(args: &LimitationsArgs) {
    let all: Vec<serde_json::Value> =
        serde_json::from_str(LIMITATIONS_JSON).expect("the build script writes a JSON array");
    let selected: Vec<&serde_json::Value> = all
        .iter()
        .filter(|entry| match args.format.as_deref() {
            Some(format) => entry["format"] == format || entry["format"] == "all",
            None => true,
        })
        .collect();
    let mut out = std::io::stdout().lock();
    if args.json {
        let record = oxoffice_cli_support::json_envelope(serde_json::json!({
            "limitations": selected,
        }))
        .expect("the record is an object without a schema key");
        let json = serde_json::to_string_pretty(&record).expect("entries serialise");
        let _ = writeln!(out, "{json}");
        return;
    }
    if selected.is_empty() {
        let _ = writeln!(out, "No known limitation.");
        return;
    }
    let text = |entry: &serde_json::Value, key: &str| entry[key].as_str().unwrap_or("").to_owned();
    for entry in selected {
        let lines = [
            format!(
                "{} [{}] {}",
                text(entry, "format"),
                text(entry, "area"),
                text(entry, "title")
            ),
            format!("  symptom:    {}", text(entry, "symptom")),
            format!("  workaround: {}", text(entry, "workaround")),
            format!(
                "  tracked in: https://github.com/oxoffice-dev/oxoffice/issues/{}",
                entry["issue"]
            ),
            String::new(),
        ];
        for line in lines {
            if writeln!(out, "{line}").is_err() {
                return;
            }
        }
    }
}
