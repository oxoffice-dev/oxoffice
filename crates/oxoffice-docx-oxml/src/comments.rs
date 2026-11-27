//! Word comment elements: `CT_Comments` and `CT_Comment`.

use std::io::Write;

use oxoffice_core::xml_reader::Reader;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::{Writer, XmlVersion};

use crate::error::{OxmlError, Result};
use crate::namespace::{W_NS, matches_local_name};
use crate::numbering::word_prefixes_at;
use crate::properties::is_word_element;
use crate::raw_xml::{capture_element, capture_empty};
use crate::text::{CT_P, declare_w14_on_part_root};

const W14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordml";

/// One entry in a Word comments part.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Comment {
    pub id: i32,
    pub author: Option<String>,
    pub date: Option<String>,
    pub initials: Option<String>,
    pub paragraphs: Vec<CT_P>,
    /// `w14:paraId` values aligned with `paragraphs`.
    pub paragraph_ids: Vec<Option<String>>,
    /// Unmodelled attributes, including producer namespace declarations.
    pub extra_attributes: Vec<(String, String)>,
    /// Unmodelled children retained at paragraph boundaries.
    pub extra_xml: Vec<(usize, Vec<u8>)>,
}

/// The typed contents of `word/comments.xml`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CT_Comments {
    pub comments: Vec<CT_Comment>,
    /// Namespace declarations and compatibility attributes from the root.
    pub root_attributes: Vec<(String, String)>,
    /// Unmodelled root children retained at comment boundaries.
    pub extra_xml: Vec<(usize, Vec<u8>)>,
}

#[derive(Default)]
pub(crate) struct CommentsReadFacts {
    pub(crate) comments: Vec<CommentReadFacts>,
    pub(crate) raws: Vec<Option<crate::document::WordSourceMap>>,
}

#[derive(Default)]
pub(crate) struct CommentReadFacts {
    pub(crate) paragraphs: Vec<(usize, crate::text::WordBlockSourceMaps)>,
    pub(crate) raws: Vec<Option<crate::document::WordSourceMap>>,
}

