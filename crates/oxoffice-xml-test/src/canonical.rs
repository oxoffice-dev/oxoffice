//! The canonical form of a part: what its XML says, whatever the writing.
//! One line per element start and end, its name expanded to
//! `{namespace}local`, its attributes sorted by expanded name without
//! namespace declarations and with references resolved (as are the prefixes
//! the markup compatibility lists name); one line per text, references
//! resolved. An empty-element tag reads as a start and an end tag; text of
//! whitespace only, outside `xml:space="preserve"`, in an element holding
//! elements is no line (element-only content in these vocabularies). Read
//! with quick-xml's own attribute reader, independently of the rewrites and
//! of the library.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// An open element: its expanded name, whether it holds an element, whether
/// it is preserved, its pending text and the bindings it declares.
struct Open(String, bool, bool, String, Vec<(String, String)>);

fn text_of<E: std::fmt::Display>(
    result: Result<std::borrow::Cow<'_, str>, E>,
) -> Result<String, String> {
    result
        .map(|text| text.into_owned())
        .map_err(|error| error.to_string())
}

/// The namespace `prefix` is bound to in `open`, `""` for no prefix unbound.
fn resolve<'a>(open: &'a [Open], prefix: &str) -> Option<&'a str> {
    if prefix == "xml" {
        return Some("http://www.w3.org/XML/1998/namespace");
    }
    open.iter()
        .rev()
        .flat_map(|element| element.4.iter().rev())
        .find(|(name, _)| name == prefix)
        .map(|(_, uri)| uri.as_str())
        .or(prefix.is_empty().then_some(""))
}

fn expand(open: &[Open], name: &str, element: bool) -> Result<String, String> {
    match name.split_once(':') {
        None if !element => Ok(name.to_owned()),
        split => {
            let (prefix, local) = split.unwrap_or(("", name));
            let uri = resolve(open, prefix)
                .ok_or_else(|| format!("the prefix of {name} is not bound"))?;
            Ok(format!("{{{uri}}}{local}"))
        }
    }
}

/// Writes the pending text of the innermost element as a line, unless it is
/// whitespace between elements.
fn flush(open: &mut [Open], lines: &mut Vec<String>, child_follows: bool) {
    let depth = open.len();
    let Some(element) = open.last_mut() else {
        return;
    };
    let text = std::mem::take(&mut element.3);
    let blank = text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
    if !text.is_empty() && !(blank && !element.2 && (child_follows || element.1)) {
        lines.push(format!("{}{text:?}", "  ".repeat(depth)));
    }
}

fn start(
    open: &mut Vec<Open>,
    lines: &mut Vec<String>,
    element: &BytesStart<'_>,
) -> Result<(), String> {
    flush(open, lines, true);
    if let Some(parent) = open.last_mut() {
        parent.1 = true;
    }
    let (mut declared, mut attributes) = (Vec::new(), Vec::new());
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        let raw = String::from_utf8_lossy(&attribute.value).replace(['\t', '\n', '\r'], " ");
        let value = text_of(quick_xml::escape::unescape(&raw))?;
        match key.strip_prefix("xmlns") {
            Some(prefix) if prefix.is_empty() || prefix.starts_with(':') => {
                declared.push((prefix.trim_start_matches(':').to_owned(), value));
            }
            _ => attributes.push((key, value)),
        }
    }
    let preserve = match attributes.iter().find(|(key, _)| key == "xml:space") {
        Some((_, value)) => value == "preserve",
        None => open.last().is_some_and(|parent| parent.2),
    };
    open.push(Open(
        String::new(),
        false,
        preserve,
        String::new(),
        declared,
    ));
    let name = expand(
        open,
        &String::from_utf8_lossy(element.name().as_ref()),
        true,
    )?;
    let mut expanded = Vec::new();
    let mc = "{http://schemas.openxmlformats.org/markup-compatibility/2006}";
    for (key, mut value) in attributes {
        let key = expand(open, &key, false)?;
        // A markup compatibility list names namespaces by their prefixes.
        let lists = [
            "Ignorable",
            "MustUnderstand",
            "ProcessContent",
            "PreserveElements",
        ];
        let listed = key
            .strip_prefix(mc)
            .is_some_and(|local| lists.contains(&local))
            || (key == "Requires" && name == format!("{mc}Choice"));
        if listed {
            let tokens = value
                .split_ascii_whitespace()
                .map(|token| match token.split_once(':') {
                    Some(_) => expand(open, token, true),
                    None => resolve(open, token)
                        .map(|uri| format!("{{{uri}}}"))
                        .ok_or(token.to_owned()),
                });
            value = tokens.collect::<Result<Vec<_>, _>>()?.join(" ");
        }
        expanded.push((key, value));
    }
    expanded.sort();
    let attributes: String = expanded
        .iter()
        .map(|(key, value)| format!(" {key}={value:?}"))
        .collect();
    lines.push(format!(
        "{}<{name}{attributes}>",
        "  ".repeat(open.len() - 1)
    ));
    open.last_mut().expect("just opened").0 = name;
    Ok(())
}

