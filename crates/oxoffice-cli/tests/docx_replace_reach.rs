//! `oxoffice docx replace` refuses a placeholder left out of its
//! replacement's reach, names where it is left and writes nothing, and
//! publishes with `--allow-unreached`, on the Word and Google Docs files of
//! the corpus.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use oxoffice_opc::OpcPackage;

/// ACME once in a plain paragraph, then cut by the edge of a content control.
const SPLIT: &str = r#"<w:p><w:r><w:t>ACME plain</w:t></w:r></w:p><w:p><w:r><w:t>AC</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t>ME control</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oxoffice-cli-replace-reach-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch folder");
        Self(dir)
    }

    /// The corpus report of `producer` with the split ACME before its last
    /// section properties.
    fn split_report(&self, producer: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/files")
            .join(producer)
            .join("report.docx");
        let mut package = OpcPackage::from_reader(Cursor::new(fs::read(source).unwrap())).unwrap();
        let main =
            String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
        let at = main.rfind("<w:sectPr").unwrap();
        package.set_part(
            "/word/document.xml",
            format!("{}{SPLIT}{}", &main[..at], &main[at..]).into_bytes(),
        );
        let path = self.0.join(format!("{producer}.docx"));
        package.save(&path).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn oxoffice(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(args)
        .output()
        .expect("oxoffice runs")
}

#[test]
fn replace_refuses_a_placeholder_left_out_of_reach_and_writes_nothing() {
    let scratch = Scratch::new("refused");
    for producer in ["word", "google"] {
        let input = scratch.split_report(producer);
        let output = scratch.0.join(format!("{producer}-out.docx"));
        let (input, output) = (input.to_str().unwrap(), output.to_str().unwrap());
        let refused = oxoffice(&[
            "docx", "replace", input, "-p", "ACME", "-v", "Globex", "-o", output,
        ]);
        assert!(!refused.status.success(), "{producer}");
        let message = String::from_utf8_lossy(&refused.stderr);
        assert!(
            message.contains(
                "1 occurrence(s) of \"ACME\" left after its replacement, out of its reach: \
                 /word/document.xml: \"ACME control\""
            ) && message.contains("--allow-unreached"),
            "{producer}: {message}"
        );
        assert!(
            !Path::new(output).exists(),
            "{producer}: nothing is written"
        );

        let allowed = oxoffice(&[
            "docx",
            "replace",
            input,
            "-p",
            "ACME",
            "-v",
            "Globex",
            "--expect",
            "1",
            "--allow-unreached",
            "-o",
            output,
        ]);
        assert!(
            allowed.status.success(),
            "{producer}: {}",
            String::from_utf8_lossy(&allowed.stderr)
        );
        let left = oxoffice(&["docx", "count", output, "ACME"]);
        assert_eq!(
            String::from_utf8(left.stdout).unwrap(),
            "body: 1\ntotal: 1\noutside comments: 1\n"
        );
    }
}

#[test]
fn replace_map_names_the_pair_left_out_of_reach() {
    let scratch = Scratch::new("map");
    let input = scratch.split_report("word");
    let map = scratch.0.join("pairs.json");
    fs::write(
        &map,
        r#"[{"placeholder": "Northwind", "value": "Southwind", "expect": 1},
            {"placeholder": "ACME", "value": "Globex"}]"#,
    )
    .unwrap();
    let output = scratch.0.join("out.docx");
    let refused = oxoffice(&[
        "docx",
        "replace",
        input.to_str().unwrap(),
        "--map",
        map.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("pair 1: 1 occurrence(s) of \"ACME\"")
    );
    assert!(!output.exists());
}

#[test]
fn replace_map_with_a_wrong_count_writes_nothing_and_leaves_the_input_as_it_was() {
    let scratch = Scratch::new("count");
    let input = scratch.split_report("google");
    let before = fs::read(&input).unwrap();
    let map = scratch.0.join("pairs.json");
    fs::write(
        &map,
        r#"[{"placeholder": "ACME plain", "value": "Globex plain", "expect": 1},
            {"placeholder": "Northwind", "value": "Southwind", "expect": 7}]"#,
    )
    .unwrap();
    let output = scratch.0.join("out.docx");
    let refused = oxoffice(&[
        "docx",
        "replace",
        input.to_str().unwrap(),
        "--map",
        map.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("pair 1: expected 7 replacement(s) of \"Northwind\", found 1"),
        "{stderr}"
    );
    assert!(!output.exists(), "the first pair is not published alone");
    assert_eq!(fs::read(&input).unwrap(), before);
}

#[test]
fn allow_unreached_is_refused_with_regex() {
    let scratch = Scratch::new("regex");
    let input = scratch.split_report("word");
    let output = scratch.0.join("out.docx");
    let usage = oxoffice(&[
        "docx",
        "replace",
        input.to_str().unwrap(),
        "-p",
        "AC.E",
        "-v",
        "Globex",
        "--regex",
        "--allow-unreached",
        "-o",
        output.to_str().unwrap(),
    ]);
    assert!(!usage.status.success());
    assert!(!output.exists());
}

#[test]
fn replace_on_a_document_the_check_cannot_read_points_at_allow_unreached() {
    let scratch = Scratch::new("unreadable");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files/word/report.docx");
    let mut package = OpcPackage::from_reader(Cursor::new(fs::read(source).unwrap())).unwrap();
    package.remove_part("/word/header1.xml").unwrap();
    let input = scratch.0.join("no-header.docx");
    package.save(&input).unwrap();
    let output = scratch.0.join("out.docx");
    let (input, output) = (input.to_str().unwrap(), output.to_str().unwrap());
    let refused = oxoffice(&[
        "docx", "replace", input, "-p", "Summary", "-v", "Overview", "-o", output,
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("cannot be checked, as a story does not read")
            && String::from_utf8_lossy(&refused.stderr).contains("--allow-unreached"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!Path::new(output).exists());
    let allowed = oxoffice(&[
        "docx",
        "replace",
        input,
        "-p",
        "Summary",
        "-v",
        "Overview",
        "--allow-unreached",
        "-o",
        output,
    ]);
    assert!(
        allowed.status.success(),
        "{}",
        String::from_utf8_lossy(&allowed.stderr)
    );
}
