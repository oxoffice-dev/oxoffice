//! Footnote and endnote elements: `CT_Footnotes`, `CT_Footnote`.

use oxoffice_core::xml_reader::Reader;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::{Writer, XmlVersion};

use crate::error::Result;
use crate::namespace::{W_NS, matches_local_name};
use crate::numbering::word_prefixes_at;
use crate::properties::{is_word_attribute, is_word_element};
use crate::text::{CT_P, ROOT_R_BINDING, declare_w14_on_part_root};

/// `ST_FtnEdn` — what a note in the stream is for.
///
/// The stream holds the document's real notes alongside the separator marks
/// Word draws above them. The distinction is carried by `w:type`, not by the
/// id: the conventional ids 0 and 1 are a convention, not a guarantee, and
/// reading a `continuationSeparator` as if it were note number 1 is how a
/// separator ends up rendered as body content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NoteType {
    /// A real note, the only kind a reference can resolve to.
    #[default]
    Normal,
    /// The rule drawn above the notes on a page.
    Separator,
    /// The rule drawn above a note carried over from the previous page.
    ContinuationSeparator,
    /// The notice Word can place when a note continues.
    ContinuationNotice,
}

impl NoteType {
    pub(crate) fn from_str(s: &str) -> Self {
        match s {
            "separator" => NoteType::Separator,
            "continuationSeparator" => NoteType::ContinuationSeparator,
            "continuationNotice" => NoteType::ContinuationNotice,
            _ => NoteType::Normal,
        }
    }

    fn to_str(self) -> Option<&'static str> {
        match self {
            NoteType::Normal => None,
            NoteType::Separator => Some("separator"),
            NoteType::ContinuationSeparator => Some("continuationSeparator"),
            NoteType::ContinuationNotice => Some("continuationNotice"),
        }
    }
}

/// A single footnote or endnote.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Footnote {
    /// Footnote ID (matches w:footnoteReference w:id in the document body).
    pub id: i32,
    /// What this entry is for. Separators are retained so a round trip does
    /// not discard them, and filtered out of `get_by_id` so no reference can
    /// resolve to one.
    pub note_type: NoteType,
    /// Paragraphs making up the footnote content.
    pub paragraphs: Vec<CT_P>,
}

/// Collection of footnotes parsed from `word/footnotes.xml`.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Footnotes {
    pub footnotes: Vec<CT_Footnote>,
}

#[allow(non_snake_case)]
impl CT_Footnotes {
    pub fn new() -> Self {
        CT_Footnotes {
            footnotes: Vec::new(),
        }
    }

    /// Get a real note by its ID.
    ///
    /// Separator entries are never returned. A reference can only ever mean a
    /// real note, and some documents place a `continuationSeparator` at an id
    /// a reference could otherwise collide with.
    pub fn get_by_id(&self, id: i32) -> Option<&CT_Footnote> {
        self.footnotes
            .iter()
            .find(|f| f.id == id && f.note_type == NoteType::Normal)
    }

    /// Whether the stream defines the rule drawn above a carried-over note.
    pub fn has_continuation_separator(&self) -> bool {
        self.footnotes
            .iter()
            .any(|f| f.note_type == NoteType::ContinuationSeparator)
    }

