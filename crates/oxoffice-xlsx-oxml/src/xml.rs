//! Reading and writing the elements of a part without loss.
//!
//! A start tag is kept as read ([`Tag`]): its name, its attributes in their
//! order, quotes and references, so an element written back unchanged says
//! what it said, LibreOffice's `true` staying `true` and Google's `1.0`
//! staying `1.0`. Attributes are read through typed accessors, tolerant of
//! the spellings producers use (`1` or `true`, `1` or `1.0`); an attribute an
//! edit sets is written in the fixed form (`1`/`0`, the shortest number).
//!
//! [`Element`] keeps an element whose content is elements, or text: the
//! records of the styles part, the elements of the workbook part and of the
//! page setup. Comments, processing instructions and text between elements
//! that is not white space are kept as written ([`Node::Raw`]).

use std::ops::Range;

use oxoffice_core::OxmlError;
use oxoffice_core::xml::{attribute_text, escape_attribute_value, escape_text, push_raw_attribute};
use oxoffice_core::xml_element::ElementSpan;
use oxoffice_core::xml_reader::Reader;
use oxoffice_core::xml_text::{decode_plain, normalize_line_ends, resolve_entity};
use quick_xml::events::{BytesStart, Event};

pub(crate) type Result<T> = std::result::Result<T, OxmlError>;

/// How deep [`Element`] models nested elements; deeper content is kept as
/// written.
const MAX_DEPTH: usize = 32;

/// A start tag as read: its qualified name and its attributes, each as
/// written. Setting an attribute writes it in the fixed form and keeps the
/// others as they were.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag(BytesStart<'static>);

impl Tag {
    /// A tag named `name` (qualified), without attributes.
    pub fn new(name: &str) -> Self {
        Tag(BytesStart::new(name.to_owned()))
    }

    pub(crate) fn read(start: &BytesStart<'_>) -> Self {
        Tag(start.clone().into_owned())
    }

    /// The qualified name, as written.
    pub fn name(&self) -> String {
        String::from_utf8_lossy(self.0.name().as_ref()).into_owned()
    }

    /// The local part of the name.
    pub fn local_name(&self) -> String {
        String::from_utf8_lossy(self.0.local_name().as_ref()).into_owned()
    }

    /// Whether the local part of the name is `local`.
    pub fn is(&self, local: &str) -> bool {
        self.0.local_name().as_ref() == local.as_bytes()
    }

    /// The value of the attribute written `name` (an unprefixed SpreadsheetML
    /// attribute, or a qualified one such as `xml:space`), its references
    /// resolved.
    pub fn get(&self, name: &str) -> Option<String> {
        self.0
            .attributes()
            .flatten()
            .find(|attribute| attribute.key.as_ref() == name.as_bytes())
            .and_then(|attribute| {
                attribute_text(&attribute)
                    .ok()
                    .map(|text| text.into_owned())
            })
    }

    /// The value of the prefixed attribute whose local part is `local`, under
    /// any prefix but `xmlns`: `r:id` whatever prefix binds the relationships
    /// namespace, Transitional or Strict.
    pub fn get_prefixed(&self, local: &str) -> Option<String> {
        self.0
            .attributes()
            .flatten()
            .find(|attribute| {
                let key = attribute.key.as_ref();
                match key.iter().position(|&byte| byte == b':') {
                    Some(at) => &key[..at] != b"xmlns" && &key[at + 1..] == local.as_bytes(),
                    None => false,
                }
            })
            .and_then(|attribute| {
                attribute_text(&attribute)
                    .ok()
                    .map(|text| text.into_owned())
            })
    }

    /// The attribute `name` read as a boolean: `1` or `true`, `0` or `false`.
    pub fn bool(&self, name: &str) -> Option<bool> {
        self.get(name).as_deref().and_then(parse_bool)
    }

    /// The attribute `name` read as an unsigned integer (`3`, or `3.0`).
    pub fn u32(&self, name: &str) -> Option<u32> {
        self.get(name).as_deref().and_then(parse_u32)
    }

