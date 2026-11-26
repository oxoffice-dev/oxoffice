//! The XML readers of the models: quick-xml's readers, with an element that
//! holds nothing read the same way in both its writings.
//!
//! An element with no content is written `<w:b/>` by one producer and
//! `<w:b></w:b>` by another; the two say the same. quick-xml delivers the
//! first as one [`Event::Empty`] and the second as an [`Event::Start`] and an
//! [`Event::End`], so a reader that models the element on `Empty` alone keeps
//! the second raw or drops it: a tab leaves the text, a bold toggle is lost.
//! [`Reader`] and [`NsReader`] deliver both writings as one `Empty` event, the
//! reader positioned after the end tag, and pass everything else through.
//! [`Reader::empty_writing`] says which writing the last `Empty` event had,
//! for a writer that keeps it ([`crate::raw_xml::capture_empty`]).
//!
//! An element holding whitespace, a comment, a processing instruction, a
//! reference or CDATA is not collapsed: whitespace is content in a text
//! element. Under quick-xml's `expand_empty_elements`, nothing is collapsed.
//!
//! Each derefs to its quick-xml reader, so every other method is quick-xml's
//! own, and a function that takes the quick-xml reader takes this one too.
//! Through the quick-xml reader (or [`Reader::verbatim`]), events come as
//! written: verbatim capture ([`crate::raw_xml::capture_element`]) reads that
//! way, so unmodelled XML keeps the writing it was read in.

use std::io::BufRead;
use std::ops::{Deref, DerefMut};

use quick_xml::Result;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

/// How an element with nothing inside was written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EmptyWriting {
    /// One empty-element tag: `<x/>`.
    #[default]
    SelfClosed,
    /// A start tag followed at once by its end tag: `<x></x>`.
    EndTag,
}

/// quick-xml's [`quick_xml::Reader`], an element written with an end tag and
/// nothing inside delivered as an [`Event::Empty`]. See the [module](self).
#[derive(Clone, Debug)]
pub struct Reader<R> {
    inner: quick_xml::Reader<R>,
    /// Where the end tag of a collapsed element is read.
    end: Vec<u8>,
    /// The writing of the last event read, when it is `Empty`.
    writing: EmptyWriting,
}

/// quick-xml's [`quick_xml::NsReader`], an element written with an end tag
/// and nothing inside delivered as an [`Event::Empty`], its namespace scope
/// closed by the next read as an empty-element tag's is. See the
/// [module](self).
#[derive(Clone, Debug)]
pub struct NsReader<R> {
    inner: quick_xml::NsReader<R>,
    /// Where the end tag of a collapsed element is read.
    end: Vec<u8>,
    /// The writing of the last event read, when it is `Empty`.
    writing: EmptyWriting,
}

/// Whether `source`, read from just after the start tag of `name`, begins
/// with the end tag of the same element: the element holds nothing, not even
/// whitespace. Reads a copy of `source`, which stays where it was.
fn closes_at_once<R: BufRead + Clone>(source: &R, name: &[u8]) -> bool {
    let mut probe = source.clone();
    let mut next = || {
        let byte = *probe.fill_buf().ok()?.first()?;
        probe.consume(1);
        Some(byte)
    };
    if !b"</".iter().chain(name).all(|want| next() == Some(*want)) {
        return false;
    }
    loop {
        match next() {
            Some(b' ' | b'\t' | b'\r' | b'\n') => {}
            byte => return byte == Some(b'>'),
        }
    }
}

/// Whether `event` starts an element whose end tag follows at once in
/// `source`, and `config` does not ask for empty elements expanded.
fn collapses<R: BufRead + Clone>(
    event: &Event<'_>,
    config: &quick_xml::reader::Config,
    source: &R,
) -> bool {
    matches!(event, Event::Start(start)
        if !config.expand_empty_elements && closes_at_once(source, start.name().as_ref()))
}

/// `event`, a start tag that [`collapses`], as one empty-element tag.
fn as_empty(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Start(start) => Event::Empty(start),
        event => event,
    }
}

impl<R> Reader<R> {
    /// A reader of `source`.
    pub fn from_reader(source: R) -> Self {
        Self {
            inner: quick_xml::Reader::from_reader(source),
            end: Vec::new(),
            writing: EmptyWriting::SelfClosed,
        }
    }

    /// The quick-xml reader, which delivers every event as written.
    pub fn verbatim(&mut self) -> &mut quick_xml::Reader<R> {
        &mut self.inner
    }

    /// How the last [`Event::Empty`] read was written: valid until the next
    /// read, [`EmptyWriting::SelfClosed`] after any other event.
    pub fn empty_writing(&self) -> EmptyWriting {
        self.writing
    }
}

