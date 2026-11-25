//! The writings: each rewrite writes one part another way, the way a real
//! producer may, and copies every other byte, so that a failure points at one
//! writing. A rewritten package is refused unless every part it changed reads
//! the same in the canonical form.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::ops::Range;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::canonical::{canonical, first_difference};

/// The main namespaces of WordprocessingML, PresentationML and DrawingML:
/// the ones [`Writing::OtherPrefixes`] rebinds.
const MAIN_NAMESPACES: [&str; 8] = [
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://schemas.openxmlformats.org/presentationml/2006/main",
    "http://schemas.openxmlformats.org/drawingml/2006/main",
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://schemas.openxmlformats.org/officeDocument/2006/math",
    "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    "http://schemas.openxmlformats.org/drawingml/2006/picture",
    "http://schemas.openxmlformats.org/drawingml/2006/chart",
];

/// One way of writing the same XML, as a producer writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writing {
    /// `<x></x>` for every `<x/>` (LibreOffice's `w:fldChar`, #413).
    EmptyWithEndTag,
    /// `<x/>` for every `<x></x>` (Google Docs' `w:sdtContent`, #656).
    EmptySelfClosed,
    /// The main namespaces under other prefixes (`ox0:p`; ElementTree's `ns0`).
    OtherPrefixes,
    /// The root's namespace as the default one, attributes keeping their prefix.
    DefaultNamespace,
    /// Line breaks and indents between tags, never into text or preserved space.
    Indented,
    /// The attributes of every tag in reverse order.
    AttributeOrder,
    /// Every attribute value between single quotes.
    SingleQuotes,
    /// Line breaks and spaces inside tags, around `=` and before `>`.
    TagSpacing,
    /// Non-ASCII as `&#x..;`, `>"'` as entities (ASCII-only writers).
    CharacterReferences,
}

impl Writing {
    pub const ALL: [Writing; 9] = {
        use Writing::*;
        [
            EmptyWithEndTag,
            EmptySelfClosed,
            OtherPrefixes,
            DefaultNamespace,
            Indented,
            AttributeOrder,
            SingleQuotes,
            TagSpacing,
            CharacterReferences,
        ]
    };

    /// The name known differences use: `empty-with-end-tag`.
    pub fn name(self) -> String {
        let mut name = String::new();
        for c in format!("{self:?}").chars() {
            if c.is_uppercase() && !name.is_empty() {
                name.push('-');
            }
            name.push(c.to_ascii_lowercase());
        }
        name
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Start,
    Empty,
    End,
    /// Text, a reference or a CDATA section.
    Content,
    /// A declaration, comment or processing instruction.
    Other,
}

/// Each piece of `xml` and the bytes it is written with.
fn tokens(xml: &[u8]) -> Result<Vec<(Kind, Range<usize>)>, String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut tokens = Vec::new();
    let mut at = 0;
    loop {
        let event = reader.read_event().map_err(message)?;
        let end = usize::try_from(reader.buffer_position()).expect("a part fits in memory");
        let kind = match event {
            Event::Eof => return Ok(tokens),
            Event::Start(_) => Kind::Start,
            Event::Empty(_) => Kind::Empty,
            Event::End(_) => Kind::End,
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_) => Kind::Content,
            _ => Kind::Other,
        };
        tokens.push((kind, at..end));
        at = end;
    }
}

/// A start or empty-element tag as written: its name, and each attribute's
/// name, value (still escaped) and quote.
struct Tag<'a> {
    name: &'a str,
    attributes: Vec<(String, String, char)>,
    empty: bool,
}

fn parse_tag(raw: &str) -> Result<Tag<'_>, String> {
    let empty = raw.ends_with("/>");
    let inner = &raw[1..raw.len() - if empty { 2 } else { 1 }];
    let (name, mut rest) = inner
        .split_once(|c: char| c.is_ascii_whitespace())
        .unwrap_or((inner, ""));
    let mut attributes = Vec::new();
    while let Some((key, after)) = rest.split_once('=') {
        let after = after.trim_start();
        let quote = after.chars().next().ok_or("an attribute without a value")?;
        let end = after[1..]
            .find(quote)
            .ok_or("an unclosed attribute value")?;
        let value = after[1..1 + end].to_owned();
        attributes.push((key.trim().to_owned(), value, quote));
        rest = &after[2 + end..];
    }
    Ok(Tag {
        name,
        attributes,
        empty,
    })
}

