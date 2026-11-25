//! Where the parts of one element are in its written XML: its start tag, its
//! content and its end tag, found by reading the XML whatever the writing.
//! The element may use any prefix, be written self-closing (`<w:sdtContent/>`,
//! as Google Docs writes an empty one) or with spaces in its end tag
//! (`</w:p >`). Code that needs where an element's content starts or ends,
//! or that writes content into an element, goes through here, never through
//! a search for the text `</name>`: `tests/xml_element.rs` refuses such a
//! search anywhere else in `crates/*/src`.

use std::ops::Range;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::{OxmlError, Result};

/// The parts of one element in the XML it is read from, as byte ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementSpan {
    /// The qualified name, as written.
    pub name: String,
    /// The start tag, or the whole element when it is written self-closing.
    pub start_tag: Range<usize>,
    /// The content: empty, at the end of the tag, for a self-closing element.
    pub content: Range<usize>,
    /// The end tag: empty, at the end of the tag, for a self-closing element.
    pub end_tag: Range<usize>,
}

impl ElementSpan {
    /// Whether the element is written as one empty-element tag.
    pub fn is_self_closing(&self) -> bool {
        self.end_tag.is_empty()
    }

    /// The whole element.
    pub fn range(&self) -> Range<usize> {
        self.start_tag.start..self.end_tag.end
    }

    /// The local part of the name.
    pub fn local_name(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    fn shifted(mut self, by: usize) -> Self {
        for range in [&mut self.start_tag, &mut self.content, &mut self.end_tag] {
            *range = range.start + by..range.end + by;
        }
        self
    }
}

fn invalid(message: String) -> OxmlError {
    OxmlError::InvalidValue(message)
}

fn position(reader: &Reader<&[u8]>) -> usize {
    usize::try_from(reader.buffer_position()).expect("an XML text fits in memory")
}

/// The element `xml` starts with: `xml` must start with its start tag, and
/// may go on after its end.
pub fn element_span(xml: &str) -> Result<ElementSpan> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = false;
    let (name, empty) = match reader.read_event()? {
        Event::Start(start) => (
            std::str::from_utf8(start.name().as_ref())?.to_owned(),
            false,
        ),
        Event::Empty(start) => (std::str::from_utf8(start.name().as_ref())?.to_owned(), true),
        _ => return Err(invalid("the XML does not start with an element".to_owned())),
    };
    let start_end = position(&reader);
    if empty {
        return Ok(ElementSpan {
            name,
            start_tag: 0..start_end,
            content: start_end..start_end,
            end_tag: start_end..start_end,
        });
    }
    let mut depth = 0usize;
    loop {
        let before = position(&reader);
        match reader.read_event()? {
            Event::Start(_) => depth += 1,
            Event::End(end) if depth == 0 => {
                if end.name().as_ref() != name.as_bytes() {
                    return Err(invalid(format!(
                        "{name} is closed by {}",
                        String::from_utf8_lossy(end.name().as_ref())
                    )));
                }
                return Ok(ElementSpan {
                    name,
                    start_tag: 0..start_end,
                    content: start_end..before,
                    end_tag: before..position(&reader),
                });
            }
            Event::End(_) => depth -= 1,
            Event::Eof => return Err(invalid(format!("{name} has no end tag"))),
            _ => {}
        }
    }
}

/// The root element of a part, after its XML declaration, comments,
/// processing instructions and whitespace, its ranges in `xml`.
pub fn root_element_span(xml: &str) -> Result<ElementSpan> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    loop {
        let before = position(&reader);
        match reader.read_event()? {
            Event::Start(_) | Event::Empty(_) => {
                return element_span(&xml[before..]).map(|span| span.shifted(before));
            }
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) | Event::DocType(_) => {}
            _ => return Err(invalid("the XML has no root element".to_owned())),
        }
    }
}