impl<R: BufRead + Clone> Reader<R> {
    /// The next event, read into `buf`, as [`quick_xml::Reader::read_event_into`]
    /// reads it but for an element with nothing inside: one `Empty` event.
    pub fn read_event_into<'b>(&mut self, buf: &'b mut Vec<u8>) -> Result<Event<'b>> {
        self.writing = EmptyWriting::SelfClosed;
        let event = self.inner.read_event_into(buf)?;
        if !collapses(&event, self.inner.config(), self.inner.get_ref()) {
            return Ok(event);
        }
        self.end.clear();
        self.inner.read_event_into(&mut self.end)?;
        self.writing = EmptyWriting::EndTag;
        Ok(as_empty(event))
    }
}

impl<'i> Reader<&'i [u8]> {
    /// A reader of `input`.
    #[allow(clippy::should_implement_trait)] // quick-xml's own name.
    pub fn from_str(input: &'i str) -> Self {
        Self::from_reader(input.as_bytes())
    }

    /// The next event, borrowed from the input, as
    /// [`quick_xml::Reader::read_event`] reads it but for an element with
    /// nothing inside: one `Empty` event.
    pub fn read_event(&mut self) -> Result<Event<'i>> {
        self.writing = EmptyWriting::SelfClosed;
        let event = self.inner.read_event()?;
        if !collapses(&event, self.inner.config(), self.inner.get_ref()) {
            return Ok(event);
        }
        self.inner.read_event()?;
        self.writing = EmptyWriting::EndTag;
        Ok(as_empty(event))
    }
}

impl<R> Deref for Reader<R> {
    type Target = quick_xml::Reader<R>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<R> DerefMut for Reader<R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<R> NsReader<R> {
    /// A reader of `source`.
    pub fn from_reader(source: R) -> Self {
        Self {
            inner: quick_xml::NsReader::from_reader(source),
            end: Vec::new(),
            writing: EmptyWriting::SelfClosed,
        }
    }

    /// The quick-xml reader, which delivers every event as written.
    pub fn verbatim(&mut self) -> &mut quick_xml::NsReader<R> {
        &mut self.inner
    }

    /// How the last [`Event::Empty`] read was written: valid until the next
    /// read, [`EmptyWriting::SelfClosed`] after any other event.
    pub fn empty_writing(&self) -> EmptyWriting {
        self.writing
    }
}

// The end tag of a collapsed element is read before the element is returned:
// quick-xml closes its namespace scope at the next read, as it does an
// empty-element tag's, so the element's own declarations still resolve its
// names until then.
impl<R: BufRead + Clone> NsReader<R> {
    /// The next event, read into `buf`, as
    /// [`quick_xml::NsReader::read_event_into`] reads it but for an element
    /// with nothing inside: one `Empty` event.
    pub fn read_event_into<'b>(&mut self, buf: &'b mut Vec<u8>) -> Result<Event<'b>> {
        self.writing = EmptyWriting::SelfClosed;
        let event = self.inner.read_event_into(buf)?;
        if !collapses(&event, self.inner.config(), self.inner.get_ref()) {
            return Ok(event);
        }
        self.end.clear();
        self.inner.read_event_into(&mut self.end)?;
        self.writing = EmptyWriting::EndTag;
        Ok(as_empty(event))
    }

    /// The next event, read into `buf`, and the namespace of its name, as
    /// [`quick_xml::NsReader::read_resolved_event_into`] reads them but for an
    /// element with nothing inside: one `Empty` event.
    pub fn read_resolved_event_into<'b>(
        &mut self,
        buf: &'b mut Vec<u8>,
    ) -> Result<(ResolveResult<'_>, Event<'b>)> {
        let event = self.read_event_into(buf)?;
        Ok(self.inner.resolver().resolve_event(event))
    }
}

impl<'i> NsReader<&'i [u8]> {
    /// A reader of `input`.
    #[allow(clippy::should_implement_trait)] // quick-xml's own name.
    pub fn from_str(input: &'i str) -> Self {
        Self::from_reader(input.as_bytes())
    }

    /// The next event, borrowed from the input, as
    /// [`quick_xml::NsReader::read_event`] reads it but for an element with
    /// nothing inside: one `Empty` event.
    pub fn read_event(&mut self) -> Result<Event<'i>> {
        self.writing = EmptyWriting::SelfClosed;
        let event = self.inner.read_event()?;
        if !collapses(&event, self.inner.config(), self.inner.get_ref()) {
            return Ok(event);
        }
        self.inner.read_event()?;
        self.writing = EmptyWriting::EndTag;
        Ok(as_empty(event))
    }

    /// The next event, borrowed from the input, and the namespace of its
    /// name, as [`quick_xml::NsReader::read_resolved_event`] reads them but
    /// for an element with nothing inside: one `Empty` event.
    pub fn read_resolved_event(&mut self) -> Result<(ResolveResult<'_>, Event<'i>)> {
        let event = self.read_event()?;
        Ok(self.inner.resolver().resolve_event(event))
    }
}