impl CT_Comments {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a complete comments part.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let admitted = crate::document::admit_reader_source(
            xml,
            crate::document::WordSourceReader::Part(crate::document::PartReaderKind::Comments),
        )?;
        let mut controls = admitted.controls_for(xml);
        Self::from_xml_with_read_output(xml, &mut controls, None)
    }

    pub(crate) fn from_xml_with_read_output(
        xml: &[u8],
        controls: &mut crate::document::ControlSourceCursor<'_>,
        read_output: Option<&mut CommentsReadFacts>,
    ) -> Result<Self> {
        let mut pending = CommentsReadFacts::default();
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);
        let mut comments = Vec::new();
        let mut root_attributes = Vec::new();
        let mut extra_xml = Vec::new();
        let mut word_prefixes = Vec::new();
        let mut w14_prefixes = Vec::new();
        let mut saw_root = false;
        let mut buf = Vec::new();

        loop {
            let event_start = reader.buffer_position();
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref element)) => {
                    let prefixes = word_prefixes_at(element, &word_prefixes)?;
                    let name = element.name();
                    if is_word_element(name.as_ref(), b"comments", &prefixes) {
                        root_attributes = capture_attributes(element, &[], &prefixes)?;
                        word_prefixes = prefixes.into_owned();
                        w14_prefixes = namespace_prefixes_at(element, &w14_prefixes, W14_NS)?;
                        saw_root = true;
                    } else if is_word_element(name.as_ref(), b"comment", &prefixes) {
                        let mut child_read = CommentReadFacts::default();
                        let comment = parse_comment(
                            &mut reader,
                            element,
                            &prefixes,
                            &w14_prefixes,
                            controls,
                            read_output.as_ref().map(|_| &mut child_read),
                        )?;
                        comments.push(comment);
                        if read_output.is_some() {
                            pending.comments.push(child_read);
                        }
                    } else if saw_root {
                        let (raw, map) = if read_output.is_some() {
                            let (raw, input) =
                                controls.capture_element(&mut reader, element, event_start)?;
                            (raw, input.map(|input| input.projection_parts().0.clone()))
                        } else {
                            (capture_element(&mut reader, element)?, None)
                        };
                        extra_xml.push((comments.len(), raw));
                        if read_output.is_some() {
                            pending.raws.push(map);
                        }
                    } else {
                        reader.read_to_end_into(name, &mut Vec::new())?;
                    }
                }
                Ok(Event::Empty(ref element)) => {
                    let prefixes = word_prefixes_at(element, &word_prefixes)?;
                    let name = element.name();
                    if is_word_element(name.as_ref(), b"comments", &prefixes) {
                        root_attributes = capture_attributes(element, &[], &prefixes)?;
                        w14_prefixes = namespace_prefixes_at(element, &w14_prefixes, W14_NS)?;
                        saw_root = true;
                    } else if is_word_element(name.as_ref(), b"comment", &prefixes) {
                        comments.push(parse_empty_comment(element, &prefixes)?);
                        if read_output.is_some() {
                            pending.comments.push(CommentReadFacts::default());
                        }
                    } else if saw_root {
                        let raw = capture_empty(element, reader.empty_writing())?;
                        if read_output.is_some() {
                            pending.raws.push(
                                controls
                                    .field_input(
                                        &reader,
                                        event_start..reader.buffer_position(),
                                        &raw,
                                    )
                                    .map(|input| input.projection_parts().0.clone()),
                            );
                        }
                        extra_xml.push((comments.len(), raw));
                    }
                }
                Ok(Event::Eof) => break,
                Err(error) => return Err(error.into()),
                _ => {}
            }
            buf.clear();
        }

        if !saw_root {
            return Err(OxmlError::MissingElement("comments root".to_owned()));
        }
        if let Some(output) = read_output {
            *output = pending;
        }
        Ok(Self {
            comments,
            root_attributes,
            extra_xml,
        })
    }

    /// Serialize with fixed WordprocessingML prefixes and schema child order.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        self.validate_writer()?;
        let mut writer = Writer::new(Vec::new());
        self.write_xml_in(&mut writer, None)?;
        let mut xml = writer.into_inner();
        declare_w14_on_part_root(&mut xml)?;
        Ok(xml)
    }

    pub(crate) fn validate_writer(&self) -> Result<()> {
        validate_writer_bindings(&self.root_attributes, "comments root")?;
        for (comment_index, comment) in self.comments.iter().enumerate() {
            validate_writer_bindings(
                &comment.extra_attributes,
                &format!("comment {comment_index}"),
            )?;
            for (paragraph_index, paragraph) in comment.paragraphs.iter().enumerate() {
                for (position, raw) in &paragraph.extra_xml {
                    if CT_P::raw_is_root_attributes(*position, raw) {
                        let mut reader = Reader::from_reader(raw.as_slice());
                        loop {
                            match reader.read_event()? {
                                Event::Start(start) | Event::Empty(start) => {
                                    validate_writer_bindings(
                                        &capture_attributes(&start, &[], &[])?,
                                        &format!(
                                            "comment {comment_index} paragraph {paragraph_index}"
                                        ),
                                    )?;
                                    break;
                                }
                                Event::Eof => break,
                                _ => {}
                            }
                        }
                    }
                }
                paragraph.validate_resource_limits()?;
            }
        }
        Ok(())
    }

    pub(crate) fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        mut work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<()> {
        if let Some(work) = work.as_deref_mut() {
            let mut fields = Vec::new();
            for comment in &self.comments {
                for paragraph in &comment.paragraphs {
                    fields.extend(paragraph.writer_field_roots());
                    fields.extend(
                        crate::text::ordered_normalization_children(paragraph)?
                            .into_iter()
                            .map(|(field, _)| field),
                    );
                }
            }
            work.register_original_fields(fields)?;
        }
        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;

        // Retained root attributes keep their source order. The fixed `w` and
        // `w14` declarations are added ahead of them only when the source
        // lacks them, so a producer declaration listed in `mc:Ignorable`
        // stays on the root whether or not a paragraph id still uses it.
        let declared = |name: &str| {
            self.root_attributes
                .iter()
                .any(|(candidate, _)| candidate == name)
        };
        let mut root = BytesStart::new("w:comments");
        if !declared("xmlns:w") {
            root.push_attribute(("xmlns:w", W_NS));
        }
        if !declared("xmlns:w14")
            && self
                .comments
                .iter()
                .any(|comment| comment.paragraph_ids.iter().any(Option::is_some))
        {
            root.push_attribute(("xmlns:w14", W14_NS));
        }
        push_preserved_attributes(&mut root, &self.root_attributes);
        let _binding_scope = crate::text::emitted_root_binding_scope(&root, 0, true)?;
        writer.write_event(Event::Start(root))?;

        write_raw_at(writer, &self.extra_xml, 0)?;
        for (index, comment) in self.comments.iter().enumerate() {
            write_comment(writer, comment, work.as_deref_mut())?;
            write_raw_at(writer, &self.extra_xml, index + 1)?;
        }
        writer.write_event(Event::End(BytesEnd::new("w:comments")))?;
        Ok(())
    }
}

