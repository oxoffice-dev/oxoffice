//! `oxoffice pptx slide duplicate` copies a slide's modern comments, with
//! their replies, under fresh ids.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn oxoffice(args: &[&str], paths: &[&Path]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxoffice"));
    command.args(args);
    command.args(paths);
    let output = command.output().expect("oxoffice runs");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("output is UTF-8")
}

fn comments(path: &Path) -> Vec<Value> {
    let listed: Value =
        serde_json::from_str(&oxoffice(&["pptx", "comment", "list", "--json"], &[path]))
            .expect("comment list prints JSON");
    listed["comments"]
        .as_array()
        .expect("a comment array")
        .clone()
}

#[test]
fn a_duplicated_slide_carries_its_comments_under_fresh_ids() {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("oxoffice-cli-duplicate-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch folder");
    let deck =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files/powerpoint/deck.pptx");
    let (commented, replied, copied) = (
        dir.join("commented.pptx"),
        dir.join("replied.pptx"),
        dir.join("copied.pptx"),
    );

    oxoffice(
        &[
            "pptx",
            "comment",
            "add",
            "--slide",
            "2",
            "--author",
            "Ada Lovelace",
            "--initials",
            "AL",
            "--text",
            "Check the figures",
            "--date",
            "2026-11-21T10:00:00Z",
            "-o",
        ],
        &[&commented, &deck],
    );
    let thread = comments(&commented)[0]["id"].as_str().unwrap().to_owned();
    oxoffice(
        &[
            "pptx",
            "comment",
            "reply",
            "--id",
            &thread,
            "--author",
            "Ada Lovelace",
            "--text",
            "Done",
            "--date",
            "2026-11-21T10:05:00Z",
            "-o",
        ],
        &[&replied, &commented],
    );

    oxoffice(
        &["pptx", "slide", "duplicate", "-o"],
        &[&copied, &replied, Path::new("2")],
    );

    let listed = comments(&copied);
    let on = |slide: u64| -> Vec<&Value> {
        listed
            .iter()
            .filter(|entry| entry["slide"] == slide)
            .collect()
    };
    let (original, copy) = (on(2), on(3));
    let texts = |entries: &[&Value]| -> Vec<String> {
        entries
            .iter()
            .map(|entry| entry["text"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(texts(&original), ["Check the figures", "Done"]);
    assert_eq!(texts(&copy), texts(&original));
    for entry in &copy {
        assert_eq!(entry["author"], "Ada Lovelace");
        assert!(
            original.iter().all(|other| other["id"] != entry["id"]),
            "{entry}"
        );
    }
    assert_eq!(
        copy[1]["parent_id"], copy[0]["id"],
        "the reply follows its copied thread"
    );
    let _ = fs::remove_dir_all(&dir);
}
