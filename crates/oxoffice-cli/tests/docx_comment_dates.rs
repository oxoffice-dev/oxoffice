//! `oxoffice docx comment add` and `comment reply` date a comment now unless
//! given `--date`, and write none with `--undated`, on the Word and Google
//! Docs files of the corpus.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const STAMP: &str = "2026-01-02T03:04:05Z";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oxoffice-cli-comment-dates-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch folder");
        Self(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn report(producer: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(producer)
        .join("report.docx")
        .to_str()
        .unwrap()
        .to_owned()
}

fn oxoffice(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(args)
        .output()
        .expect("oxoffice runs")
}

fn succeeded(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// The comments of `file`, by id, as `comment list --json` gives them.
fn comments(file: &str) -> Vec<Value> {
    let output = succeeded(oxoffice(&["docx", "comment", "list", file, "--json"]));
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    record["comments"].as_array().unwrap().clone()
}

fn date_of(file: &str, id: i64) -> Value {
    comments(file)
        .into_iter()
        .find(|comment| comment["id"] == id)
        .unwrap_or_else(|| panic!("comment {id} in {file}"))["date"]
        .clone()
}

/// Add a comment on "Summary" with `extra` flags; its id.
fn add(input: &str, output: &str, extra: &[&str]) -> i64 {
    let mut args = vec![
        "docx", "comment", "add", input, "--anchor", "Summary", "--author", "Reviewer", "--text",
        "Check", "-o", output, "--json",
    ];
    args.extend_from_slice(extra);
    let record: Value = serde_json::from_slice(&succeeded(oxoffice(&args)).stdout).unwrap();
    record["comment_id"].as_i64().unwrap()
}

fn reply(input: &str, id: i64, output: &str, extra: &[&str]) -> i64 {
    let id = id.to_string();
    let mut args = vec![
        "docx", "comment", "reply", input, "--id", &id, "--author", "Author", "--text", "Done",
        "-o", output, "--json",
    ];
    args.extend_from_slice(extra);
    let record: Value = serde_json::from_slice(&succeeded(oxoffice(&args)).stdout).unwrap();
    record["comment_id"].as_i64().unwrap()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The Unix time of `YYYY-MM-DDTHH:MM:SSZ`.
fn unix_seconds(date: &str) -> u64 {
    assert_eq!(date.len(), 20, "{date:?}");
    assert!(date.ends_with('Z') && &date[10..11] == "T", "{date:?}");
    let field = |range: std::ops::Range<usize>| date[range].parse::<i64>().unwrap();
    let (year, month, day) = (field(0..4), field(5..7), field(8..10));
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + field(11..13) * 3_600 + field(14..16) * 60 + field(17..19))
        .unwrap()
}

#[test]
fn a_comment_and_a_reply_without_a_date_are_dated_now() {
    let scratch = Scratch::new("now");
    for producer in ["word", "google"] {
        let commented = scratch.path(&format!("{producer}-commented.docx"));
        let replied = scratch.path(&format!("{producer}-replied.docx"));
        let before = now();
        let id = add(&report(producer), &commented, &[]);
        let reply_id = reply(&commented, id, &replied, &[]);
        let after = now();
        for id in [id, reply_id] {
            let date = date_of(&replied, id);
            let seconds = unix_seconds(date.as_str().expect("a date"));
            assert!(
                before <= seconds && seconds <= after,
                "{producer}: {date} not between {before} and {after}"
            );
        }
    }
}

#[test]
fn date_writes_the_timestamp_and_undated_writes_none() {
    let scratch = Scratch::new("explicit");
    for producer in ["word", "google"] {
        let dated = scratch.path(&format!("{producer}-dated.docx"));
        let id = add(&report(producer), &dated, &["--date", STAMP]);
        assert_eq!(date_of(&dated, id), STAMP, "{producer}");
        let undated = scratch.path(&format!("{producer}-undated.docx"));
        let id = add(&report(producer), &undated, &["--undated"]);
        assert!(date_of(&undated, id).is_null(), "{producer}");
        let replied = scratch.path(&format!("{producer}-replied.docx"));
        let reply_id = reply(&undated, id, &replied, &["--undated"]);
        assert!(date_of(&replied, reply_id).is_null(), "{producer}");
    }
}

#[test]
fn date_and_undated_together_are_a_usage_error() {
    let scratch = Scratch::new("conflict");
    let output = scratch.path("out.docx");
    let refused = oxoffice(&[
        "docx",
        "comment",
        "add",
        &report("word"),
        "--anchor",
        "Summary",
        "--author",
        "R",
        "--text",
        "c",
        "--date",
        STAMP,
        "--undated",
        "-o",
        &output,
    ]);
    assert!(!refused.status.success());
    assert!(!Path::new(&output).exists());
}

#[test]
fn tables_only_anchors_on_a_match_in_a_table_cell() {
    let scratch = Scratch::new("tables-only");
    for producer in ["word", "google"] {
        for (flags, story) in [(&[][..], "body"), (&["--tables-only"][..], "table_cell")] {
            let output = scratch.path(&format!("{producer}-{story}.docx"));
            let mut args = vec![
                "docx",
                "comment",
                "add",
                &report(producer),
                "--anchor",
                "Churn",
                "--author",
                "R",
                "--text",
                "c",
                "--date",
                STAMP,
                "-o",
                &output,
                "--json",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
            args.extend(flags.iter().map(|flag| (*flag).to_owned()));
            let record: Value = serde_json::from_slice(
                &succeeded(oxoffice(
                    &args.iter().map(String::as_str).collect::<Vec<_>>(),
                ))
                .stdout,
            )
            .unwrap();
            let id = record["comment_id"].as_i64().unwrap();
            let comment = comments(&output)
                .into_iter()
                .find(|comment| comment["id"] == id)
                .unwrap();
            assert_eq!(
                comment["anchor"]["start"]["story_kind"], story,
                "{producer}"
            );
            assert_eq!(comment["anchor_text"], "Churn", "{producer}");
        }
    }
    let output = scratch.path("range.docx");
    let usage = oxoffice(&[
        "docx",
        "comment",
        "add",
        &report("word"),
        "--start-paragraph",
        "1",
        "--start-run",
        "0",
        "--end-paragraph",
        "1",
        "--end-run",
        "1",
        "--tables-only",
        "--author",
        "R",
        "--text",
        "c",
        "-o",
        &output,
    ]);
    assert!(!usage.status.success(), "--tables-only needs --anchor");
}
