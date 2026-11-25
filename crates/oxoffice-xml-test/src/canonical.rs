//! The canonical form of a part: what its XML says, whatever the writing.
//!
//! One line per element start and end, its name expanded to `{namespace}local`
//! and its attributes sorted by expanded name, without namespace declarations;
//! one line per text, with its references resolved and its CDATA sections
//! read. An empty-element tag reads as a start tag and an end tag. A text of
//! whitespace only, outside `xml:space="preserve"`, in an element that holds
//! elements, is no line: in the element-only content of WordprocessingML,
//! PresentationML and DrawingML it is not content. The prefixes listed by the
//! markup compatibility attributes (`mc:Ignorable`, `mc:ProcessContent`,
//! `Requires`...) are expanded too. It reads the part with quick-xml's own
//! attribute reader, not with the rewrites' one, so that a rewrite is checked
//! by other code than its own.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::tokens::XML_NAMESPACE;
use crate::writing::MC_NAMESPACE;

struct Open {
    end: String,
    has_child: bool,
    preserve: bool,
    text: String,
    declared: Vec<(String, String)>,
}

/// The canonical form of `xml`, or why it is no well-formed, namespace
/// well-formed part.
pub fn canonical(xml: &[u8]) -> Result<String, String> {
    let xml = xml.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(xml);
    let mut reader = Reader::from_reader(xml);
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = true;
    let mut lines: Vec<String> = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    // The text read before the root, kept by no element.
    let mut loose = String::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("at byte {}: {error}", reader.error_position()))?;
        match event {
            Event::Eof => break,
            Event::Start(start) => start_element(&mut open, &mut lines, &start, false)?,
            Event::Empty(start) => start_element(&mut open, &mut lines, &start, true)?,
            Event::End(_) => close(&mut open, &mut lines),
            Event::Text(text) => {
                let text = text.decode().map_err(|error| error.to_string())?;
                match open.last_mut() {
                    Some(element) => element.text.push_str(&text),
                    None => loose.push_str(&text),
                }
            }
            Event::GeneralRef(reference) => {
                let c = match reference.resolve_char_ref().map_err(|e| e.to_string())? {
                    Some(c) => c,
                    None => {
                        let name = reference.decode().map_err(|e| e.to_string())?;
                        match name.as_ref() {
                            "amp" => '&',
                            "lt" => '<',
                            "gt" => '>',
                            "quot" => '"',
                            "apos" => '\'',
                            other => return Err(format!("the entity &{other}; is not defined")),
                        }
                    }
                };
                match open.last_mut() {
                    Some(element) => element.text.push(c),
                    None => loose.push(c),
                }
            }
            Event::CData(data) => {
                let data = data.decode().map_err(|error| error.to_string())?;
                match open.last_mut() {
                    Some(element) => element.text.push_str(&data),
                    None => return Err("a CDATA section outside the root".to_owned()),
                }
            }
            Event::Comment(comment) => {
                let depth = open.len();
                if let Some(element) = open.last_mut() {
                    flush(element, depth, &mut lines, true);
                }
                let comment = comment.decode().map_err(|error| error.to_string())?;
                lines.push(format!("{}<!--{comment}-->", "  ".repeat(depth)));
            }
            Event::PI(pi) => {
                let depth = open.len();
                if let Some(element) = open.last_mut() {
                    flush(element, depth, &mut lines, true);
                }
                lines.push(format!(
                    "{}<?{}?>",
                    "  ".repeat(depth),
                    String::from_utf8_lossy(&pi)
                ));
            }
            Event::Decl(_) | Event::DocType(_) => {}
        }
    }
    if !open.is_empty() {
        return Err("an element is not closed".to_owned());
    }
    if !loose.trim().is_empty() {
        return Err(format!("text outside the root: {loose:?}"));
    }
    Ok(lines.join("\n"))
}

fn start_element(
    open: &mut Vec<Open>,
    lines: &mut Vec<String>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<(), String> {
    let depth = open.len();
    if let Some(parent) = open.last_mut() {
        flush(parent, depth, lines, true);
        parent.has_child = true;
    }
    let declared = declarations(start)?;
    let preserve = match xml_space(start)? {
        Some(preserve) => preserve,
        None => open.last().is_some_and(|parent| parent.preserve),
    };
    open.push(Open {
        end: String::new(),
        has_child: false,
        preserve,
        text: String::new(),
        declared,
    });
    let name = expanded_element(open, start)?;
    let attributes = attributes(open, start, &name)?;
    lines.push(format!("{}<{name}{attributes}>", "  ".repeat(depth)));
    open.last_mut().expect("just pushed").end = format!("</{name}>");
    if empty {
        close(open, lines);
    }
    Ok(())
}

/// Writes the text gathered in `element` as one line, unless it is
/// whitespace in element content. `child` says an element follows it.
fn flush(element: &mut Open, depth: usize, lines: &mut Vec<String>, child: bool) {
    let text = std::mem::take(&mut element.text);
    let whitespace = text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
    if text.is_empty() || (whitespace && !element.preserve && (child || element.has_child)) {
        return;
    }
    lines.push(format!("{}{text:?}", "  ".repeat(depth)));
}

fn close(open: &mut Vec<Open>, lines: &mut Vec<String>) {
    let depth = open.len();
    let mut element = open.pop().expect("an open element");
    flush(&mut element, depth, lines, false);
    lines.push(format!("{}{}", "  ".repeat(depth - 1), element.end));
}

fn utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|error| error.to_string())
}