fn parse_comment(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart<'_>,
    prefixes: &[String],
    inherited_w14_prefixes: &[String],
    controls: &mut crate::document::ControlSourceCursor<'_>,
    mut read_output: Option<&mut CommentReadFacts>,
) -> Result<CT_Comment> {
    let (id, author, date, initials, extra_attributes) = comment_attributes(start, prefixes)?;
    let mut paragraphs = Vec::new();
    let mut paragraph_ids = Vec::new();
    let mut extra_xml = Vec::new();
    let w14_prefixes = namespace_prefixes_at(start, inherited_w14_prefixes, W14_NS)?;
    let mut buf = Vec::new();

    loop {
        let event_start = reader.buffer_position();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref element)) => {
                let child_prefixes = word_prefixes_at(element, prefixes)?;
                if is_word_element(element.name().as_ref(), b"p", &child_prefixes) {
                    let child_w14_prefixes = namespace_prefixes_at(element, &w14_prefixes, W14_NS)?;
                    paragraph_ids.push(namespace_attribute(
                        element,
                        b"paraId",
                        &child_w14_prefixes,
                    )?);
                    let mut extras = Vec::new();
                    let mut texts = Vec::new();
                    let mut maps = crate::text::WordBlockSourceMaps::default();
                    let paragraph = CT_P::from_xml_with_read_output(
                        reader,
                        &child_prefixes,
                        Some(element),
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
                        output.paragraphs.push((paragraphs.len(), maps));
                    }
                    paragraphs.push(paragraph);
                } else {
                    let (raw, map) = if read_output.is_some() {
                        let (raw, input) =
                            controls.capture_element(reader, element, event_start)?;
                        (raw, input.map(|input| input.projection_parts().0.clone()))
                    } else {
                        (capture_element(reader, element)?, None)
                    };
                    extra_xml.push((paragraphs.len(), raw));
                    if let Some(output) = read_output.as_deref_mut() {
                        output.raws.push(map);
                    }
                }
            }
            Ok(Event::Empty(ref element)) => {
                let child_prefixes = word_prefixes_at(element, prefixes)?;
                if is_word_element(element.name().as_ref(), b"p", &child_prefixes) {
                    let child_w14_prefixes = namespace_prefixes_at(element, &w14_prefixes, W14_NS)?;
                    paragraph_ids.push(namespace_attribute(
                        element,
                        b"paraId",
                        &child_w14_prefixes,
                    )?);
                    paragraphs.push(CT_P::from_empty_root(element, &child_prefixes)?);
                } else {
                    let raw = capture_empty(element, reader.empty_writing())?;
                    if let Some(output) = read_output.as_deref_mut() {
                        output.raws.push(
                            controls
                                .field_input(reader, event_start..reader.buffer_position(), &raw)
                                .map(|input| input.projection_parts().0.clone()),
                        );
                    }
                    extra_xml.push((paragraphs.len(), raw));
                }
            }
            Ok(Event::End(ref element))
                if matches_local_name(element.name().as_ref(), b"comment") =>
            {
                break;
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(error.into()),
            _ => {}
        }
        buf.clear();
    }

    Ok(CT_Comment {
        id,
        author,
        date,
        initials,
        paragraphs,
        paragraph_ids,
        extra_attributes,
        extra_xml,
    })
}

fn parse_empty_comment(start: &BytesStart<'_>, prefixes: &[String]) -> Result<CT_Comment> {
    let (id, author, date, initials, extra_attributes) = comment_attributes(start, prefixes)?;
    Ok(CT_Comment {
        id,
        author,
        date,
        initials,
        paragraphs: Vec::new(),
        paragraph_ids: Vec::new(),
        extra_attributes,
        extra_xml: Vec::new(),
    })
}

type CommentAttributes = (
    i32,
    Option<String>,
    Option<String>,
    Option<String>,
    Vec<(String, String)>,
);

fn comment_attributes(start: &BytesStart<'_>, prefixes: &[String]) -> Result<CommentAttributes> {
    let prefixes = word_prefixes_at(start, prefixes)?;
    let id = required_i32_attribute(start, b"id", &prefixes)?;
    let author = word_attribute(start, b"author", &prefixes)?;
    let date = word_attribute(start, b"date", &prefixes)?;
    let initials = word_attribute(start, b"initials", &prefixes)?;
    let extra_attributes =
        capture_attributes(start, &[b"id", b"author", b"date", b"initials"], &prefixes)?;
    Ok((id, author, date, initials, extra_attributes))
}

fn required_i32_attribute(
    start: &BytesStart<'_>,
    local: &[u8],
    prefixes: &[String],
) -> Result<i32> {
    let value = word_attribute(start, local, prefixes)?.ok_or_else(|| {
        OxmlError::MissingElement(format!(
            "comment {} attribute",
            String::from_utf8_lossy(local)
        ))
    })?;
    value.parse().map_err(OxmlError::from)
}