/// A tag written with this name and these attributes, each separated by
/// `space` and its `=` written `equals`, `quote` around every value when
/// given (a value holding it takes a reference), and `close` before `>`.
fn write_tag(
    name: &str,
    attributes: &[(String, String, char)],
    empty: bool,
    (space, equals, close): (&str, &str, &str),
    quote: Option<char>,
) -> String {
    let mut out = format!("<{name}");
    for (key, value, own) in attributes {
        let quote = quote.unwrap_or(*own);
        let reference = if quote == '"' { "&quot;" } else { "&apos;" };
        let value = value.replace(quote, reference);
        out.push_str(&format!("{space}{key}{equals}{quote}{value}{quote}"));
    }
    out + close + if empty { "/>" } else { ">" }
}

const PLAIN: (&str, &str, &str) = (" ", "=", "");

/// `raw` (text or an attribute value, escaped) with its characters outside
/// ASCII as numeric references and `>`, `"`, `'` as entities.
fn with_references(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&apos;".to_owned(),
            c if !c.is_ascii() => format!("&#x{:X};", u32::from(c)),
            c => c.to_string(),
        })
        .collect()
}

/// The prefixes `xml` declares, each with its namespace. No rewrite here
/// tracks scopes: a prefix bound twice makes the canonical check refuse it.
fn declarations(
    xml: &str,
    tokens: &[(Kind, Range<usize>)],
) -> Result<BTreeMap<String, String>, String> {
    let mut declared = BTreeMap::new();
    for (_, raw) in tokens
        .iter()
        .filter(|(kind, _)| matches!(kind, Kind::Start | Kind::Empty))
    {
        for (key, value, _) in parse_tag(&xml[raw.clone()])?.attributes {
            if let Some(prefix) = key.strip_prefix("xmlns") {
                declared.insert(prefix.trim_start_matches(':').to_owned(), value);
            }
        }
    }
    Ok(declared)
}

/// `name` with its prefix renamed by `prefixes` (a missing prefix kept, an
/// empty new one dropped).
fn renamed(name: &str, prefixes: &BTreeMap<String, String>) -> String {
    match name.split_once(':') {
        Some((prefix, local)) => match prefixes.get(prefix) {
            Some(new) if new.is_empty() => local.to_owned(),
            Some(new) => format!("{new}:{local}"),
            None => name.to_owned(),
        },
        None => name.to_owned(),
    }
}

/// The prefix renames a writing makes: to `oxN` for the main namespaces
/// ([`Writing::OtherPrefixes`]), or to none for the root's namespace
/// ([`Writing::DefaultNamespace`], with that namespace to declare on the
/// root), only in a part where no element goes without a prefix and no
/// default namespace is declared, so that no scope needs tracking.
fn prefix_renames(
    writing: Writing,
    text: &str,
    tokens: &[(Kind, Range<usize>)],
) -> Result<(BTreeMap<String, String>, Option<String>), String> {
    let mut renames = BTreeMap::new();
    if !matches!(writing, Writing::OtherPrefixes | Writing::DefaultNamespace) {
        return Ok((renames, None));
    }
    let declared = declarations(text, tokens)?;
    let tags = || {
        tokens
            .iter()
            .filter(|(kind, _)| matches!(kind, Kind::Start | Kind::Empty))
    };
    if writing == Writing::OtherPrefixes {
        let main = declared
            .iter()
            .filter(|(prefix, uri)| !prefix.is_empty() && MAIN_NAMESPACES.contains(&uri.as_str()));
        for (index, (prefix, _)) in main.enumerate() {
            if declared.contains_key(&format!("ox{index}")) {
                return Err(format!("the part already binds ox{index}"));
            }
            renames.insert(prefix.clone(), format!("ox{index}"));
        }
        return Ok((renames, None));
    }
    let unprefixed = tags().any(|(_, raw)| {
        !text[raw.clone()]
            .split(['>', '/', ' ', '\n', '\t', '\r'])
            .next()
            .unwrap_or("")
            .contains(':')
    });
    let root = tags()
        .next()
        .map(|(_, raw)| parse_tag(&text[raw.clone()]))
        .transpose()?;
    let uri = root.and_then(|root| declared.get(root.name.split_once(':')?.0).cloned());
    if unprefixed || declared.contains_key("") || uri.is_none() {
        return Ok((renames, None));
    }
    for (prefix, bound) in &declared {
        if Some(bound) == uri.as_ref() {
            renames.insert(prefix.clone(), String::new());
        }
    }
    Ok((renames, uri))
}