    /// The attribute `name` read as a finite number.
    pub fn f64(&self, name: &str) -> Option<f64> {
        self.get(name).as_deref().and_then(parse_f64)
    }

    /// The namespace declarations of the tag: `(prefix, namespace)`, the
    /// prefix empty for a default namespace.
    pub fn namespace_declarations(&self) -> Vec<(String, String)> {
        self.0
            .attributes()
            .flatten()
            .filter_map(|attribute| {
                let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
                let prefix = match key.strip_prefix("xmlns") {
                    Some("") => String::new(),
                    Some(rest) => rest.strip_prefix(':')?.to_owned(),
                    None => return None,
                };
                let value = attribute_text(&attribute).ok()?.into_owned();
                Some((prefix, value))
            })
            .collect()
    }

    /// Sets the attribute written `name` to `value`, in place when the tag
    /// has it and at the end otherwise, or removes it (`None`). The other
    /// attributes keep their writing.
    pub fn set(&mut self, name: &str, value: Option<&str>) {
        let mut tag = BytesStart::new(self.name());
        let mut placed = false;
        for attribute in self.0.attributes().flatten() {
            if attribute.key.as_ref() != name.as_bytes() {
                push_raw_attribute(&mut tag, attribute);
            } else if let Some(value) = value.filter(|_| !placed) {
                tag.push_attribute((name, escape_attribute_value(value).as_ref()));
                placed = true;
            }
        }
        if let Some(value) = value.filter(|_| !placed) {
            tag.push_attribute((name, escape_attribute_value(value).as_ref()));
        }
        self.0 = tag;
    }

    /// Sets a boolean attribute in the fixed form, `1` or `0`.
    pub fn set_bool(&mut self, name: &str, value: Option<bool>) {
        self.set(name, value.map(|value| if value { "1" } else { "0" }));
    }

    /// Sets a numeric attribute in the fixed form, the shortest text that
    /// reads back the same number.
    pub fn set_f64(&mut self, name: &str, value: Option<f64>) {
        self.set(name, value.map(format_f64).as_deref());
    }

    /// Writes the start tag.
    pub fn write_start(&self, out: &mut Vec<u8>) {
        out.push(b'<');
        out.extend_from_slice(&self.0);
        out.push(b'>');
    }

    /// Writes the tag as an empty-element tag.
    pub fn write_empty(&self, out: &mut Vec<u8>) {
        out.push(b'<');
        out.extend_from_slice(&self.0);
        out.extend_from_slice(b"/>");
    }

    /// Writes the end tag of the element this tag starts.
    pub fn write_end(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"</");
        out.extend_from_slice(self.0.name().as_ref());
        out.push(b'>');
    }
}

/// The type of an attribute whose value has one fixed writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lexical {
    /// `1` or `0`.
    Bool,
    /// Digits.
    UInt,
    /// The shortest text that reads back the same number.
    Double,
}

impl Tag {
    /// Writes each of `attributes` the tag has in its fixed form (`true` as
    /// `1`, `15.0` as `15`), in place; a value that does not read as its
    /// type is kept as written.
    pub fn normalize(&mut self, attributes: &[(&str, Lexical)]) {
        for &(name, lexical) in attributes {
            let Some(value) = self.get(name) else {
                continue;
            };
            let fixed = match lexical {
                Lexical::Bool => parse_bool(&value).map(|value| u8::from(value).to_string()),
                Lexical::UInt => parse_u32(&value).map(|value| value.to_string()),
                Lexical::Double => parse_f64(&value).map(format_f64),
            };
            if let Some(fixed) = fixed.filter(|fixed| *fixed != value) {
                self.set(name, Some(&fixed));
            }
        }
    }
}

