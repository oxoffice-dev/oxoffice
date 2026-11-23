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
        // A render takes a system font for what the bundled fonts lack
        // unless told otherwise.
        let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
            .args([family, "convert"])
            .arg(&input)
            .arg("--bundled-fonts-only")
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
        assert!(han.contains("drop --bundled-fonts-only"), "{han}");
        if family == "pptx" {
            assert!(!han.contains("--system-fonts"), "{han}");
        }
    }
}

// What the default render draws depends on the machine's fonts: the Han text
// of the minutes and of the deck is drawn with a system font that the output
// names, or drawn as boxes with a line saying no system font has it.
#[test]
fn a_render_names_the_system_font_it_took_or_says_none_was_found() {
    for (family, input) in [
        ("docx", corpus("word/minutes.docx")),
        ("pptx", corpus("powerpoint/multilingual.pptx")),
    ] {
        let path = scratch(&format!("fallback-{family}.pdf"));
        let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
            .args([family, "convert"])
            .arg(&input)
            .args(["--to", "pdf", "--force", "-o"])
            .arg(&path)
            .output()
            .expect("oxoffice runs");
        assert!(output.status.success(), "{family}: {}", stderr(&output));
        let stderr = stderr(&output);
        let han: Vec<&str> = stderr
            .lines()
            .filter(|line| line.starts_with("Chinese, Japanese or Korean ideograph (Han) text"))
            .collect();
        assert!(!han.is_empty(), "{family}: {stderr}");
        for line in han {
            assert!(
                line.contains("is drawn with the system font '")
                    || (line.contains("drawn as empty boxes")
                        && line.contains("no system font on this machine has them")),
                "{family}: {line}"
            );
        }
    }
}

// The deck's image commands take the same switch; what it changes is checked
// on the Rust entry they call (`Presentation::render_with_options`).
#[test]
fn the_deck_image_commands_accept_the_bundled_fonts_only_switch() {
    for command in ["render", "thumbnail"] {
        let dir = scratch(&format!("deck-{command}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
            .args(["pptx", command])
            .arg(corpus("powerpoint/multilingual.pptx"))
            .arg("--bundled-fonts-only")
            .arg("-o")
            .arg(if command == "render" {
                dir.clone()
            } else {
                dir.join("thumb.png")
            })
            .output()
            .expect("oxoffice runs");
        assert!(output.status.success(), "{command}: {}", stderr(&output));
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