/// A start or empty-element tag as `writing` writes it, or `None` when it
/// writes it as it was.
fn written_tag(
    writing: Writing,
    raw: &str,
    renames: &BTreeMap<String, String>,
    root: Option<&str>,
) -> Result<Option<String>, String> {
    let mut tag = parse_tag(raw)?;
    let before = write_tag(tag.name, &tag.attributes, tag.empty, PLAIN, None);
    let (mut style, mut quote) = (PLAIN, None);
    match writing {
        Writing::AttributeOrder => tag.attributes.reverse(),
        Writing::SingleQuotes => quote = Some('\''),
        Writing::TagSpacing => style = ("\n\t", " = ", " "),
        Writing::CharacterReferences => {
            for (key, value, _) in &mut tag.attributes {
                if !key.starts_with("xmlns") {
                    *value = with_references(value);
                }
            }
        }
        Writing::OtherPrefixes | Writing::DefaultNamespace => {
            for (key, value, _) in &mut tag.attributes {
                match key.strip_prefix("xmlns:") {
                    Some(prefix) if writing == Writing::OtherPrefixes => {
                        if let Some(new) = renames.get(prefix) {
                            *key = format!("xmlns:{new}");
                        }
                    }
                    Some(_) => {}
                    // Under a default namespace attributes keep their prefix.
                    None if writing == Writing::DefaultNamespace => {}
                    None => {
                        // A markup compatibility list naming a renamed prefix
                        // (never a main one in practice) is not rewritten.
                        let lists = ["Ignorable", "MustUnderstand", "ProcessContent", "Requires"];
                        let mut tokens = value.split([' ', ':']);
                        if lists.contains(&key.rsplit(':').next().unwrap_or(key))
                            && tokens.any(|token| renames.contains_key(token))
                        {
                            return Err(format!("{key} names a renamed prefix"));
                        }
                        *key = renamed(key, renames);
                    }
                }
            }
            if let Some(uri) = root {
                tag.attributes
                    .insert(0, ("xmlns".to_owned(), uri.to_owned(), '"'));
            }
            let name = renamed(tag.name, renames);
            let written = write_tag(&name, &tag.attributes, tag.empty, style, quote);
            return Ok((written != before).then_some(written));
        }
        _ => return Ok(None),
    }
    let written = write_tag(tag.name, &tag.attributes, tag.empty, style, quote);
    Ok((written != before || writing == Writing::TagSpacing).then_some(written))
}

/// `xml` written the way `writing` says.
pub fn rewrite(xml: &[u8], writing: Writing) -> Result<Vec<u8>, String> {
    let bom = if xml.starts_with(b"\xEF\xBB\xBF") {
        3
    } else {
        0
    };
    let text = std::str::from_utf8(&xml[bom..]).map_err(message)?;
    let tokens = tokens(text.as_bytes())?;
    let (renames, root_default) = prefix_renames(writing, text, &tokens)?;
    let mixed = mixed_elements(text, &tokens);
    let mut out = String::from_utf8(xml[..bom].to_vec()).expect("a byte order mark");
    // Each open element: its start token and whether it is preserved.
    let mut open: Vec<(usize, bool)> = Vec::new();
    let mut previous = Kind::Other;
    let mut index = 0;
    while index < tokens.len() {
        let (kind, range) = (tokens[index].0, tokens[index].1.clone());
        let raw = &text[range];
        let at_root = open.is_empty() && previous != Kind::End;
        index += 1;
        let written = match (writing, kind) {
            (Writing::EmptyWithEndTag, Kind::Empty) => Some(format!(
                "{}></{}>",
                raw[..raw.len() - 2].trim_end(),
                parse_tag(raw)?.name
            )),
            (Writing::EmptySelfClosed, Kind::Start)
                if tokens
                    .get(index)
                    .is_some_and(|(next, _)| *next == Kind::End) =>
            {
                index += 1;
                previous = Kind::End;
                out.push_str(&format!("{}/>", &raw[..raw.len() - 1]));
                continue;
            }
            (Writing::EmptyWithEndTag | Writing::EmptySelfClosed | Writing::Indented, _) => None,
            (_, Kind::Start | Kind::Empty) => written_tag(
                writing,
                raw,
                &renames,
                root_default.as_deref().filter(|_| at_root),
            )?,
            (Writing::TagSpacing, Kind::End) => Some(format!("{}\n>", &raw[..raw.len() - 1])),
            (_, Kind::End) => {
                let name = renamed(raw[2..raw.len() - 1].trim(), &renames);
                (name != raw[2..raw.len() - 1].trim()).then(|| format!("</{name}>"))
            }
            (Writing::CharacterReferences, Kind::Content)
                if !raw.starts_with('&') && !raw.starts_with("<!") =>
            {
                Some(with_references(raw))
            }
            _ => None,
        };
        // Indentation goes between two tags, never in an element without
        // child elements (between its own tags), with text of its own, or
        // preserved, nor after the root.
        if writing == Writing::Indented
            && matches!(kind, Kind::Start | Kind::Empty | Kind::End)
            && matches!(
                previous,
                Kind::Start | Kind::Empty | Kind::End | Kind::Other
            )
        {
            let allowed = match open.last() {
                None => kind != Kind::End && previous == Kind::Other,
                Some((start, preserve)) => {
                    !preserve && !mixed[*start] && !(kind == Kind::End && previous == Kind::Start)
                }
            };
            if allowed {
                out.push('\n');
                out.push_str(&"  ".repeat(open.len() - usize::from(kind == Kind::End)));
            }
        }
        out.push_str(written.as_deref().unwrap_or(raw));
        match kind {
            Kind::Start => {
                let inherited = open.last().is_some_and(|(_, preserve)| *preserve);
                let preserve = parse_tag(raw)?
                    .attributes
                    .iter()
                    .find(|(key, ..)| key == "xml:space")
                    .map_or(inherited, |(_, value, _)| value == "preserve");
                open.push((index - 1, preserve));
            }
            Kind::End => {
                open.pop();
            }
            _ => {}
        }
        previous = kind;
    }
    Ok(out.into_bytes())
}

