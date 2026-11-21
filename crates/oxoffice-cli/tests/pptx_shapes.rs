//! `oxoffice pptx shapes` lists every shape of a slide, those inside groups
//! included, depth first in z-order, with the path of each one in `--json`;
//! the deck read commands keep their JSON keys.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use oxoffice_pptx::{Emu, Presentation};
use serde_json::Value;

#[path = "support/snapshots.rs"]
mod snapshots;

use snapshots::{assert_keys, corpus};

const INCH: Emu = Emu(914_400);

/// The corpus deck with `a`, `outer` [`b`, `inner` [`c`]] added on top of
/// the `n` shapes of its second slide. Returns the file and `n`.
fn grouped_deck() -> (PathBuf, usize) {
    let corpus =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files/powerpoint/deck.pptx");
    let mut prs = Presentation::open(corpus).unwrap();
    let n = prs.slide(1).unwrap().shapes().len();
    let mut add = |group: &[usize], name: &str, is_group: bool| {
        let mut shapes = prs.shapes_mut(1, group).unwrap();
        let mut shape = if is_group {
            shapes.add_group_shape().unwrap()
        } else {
            let mut shape = shapes.add_textbox(INCH, INCH, INCH, INCH).unwrap();
            shape.set_text(name).unwrap();
            shape
        };
        shape.set_name(name).unwrap();
    };
    add(&[], "a", false);
    add(&[], "outer", true);
    add(&[n + 1], "b", false);
    add(&[n + 1], "inner", true);
    add(&[n + 1, 1], "c", false);
    let path =
        std::env::temp_dir().join(format!("oxoffice-cli-shapes-{}.pptx", std::process::id()));
    prs.save(&path).unwrap();
    (path, n)
}

fn shapes(path: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["pptx", "shapes"])
        .arg(path)
        .args(args)
        .output()
        .expect("oxoffice runs")
}

#[test]
fn shapes_lists_grouped_shapes_with_their_paths() {
    let (path, n) = grouped_deck();
    let output = shapes(&path, &["--slide", "2", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let slides = value["slides"].as_array().unwrap();
    assert_eq!(slides.len(), 1);
    assert_eq!(slides[0]["slide"], 2);
    let rows: Vec<(Value, u64, String)> = slides[0]["shapes"].as_array().unwrap()[n..]
        .iter()
        .map(|shape| {
            (
                shape["path"].clone(),
                shape["depth"].as_u64().unwrap(),
                shape["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let o = n + 1;
    let expected = [
        (serde_json::json!([n]), 0, "a"),
        (serde_json::json!([o]), 0, "outer"),
        (serde_json::json!([o, 0]), 1, "b"),
        (serde_json::json!([o, 1]), 1, "inner"),
        (serde_json::json!([o, 1, 0]), 2, "c"),
    ]
    .map(|(path, depth, name)| (path, depth, name.to_owned()));
    assert_eq!(rows, expected);
    let c = &slides[0]["shapes"][n + 4];
    assert_eq!(c["text"], "c");
    assert_eq!(c["kind"], "shape");

    let text = String::from_utf8(shapes(&path, &["--slide", "2"]).stdout).unwrap();
    assert!(text.starts_with("Slide 2\n"), "{text}");
    assert!(
        text.contains("\n    ") && text.contains(" group \"inner\""),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|line| line.starts_with("      ") && line.ends_with(" \"c\"")),
        "{text}"
    );

    let all = String::from_utf8(shapes(&path, &[]).stdout).unwrap();
    assert!(
        all.contains("Slide 1\n") && all.contains("Slide 2\n"),
        "{all}"
    );
    assert!(!shapes(&path, &["--slide", "0"]).status.success());
    let _ = std::fs::remove_file(path);
}

#[test]
fn the_deck_read_commands_keep_their_json_keys() {
    let decks = ["powerpoint/training.pptx", "google/training.pptx"].map(corpus);
    let read = |command: &str, deck: &PathBuf| {
        let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
            .args(["pptx", command])
            .arg(deck)
            .arg("--json")
            .output()
            .expect("oxoffice runs");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice::<Value>(&output.stdout).expect("JSON output")
    };
    for command in ["text", "fit", "shapes"] {
        let outputs = decks.iter().map(|deck| read(command, deck)).collect::<Vec<_>>();
        assert_keys(&format!("pptx-{command}"), &outputs);
    }
}