/// An XML Schema boolean as producers write it: `1` or `true`, `0` or
/// `false`.
pub fn parse_bool(text: &str) -> Option<bool> {
    match text.trim() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// An unsigned integer, also written as a number with a zero fraction
/// (`3.0`).
pub fn parse_u32(text: &str) -> Option<u32> {
    let text = text.trim();
    text.parse().ok().or_else(|| {
        parse_f64(text)
            .filter(|value| value.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(value))
            .map(|value| value as u32)
    })
}

/// A finite number, as an XML Schema double reads it.
pub fn parse_f64(text: &str) -> Option<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// The shortest text that reads back as `value`: `1` for one, `0.1`,
/// `1E+21` rather than twenty-two digits.
pub fn format_f64(value: f64) -> String {
    let plain = format!("{value}");
    let scientific = format!("{value:E}");
    let scientific = match scientific.split_once('E') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => {
            format!("{mantissa}E+{exponent}")
        }
        _ => scientific,
    };
    if scientific.len() < plain.len() {
        scientific
    } else {
        plain
    }
}

/// The qualified name of `local` under `prefix`, no prefix when it is
/// empty.
pub fn qualified(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        local.to_owned()
    } else {
        format!("{prefix}:{local}")
    }
}

/// An element whose content is elements or text, its tags as read. See the
/// [module](self).
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub tag: Tag,
    pub children: Vec<Node>,
}

/// A child of an [`Element`].
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Element(Element),
    Text(String),
    /// Kept as written: a comment, a processing instruction, text between
    /// elements, or an element nested too deep.
    Raw(Vec<u8>),
}

impl Element {
    /// An element named `name` (qualified), with no attribute and no content.
    pub fn new(name: &str) -> Self {
        Element {
            tag: Tag::new(name),
            children: Vec::new(),
        }
    }

    /// The element `xml` holds: its start tag first, after an XML
    /// declaration or white space.
    pub fn parse(xml: &[u8]) -> Result<Self> {
        let mut source = XmlSource::new(xml);
        let opened = source.root()?;
        Element::read(&mut source, &opened)
    }

    pub(crate) fn read(source: &mut XmlSource<'_>, opened: &Opened<'_>) -> Result<Self> {
        Self::read_at(source, opened, 0)
    }

    fn read_at(source: &mut XmlSource<'_>, opened: &Opened<'_>, depth: usize) -> Result<Self> {
        let mut element = Element {
            tag: Tag::read(&opened.start),
            children: Vec::new(),
        };
        if opened.empty {
            return Ok(element);
        }
        loop {
            match source.next_node()? {
                Content::Element(child) if depth + 1 < MAX_DEPTH => {
                    let child = Element::read_at(source, &child, depth + 1)?;
                    element.children.push(Node::Element(child));
                }
                Content::Element(child) => {
                    let range = source.finish(&child)?.range();
                    element
                        .children
                        .push(Node::Raw(source.bytes(range).to_vec()));
                }
                Content::Text(text) => match element.children.last_mut() {
                    Some(Node::Text(before)) => before.push_str(&text),
                    _ => element.children.push(Node::Text(text)),
                },
                Content::Raw(range) => {
                    element
                        .children
                        .push(Node::Raw(source.bytes(range).to_vec()));
                }
                Content::End(_) => break,
            }
        }
        // White space between elements is no content.
        let holds_elements = element
            .children
            .iter()
            .any(|child| matches!(child, Node::Element(_)));
        if holds_elements && element.tag.get("xml:space").as_deref() != Some("preserve") {
            element.children.retain(|child| match child {
                Node::Text(text) => !text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')),
                _ => true,
            });
        }
        Ok(element)
    }

    /// Writes the element: an empty-element tag when it holds nothing.
    pub fn write(&self, out: &mut Vec<u8>) {
        if self.children.is_empty() {
            self.tag.write_empty(out);
            return;
        }
        self.tag.write_start(out);
        for child in &self.children {
            match child {
                Node::Element(element) => element.write(out),
                Node::Text(text) => out.extend_from_slice(escape_text(text).as_bytes()),
                Node::Raw(bytes) => out.extend_from_slice(bytes),
            }
        }
        self.tag.write_end(out);
    }

