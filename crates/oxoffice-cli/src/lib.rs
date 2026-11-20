//! The `oxoffice` command line. `oxoffice docx` and `oxoffice pptx` carry the
//! complete command trees of the two document families.
//!
//! The native binary and the Python console script both enter through
//! [`main`], so they share parsing, exit statuses and output behaviour.

use std::ffi::OsString;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "oxoffice",
    version,
    about = "Inspect, convert, edit and render Word and PowerPoint files"
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
    }
}
