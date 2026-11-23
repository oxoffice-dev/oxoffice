//! Our results against Word's, field by field. The probes come from the
//! `bibliography_probe` example; Word for Mac 16.115 updated every field
//! and saved them as `tests/docx/bibliography/word/<style>.docx`. Each
//! citation and each source's entry is compared by its text and the run
//! formatting a style uses. `KNOWN` lists the fields that differ today and
//! must shrink: a field it lists that matches Word fails the test too.

use std::collections::BTreeSet;
use std::path::PathBuf;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use super::*;

/// Styles whose Word result is not recorded yet: Word for Mac takes the
/// style from its own setting, not from the file, and no automatic route
/// sets it. Dropping a style's file in `word/` turns its check on.
const WORD_PENDING: [&str; 11] = [
    "chicago",
    "gb7714",
    "gost_name",
    "gost_title",
    "harvard_anglia",
    "ieee",
    "iso690_author_date",
    "iso690_numeric",
    "mla7",
    "sist02",
    "turabian",
];

/// `(style, field)` pairs whose result differs from Word's today.
const KNOWN: &[(&str, &str)] = &[
    ("apa6", r#"CITATION Book \l 1033 \f "see ""#),
    ("apa6", r#"CITATION Book \l 1033 \m Journal"#),
    ("apa6", r#"CITATION Book \l 1033 \p 12"#),
    ("apa6", r#"CITATION Book \l 1033 \p 12 \v 2"#),
    ("apa6", r#"CITATION Book \l 1033 \p 12-14"#),
    ("apa6", r#"CITATION Book \l 1033 \s ", emphasis added""#),
    ("apa6", r#"CITATION Book \l 1033 \v 2"#),
    ("apa6", r#"CITATION Book \l 1033 \y"#),
    ("apa6", r#"CITATION NoYear \l 1033"#),
    ("apa6", "entry Patent"),
    ("apa6", "entry Sound"),
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Format {
    italic: bool,
    bold: bool,
    underline: bool,
    small_caps: bool,
    vert_align: Option<String>,
}

type Segments = Vec<(String, Format)>;

fn push(segments: &mut Segments, text: &str, format: &Format) {
    if text.is_empty() {
        return;
    }
    match segments.last_mut() {
        Some((last, last_format)) if last_format == format => last.push_str(text),
        _ => segments.push((text.to_owned(), format.clone())),
    }
}

/// Text with its formatting marked: `_italic_`, `*bold*`, `~underline~`,
/// `^caps^`, `{superscript}`.
fn shown(segments: &Segments) -> String {
    segments
        .iter()
        .map(|(text, format)| {
            let mut text = text.clone();
            for (on, mark) in [
                (format.italic, "_"),
                (format.bold, "*"),
                (format.underline, "~"),
                (format.small_caps, "^"),
            ] {
                if on {
                    text = format!("{mark}{text}{mark}");
                }
            }
            match format.vert_align.as_deref() {
                Some(align) if align != "baseline" => format!("{{{align}:{text}}}"),
                _ => text,
            }
        })
        .collect()
}

#[derive(Debug, Default)]
struct WordField {
    instruction: String,
    paragraphs: Vec<Segments>,
}

fn toggle(element: &quick_xml::events::BytesStart<'_>) -> bool {
    !element.attributes().flatten().any(|attribute| {
        attribute.key.local_name().as_ref() == b"val"
            && matches!(attribute.value.as_ref(), b"0" | b"false" | b"none")
    })
}

fn value(element: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == b"val")
        .map(|attribute| String::from_utf8_lossy(&attribute.value).into_owned())
}

/// The complex fields of a Word main part, with the formatted paragraphs of
/// their results. The probes hold no nested field.
fn word_fields(xml: &[u8]) -> Vec<WordField> {
    #[derive(PartialEq)]
    enum Phase {
        Outside,
        Instruction,
        Result,
    }
    let mut reader = Reader::from_reader(xml);
    let mut phase = Phase::Outside;
    let mut fields = Vec::new();
    let mut field = WordField::default();
    let mut paragraph = Segments::new();
    let mut format = Format::default();
    let (mut in_properties, mut in_text, mut in_instruction) = (false, false, false);
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .expect("main part reads");
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let empty = matches!(event, Event::Empty(_));
                match element.local_name().as_ref() {
                    b"r" if !empty => format = Format::default(),
                    b"rPr" if !empty => in_properties = true,
                    b"i" if in_properties => format.italic = toggle(element),
                    b"b" if in_properties => format.bold = toggle(element),
                    b"u" if in_properties => format.underline = toggle(element),
                    b"smallCaps" if in_properties => format.small_caps = toggle(element),
                    b"vertAlign" if in_properties => format.vert_align = value(element),
                    b"t" if !empty => in_text = true,
                    b"instrText" if !empty => in_instruction = true,
                    b"tab" if phase == Phase::Result && !in_properties => {
                        push(&mut paragraph, "\t", &format)
                    }
                    b"fldChar" => {
                        let kind = element
                            .attributes()
                            .flatten()
                            .find(|attribute| attribute.key.local_name().as_ref() == b"fldCharType")
                            .map(|attribute| attribute.value.into_owned());
                        match kind.as_deref() {
                            Some(b"begin") => {
                                assert!(phase == Phase::Outside, "the probes hold no nested field");
                                phase = Phase::Instruction;
                            }
                            Some(b"separate") => phase = Phase::Result,
                            Some(b"end") => {
                                if !paragraph.is_empty() {
                                    field.paragraphs.push(std::mem::take(&mut paragraph));
                                }
                                fields.push(std::mem::take(&mut field));
                                phase = Phase::Outside;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Event::End(element) => match element.local_name().as_ref() {
                b"rPr" => in_properties = false,
                b"t" => in_text = false,
                b"instrText" => in_instruction = false,
                b"p" if phase == Phase::Result && !paragraph.is_empty() => {
                    field.paragraphs.push(std::mem::take(&mut paragraph));
                }
                _ => {}
            },
            Event::Text(text) => {
                let text = text
                    .xml_content(XmlVersion::Implicit1_0)
                    .expect("text decodes");
                if in_instruction && phase == Phase::Instruction {
                    field.instruction.push_str(&text);
                } else if in_text && phase == Phase::Result {
                    push(&mut paragraph, &text, &format);
                }
            }
            Event::GeneralRef(reference) => {
                let character = reference
                    .resolve_char_ref()
                    .ok()
                    .flatten()
                    .map(String::from)
                    .or_else(|| {
                        quick_xml::escape::resolve_predefined_entity(&String::from_utf8_lossy(
                            reference,
                        ))
                        .map(str::to_owned)
                    })
                    .expect("known entity");
                if in_instruction && phase == Phase::Instruction {
                    field.instruction.push_str(&character);
                } else if in_text && phase == Phase::Result {
                    push(&mut paragraph, &character, &format);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    fields
}

fn run_segments(runs: &[oxoffice_docx_oxml::text::CT_R]) -> Segments {
    use oxoffice_docx_oxml::text::RunContent;
    let mut segments = Segments::new();
    for run in runs {
        let format = run
            .properties
            .as_ref()
            .map(|properties| Format {
                italic: properties.italic == Some(true),
                bold: properties.bold == Some(true),
                underline: properties.underline.is_some_and(|underline| {
                    underline != oxoffice_docx_oxml::shared::ST_Underline::None
                }),
                small_caps: properties.small_caps == Some(true),
                vert_align: properties.vert_align.clone(),
            })
            .unwrap_or_default();
        for content in &run.content {
            match content {
                RunContent::Text(text) => push(&mut segments, &text.text, &format),
                RunContent::Tab => push(&mut segments, "\t", &format),
                _ => {}
            }
        }
    }
    segments
}

/// The source a Word entry shows: the only one whose title, patent number
/// or case number it holds.
fn entry_source<'a>(entry: &str, sources: &'a [BibliographySource]) -> &'a str {
    let entry = entry.to_lowercase();
    let shows = |source: &BibliographySource, field| {
        let value = first_source_property(source, field).to_lowercase();
        !value.is_empty() && entry.contains(&value)
    };
    for field in [
        BibliographySourceField::Title,
        BibliographySourceField::PatentNumber,
        BibliographySourceField::CaseNumber,
    ] {
        let matches = sources
            .iter()
            .filter(|source| shows(source, field))
            .collect::<Vec<_>>();
        if let [source] = matches.as_slice() {
            return &source.tag;
        }
    }
    panic!("no single source for the Word entry {entry:?}")
}

fn fixture(style: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/docx/bibliography/word")
        .join(format!("{style}.docx"))
}

/// Every field of one style compared with Word's; the keys that differ.
fn differences(style: &str) -> BTreeSet<String> {
    let bytes = std::fs::read(fixture(style)).expect("Word result reads");
    let document = Document::from_bytes(&bytes).expect("Word result opens");
    let xml = document
        .package
        .get_part(&document.doc_part_name)
        .expect("main part")
        .to_vec();
    let state = BibliographyUpdateState::new(&document, Some(1033)).expect("update state");
    let sources = document
        .bibliography_sources()
        .expect("sources read")
        .into_iter()
        .map(|source| source.supported.expect("probe sources are supported"))
        .collect::<Vec<_>>();
    let mut differ = BTreeSet::new();
    let mut report =
        |key: String, ours: std::result::Result<Segments, String>, word: &Segments| match ours {
            Ok(ours) if &ours == word => {}
            Ok(ours) => {
                eprintln!(
                    "{style} {key}\n  word: {}\n  ours: {}",
                    shown(word),
                    shown(&ours)
                );
                differ.insert(key);
            }
            Err(error) => {
                eprintln!("{style} {key}\n  word: {}\n  ours: {error}", shown(word));
                differ.insert(key);
            }
        };
    for field in word_fields(&xml) {
        let instruction =
            oxoffice_docx_oxml::text::Field::new(&field.instruction, "").effective_instruction();
        match instruction.name.as_str() {
            "CITATION" => {
                let word = field.paragraphs.concat();
                let ours = state
                    .citation_runs(&instruction)
                    .map_err(|error| error.to_string())
                    .and_then(|runs| runs.ok_or_else(|| "kept the cache".to_owned()))
                    .map(|runs| run_segments(&runs));
                report(field.instruction.trim().to_owned(), ours, &word);
            }
            "BIBLIOGRAPHY" => {
                let mut shown_tags = BTreeSet::new();
                for paragraph in &field.paragraphs {
                    let text = paragraph
                        .iter()
                        .map(|(text, _)| text.as_str())
                        .collect::<String>();
                    let tag = entry_source(&text, &sources);
                    assert!(
                        shown_tags.insert(tag),
                        "{style}: two Word entries for {tag}"
                    );
                    let ours = state
                        .source_entry_for_test(tag, Some("1033"))
                        .map_err(|error| error.to_string())
                        .map(|paragraph| run_segments(&paragraph.runs));
                    report(format!("entry {tag}"), ours, paragraph);
                }
                assert_eq!(
                    shown_tags.len(),
                    sources.len(),
                    "{style}: an entry per source"
                );
            }
            _ => {}
        }
    }
    differ
}

#[test]
fn bibliography_results_match_word_or_are_listed_as_known() {
    for style in ["apa6"].into_iter().chain(WORD_PENDING) {
        let known = KNOWN
            .iter()
            .filter(|(known_style, _)| *known_style == style)
            .map(|(_, key)| (*key).to_owned())
            .collect::<BTreeSet<_>>();
        if WORD_PENDING.contains(&style) {
            assert!(
                !fixture(style).exists(),
                "{style}: Word's result is recorded, take it out of WORD_PENDING"
            );
            assert!(
                known.is_empty(),
                "{style}: pending styles list no known field"
            );
            continue;
        }
        let differ = differences(style);
        let fixed = known.difference(&differ).collect::<Vec<_>>();
        let new = differ.difference(&known).collect::<Vec<_>>();
        assert!(
            fixed.is_empty() && new.is_empty(),
            "{style}: matching Word now, take out of KNOWN: {fixed:?}; differing from Word: {new:?}"
        );
    }
}
