//! Dublin Core metadata from `docProps/core.xml`.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::Result;
use crate::xml::{escape_text, local_name};

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
    /// Each field with the name messages give it. The struct is destructured
    /// without `..`, so a field added later has to be named here.
    pub fn named_fields(&self) -> [(&'static str, Option<&str>); 15] {
        let Self {
            title,
            creator,
            subject,
            description,
            keywords,
            last_modified_by,
            created,
            modified,
            category,
            content_status,
            identifier,
            language,
            last_printed,
            revision,
            version,
        } = self;
        [
            ("title", title.as_deref()),
            ("creator", creator.as_deref()),
            ("subject", subject.as_deref()),
            ("description", description.as_deref()),
            ("keywords", keywords.as_deref()),
            ("last modified by", last_modified_by.as_deref()),
            ("created", created.as_deref()),
            ("modified", modified.as_deref()),
            ("category", category.as_deref()),
            ("content status", content_status.as_deref()),
            ("identifier", identifier.as_deref()),
            ("language", language.as_deref()),
            ("last printed", last_printed.as_deref()),
            ("revision", revision.as_deref()),
            ("version", version.as_deref()),
        ]
    }

    /// Refuses a field holding a character XML 1.0 cannot carry, naming it,
    /// for a writer that writes every field again. A field equal to the one
    /// in `current` came from the file as read, and the message says so: the
    /// caller did not type that character, but has to clean that value.
    pub fn reject_non_xml_characters(&self, current: Option<&Self>) -> Result<()> {
        self.reject_fields(current, true)
    }

    /// Refuses a changed field holding a character XML 1.0 cannot carry,
    /// naming it, for a writer that keeps the bytes of an unchanged field: a
    /// field equal to the one in `current` stays as the file wrote it.
    pub fn reject_new_non_xml_characters(&self, current: Option<&Self>) -> Result<()> {
        self.reject_fields(current, false)
    }

    fn reject_fields(&self, current: Option<&Self>, check_unchanged: bool) -> Result<()> {
        let current = current.map(Self::named_fields);
        for (index, (name, value)) in self.named_fields().into_iter().enumerate() {
            let Some(value) = value else {
                continue;
            };
            let unchanged = current
                .as_ref()
                .is_some_and(|fields| fields[index].1 == Some(value));
            if unchanged && !check_unchanged {
                continue;
            }
            let what = if unchanged {
                format!("core property {name}, as the file holds it,")
            } else {
                format!("core property {name}")
            };
            crate::xml::reject_non_xml_characters(&what, value)?;
        }
        Ok(())
    }

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
                writer.write_event(Event::Text(BytesText::from_escaped(escape_text(val))))?;
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
                writer.write_event(Event::Text(BytesText::from_escaped(escape_text(val))))?;
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
