//! Snapshots shared by the CLI test files that include this module: the key
//! paths of a JSON output, and short text listings, each kept in a file under
//! `tests/snapshots/`. A change shows as a diff of that file in the pull
//! request that makes it. `OXOFFICE_SNAPSHOTS_UPDATE=1` rewrites the files a
//! run reads, outside CI only; run it with `OXOFFICE_WORKFLOWS=full` so that
//! every file is read. A changed key is a change users see: say it in the
//! pull request.

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

/// Whether this run rewrites the snapshots instead of checking them.
fn updating() -> bool {
    let update = std::env::var("OXOFFICE_SNAPSHOTS_UPDATE").is_ok_and(|value| value == "1");
    assert!(
        !(update && std::env::var_os("CI").is_some()),
        "OXOFFICE_SNAPSHOTS_UPDATE is for a local run, never under CI"
    );
    update
}

/// `tests/snapshots/<name>.txt`.
pub fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"))
}

/// `lines` equal `tests/snapshots/<name>.txt`, one line each, in order. Line
/// ends are read either way: a Windows checkout may turn them into CRLF.
pub fn assert_snapshot(name: &str, lines: &[String]) {
    let path = snapshot_path(name);
    if updating() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>();
        fs::write(&path, text).unwrap();
        return;
    }
    let kept = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; write it with OXOFFICE_SNAPSHOTS_UPDATE=1",
            path.display()
        )
    });
    let kept = kept
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect::<Vec<_>>();
    let found = lines.iter().map(String::as_str).collect::<Vec<_>>();
    if kept != found {
        let gone = kept
            .iter()
            .filter(|line| !found.contains(line))
            .collect::<Vec<_>>();
        let new = found
            .iter()
            .filter(|line| !kept.contains(line))
            .collect::<Vec<_>>();
        panic!(
            "{} moved: gone {gone:?}, new {new:?}, kept {kept:#?}, found {found:#?}; if that is \
             intended, rerun with OXOFFICE_SNAPSHOTS_UPDATE=1 OXOFFICE_WORKFLOWS=full and say it \
             in the pull request",
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
