//! Helpers for turning quick-xml text events into plain Rust strings.
//!
//! Since quick-xml 0.41 a character or general entity reference (`&amp;`,
//! `&#65;`) is reported as its own [`Event::GeneralRef`] rather than being
//! folded into the surrounding [`Event::Text`]. Any loop that accumulates the
//! text of an element therefore has to handle both events, or entities are
//! silently dropped from the result.
//!
//! [`Event::GeneralRef`]: quick_xml::events::Event::GeneralRef
//! [`Event::Text`]: quick_xml::events::Event::Text

use std::borrow::Cow;

use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesRef, BytesText, Event};
use quick_xml::name::QName;

/// `text` after XML end-of-line handling, as every XML reader reads it: a
/// literal carriage return, alone or before a line feed, reads as a line
/// feed. A carriage return written as `&#13;` is not literal and stays one,
/// so apply this before resolving references.
pub fn normalize_line_ends(text: &str) -> Cow<'_, str> {
    if text.contains('\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

/// Decode a [`BytesText`] and resolve any XML entities it still contains.
///
/// Use this for spans produced by [`Reader::read_text`], which returns the raw
/// markup between the tags — entities in it are *not* pre-resolved.
pub fn decode_escaped(text: &BytesText<'_>) -> String {
    let Ok(raw) = text.decode() else {
        return String::new();
    };
    let raw = normalize_line_ends(&raw);
    match unescape(&raw) {
        Ok(unescaped) => unescaped.into_owned(),
        // Malformed or unknown entity: keep the raw text rather than losing it.
        Err(_) => raw.into_owned(),
    }
}

/// Decode an [`Event::Text`] payload, which quick-xml has already stripped of
/// entity references (those arrive separately as [`Event::GeneralRef`]).
///
/// [`Event::GeneralRef`]: quick_xml::events::Event::GeneralRef
/// [`Event::Text`]: quick_xml::events::Event::Text
pub fn decode_plain(text: &BytesText<'_>) -> String {
    text.decode()
        .map(|c| normalize_line_ends(&c).into_owned())
        .unwrap_or_default()
}

/// Resolve a single entity reference event to the text it stands for.
///
/// Handles numeric references (`&#65;`, `&#x41;`) and the five XML predefined
/// entities. An unresolvable reference is reproduced verbatim (`&name;`) so it
/// survives a round trip instead of vanishing.
pub fn resolve_entity(entity: &BytesRef<'_>) -> String {
    let Ok(name) = entity.decode() else {
        return String::new();
    };
    match unescape(&format!("&{name};")) {
        Ok(resolved) => resolved.into_owned(),
        Err(_) => format!("&{name};"),
    }
}

/// Read the full text content of the element that `start_name` opened,
/// resolving entity references.
///
/// This is the event-loop counterpart to [`decode_escaped`]: it consumes events
/// up to the matching end tag, concatenating [`Event::Text`], [`Event::CData`]
/// and [`Event::GeneralRef`] payloads.
///
/// [`Event::CData`]: quick_xml::events::Event::CData
/// [`Event::GeneralRef`]: quick_xml::events::Event::GeneralRef
/// [`Event::Text`]: quick_xml::events::Event::Text
pub fn read_element_text(reader: &mut Reader<&[u8]>, start_name: QName<'_>) -> String {
    let end = start_name.as_ref().to_vec();
    let mut out = String::new();
    let mut buf = Vec::new();
    let mut depth = 1u32;

    // An entity splits the value into several Text events, and trimming each of
    // them individually would eat the spaces on either side of the entity —
    // "Title &amp; Co." would come back as "Title&Co.". Read untrimmed and
    // restore the caller's setting afterwards.
    let (trim_start, trim_end) = {
        let config = reader.config_mut();
        let previous = (config.trim_text_start, config.trim_text_end);
        config.trim_text(false);
        previous
    };

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == end {
                    depth += 1;
                }
            }
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == end {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            Ok(Event::Text(ref e)) => out.push_str(&decode_plain(e)),
            Ok(Event::CData(ref e)) => {
                if let Ok(decoded) = e.decode() {
                    out.push_str(&normalize_line_ends(&decoded));
                }
            }
            Ok(Event::GeneralRef(ref e)) => out.push_str(&resolve_entity(e)),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    let config = reader.config_mut();
    config.trim_text_start = trim_start;
    config.trim_text_end = trim_end;

    out
}