fn word_attribute(
    start: &BytesStart<'_>,
    local: &[u8],
    prefixes: &[String],
) -> Result<Option<String>> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        if is_word_attribute(attribute.key.as_ref(), local, prefixes) {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn is_word_attribute(key: &[u8], local: &[u8], prefixes: &[String]) -> bool {
    let Some(separator) = key.iter().position(|byte| *byte == b':') else {
        return false;
    };
    key.get(separator + 1..) == Some(local)
        && prefixes
            .iter()
            .any(|prefix| prefix.as_bytes() == &key[..separator])
}

fn capture_attributes(
    start: &BytesStart<'_>,
    modelled: &[&[u8]],
    prefixes: &[String],
) -> Result<Vec<(String, String)>> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute?;
        if modelled
            .iter()
            .any(|local| is_word_attribute(attribute.key.as_ref(), local, prefixes))
        {
            continue;
        }
        let name = std::str::from_utf8(attribute.key.as_ref())?.to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
            .into_owned();
        attributes.push((name, value));
    }
    Ok(attributes)
}

fn write_comment<W: Write>(
    writer: &mut Writer<W>,
    comment: &CT_Comment,
    mut work: Option<&mut crate::text::WordOperationWork>,
) -> Result<()> {
    let mut start = BytesStart::new("w:comment");
    if let Some(author) = &comment.author {
        oxoffice_core::xml::push_escaped_attribute(&mut start, "w:author", author);
    }
    if let Some(date) = &comment.date {
        start.push_attribute(("w:date", date.as_str()));
    }
    if let Some(initials) = &comment.initials {
        oxoffice_core::xml::push_escaped_attribute(&mut start, "w:initials", initials);
    }
    let mut id = itoa::Buffer::new();
    start.push_attribute(("w:id", id.format(comment.id)));
    push_preserved_attributes(&mut start, &comment.extra_attributes);
    writer.write_event(Event::Start(start))?;

    write_raw_at(writer, &comment.extra_xml, 0)?;
    for (index, paragraph) in comment.paragraphs.iter().enumerate() {
        let para_id = comment.paragraph_ids.get(index).and_then(Option::as_deref);
        if let Some(work) = work.as_deref_mut() {
            paragraph.write_xml_core_in(
                writer,
                para_id,
                crate::text::XmlWriteRole::Publication,
                None,
                work,
                None,
            )?;
        } else {
            paragraph.to_xml_with_para_id(writer, para_id)?;
        }
        write_raw_at(writer, &comment.extra_xml, index + 1)?;
    }
    writer.write_event(Event::End(BytesEnd::new("w:comment")))?;
    Ok(())
}

fn validate_writer_bindings(attributes: &[(String, String)], location: &str) -> Result<()> {
    for (name, value) in attributes {
        let required = match name.as_str() {
            "xmlns:w" => W_NS,
            "xmlns:w14" => W14_NS,
            _ => continue,
        };
        if value != required {
            return Err(OxmlError::InvalidValue(format!(
                "{location}: fixed writer prefix {} has foreign namespace {value}",
                name.trim_start_matches("xmlns:")
            )));
        }
    }
    Ok(())
}

fn push_preserved_attributes(start: &mut BytesStart<'_>, attributes: &[(String, String)]) {
    for (name, value) in attributes {
        oxoffice_core::xml::push_escaped_attribute(start, name, value);
    }
}

fn namespace_prefixes_at(
    start: &BytesStart<'_>,
    inherited: &[String],
    namespace: &str,
) -> Result<Vec<String>> {
    let mut prefixes = inherited.to_vec();
    for attribute in start.attributes() {
        let attribute = attribute?;
        let name = attribute.key.as_ref();
        let prefix = if name == b"xmlns" {
            b"".as_slice()
        } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
            prefix
        } else {
            continue;
        };
        let prefix = std::str::from_utf8(prefix)?.to_owned();
        prefixes.retain(|candidate| candidate != &prefix);
        let value =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?;
        if value.as_ref() == namespace {
            prefixes.push(prefix);
        }
    }
    Ok(prefixes)
}

fn namespace_attribute(
    start: &BytesStart<'_>,
    local: &[u8],
    prefixes: &[String],
) -> Result<Option<String>> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let Some(separator) = key.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if key.get(separator + 1..) == Some(local)
            && prefixes
                .iter()
                .any(|prefix| prefix.as_bytes() == &key[..separator])
        {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn write_raw_at<W: Write>(
    writer: &mut Writer<W>,
    extra_xml: &[(usize, Vec<u8>)],
    position: usize,
) -> Result<()> {
    for (at, raw) in extra_xml {
        if *at == position {
            writer.get_mut().write_all(raw)?;
        }
    }
    Ok(())
}