/// The first element of `xml`, at any depth, whose qualified name is `name`
/// as written, its ranges in `xml`.
pub fn find_element(xml: &str, name: &str) -> Result<Option<ElementSpan>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = false;
    loop {
        let before = position(&reader);
        match reader.read_event()? {
            Event::Start(start) | Event::Empty(start)
                if start.name().as_ref() == name.as_bytes() =>
            {
                return element_span(&xml[before..]).map(|span| Some(span.shifted(before)));
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
    }
}

/// The element `xml` starts with (and what follows it), its content
/// replaced by `content`, and where `content` starts in the result. An
/// element written self-closing gets an end tag to hold `content`, and stays
/// as written when `content` is empty.
pub fn replace_content(xml: &str, content: &str) -> Result<(String, usize)> {
    let span = element_span(xml)?;
    if span.is_self_closing() {
        if content.is_empty() {
            return Ok((xml.to_owned(), span.content.start));
        }
        let tag = xml[span.start_tag.clone()]
            .strip_suffix("/>")
            .ok_or_else(|| invalid(format!("{} has no empty-element tag", span.name)))?;
        let opening = format!("{}>", tag.trim_end());
        let at = opening.len();
        return Ok((
            format!(
                "{opening}{content}</{}>{}",
                span.name,
                &xml[span.end_tag.end..]
            ),
            at,
        ));
    }
    let mut output = xml.to_owned();
    output.replace_range(span.content.clone(), content);
    Ok((output, span.content.start))
}

/// The element `xml` starts with, `addition` written at the end of its
/// content (an element written self-closing gets an end tag).
pub fn append_content(xml: &str, addition: &str) -> Result<String> {
    let span = element_span(xml)?;
    let content = format!("{}{addition}", &xml[span.content.clone()]);
    Ok(replace_content(xml, &content)?.0)
}

/// The element `xml` starts with, renamed `name` in its start tag and in its
/// end tag.
pub fn rename(xml: &str, name: &str) -> Result<String> {
    let span = element_span(xml)?;
    let mut output = xml.to_owned();
    if !span.is_self_closing() {
        output.replace_range(
            span.end_tag.clone(),
            &format!(
                "</{name}{}",
                &xml[span.end_tag.start + 2 + span.name.len()..span.end_tag.end]
            ),
        );
    }
    output.replace_range(1..1 + span.name.len(), name);
    Ok(output)
}

/// What a fragment of XML says, whatever its writing, for comparing two
/// fragments: each element start and end on a line, its name expanded to
/// `{namespace}local` with the declarations the fragment holds (as written
/// when the fragment does not declare its prefix), its attributes sorted
/// without namespace declarations and their values with references
/// resolved; each text with references resolved, but whitespace between
/// elements outside `xml:space="preserve"`, which is no content of
/// WordprocessingML, PresentationML or DrawingML. An empty-element tag reads
/// as a start tag and an end tag, and the order, quotes and spacing of
/// attributes do not show.
pub fn canonical_fragment(xml: &str) -> Result<String> {
    canonical_fragment_in(xml, &[])
}

/// [`canonical_fragment`] of a fragment read where `bindings` (prefix,
/// namespace) are in scope, as a fragment cut out of a part is.
pub fn canonical_fragment_in(xml: &str, bindings: &[(String, String)]) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = false;
    let mut scopes: Vec<Vec<(String, String)>> = vec![bindings.to_vec()];
    let mut lines = Vec::new();
    let mut text = String::new();
    let expand = |scopes: &[Vec<(String, String)>], name: &str, element: bool| -> String {
        let (prefix, local) = name.split_once(':').unwrap_or(("", name));
        if prefix.is_empty() && !element {
            return local.to_owned();
        }
        scopes
            .iter()
            .rev()
            .flat_map(|frame| frame.iter().rev())
            .find(|(bound, _)| bound == prefix)
            .map_or_else(|| name.to_owned(), |(_, uri)| format!("{{{uri}}}{local}"))
    };
    let decoded = |result: std::result::Result<
        std::borrow::Cow<'_, str>,
        quick_xml::encoding::EncodingError,
    >| {
        result
            .map(std::borrow::Cow::into_owned)
            .map_err(|error| invalid(error.to_string()))
    };
    // Each open element: whether it holds an element, and whether it is
    // under `xml:space="preserve"`.
    let mut open: Vec<(bool, bool)> = Vec::new();
    loop {
        let event = reader.read_event()?;
        if !matches!(
            event,
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_)
        ) && !text.is_empty()
        {
            // Whitespace between elements, outside `xml:space="preserve"`,
            // is no content of element-only markup.
            let parent = open.last().copied();
            let child_follows = matches!(event, Event::Start(_) | Event::Empty(_));
            let insignificant = text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
                && parent
                    .is_none_or(|(has_child, preserve)| !preserve && (has_child || child_follows));
            let text = std::mem::take(&mut text);
            if !insignificant {
                lines.push(format!("{text:?}"));
            }
        }
        match &event {
            Event::Start(start) | Event::Empty(start) => {
                let mut frame = Vec::new();
                let mut attributes = Vec::new();
                let mut preserve = open.last().is_some_and(|(_, preserve)| *preserve);
                for attribute in start.attributes() {
                    let attribute = attribute?;
                    let key = std::str::from_utf8(attribute.key.as_ref())?;
                    let value = crate::xml::attribute_text(&attribute)?.into_owned();
                    if key == "xml:space" {
                        preserve = value == "preserve";
                    }
                    match key.strip_prefix("xmlns") {
                        Some("") => frame.push((String::new(), value)),
                        Some(prefix) if prefix.starts_with(':') => {
                            frame.push((prefix[1..].to_owned(), value));
                        }
                        _ => attributes.push((key.to_owned(), value)),
                    }
                }
                if let Some(parent) = open.last_mut() {
                    parent.0 = true;
                }
                if matches!(event, Event::Start(_)) {
                    open.push((false, preserve));
                }
                scopes.push(frame);
                let name = expand(&scopes, std::str::from_utf8(start.name().as_ref())?, true);
                let mut attributes: Vec<(String, String)> = attributes
                    .into_iter()
                    .map(|(key, value)| (expand(&scopes, &key, false), value))
                    .collect();
                attributes.sort();
                lines.push(format!("<{name}{attributes:?}"));
                if matches!(event, Event::Empty(_)) {
                    scopes.pop();
                    lines.push(">".to_owned());
                }
            }
            Event::End(_) => {
                scopes.pop();
                open.pop();
                lines.push(">".to_owned());
            }
            Event::Text(value) => text.push_str(&decoded(value.decode())?),
            Event::GeneralRef(reference) => match reference.resolve_char_ref()? {
                Some(c) => text.push(c),
                None => match decoded(reference.decode())?.as_str() {
                    "amp" => text.push('&'),
                    "lt" => text.push('<'),
                    "gt" => text.push('>'),
                    "quot" => text.push('"'),
                    "apos" => text.push('\''),
                    other => text.push_str(&format!("&{other};")),
                },
            },
            Event::CData(data) => text.push_str(&decoded(data.decode())?),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(lines.join("\n"))
}
