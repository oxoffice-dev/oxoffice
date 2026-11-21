//! Joins `limitations/<id>.json`, one known limitation per file, into the
//! JSON array that `LIMITATIONS_JSON` embeds. One file per entry keeps pull
//! requests that add or remove different limitations from conflicting.
//!
//! Each file holds one JSON object whose `id` is the file name without
//! `.json`, in lower-case ASCII letters, digits and dashes. Entries are
//! joined in file name order, as written, so the array is the same on every
//! platform.

use std::fs;
use std::path::{Path, PathBuf};

const DIR: &str = "limitations";

fn main() {
    println!("cargo:rerun-if-changed={DIR}");
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let dir = manifest.join(DIR);
    let mut files: Vec<(String, PathBuf)> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| {
            let path = entry.expect("directory entry").path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_else(|| fail(&path, "the file name is not UTF-8"))
                .to_owned();
            (name, path)
        })
        .collect();
    files.sort();

    let mut entries = Vec::with_capacity(files.len());
    for (name, path) in &files {
        let Some(id) = name.strip_suffix(".json") else {
            fail(path, "only <id>.json files belong in this directory");
        };
        // Lower case only: two ids that differ in case would be one file on
        // Windows and macOS.
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            fail(path, "an id is lower-case ASCII letters, digits and dashes");
        }
        let text = fs::read_to_string(path).unwrap_or_else(|e| fail(path, &e.to_string()));
        let entry: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| fail(path, &format!("not JSON: {e}")));
        match entry.get("id").and_then(|v| v.as_str()) {
            Some(found) if found == id => {}
            Some(found) => fail(
                path,
                &format!("its id is {found:?}, its file name says {id:?}"),
            ),
            None => fail(path, "not an object with a string \"id\""),
        }
        entries.push(text.trim().to_owned());
    }

    let array = format!("[\n{}\n]\n", entries.join(",\n"));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets it"));
    fs::write(out.join("limitations.json"), array).expect("write limitations.json");
}

fn fail(path: &Path, message: &str) -> ! {
    let shown = Path::new(DIR).join(path.file_name().unwrap_or_default());
    panic!("{}: {message}", shown.display());
}