/// For each token, whether it starts an element holding text of its own (so
/// no whitespace may be added in it).
fn mixed_elements(text: &str, tokens: &[(Kind, Range<usize>)]) -> Vec<bool> {
    let mut mixed = vec![false; tokens.len()];
    let mut open = Vec::new();
    for (index, (kind, raw)) in tokens.iter().enumerate() {
        match kind {
            Kind::Start => open.push(index),
            Kind::End => {
                open.pop();
            }
            Kind::Content if !text[raw.clone()].trim().is_empty() => {
                if let Some(parent) = open.last() {
                    mixed[*parent] = true;
                }
            }
            _ => {}
        }
    }
    mixed
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Every part of a package by its ZIP entry name, in the order of entries.
pub fn package_parts(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(message)?;
    (0..zip.len())
        .map(|index| {
            let (mut entry, mut data) = (zip.by_index(index).map_err(message)?, Vec::new());
            entry.read_to_end(&mut data).map_err(message)?;
            Ok((entry.name().map_err(message)?.into_owned(), data))
        })
        .collect()
}

/// A package holding these parts, in this order.
pub fn write_package(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        out.start_file(name.as_str(), zip::write::SimpleFileOptions::DEFAULT)
            .expect("a ZIP entry starts");
        out.write_all(data).expect("a ZIP entry is written");
    }
    out.finish().expect("the ZIP is written").into_inner()
}

/// `bytes` with every XML part of the WordprocessingML, PresentationML and
/// DrawingML vocabularies (not content types, relationships, `docProps/` or
/// `customXml/`, which other readers parse) written as `writing` says, and
/// the names of the parts that changed. Each must read the same in the
/// canonical form, or the rewrite is refused: it would prove nothing.
pub fn rewrite_package(bytes: &[u8], writing: Writing) -> Result<(Vec<u8>, Vec<String>), String> {
    let mut parts = package_parts(bytes)?;
    let mut changed = Vec::new();
    for (name, data) in &mut parts {
        if !name.ends_with(".xml")
            || name == "[Content_Types].xml"
            || name.contains("_rels/")
            || name.starts_with("docProps/")
            || name.starts_with("customXml/")
        {
            continue;
        }
        let written = rewrite(data, writing).map_err(|error| format!("{name}: {error}"))?;
        if written != *data {
            let (before, after) = (canonical(data)?, canonical(&written)?);
            if let Some(difference) = first_difference(&before, &after) {
                return Err(format!(
                    "{name}: {} changes what it says, {difference}",
                    writing.name()
                ));
            }
            *data = written;
            changed.push(name.clone());
        }
    }
    Ok((write_package(&parts), changed))
}
