//! Raw XML capture utilities for preserving unknown elements during round-trip.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use std::io::Write;
use std::ops::Range;

use crate::error::Result;

/// Capture a full XML subtree (from after the start tag through the matching end tag)
/// and return it as raw bytes. The returned bytes include the start tag, all children,
/// and the end tag.
pub fn capture_element(reader: &mut Reader<&[u8]>, start: &BytesStart) -> Result<Vec<u8>> {
    Ok(capture_element_in(reader, start, None)?.0)
}

/// Capture with numeric copy ranges from this same live reader input.
/// Unmapped output gaps are generated. Unproved modes preserve capture behavior
/// and return no transcript rather than creating a new parsing refusal.
#[doc(hidden)]
#[allow(clippy::type_complexity)] // Exact approved transport tuple, no public type.
pub fn capture_element_with_source_ranges(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart<'_>,
    input: &[u8],
    opening: Range<usize>,
) -> Result<(Vec<u8>, Option<Vec<(Range<usize>, Range<usize>)>>)> {
    let ranges = CaptureSourceRanges::begin(reader, start, input, opening);
    capture_element_in(reader, start, ranges)
}

struct CaptureSourceRanges<'a> {
    input: &'a [u8],
    reader_offset: usize,
    trim_start: bool,
    trim_end: bool,
    pieces: Vec<(Range<usize>, Range<usize>)>,
}

impl<'a> CaptureSourceRanges<'a> {
    fn position(&self, reader: &Reader<&[u8]>) -> Option<usize> {
        let consumed = usize::try_from(reader.buffer_position()).ok()?;
        let position = consumed.checked_add(self.reader_offset)?;
        let remaining = *reader.get_ref();
        (self.input.len().checked_sub(position) == Some(remaining.len())
            && (self.input.as_ptr() as usize).checked_add(position)
                == Some(remaining.as_ptr() as usize))
        .then_some(position)
    }

    fn begin(
        reader: &Reader<&[u8]>,
        start: &BytesStart<'_>,
        input: &'a [u8],
        opening: Range<usize>,
    ) -> Option<Self> {
        let config = reader.config();
        if config.expand_empty_elements {
            return None;
        }
        // Reader initialization can remove a BOM without adding it to the
        // logical buffer_position. Prove its exact displacement in this input.
        let physical = input.len().checked_sub(reader.get_ref().len())?;
        let offset = physical.checked_sub(usize::try_from(reader.buffer_position()).ok()?)?;
        if offset != 0 && !(offset == 3 && input.starts_with(b"\xef\xbb\xbf")) {
            return None;
        }
        let mut ranges = Self {
            input,
            reader_offset: offset,
            trim_start: config.trim_text_start,
            trim_end: config.trim_text_end,
            pieces: Vec::new(),
        };
        if ranges.position(reader)? != opening.end
            || opening.start > opening.end
            || !ranges.record(&Event::Start(start.borrow()), opening, 0)
        {
            return None;
        }
        Some(ranges)
    }

    fn push(&mut self, source: Range<usize>, output: Range<usize>) {
        if source.is_empty() {
            return;
        }
        if let Some((previous_source, previous_output)) = self.pieces.last_mut()
            && previous_source.end == source.start
            && previous_output.end == output.start
        {
            previous_source.end = source.end;
            previous_output.end = output.end;
        } else {
            self.pieces.push((source, output));
        }
    }

    fn record(&mut self, event: &Event<'_>, mut source: Range<usize>, output: usize) -> bool {
        let Some(mut raw) = self.input.get(source.clone()) else {
            return false;
        };
        let payload: &[u8] = event;
        let whitespace = |byte: &u8| matches!(byte, b' ' | b'\t' | b'\r' | b'\n');
        // QuickXML physically skips leading whitespace before dispatch. One
        // consumed window may therefore include that prefix and the next tag.
        if self.trim_start {
            let skipped = raw.iter().take_while(|byte| whitespace(byte)).count();
            source.start += skipped;
            raw = &raw[skipped..];
        }
        match event {
            Event::Eof => true,
            Event::Text(_) => {
                let end = if self.trim_end {
                    raw.iter()
                        .rposition(|byte| !whitespace(byte))
                        .map_or(0, |at| at + 1)
                } else {
                    raw.len()
                };
                if raw.get(..end) != Some(payload) {
                    return false;
                }
                let Some(produced_end) = output.checked_add(payload.len()) else {
                    return false;
                };
                self.push(source.start..source.start + end, output..produced_end);
                true
            }
            Event::End(_) => {
                let Some(name_end) = 2usize.checked_add(payload.len()) else {
                    return false;
                };
                if raw.len() <= name_end
                    || !raw.starts_with(b"</")
                    || !raw.ends_with(b">")
                    || raw.get(2..name_end) != Some(payload)
                    || !raw[name_end..raw.len() - 1]
                        .iter()
                        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
                {
                    return false;
                }
                let Some(end) = output.checked_add(name_end) else {
                    return false;
                };
                self.push(source.start..source.start + name_end, output..end);
                self.push(source.end - 1..source.end, end..end + 1);
                true
            }
            Event::DocType(_) => {
                let Some(start) = raw.len().checked_sub(payload.len() + 1) else {
                    return false;
                };
                if !raw.ends_with(b">") || raw.get(start..raw.len() - 1) != Some(payload) {
                    return false;
                }
                let Some(at) = output.checked_add(10) else {
                    return false;
                };
                self.push(source.start + start..source.end - 1, at..at + payload.len());
                self.push(
                    source.end - 1..source.end,
                    at + payload.len()..at + payload.len() + 1,
                );
                true
            }
            _ => {
                let (prefix, suffix): (&[u8], &[u8]) = match event {
                    Event::Start(_) => (b"<", b">"),
                    Event::Empty(_) => (b"<", b"/>"),

                    Event::CData(_) => (b"<![CDATA[", b"]]>"),
                    Event::Comment(_) => (b"<!--", b"-->"),
                    Event::PI(_) | Event::Decl(_) => (b"<?", b"?>"),
                    Event::GeneralRef(_) => (b"&", b";"),
                    Event::Text(_) | Event::End(_) | Event::DocType(_) | Event::Eof => {
                        return false;
                    }
                };
                let Some(length) = prefix
                    .len()
                    .checked_add(payload.len())
                    .and_then(|length| length.checked_add(suffix.len()))
                else {
                    return false;
                };
                if raw.len() != length
                    || !raw.starts_with(prefix)
                    || !raw.ends_with(suffix)
                    || raw.get(prefix.len()..length - suffix.len()) != Some(payload)
                {
                    return false;
                }
                let Some(end) = output.checked_add(length) else {
                    return false;
                };
                self.push(source, output..end);
                true
            }
        }
    }
}

