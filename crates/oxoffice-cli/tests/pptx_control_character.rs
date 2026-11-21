//! A deck whose part holds a character XML 1.0 cannot carry is refused, by
//! `oxoffice pptx text` and `oxoffice pptx validate` alike, naming the part,
//! the line and the column, as PowerPoint cannot read that part either.

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The corpus PowerPoint deck with U+0001 at the start of the first text of
/// its first slide, written to a file of its own, removed on drop.
struct Deck(PathBuf);

impl Deck {
    fn new() -> Self {
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/files/powerpoint/deck.pptx");
        let mut source = zip::ZipArchive::new(Cursor::new(std::fs::read(corpus).unwrap())).unwrap();
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..source.len() {
            let mut entry = source.by_index(index).unwrap();
            let name = entry.name().unwrap().into_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            if name == "ppt/slides/slide1.xml" {
                let xml = String::from_utf8(bytes).unwrap();
                bytes = xml.replacen("<a:t>", "<a:t>\u{1}", 1).into_bytes();
            }
            out.start_file(name, zip::write::SimpleFileOptions::DEFAULT)
                .unwrap();
            out.write_all(&bytes).unwrap();
        }
        let path = std::env::temp_dir().join(format!(
            "oxoffice-cli-control-character-{}.pptx",
            std::process::id()
        ));
        std::fs::write(&path, out.finish().unwrap().into_inner()).unwrap();
        Self(path)
    }
}

impl Drop for Deck {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn oxoffice(args: &[&str], deck: &Deck) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(args)
        .arg(&deck.0)
        .output()
        .expect("oxoffice runs")
}

#[test]
fn a_part_holding_a_control_character_is_refused_with_where_it_is() {
    let deck = Deck::new();

    let text = oxoffice(&["pptx", "text"], &deck);
    assert_eq!(text.status.code(), Some(1));
    let error = String::from_utf8_lossy(&text.stderr);
    assert!(
        error.contains("/ppt/slides/slide1.xml holds U+0001 at line 2, column ")
            && error.contains("remove the character from the part"),
        "{error}"
    );

    let validate = oxoffice(&["pptx", "validate"], &deck);
    assert_eq!(validate.status.code(), Some(1));
    let report = String::from_utf8_lossy(&validate.stderr);
    assert!(
        report.contains("/ppt/slides/slide1.xml /: holds U+0001 at line 2, column ")
            && report.contains("PowerPoint cannot read that part either"),
        "{report}"
    );
}
