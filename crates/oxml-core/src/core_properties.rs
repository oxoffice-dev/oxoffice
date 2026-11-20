//! Dublin Core metadata from `docProps/core.xml`.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::Result;
use crate::xml::local_name;

/// Document metadata from `docProps/core.xml` (Dublin Core).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoreProperties {
    /// Document title (`dc:title`).
    pub title: Option<String>,
    /// Document creator/author (`dc:creator`).
    pub creator: Option<String>,
    /// Subject (`dc:subject`).
    pub subject: Option<String>,
    /// Description/comments (`dc:description`).
    pub description: Option<String>,
    /// Keywords (`cp:keywords`).
    pub keywords: Option<String>,
    /// Last modified by (`cp:lastModifiedBy`).
    pub last_modified_by: Option<String>,
    /// Date created (`dcterms:created`).
    pub created: Option<String>,
    /// Date modified (`dcterms:modified`).
    pub modified: Option<String>,
    /// Category (`cp:category`).
    pub category: Option<String>,
    /// Content status, such as "Draft" (`cp:contentStatus`).
    pub content_status: Option<String>,
    /// Identifier (`dc:identifier`).
    pub identifier: Option<String>,
    /// Language (`dc:language`).
    pub language: Option<String>,
    /// Date last printed (`cp:lastPrinted`).
    pub last_printed: Option<String>,
    /// Revision number (`cp:revision`).
    pub revision: Option<String>,
    /// Version (`cp:version`).
    pub version: Option<String>,
}

impl CoreProperties {
    /// Parse `docProps/core.xml` content.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut props = CoreProperties::default();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let field = match local_name(name.as_ref()) {
                        b"title" => &mut props.title,
                        b"creator" => &mut props.creator,
                        b"subject" => &mut props.subject,
                        b"description" => &mut props.description,
                        b"keywords" => &mut props.keywords,
                        b"lastModifiedBy" => &mut props.last_modified_by,
                        b"created" => &mut props.created,
                        b"modified" => &mut props.modified,
                        b"category" => &mut props.category,
                        b"contentStatus" => &mut props.content_status,
                        b"identifier" => &mut props.identifier,
                        b"language" => &mut props.language,
                        b"lastPrinted" => &mut props.last_printed,
                        b"revision" => &mut props.revision,
                        b"version" => &mut props.version,
                        _ => {
                            buf.clear();
                            continue;
                        }
                    };
                    // Consume the whole element: a value containing an entity
                    // arrives as several events, so a single Text event is not
                    // enough to reconstruct it.
                    let text = crate::xml_text::read_element_text(&mut reader, name);
                    if !text.is_empty() {
                        *field = Some(text);
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(props)
    }

    /// Serialize to `docProps/core.xml` bytes.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        use quick_xml::Writer;
        use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText};

        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;

        let mut root = BytesStart::new("cp:coreProperties");
        root.push_attribute((
            "xmlns:cp",
            "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
        ));
        root.push_attribute(("xmlns:dc", "http://purl.org/dc/elements/1.1/"));
        root.push_attribute(("xmlns:dcterms", "http://purl.org/dc/terms/"));
        root.push_attribute(("xmlns:dcmitype", "http://purl.org/dc/dcmitype/"));
        root.push_attribute(("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"));
        writer.write_event(Event::Start(root))?;

        fn write_element<W: std::io::Write>(
            writer: &mut Writer<W>,
            tag: &str,
            value: &Option<String>,
        ) -> Result<()> {
            if let Some(val) = value {
                writer.write_event(Event::Start(BytesStart::new(tag)))?;
                writer.write_event(Event::Text(BytesText::new(val)))?;
                writer.write_event(Event::End(BytesEnd::new(tag)))?;
            }
            Ok(())
        }

        fn write_date_element<W: std::io::Write>(
            writer: &mut Writer<W>,
            tag: &str,
            value: &Option<String>,
        ) -> Result<()> {
            if let Some(val) = value {
                let mut e = BytesStart::new(tag);
                e.push_attribute(("xsi:type", "dcterms:W3CDTF"));
                writer.write_event(Event::Start(e))?;
                writer.write_event(Event::Text(BytesText::new(val)))?;
                writer.write_event(Event::End(BytesEnd::new(tag)))?;
            }
            Ok(())
        }

        write_element(&mut writer, "dc:title", &self.title)?;
        write_element(&mut writer, "dc:subject", &self.subject)?;
        write_element(&mut writer, "dc:creator", &self.creator)?;
        write_element(&mut writer, "cp:keywords", &self.keywords)?;
        write_element(&mut writer, "dc:description", &self.description)?;
        write_element(&mut writer, "cp:lastModifiedBy", &self.last_modified_by)?;
        write_element(&mut writer, "cp:revision", &self.revision)?;
        write_element(&mut writer, "cp:lastPrinted", &self.last_printed)?;
        write_date_element(&mut writer, "dcterms:created", &self.created)?;
        write_date_element(&mut writer, "dcterms:modified", &self.modified)?;
        write_element(&mut writer, "cp:category", &self.category)?;
        write_element(&mut writer, "cp:contentStatus", &self.content_status)?;
        write_element(&mut writer, "dc:identifier", &self.identifier)?;
        write_element(&mut writer, "dc:language", &self.language)?;
        write_element(&mut writer, "cp:version", &self.version)?;

        writer.write_event(Event::End(BytesEnd::new("cp:coreProperties")))?;

        Ok(writer.into_inner())
    }
}