    /// The element written.
    pub fn to_xml(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    /// The child elements, in order.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|child| match child {
            Node::Element(element) => Some(element),
            _ => None,
        })
    }

    /// The child elements whose local name is `local`, in order.
    pub fn elements_named<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a Element> {
        self.elements().filter(move |element| element.tag.is(local))
    }

    /// The first child element whose local name is `local`.
    pub fn child(&self, local: &str) -> Option<&Element> {
        self.elements().find(|element| element.tag.is(local))
    }

    /// The text the element holds, its references resolved.
    pub fn text(&self) -> String {
        self.children
            .iter()
            .filter_map(|child| match child {
                Node::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// An element start read by [`XmlSource`]: its tag, whether it holds
/// nothing, and the range of its start tag (the whole element when it holds
/// nothing).
pub(crate) struct Opened<'a> {
    pub start: BytesStart<'a>,
    pub empty: bool,
    pub tag: Range<usize>,
}

impl Opened<'_> {
    pub fn is(&self, local: &str) -> bool {
        self.start.local_name().as_ref() == local.as_bytes()
    }

    /// The ranges of an element that holds nothing.
    fn empty_span(&self) -> ElementSpan {
        ElementSpan {
            name: String::from_utf8_lossy(self.start.name().as_ref()).into_owned(),
            start_tag: self.tag.clone(),
            content: self.tag.end..self.tag.end,
            end_tag: self.tag.end..self.tag.end,
        }
    }
}

/// What comes next in an element's content.
pub(crate) enum Content<'a> {
    Element(Opened<'a>),
    /// Character data, its references resolved.
    Text(String),
    /// A comment or a processing instruction.
    Raw(Range<usize>),
    /// The end tag of the element being read, its range.
    End(Range<usize>),
}

/// A reader of one part, or one element, that knows where it is in the
/// bytes it reads: what it does not model is kept by its range.
pub(crate) struct XmlSource<'a> {
    xml: &'a [u8],
    reader: Reader<&'a [u8]>,
}

impl<'a> XmlSource<'a> {
    pub fn new(xml: &'a [u8]) -> Self {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        XmlSource { xml, reader }
    }

    pub fn position(&self) -> usize {
        usize::try_from(self.reader.buffer_position()).expect("a part fits in memory")
    }

    pub fn bytes(&self, range: Range<usize>) -> &'a [u8] {
        &self.xml[range]
    }

    /// The first element, after the XML declaration, comments, processing
    /// instructions and white space.
    pub fn root(&mut self) -> Result<Opened<'a>> {
        loop {
            let at = self.position();
            match self.reader.read_event()? {
                Event::Start(start) => return Ok(self.opened(start, false, at)),
                Event::Empty(start) => return Ok(self.opened(start, true, at)),
                Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
                Event::Decl(_) | Event::PI(_) | Event::Comment(_) | Event::DocType(_) => {}
                _ => {
                    return Err(OxmlError::MissingElement(
                        "the part has no root element".to_owned(),
                    ));
                }
            }
        }
    }

    fn opened(&self, start: BytesStart<'a>, empty: bool, at: usize) -> Opened<'a> {
        Opened {
            start,
            empty,
            tag: at..self.position(),
        }
    }

    /// The next thing in the content of the element being read.
    pub fn next_node(&mut self) -> Result<Content<'a>> {
        let at = self.position();
        Ok(match self.reader.read_event()? {
            Event::Start(start) => Content::Element(self.opened(start, false, at)),
            Event::Empty(start) => Content::Element(self.opened(start, true, at)),
            Event::End(_) => Content::End(at..self.position()),
            Event::Text(text) => Content::Text(decode_plain(&text)),
            Event::CData(data) => {
                let text = data.decode().map_err(quick_xml::Error::from)?;
                Content::Text(normalize_line_ends(&text).into_owned())
            }
            Event::GeneralRef(reference) => Content::Text(resolve_entity(&reference)),
            Event::Comment(_) | Event::PI(_) | Event::Decl(_) | Event::DocType(_) => {
                Content::Raw(at..self.position())
            }
            Event::Eof => {
                return Err(OxmlError::MissingElement(
                    "an element has no end tag".to_owned(),
                ));
            }
        })
    }

    /// The next child element, its end tag ([`Child::End`]) or something
    /// kept as written: text other than white space between elements, a
    /// comment, a processing instruction.
    pub fn next_child(&mut self) -> Result<Child<'a>> {
        loop {
            let at = self.position();
            return Ok(match self.next_node()? {
                Content::Element(opened) => Child::Element(opened),
                Content::End(range) => Child::End(range),
                Content::Text(text)
                    if text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')) =>
                {
                    continue;
                }
                Content::Text(_) | Content::Raw(_) => Child::Raw(at..self.position()),
            });
        }
    }

    /// Reads to the end of `opened` and gives its ranges.
    pub fn finish(&mut self, opened: &Opened<'_>) -> Result<ElementSpan> {
        if opened.empty {
            return Ok(opened.empty_span());
        }
        let end = self.reader.read_to_end(opened.start.name())?;
        let end_tag_start = usize::try_from(end.end).expect("a part fits in memory");
        Ok(ElementSpan {
            name: String::from_utf8_lossy(opened.start.name().as_ref()).into_owned(),
            start_tag: opened.tag.clone(),
            content: opened.tag.end..end_tag_start,
            end_tag: end_tag_start..self.position(),
        })
    }

    /// The text `opened` holds, up to its end tag; an element inside it is
    /// refused.
    pub fn text(&mut self, opened: &Opened<'_>) -> Result<String> {
        let mut text = String::new();
        if opened.empty {
            return Ok(text);
        }
        loop {
            match self.next_node()? {
                Content::Text(more) => text.push_str(&more),
                Content::Raw(_) => {}
                Content::End(_) => return Ok(text),
                Content::Element(child) => {
                    return Err(OxmlError::UnexpectedElement(format!(
                        "{} inside {}",
                        String::from_utf8_lossy(child.start.name().as_ref()),
                        String::from_utf8_lossy(opened.start.name().as_ref())
                    )));
                }
            }
        }
    }
}