/// An attribute value as the XML reader gives it: literal whitespace as a
/// space, then the references resolved.
fn attribute_value(raw: &[u8]) -> Result<String, String> {
    let normalised: String = utf8(raw)?
        .chars()
        .map(|c| {
            if matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect();
    quick_xml::escape::unescape(&normalised)
        .map(|value| value.into_owned())
        .map_err(|error| error.to_string())
}

fn declarations(start: &BytesStart<'_>) -> Result<Vec<(String, String)>, String> {
    let mut declared = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let key = utf8(attribute.key.as_ref())?;
        let prefix = if key == "xmlns" {
            Some("")
        } else {
            key.strip_prefix("xmlns:")
        };
        if let Some(prefix) = prefix {
            declared.push((prefix.to_owned(), attribute_value(&attribute.value)?));
        }
    }
    Ok(declared)
}

fn xml_space(start: &BytesStart<'_>) -> Result<Option<bool>, String> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        if attribute.key.as_ref() == b"xml:space" {
            return Ok(Some(attribute_value(&attribute.value)? == "preserve"));
        }
    }
    Ok(None)
}

/// The namespace bound to `prefix` in the scope of the innermost element.
fn namespace<'a>(open: &'a [Open], prefix: &str) -> Option<&'a str> {
    if prefix == "xml" {
        return Some(XML_NAMESPACE);
    }
    open.iter()
        .rev()
        .flat_map(|element| element.declared.iter().rev())
        .find(|(bound, _)| bound == prefix)
        .map(|(_, uri)| uri.as_str())
        .or(prefix.is_empty().then_some(""))
}

fn expand(open: &[Open], name: &str, element: bool) -> Result<String, String> {
    let (prefix, local) = name.split_once(':').unwrap_or(("", name));
    if prefix.is_empty() && !element {
        return Ok(local.to_owned());
    }
    let uri =
        namespace(open, prefix).ok_or_else(|| format!("the prefix of {name} is not bound"))?;
    Ok(format!("{{{uri}}}{local}"))
}

fn expanded_element(open: &[Open], start: &BytesStart<'_>) -> Result<String, String> {
    expand(open, utf8(start.name().as_ref())?, true)
}

/// The attributes but namespace declarations, ` name="value"` each, sorted
/// by expanded name.
fn attributes(open: &[Open], start: &BytesStart<'_>, element: &str) -> Result<String, String> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let key = utf8(attribute.key.as_ref())?;
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        let name = expand(open, key, false)?;
        let mut value = attribute_value(&attribute.value)?;
        let mc = format!("{{{MC_NAMESPACE}}}");
        let listed = match name.strip_prefix(&mc) {
            Some("Ignorable" | "MustUnderstand") => Some(false),
            Some("ProcessContent" | "PreserveElements" | "PreserveAttributes") => Some(true),
            _ => (name == "Requires" && element == format!("{mc}Choice")).then_some(false),
        };
        if let Some(qualified) = listed {
            let mut expanded = Vec::new();
            for token in value.split_ascii_whitespace() {
                expanded.push(if qualified {
                    expand(open, token, true)?
                } else {
                    let uri = namespace(open, token)
                        .ok_or_else(|| format!("{key} lists {token}, which is not bound"))?;
                    format!("{{{uri}}}")
                });
            }
            value = expanded.join(" ");
        }
        attributes.push((name, value));
    }
    attributes.sort();
    Ok(attributes
        .iter()
        .map(|(name, value)| format!(" {name}={value:?}"))
        .collect())
}

/// Where two canonical forms first differ: the element or text around it
/// and the two lines from a little before the first character that differs,
/// or `None` when they are the same.
pub fn first_difference(left: &str, right: &str) -> Option<String> {
    if left == right {
        return None;
    }
    let (left, right): (Vec<&str>, Vec<&str>) = (left.lines().collect(), right.lines().collect());
    let at = left
        .iter()
        .zip(&right)
        .position(|(l, r)| l != r)
        .unwrap_or(left.len().min(right.len()));
    // The nearest element start before, for where the difference is.
    let within = left[..at]
        .iter()
        .rev()
        .find(|line| line.trim_start().starts_with("<{"))
        .map(|line| {
            let line = line.trim_start();
            line.split(' ')
                .next()
                .unwrap_or(line)
                .trim_end_matches('>')
                .to_owned()
        })
        .unwrap_or_default();
    let (l, r) = (
        left.get(at).map(|l| l.trim_start()),
        right.get(at).map(|r| r.trim_start()),
    );
    let common = match (l, r) {
        (Some(l), Some(r)) => l
            .char_indices()
            .zip(r.chars())
            .find(|((_, a), b)| a != b)
            .map_or(l.len().min(r.len()), |((index, _), _)| index),
        _ => 0,
    };
    let excerpt = |line: Option<&str>| match line {
        None => "(end)".to_owned(),
        Some(line) => {
            let from = line[..common.min(line.len())]
                .char_indices()
                .rev()
                .nth(60)
                .map_or(0, |(index, _)| index);
            let shown: String = line[from..].chars().take(160).collect();
            if from > 0 {
                format!("...{shown}")
            } else {
                shown
            }
        }
    };
    Some(format!(
        "line {} in {within}: - {} + {}",
        at + 1,
        excerpt(l),
        excerpt(r)
    ))
}