#[allow(clippy::type_complexity)] // Same concrete plain/mapped capture result.
fn capture_element_in(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart<'_>,
    mut ranges: Option<CaptureSourceRanges<'_>>,
) -> Result<(Vec<u8>, Option<Vec<(Range<usize>, Range<usize>)>>)> {
    let mut writer = Writer::new(Vec::new());

    // Write the start tag
    writer.write_event(Event::Start(start.to_owned()))?;

    // Track nesting depth for the tag name
    let tag_name = start.name().as_ref().to_vec();
    let mut depth = 1u32;
    let mut buf = Vec::new();

    loop {
        let before = ranges.as_ref().and_then(|ranges| ranges.position(reader));
        let event = reader.read_event_into(&mut buf);
        if let Some(current) = ranges.as_mut() {
            let proved = if let (Some(before), Ok(event), Some(after)) =
                (before, event.as_ref(), current.position(reader))
            {
                current.record(event, before..after, writer.get_ref().len())
            } else {
                false
            };
            if !proved {
                ranges = None;
            }
        }
        match event {
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == tag_name {
                    depth += 1;
                }
                writer.write_event(Event::Start(e.to_owned()))?;
            }
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == tag_name {
                    depth -= 1;
                }
                writer.write_event(Event::End(e.to_owned()))?;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Empty(ref e)) => {
                writer.write_event(Event::Empty(e.to_owned()))?;
            }
            Ok(Event::Text(ref e)) => {
                writer.write_event(Event::Text(e.to_owned().into_owned()))?;
            }
            Ok(Event::CData(ref e)) => {
                writer.write_event(Event::CData(e.to_owned().into_owned()))?;
            }
            Ok(Event::Comment(ref e)) => {
                writer.write_event(Event::Comment(e.to_owned().into_owned()))?;
            }
            Ok(Event::PI(ref e)) => {
                writer.write_event(Event::PI(e.to_owned().into_owned()))?;
            }
            Ok(Event::Decl(ref e)) => {
                writer.write_event(Event::Decl(e.to_owned().into_owned()))?;
            }
            Ok(Event::DocType(ref e)) => {
                writer.write_event(Event::DocType(e.to_owned().into_owned()))?;
            }
            // Entity references are their own event since quick-xml 0.41;
            // re-emit them verbatim so captured markup round-trips unchanged.
            Ok(Event::GeneralRef(ref e)) => {
                writer.write_event(Event::GeneralRef(e.to_owned().into_owned()))?;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.into()),
        }
        buf.clear();
    }

    Ok((writer.into_inner(), ranges.map(|ranges| ranges.pieces)))
}

/// Capture an empty (self-closing) element as raw bytes.
pub fn capture_empty_element(e: &BytesStart) -> Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Empty(e.to_owned()))?;
    Ok(writer.into_inner())
}

/// Write already serialized XML fragments to `writer` verbatim, in order.
pub fn emit_raw<'a, W: Write>(
    writer: &mut Writer<W>,
    children: impl Iterator<Item = &'a [u8]>,
) -> Result<()> {
    for child in children {
        writer.get_mut().write_all(child)?;
    }
    Ok(())
}

/// Push attribute name and value pairs onto a start tag, in order.
pub fn push_attributes(start: &mut BytesStart<'_>, attributes: &[(String, String)]) {
    for (name, value) in attributes {
        start.push_attribute((name.as_str(), value.as_str()));
    }
}
