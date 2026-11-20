//! The `oxoffice._oxoffice` Python extension.
//!
//! One native module serves the whole `oxoffice` package. It registers the
//! Word bindings as `oxoffice.docx._native`, the PowerPoint bindings as
//! `oxoffice.pptx._native`, and exposes `run_cli` for the `oxoffice` console
//! script.

use std::ffi::OsString;
use std::io::Write;

use pyo3::prelude::*;
use pyo3::types::PyModule;

/// Run the `oxoffice` command line on `argv`, the program name first.
///
/// A command that succeeds returns 0. A failure or a verdict ends the process
/// with its exit status, exactly as the native binary does.
#[pyfunction]
fn run_cli(py: Python<'_>, argv: Vec<OsString>) -> i32 {
    py.detach(|| {
        oxoffice_cli::main(argv);
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
    });
    0
}

/// The known limitations of this build as a JSON array, see
/// `oxoffice.limitations()`.
#[pyfunction]
fn limitations_json() -> &'static str {
    oxoffice_cli::LIMITATIONS_JSON
}

fn register_family(
    parent: &Bound<'_, PyModule>,
    qualified: &str,
    attribute: &str,
    register: fn(&Bound<'_, PyModule>) -> PyResult<()>,
) -> PyResult<()> {
    let py = parent.py();
    let module = PyModule::new(py, qualified)?;
    register(&module)?;
    parent.add(attribute, &module)?;
    py.import("sys")?
        .getattr("modules")?
        .set_item(qualified, &module)?;
    Ok(())
}

#[pymodule]
fn _oxoffice(module: &Bound<'_, PyModule>) -> PyResult<()> {
    register_family(
        module,
        "oxoffice.docx._native",
        "docx",
        oxoffice_docx_py::register,
    )?;
    register_family(
        module,
        "oxoffice.pptx._native",
        "pptx",
        oxoffice_pptx_py::register,
    )?;
    module.add_function(wrap_pyfunction!(run_cli, module)?)?;
    module.add_function(wrap_pyfunction!(limitations_json, module)?)?;
    Ok(())
}