/// A child of the element being read: see [`XmlSource::next_child`].
pub(crate) enum Child<'a> {
    Element(Opened<'a>),
    Raw(Range<usize>),
    End(Range<usize>),
}

/// The edits of one part: ranges of its bytes replaced, or places where
/// bytes are inserted, applied in one pass.
#[derive(Default)]
pub(crate) struct Splice {
    edits: Vec<(Range<usize>, Vec<u8>)>,
}

impl Splice {
    pub fn replace(&mut self, range: Range<usize>, bytes: Vec<u8>) {
        self.edits.push((range, bytes));
    }

    pub fn insert(&mut self, at: usize, bytes: Vec<u8>) {
        self.edits.push((at..at, bytes));
    }

    /// `source` with every edit made. Insertions at the same place keep the
    /// order they were made in, and go before a replacement starting there.
    pub fn apply(mut self, source: &[u8]) -> Vec<u8> {
        self.edits
            .sort_by_key(|(range, _)| (range.start, !range.is_empty()));
        let mut out = Vec::with_capacity(source.len());
        let mut at = 0;
        for (range, bytes) in self.edits {
            debug_assert!(range.start >= at, "spliced ranges overlap");
            out.extend_from_slice(&source[at..range.start]);
            out.extend_from_slice(&bytes);
            at = range.end;
        }
        out.extend_from_slice(&source[at..]);
        out
    }
}

/// Writes `content` at the end of the element `span` of the source, its
/// start tag rewritten as `tag` when given: an element written as one
/// empty-element tag is written again with a start and an end tag.
pub(crate) fn append_to(
    splice: &mut Splice,
    span: &ElementSpan,
    current: &Tag,
    tag: Option<&Tag>,
    content: Vec<u8>,
) {
    let tag = tag.unwrap_or(current);
    if span.is_self_closing() {
        let mut out = Vec::new();
        tag.write_start(&mut out);
        out.extend_from_slice(&content);
        tag.write_end(&mut out);
        splice.replace(span.range(), out);
        return;
    }
    if tag != current {
        let mut out = Vec::new();
        tag.write_start(&mut out);
        splice.replace(span.start_tag.clone(), out);
    }
    if !content.is_empty() {
        splice.insert(span.content.end, content);
    }
}

/// The root element of a part and its children, each by its local name and
/// range, in order.
#[derive(Clone, Debug)]
pub(crate) struct Outline {
    pub span: ElementSpan,
    pub tag: Tag,
    pub children: Vec<(String, Range<usize>)>,
}

