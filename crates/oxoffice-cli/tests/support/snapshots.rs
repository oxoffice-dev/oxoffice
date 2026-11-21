//! Snapshots shared by the CLI test files that include this module: the key
//! paths of a JSON output, and short text listings, each kept in a file under
//! `tests/snapshots/`. A change shows as a diff of that file in the pull
//! request that makes it. `OXOFFICE_SNAPSHOTS_UPDATE=1` rewrites the files a
//! run reads; a changed key also needs its line in the migration note.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

/// Whether the workflow tests read every producer's copy (`main` and the
/// nightly run) or one file each (pull requests). Read at run time, so that
/// a build without the variable still compiles every test target.
#[allow(dead_code)]
pub fn every_copy() -> bool {
    std::env::var("OXOFFICE_WORKFLOWS").is_ok_and(|value| value == "full")
}

/// `tests/corpus/files/<path>`.
#[allow(dead_code)]
pub fn corpus(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(path)
}

/// Every key path of `value`: `a.b` for a key of an object, `a[]` for the
/// items of an array, whatever the values.
pub fn key_paths(value: &Value) -> BTreeSet<String> {
    fn walk(value: &Value, prefix: &str, paths: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    paths.insert(path.clone());
                    walk(child, &path, paths);
                }
            }
            Value::Array(items) => {
                let path = format!("{prefix}[]");
                for item in items {
                    walk(item, &path, paths);
                }
            }
            _ => {}
        }
    }
    let mut paths = BTreeSet::new();
    walk(value, "", &mut paths);
    paths
}

/// `lines` equal `tests/snapshots/<name>.txt`, one line each.
pub fn assert_snapshot(name: &str, lines: &[String]) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    let found = lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    if std::env::var_os("OXOFFICE_SNAPSHOTS_UPDATE").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &found).unwrap();
        return;
    }
    let kept = fs::read_to_string(&path).unwrap_or_default();
    if kept != found {
        let kept_lines = kept.lines().collect::<BTreeSet<_>>();
        let found_lines = lines.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let gone = kept_lines.difference(&found_lines).collect::<Vec<_>>();
        let new = found_lines.difference(&kept_lines).collect::<Vec<_>>();
        panic!(
            "{} moved: gone {gone:?}, new {new:?}; if that is intended, rerun with \
             OXOFFICE_SNAPSHOTS_UPDATE=1 and say it in the pull request",
            path.display()
        );
    }
}

/// The key paths of every output in `outputs`, together, equal
/// `tests/snapshots/<name>.txt`.
pub fn assert_keys(name: &str, outputs: &[Value]) {
    let mut paths = BTreeSet::new();
    for output in outputs {
        paths.extend(key_paths(output));
    }
    assert_snapshot(name, &paths.into_iter().collect::<Vec<_>>());
}
