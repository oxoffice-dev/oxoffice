//! `oxoffice docx convert --fillable-forms`: a Word form becomes a fillable
//! PDF, the flag applies to Word input and PDF output in the accepted view
//! only, and a document without content controls converts with no form.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oxoffice-cli-convert-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch folder");
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(name)
}

fn report() -> PathBuf {
    corpus("word/report.docx")
}

fn convert_input(input: &Path, to: &str, output: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "convert"])
        .arg(input)
        .args(["--to", to, "--fillable-forms", "--force"])
        .args(extra)
        .arg("-o")
        .arg(output)
        .output()
        .expect("oxoffice runs")
}

fn convert(to: &str, output: &Path, extra: &[&str]) -> Output {
    convert_input(&report(), to, output, extra)
}

/// How often a byte string occurs in a PDF.
fn occurrences(pdf: &[u8], needle: &[u8]) -> usize {
    pdf.windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

#[test]
fn a_word_form_converts_to_a_fillable_pdf() {
    let path = scratch("form.pdf");
    let output = convert_input(&corpus("authored/form.docx"), "pdf", &path, &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let pdf = fs::read(&path).unwrap();
    assert_eq!(occurrences(&pdf, b"/AcroForm"), 1);
    assert_eq!(
        occurrences(&pdf, b"/FT /"),
        20,
        "one field per content control (14) and per legacy form field (6)"
    );
    assert_eq!(occurrences(&pdf, b"/FT /Sig"), 1, "the signature control");
}

#[test]
fn markdown_input_refuses_fillable_forms() {
    let input = scratch("notes.md");
    fs::write(&input, "# Notes\n").unwrap();
    let path = scratch("notes.docx");
    let output = convert_input(&input, "docx", &path, &[]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("rendering options do not apply to HTML or Markdown input"),
        "{}",
        stderr(&output)
    );
    assert!(!path.exists(), "no output on refusal");
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn fillable_forms_apply_only_to_pdf() {
    for to in ["png", "docx"] {
        let path = scratch(&format!("report-out.{to}"));
        let output = convert(to, &path, &[]);
        assert!(!output.status.success(), "{to}");
        assert!(
            stderr(&output).contains("--fillable-forms applies only to --to pdf"),
            "{to}: {}",
            stderr(&output)
        );
        assert!(!path.exists(), "{to}: no output on refusal");
    }
}

#[test]
fn fillable_forms_refuse_the_tracked_view() {
    let path = scratch("tracked.pdf");
    let output = convert("pdf", &path, &["--revision-view", "tracked"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("fillable forms need the accepted revision view"),
        "{}",
        stderr(&output)
    );
    assert!(!path.exists(), "no output on refusal");
}

#[test]
fn a_document_without_controls_converts_with_no_form() {
    let path = scratch("report.pdf");
    let output = convert("pdf", &path, &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let pdf = fs::read(&path).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(!pdf.windows(9).any(|window| window == b"/AcroForm"));
}

#[test]
fn text_drawn_as_empty_boxes_is_named_with_the_way_out() {
    for (family, input) in [
        ("docx", corpus("word/minutes.docx")),
        ("pptx", corpus("powerpoint/multilingual.pptx")),
    ] {
        let path = scratch(&format!("boxes-{family}.pdf"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_oxoffice"));
        command.args([family, "convert"]).arg(&input);
        // A Word render takes a system font for what the bundled fonts lack
        // unless told otherwise; a deck uses its own and the bundled fonts.
        if family == "docx" {
            command.arg("--bundled-fonts-only");
        }
        let output = command
            .args(["--to", "pdf", "--force", "-o"])
            .arg(&path)
            .output()
            .expect("oxoffice runs");
        assert!(output.status.success(), "{}", stderr(&output));
        let stderr = stderr(&output);
        let han = stderr
            .lines()
            .find(|line| {
                line.contains("Chinese, Japanese or Korean ideograph (Han) text has no glyph")
            })
            .unwrap_or_else(|| panic!("{family}: {stderr}"));
        assert!(
            han.contains("drawn as empty boxes") && han.contains("--font-dir"),
            "{family}: {han}"
        );
        if family == "docx" {
            assert!(han.contains("drop --bundled-fonts-only"), "{han}");
        } else {
            assert!(!han.contains("--system-fonts"), "{han}");
        }
    }
}

// What the default Word render draws depends on the machine's fonts: the Han
// text of the minutes is drawn with a system font that the output names, or
// drawn as boxes with a line saying no system font has it.
#[test]
fn a_word_render_names_the_system_font_it_took_or_says_none_was_found() {
    let path = scratch("fallback-docx.pdf");
    let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "convert"])
        .arg(corpus("word/minutes.docx"))
        .args(["--to", "pdf", "--force", "-o"])
        .arg(&path)
        .output()
        .expect("oxoffice runs");
    assert!(output.status.success(), "{}", stderr(&output));
    let stderr = stderr(&output);
    let han: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("Chinese, Japanese or Korean ideograph (Han) text"))
        .collect();
    assert!(!han.is_empty(), "{stderr}");
    for line in han {
        assert!(
            line.contains("is drawn with the system font '")
                || (line.contains("drawn as empty boxes")
                    && line.contains("no system font on this machine has them")),
            "{line}"
        );
    }
}

#[test]
fn system_fonts_and_bundled_fonts_only_are_refused_together() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "convert"])
        .arg(corpus("word/minutes.docx"))
        .args([
            "--to",
            "pdf",
            "--system-fonts",
            "--bundled-fonts-only",
            "-o",
        ])
        .arg(scratch("both-switches.pdf"))
        .output()
        .expect("oxoffice runs");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot be used with"),
        "{}",
        stderr(&output)
    );
}