impl<R> Deref for NsReader<R> {
    type Target = quick_xml::NsReader<R>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<R> DerefMut for NsReader<R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw_xml::{capture_empty, write_empty};
    use EmptyWriting::{EndTag, SelfClosed};

    fn kind(event: &Event<'_>) -> &'static str {
        match event {
            Event::Start(_) => "start",
            Event::Empty(_) => "empty",
            _ => "other",
        }
    }

    /// The first event of `xml`: its kind, its writing and where the reader
    /// is after it, the same by `read_event` and by `read_event_into`.
    fn first_event(xml: &str, expand: bool) -> (&'static str, EmptyWriting, u64) {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().expand_empty_elements = expand;
        let event = reader.read_event().unwrap();
        let read = (
            kind(&event),
            reader.empty_writing(),
            reader.buffer_position(),
        );
        let mut reader = Reader::from_reader(xml.as_bytes());
        reader.config_mut().expand_empty_elements = expand;
        let mut buf = Vec::new();
        let event = reader.read_event_into(&mut buf).unwrap();
        let read_into = (
            kind(&event),
            reader.empty_writing(),
            reader.buffer_position(),
        );
        assert_eq!(read, read_into, "{xml}");
        read
    }

    #[test]
    fn an_element_with_nothing_inside_reads_as_one_empty_event_in_both_writings() {
        for (xml, expand, read) in [
            ("<x></x>", false, ("empty", EndTag, 7)),
            (r#"<p:x a="1"></p:x >"#, false, ("empty", EndTag, 18)),
            ("<x/>", false, ("empty", SelfClosed, 4)),
            // Content, even whitespace, keeps the element as written.
            ("<x> </x>", false, ("start", SelfClosed, 3)),
            ("<x><!--c--></x>", false, ("start", SelfClosed, 3)),
            ("<x>&amp;</x>", false, ("start", SelfClosed, 3)),
            ("<x><![CDATA[]]></x>", false, ("start", SelfClosed, 3)),
            ("<x><y/></x>", false, ("start", SelfClosed, 3)),
            ("<x></x>", true, ("start", SelfClosed, 3)),
        ] {
            assert_eq!(first_event(xml, expand), read, "{xml}");
        }
    }

    #[test]
    fn a_collapsed_element_resolves_its_name_from_its_own_declarations() {
        let xml = r#"<a:x xmlns:a="urn:x"></a:x><a:y/>"#;
        let mut reader = NsReader::from_str(xml);
        let (namespace, event) = reader.read_resolved_event().unwrap();
        assert!(matches!(event, Event::Empty(_)));
        assert_eq!(
            namespace,
            ResolveResult::Bound(quick_xml::name::Namespace(b"urn:x"))
        );
        assert_eq!(reader.empty_writing(), EndTag);
        // The scope of `a:x` closed with it: `a` is unbound for `a:y`.
        let (namespace, _) = reader.read_resolved_event().unwrap();
        assert!(matches!(namespace, ResolveResult::Unknown(_)));
        assert_eq!(reader.empty_writing(), SelfClosed);
        let mut reader = NsReader::from_reader(xml.as_bytes());
        let mut buf = Vec::new();
        let (namespace, _) = reader.read_resolved_event_into(&mut buf).unwrap();
        assert_eq!(
            namespace,
            ResolveResult::Bound(quick_xml::name::Namespace(b"urn:x"))
        );
    }

    #[test]
    fn an_empty_element_is_captured_and_written_back_in_its_writing() {
        for xml in [r#"<w:b w:val="1"/>"#, r#"<w:b w:val="1"></w:b>"#] {
            let mut reader = Reader::from_str(xml);
            let Event::Empty(element) = reader.read_event().unwrap() else {
                panic!("{xml} reads as one empty event");
            };
            let writing = reader.empty_writing();
            assert_eq!(capture_empty(&element, writing).unwrap(), xml.as_bytes());
            let mut writer = quick_xml::Writer::new_with_indent(Vec::new(), b' ', 2);
            write_empty(&mut writer, &element, writing).unwrap();
            assert_eq!(writer.into_inner(), xml.as_bytes());
        }
    }
}