    /// Parse from XML bytes (the content of footnotes.xml).
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let admitted = crate::document::admit_reader_source(
            xml,
            crate::document::WordSourceReader::Part(crate::document::PartReaderKind::Notes),
        )?;
        let mut controls = admitted.controls_for(xml);
        Self::from_xml_with_read_output(xml, &mut controls, None)
    }

    pub(crate) fn from_xml_with_read_output(
        xml: &[u8],
        controls: &mut crate::document::ControlSourceCursor<'_>,
        read_output: Option<&mut Vec<Vec<(usize, crate::text::WordBlockSourceMaps)>>>,
    ) -> Result<Self> {
        let mut retained_reads = Vec::new();
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut footnotes = Vec::new();
        let mut buf = Vec::new();
        let mut word_prefixes = Vec::new();

        loop {
            let event = reader.read_event_into(&mut buf);
            // A note with nothing inside, `<w:footnote w:id="3"/>`, is a note.
            let empty = matches!(event, Ok(Event::Empty(_)));
            match event {
                Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, &word_prefixes)?;
                    if is_word_element(name.as_ref(), b"footnote", &prefixes)
                        || is_word_element(name.as_ref(), b"endnote", &prefixes)
                    {
                        let mut id: i32 = 0;
                        let mut note_type = NoteType::Normal;
                        let mut raw_type = None;
                        for attr in e.attributes().flatten() {
                            if is_word_attribute(attr.key.as_ref(), b"id", &prefixes) {
                                id = attr
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        e.decoder(),
                                    )?
                                    .parse()
                                    .unwrap_or(0);
                            } else if is_word_attribute(attr.key.as_ref(), b"type", &prefixes) {
                                let value = attr.decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    e.decoder(),
                                )?;
                                note_type = NoteType::from_str(&value);
                                raw_type = Some(value.into_owned());
                            }
                        }

                        // Omitted and explicit normal types are normal at every id.
                        // Retain only the historical read of invalid empty/unknown
                        // types at nonpositive ids. Comparison discriminates those
                        // retained raw states separately before allowing an edit.
                        if id <= 0
                            && note_type == NoteType::Normal
                            && raw_type.as_deref().is_some_and(|value| value != "normal")
                        {
                            note_type = NoteType::Separator;
                        }

                        // Separators are kept rather than dropped, so a round
                        // trip preserves them.
                        let mut reads = Vec::new();
                        let paragraphs = if empty {
                            Vec::new()
                        } else {
                            parse_footnote_content(
                                &mut reader,
                                &prefixes,
                                controls,
                                read_output.as_ref().map(|_| &mut reads),
                            )?
                        };
                        retained_reads.push(reads);
                        footnotes.push(CT_Footnote {
                            id,
                            note_type,
                            paragraphs,
                        });
                    } else if is_word_element(name.as_ref(), b"footnotes", &prefixes)
                        || is_word_element(name.as_ref(), b"endnotes", &prefixes)
                    {
                        // Continue into the root element
                        word_prefixes = prefixes.into_owned();
                    } else if !empty {
                        reader.read_to_end_into(name, &mut Vec::new())?;
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if let Some(output) = read_output {
            *output = retained_reads;
        }
        Ok(CT_Footnotes { footnotes })
    }

    /// Serialize to XML bytes as footnotes.
    pub fn to_xml_footnotes(&self) -> Result<Vec<u8>> {
        self.to_xml_root("w:footnotes", "w:footnote")
    }

    /// Serialize to XML bytes as endnotes.
    pub fn to_xml_endnotes(&self) -> Result<Vec<u8>> {
        self.to_xml_root("w:endnotes", "w:endnote")
    }

    fn to_xml_root(&self, root_tag: &str, item_tag: &str) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml_root_in(&mut writer, root_tag, item_tag, None)?;
        let mut xml = writer.into_inner();
        declare_w14_on_part_root(&mut xml)?;
        Ok(xml)
    }

    pub(crate) fn write_xml_root_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        root_tag: &str,
        item_tag: &str,
        work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<()> {
        Self::write_notes_root_in(
            &self.footnotes.iter().collect::<Vec<_>>(),
            writer,
            root_tag,
            item_tag,
            work,
        )
    }

    pub(crate) fn write_notes_root_in<W: std::io::Write>(
        notes: &[&CT_Footnote],
        writer: &mut Writer<W>,
        root_tag: &str,
        item_tag: &str,
        mut work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<()> {
        for note in notes {
            for paragraph in &note.paragraphs {
                paragraph.validate_resource_limits()?;
            }
        }

        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;

        let mut start = BytesStart::new(root_tag);
        start.push_attribute(("xmlns:w", W_NS));
        start.push_attribute((
            "xmlns:r",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        ));
        let _binding_scope = crate::text::emitted_root_binding_scope(&start, ROOT_R_BINDING, true)?;
        writer.write_event(Event::Start(start))?;

        let mut buf = itoa::Buffer::new();
        for footnote in notes {
            let mut fn_start = BytesStart::new(item_tag);
            // `w:type` precedes `w:id` in the schema's attribute listing, and
            // is omitted entirely for a normal note, which is what Word writes.
            if let Some(type_str) = footnote.note_type.to_str() {
                fn_start.push_attribute(("w:type", type_str));
            }
            fn_start.push_attribute(("w:id", buf.format(footnote.id)));
            writer.write_event(Event::Start(fn_start))?;

            for p in &footnote.paragraphs {
                if let Some(work) = work.as_deref_mut() {
                    p.write_xml_core_in(
                        writer,
                        None,
                        crate::text::XmlWriteRole::Publication,
                        None,
                        work,
                        None,
                    )?;
                } else {
                    p.to_xml(writer)?;
                }
            }

            writer.write_event(Event::End(BytesEnd::new(item_tag)))?;
        }

        writer.write_event(Event::End(BytesEnd::new(root_tag)))?;

        Ok(())
    }
}

impl Default for CT_Footnotes {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse the content of a single footnote/endnote (paragraphs until closing tag).
fn parse_footnote_content(
    reader: &mut Reader<&[u8]>,
    word_prefixes: &[String],
    controls: &mut crate::document::ControlSourceCursor<'_>,
    mut read_output: Option<&mut Vec<(usize, crate::text::WordBlockSourceMaps)>>,
) -> Result<Vec<CT_P>> {
    let mut paragraphs = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let prefixes = word_prefixes_at(e, word_prefixes)?;
                if is_word_element(name.as_ref(), b"p", &prefixes) {
                    let mut extras = Vec::new();
                    let mut texts = Vec::new();
                    let mut maps = crate::text::WordBlockSourceMaps::default();
                    let paragraph = CT_P::from_xml_with_read_output(
                        reader,
                        &prefixes,
                        Some(e),
                        Some(controls),
                        crate::text::WordParagraphReadOutput {
                            extra_maps: read_output.as_ref().map(|_| &mut extras),
                            text_facts: read_output.as_ref().map(|_| &mut texts),
                            nested_maps: read_output.as_ref().map(|_| &mut maps),
                            ..Default::default()
                        },
                    )?;
                    if let Some(output) = read_output.as_deref_mut() {
                        maps.paragraph(extras);
                        maps.word_texts
                            .push((Vec::new(), controls.paragraph_word_facts(reader, texts)));
                        output.push((paragraphs.len(), maps));
                    }
                    paragraphs.push(paragraph);
                } else {
                    reader.read_to_end_into(name, &mut Vec::new())?;
                }
            }
            Ok(Event::Empty(ref e)) => {
                let prefixes = word_prefixes_at(e, word_prefixes)?;
                if is_word_element(e.name().as_ref(), b"p", &prefixes) {
                    paragraphs.push(CT_P::from_empty_root(e, &prefixes)?);
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.name();
                if matches_local_name(name.as_ref(), b"footnote")
                    || matches_local_name(name.as_ref(), b"endnote")
                {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.into()),
            _ => {}
        }
        buf.clear();
    }

    Ok(paragraphs)
}