fn close(open: &mut Vec<Open>, lines: &mut Vec<String>) {
    flush(open, lines, false);
    let element = open.pop().expect("an open element");
    lines.push(format!("{}</{}>", "  ".repeat(open.len()), element.0));
}

/// The canonical form of `xml`, or why it is no well-formed, namespace
/// well-formed part.
pub fn canonical(xml: &[u8]) -> Result<String, String> {
    let xml = xml.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(xml);
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let (mut open, mut lines) = (Vec::new(), Vec::new());
    loop {
        let event = reader.read_event().map_err(|error| error.to_string())?;
        let text = match event {
            Event::Eof => break,
            Event::Start(element) => {
                start(&mut open, &mut lines, &element)?;
                continue;
            }
            Event::Empty(element) => {
                start(&mut open, &mut lines, &element)?;
                close(&mut open, &mut lines);
                continue;
            }
            Event::End(_) => {
                close(&mut open, &mut lines);
                continue;
            }
            Event::Text(text) => text_of(text.decode())?,
            Event::CData(data) => text_of(data.decode())?,
            Event::GeneralRef(reference) => {
                match reference.resolve_char_ref().map_err(|e| e.to_string())? {
                    Some(c) => c.to_string(),
                    None => match text_of(reference.decode())?.as_str() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        other => return Err(format!("the entity &{other}; is not defined")),
                    },
                }
            }
            _ => continue,
        };
        match open.last_mut() {
            Some(element) => element.3.push_str(&text),
            None if text.trim().is_empty() => {}
            None => return Err(format!("text outside the root: {text:?}")),
        }
    }
    if !open.is_empty() {
        return Err("an element is not closed".to_owned());
    }
    Ok(lines.join("\n"))
}

/// Where two canonical forms first differ: the line, and both from a little
/// before the first character that differs.
pub fn first_difference(left: &str, right: &str) -> Option<String> {
    let (left, right): (Vec<&str>, Vec<&str>) = (left.lines().collect(), right.lines().collect());
    let at = (0..left.len().max(right.len())).find(|&at| left.get(at) != right.get(at))?;
    let line = |lines: &[&str]| {
        lines
            .get(at)
            .map_or("(end)", |line| line.trim_start())
            .to_owned()
    };
    let (l, r) = (line(&left), line(&right));
    let common = l.chars().zip(r.chars()).take_while(|(a, b)| a == b).count();
    let excerpt = |line: &str| {
        line.chars()
            .skip(common.saturating_sub(60))
            .take(160)
            .collect::<String>()
    };
    Some(format!(
        "line {}: - {} + {}",
        at + 1,
        excerpt(&l),
        excerpt(&r)
    ))
}
