//! An XML part as the byte ranges of its markup and text, start tags read
//! into their name and attributes as written, and the namespace bindings in
//! scope: what a rewrite needs to change one thing and copy the rest.

use std::ops::Range;

use quick_xml::Reader;
use quick_xml::events::Event;

/// The namespace the `xml:` prefix is bound to, always.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Start,
    Empty,
    End,
    Text,
    /// An entity or character reference in text, `&amp;` or `&#233;`.
    Reference,
    CData,
    Other,
}

/// One piece of a part: its kind and the bytes it is written with.
pub struct Token {
    pub kind: Kind,
    pub raw: Range<usize>,
}

/// The part without a leading byte order mark, and that mark.
pub fn split_bom(xml: &[u8]) -> (&[u8], &[u8]) {
    match xml.strip_prefix(b"\xEF\xBB\xBF") {
        Some(rest) => (&xml[..3], rest),
        None => (&xml[..0], xml),
    }
}

/// Every piece of `xml` (without a byte order mark), in order: together
/// their ranges cover the part.
pub fn tokens(xml: &[u8]) -> Result<Vec<Token>, String> {
    let mut reader = Reader::from_reader(xml);
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = true;
    config.expand_empty_elements = false;
    let mut tokens = Vec::new();
    let mut at = 0;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("at byte {}: {error}", reader.error_position()))?;
        let end = usize::try_from(reader.buffer_position()).expect("a part fits in memory");
        let kind = match event {
            Event::Eof => break,
            Event::Start(_) => Kind::Start,
            Event::Empty(_) => Kind::Empty,
            Event::End(_) => Kind::End,
            Event::Text(_) => Kind::Text,
            Event::GeneralRef(_) => Kind::Reference,
            Event::CData(_) => Kind::CData,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => Kind::Other,
        };
        tokens.push(Token { kind, raw: at..end });
        at = end;
    }
    if at != xml.len() {
        return Err(format!("{} bytes after the last event", xml.len() - at));
    }
    Ok(tokens)
}

/// An attribute as written: its name, its value still escaped, and the
/// quote around the value.
#[derive(Clone, Debug)]
pub struct Attribute<'a> {
    pub name: &'a str,
    pub value: &'a str,
    pub quote: char,
}

/// A start or empty-element tag as written.
#[derive(Clone, Debug)]
pub struct Tag<'a> {
    pub name: &'a str,
    pub attributes: Vec<Attribute<'a>>,
    pub empty: bool,
}

/// Reads a start tag `<name a="1">` or an empty-element tag `<name/>`.
pub fn parse_tag(raw: &str) -> Result<Tag<'_>, String> {
    let inner = raw
        .strip_prefix('<')
        .ok_or_else(|| format!("{raw:?} is no tag"))?;
    let (inner, empty) = match inner.strip_suffix("/>") {
        Some(inner) => (inner, true),
        None => (
            inner
                .strip_suffix('>')
                .ok_or_else(|| format!("{raw:?} is no tag"))?,
            false,
        ),
    };
    let name_end = inner
        .find(|c: char| c.is_ascii_whitespace())
        .unwrap_or(inner.len());
    let name = &inner[..name_end];
    let mut rest = &inner[name_end..];
    let mut attributes = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
        if rest.is_empty() {
            break;
        }
        let key_end = rest
            .find(|c: char| c == '=' || c.is_ascii_whitespace())
            .ok_or_else(|| format!("{raw:?}: an attribute without a value"))?;
        let key = &rest[..key_end];
        rest = rest[key_end..].trim_start_matches(|c: char| c.is_ascii_whitespace());
        rest = rest
            .strip_prefix('=')
            .ok_or_else(|| format!("{raw:?}: {key} has no ="))?
            .trim_start_matches(|c: char| c.is_ascii_whitespace());
        let quote = rest
            .chars()
            .next()
            .filter(|c| *c == '"' || *c == '\'')
            .ok_or_else(|| format!("{raw:?}: {key} has no quoted value"))?;
        let value_end = rest[1..]
            .find(quote)
            .ok_or_else(|| format!("{raw:?}: {key} has no closing quote"))?;
        attributes.push(Attribute {
            name: key,
            value: &rest[1..1 + value_end],
            quote,
        });
        rest = &rest[2 + value_end..];
    }
    Ok(Tag {
        name,
        attributes,
        empty,
    })
}

/// The name of an end tag `</name>`.
pub fn end_name(raw: &str) -> &str {
    raw.trim_start_matches("</")
        .trim_end_matches('>')
        .trim_end_matches(|c: char| c.is_ascii_whitespace())
}

/// The prefix and the local name of a qualified name; no prefix is `""`.
pub fn split_name(name: &str) -> (&str, &str) {
    name.split_once(':').unwrap_or(("", name))
}

/// The namespace an attribute declares: `Some("")` for `xmlns`, `Some("w")`
/// for `xmlns:w`, `None` for any other attribute.
pub fn declared_prefix(attribute: &str) -> Option<&str> {
    if attribute == "xmlns" {
        Some("")
    } else {
        attribute.strip_prefix("xmlns:")
    }
}

/// The namespace bindings in scope, one frame per open element.
#[derive(Default)]
pub struct Scopes {
    frames: Vec<Vec<(String, String)>>,
}

impl Scopes {
    /// Opens the scope of an element with these attributes.
    pub fn push(&mut self, attributes: &[Attribute<'_>]) -> Result<(), String> {
        let mut frame = Vec::new();
        for attribute in attributes {
            if let Some(prefix) = declared_prefix(attribute.name) {
                let uri = quick_xml::escape::unescape(attribute.value)
                    .map_err(|error| format!("{}: {error}", attribute.name))?;
                frame.push((prefix.to_owned(), uri.into_owned()));
            }
        }
        self.frames.push(frame);
        Ok(())
    }

    pub fn pop(&mut self) {
        self.frames.pop();
    }

    /// The namespace a prefix stands for here: `""` for no namespace (the
    /// empty prefix without a default namespace), `None` for an unbound one.
    pub fn resolve(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE);
        }
        self.frames
            .iter()
            .rev()
            .flat_map(|frame| frame.iter().rev())
            .find(|(bound, _)| bound == prefix)
            .map(|(_, uri)| uri.as_str())
            .or(prefix.is_empty().then_some(""))
    }

    /// The namespace of a name: that of its prefix, or the default one for an
    /// element without a prefix.
    pub fn element_namespace(&self, name: &str) -> Result<String, String> {
        let (prefix, _) = split_name(name);
        self.resolve(prefix)
            .map(str::to_owned)
            .ok_or_else(|| format!("the prefix of {name} is not bound"))
    }
}