impl Outline {
    /// Where a new child named `local` goes in a root whose children follow
    /// `order`: before the first child the schema puts after it, else at the
    /// end of the root. Children the order does not name stay where they
    /// are.
    pub fn insertion_point(&self, order: &[&str], local: &str) -> usize {
        let children = self
            .children
            .iter()
            .map(|(name, range)| (name.as_str(), range.start));
        insertion_point(children, self.span.content.end, order, local)
    }
}

/// Where a new child named `local` goes among `children` (each by its local
/// name and where it starts) in an element whose content ends at `end` and
/// whose children follow `order`: before the first child the schema puts
/// after it, else at the end.
pub(crate) fn insertion_point<'a>(
    children: impl IntoIterator<Item = (&'a str, usize)>,
    end: usize,
    order: &[&str],
    local: &str,
) -> usize {
    let rank = |name: &str| order.iter().position(|known| *known == name);
    let Some(own) = rank(local) else {
        return end;
    };
    children
        .into_iter()
        .find(|(name, _)| rank(name).is_some_and(|other| other > own))
        .map_or(end, |(_, start)| start)
}

/// The local name of the element `xml` starts with.
pub(crate) fn local_name_of(xml: &[u8]) -> Option<String> {
    let tag = oxoffice_core::xml_element::start_tag(xml).ok()?;
    Some(tag.name.rsplit(':').next().unwrap_or(&tag.name).to_owned())
}

/// The prefix the tag binds to one of `namespaces`: `""` for a default
/// namespace, which wins over a prefix.
pub(crate) fn prefix_of(tag: &Tag, namespaces: &[&str]) -> Option<String> {
    let declarations = tag.namespace_declarations();
    declarations
        .iter()
        .find(|(prefix, uri)| prefix.is_empty() && namespaces.contains(&uri.as_str()))
        .or_else(|| {
            declarations
                .iter()
                .find(|(_, uri)| namespaces.contains(&uri.as_str()))
        })
        .map(|(prefix, _)| prefix.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_tolerantly_and_write_fixed() {
        for (text, boolean, integer, number) in [
            ("1", Some(true), Some(1), Some(1.0)),
            ("true", Some(true), None, None),
            ("0", Some(false), Some(0), Some(0.0)),
            ("false", Some(false), None, None),
            ("1.0", None, Some(1), Some(1.0)),
            (" 15.75 ", None, None, Some(15.75)),
            ("NaN", None, None, None),
        ] {
            assert_eq!(parse_bool(text), boolean, "{text}");
            assert_eq!(parse_u32(text), integer, "{text}");
            assert_eq!(parse_f64(text), number, "{text}");
        }
        for (value, text) in [
            (1.0, "1"),
            (0.1, "0.1"),
            (152.66, "152.66"),
            (1e21, "1E+21"),
            (1.5e-7, "1.5E-7"),
            (-46266.0, "-46266"),
        ] {
            assert_eq!(format_f64(value), text);
            assert_eq!(parse_f64(&format_f64(value)), Some(value));
        }
    }

    #[test]
    fn a_tag_keeps_its_attributes_as_written_and_sets_one_in_the_fixed_form() {
        let xml = br#"<row r="1" customHeight='true' ht="15" x14ac:dyDescent="0.25"/>"#;
        let mut tag = Tag::read(&match Reader::from_reader(&xml[..]).read_event().unwrap() {
            Event::Empty(start) => start,
            other => panic!("{other:?}"),
        });
        let mut out = Vec::new();
        tag.write_empty(&mut out);
        assert_eq!(out, xml);
        assert_eq!(tag.bool("customHeight"), Some(true));
        assert_eq!(tag.get_prefixed("dyDescent").as_deref(), Some("0.25"));
        tag.set_bool("customHeight", Some(false));
        tag.set_f64("ht", None);
        tag.set("spans", Some("1:3"));
        let mut out = Vec::new();
        tag.write_start(&mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"<row r="1" customHeight="0" x14ac:dyDescent="0.25" spans="1:3">"#
        );
    }
}
