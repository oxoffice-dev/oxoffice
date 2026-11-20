//! Parsing and writing of `[Content_Types].xml`.

use std::collections::{HashMap, HashSet};

use quick_xml::encoding::Decoder;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use quick_xml::{Writer, XmlVersion};

use crate::error::{OpcError, Result};

pub const RELATIONSHIPS: &str = "application/vnd.openxmlformats-package.relationships+xml";
pub const XML: &str = "application/xml";
const CONTENT_TYPES_NAMESPACE: &[u8] =
    b"http://schemas.openxmlformats.org/package/2006/content-types";

pub const CORE_PROPERTIES: &str = "application/vnd.openxmlformats-package.core-properties+xml";
pub const EXTENDED_PROPERTIES: &str =
    "application/vnd.openxmlformats-officedocument.extended-properties+xml";
pub const CUSTOM_PROPERTIES: &str =
    "application/vnd.openxmlformats-officedocument.custom-properties+xml";
pub const THEME: &str = "application/vnd.openxmlformats-officedocument.theme+xml";
pub const CHART: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
pub const WORD_GLOSSARY: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.glossary+xml";
pub const WORD_DOCUMENT: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
pub const WORD_DOCUMENT_MACRO_ENABLED: &str =
    "application/vnd.ms-word.document.macroEnabled.main+xml";
pub const WORD_TEMPLATE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml";
pub const WORD_TEMPLATE_MACRO_ENABLED: &str =
    "application/vnd.ms-word.template.macroEnabledTemplate.main+xml";

pub const PRESENTATION: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
pub const PRESENTATION_MACRO_ENABLED: &str =
    "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml";
pub const PRESENTATION_TEMPLATE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.template.main+xml";
pub const PRESENTATION_TEMPLATE_MACRO_ENABLED: &str =
    "application/vnd.ms-powerpoint.template.macroEnabled.main+xml";
pub const SLIDESHOW: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml";
pub const SLIDESHOW_MACRO_ENABLED: &str =
    "application/vnd.ms-powerpoint.slideshow.macroEnabled.main+xml";
pub const SLIDE: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";
pub const SLIDE_LAYOUT: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml";
pub const SLIDE_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml";
pub const NOTES_SLIDE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml";
pub const NOTES_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml";
pub const PRES_PROPS: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presProps+xml";
pub const VIEW_PROPS: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.viewProps+xml";
pub const TABLE_STYLES: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml";
pub const HANDOUT_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.handoutMaster+xml";
pub const POWERPOINT_COMMENTS: &str = "application/vnd.ms-powerpoint.comments+xml";
pub const POWERPOINT_AUTHORS: &str = "application/vnd.ms-powerpoint.authors+xml";

pub const WORKBOOK: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
pub const EMBEDDED_WORKBOOK: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
pub const WORKSHEET: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
pub const SHARED_STRINGS: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml";
pub const STYLES: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml";

/// A single content type entry, either a Default by extension or an Override by part name.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentType {
    Default {
        extension: String,
        content_type: String,
    },
    Override {
        part_name: String,
        content_type: String,
    },
}

/// Parsed `[Content_Types].xml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentTypes {
    pub defaults: HashMap<String, String>,
    pub overrides: HashMap<String, String>,
}

impl ContentTypes {
    /// Parse from XML bytes.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = NsReader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut defaults = HashMap::new();
        let mut overrides = HashMap::new();
        let mut default_identities = HashSet::new();
        let mut override_identities = HashSet::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_resolved_event_into(&mut buf) {
                Ok((ResolveResult::Bound(value), Event::Empty(ref e)))
                    if value.as_ref() == CONTENT_TYPES_NAMESPACE =>
                {
                    parse_content_type_entry(
                        e,
                        reader.decoder(),
                        &mut defaults,
                        &mut overrides,
                        &mut default_identities,
                        &mut override_identities,
                    )?;
                }
                Ok((ResolveResult::Bound(value), Event::Start(ref e)))
                    if value.as_ref() == CONTENT_TYPES_NAMESPACE =>
                {
                    if parse_content_type_entry(
                        e,
                        reader.decoder(),
                        &mut defaults,
                        &mut overrides,
                        &mut default_identities,
                        &mut override_identities,
                    )? {
                        reader.read_to_end_into(e.name(), &mut Vec::new())?;
                    }
                }
                Ok((_, Event::Eof)) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(ContentTypes {
            defaults,
            overrides,
        })
    }

    /// Serialize to XML bytes.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        self.validate_identities()?;
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;

        let mut types_start = BytesStart::new("Types");
        types_start.push_attribute((
            "xmlns",
            "http://schemas.openxmlformats.org/package/2006/content-types",
        ));
        writer.write_event(Event::Start(types_start))?;

        // Write defaults sorted for deterministic output
        let mut sorted_defaults: Vec<_> = self.defaults.iter().collect();
        sorted_defaults.sort_by_key(|(k, _)| (*k).clone());
        for (ext, ct) in sorted_defaults {
            let mut elem = BytesStart::new("Default");
            elem.push_attribute(("Extension", ext.as_str()));
            elem.push_attribute(("ContentType", ct.as_str()));
            writer.write_event(Event::Empty(elem))?;
        }

        // Write overrides sorted for deterministic output
        let mut sorted_overrides: Vec<_> = self.overrides.iter().collect();
        sorted_overrides.sort_by_key(|(k, _)| (*k).clone());
        for (pn, ct) in sorted_overrides {
            let mut elem = BytesStart::new("Override");
            elem.push_attribute(("PartName", pn.as_str()));
            elem.push_attribute(("ContentType", ct.as_str()));
            writer.write_event(Event::Empty(elem))?;
        }

        writer.write_event(Event::End(BytesEnd::new("Types")))?;

        Ok(writer.into_inner())
    }

    /// Look up the content type for a given part name.
    pub fn content_type_for(&self, part_name: &str) -> Option<&str> {
        // Check overrides first
        if let Some(content_type) = self.override_for(part_name) {
            return Some(content_type);
        }
        // Fall back to defaults by extension
        if let Some(dot_pos) = part_name.rfind('.') {
            let ext = &part_name[dot_pos + 1..];
            if let Some((_, ct)) = deterministic_identity_match(&self.defaults, ext) {
                return Some(ct.as_str());
            }
        }
        None
    }

    /// Look up the exact override for a specific part name.
    pub fn override_for(&self, part_name: &str) -> Option<&str> {
        deterministic_identity_match(&self.overrides, part_name)
            .map(|(_, content_type)| content_type.as_str())
    }

    /// Add a default content type for an extension (e.g., "png" -> "image/png").
    pub fn add_default(&mut self, extension: &str, content_type: &str) {
        if !self.contains_default(extension) {
            self.defaults
                .insert(extension.to_string(), content_type.to_string());
        }
    }

    /// Add an override content type for a specific part name.
    pub fn add_override(&mut self, part_name: &str, content_type: &str) {
        let mut matching = self
            .overrides
            .keys()
            .filter(|candidate| candidate.eq_ignore_ascii_case(part_name))
            .cloned()
            .collect::<Vec<_>>();
        matching.sort_unstable();
        let stored_identity = matching
            .first()
            .cloned()
            .unwrap_or_else(|| part_name.to_string());
        for duplicate in matching.into_iter().skip(1) {
            self.overrides.remove(&duplicate);
        }
        self.overrides
            .insert(stored_identity, content_type.to_string());
    }

    /// Return whether a specific part-name override exists.
    pub fn contains_override(&self, part_name: &str) -> bool {
        self.overrides
            .keys()
            .any(|candidate| candidate.eq_ignore_ascii_case(part_name))
    }

    /// Remove and return a specific part-name override.
    pub fn remove_override(&mut self, part_name: &str) -> Option<String> {
        let mut matching = self
            .overrides
            .keys()
            .filter(|candidate| candidate.eq_ignore_ascii_case(part_name))
            .cloned()
            .collect::<Vec<_>>();
        matching.sort_unstable();
        let first = matching.first()?.clone();
        let value = self.overrides.remove(&first);
        for duplicate in matching.into_iter().skip(1) {
            self.overrides.remove(&duplicate);
        }
        value
    }

    /// Return whether a default exists for an extension.
    pub fn contains_default(&self, extension: &str) -> bool {
        self.defaults
            .keys()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    }

    /// Remove and return a default for an extension.
    pub fn remove_default(&mut self, extension: &str) -> Option<String> {
        let mut matching = self
            .defaults
            .keys()
            .filter(|candidate| candidate.eq_ignore_ascii_case(extension))
            .cloned()
            .collect::<Vec<_>>();
        matching.sort_unstable();
        let first = matching.first()?.clone();
        let value = self.defaults.remove(&first);
        for duplicate in matching.into_iter().skip(1) {
            self.defaults.remove(&duplicate);
        }
        value
    }

    pub(crate) fn validate_identities(&self) -> Result<()> {
        validate_identity_map(&self.defaults)?;
        validate_identity_map(&self.overrides)
    }

    /// Create the minimal content types shared by every OPC package.
    pub fn minimal() -> Self {
        let mut defaults = HashMap::new();
        defaults.insert("rels".to_string(), RELATIONSHIPS.to_string());
        defaults.insert("xml".to_string(), XML.to_string());

        ContentTypes {
            defaults,
            overrides: HashMap::new(),
        }
    }
}

fn parse_content_type_entry(
    element: &BytesStart<'_>,
    decoder: Decoder,
    defaults: &mut HashMap<String, String>,
    overrides: &mut HashMap<String, String>,
    default_identities: &mut HashSet<String>,
    override_identities: &mut HashSet<String>,
) -> Result<bool> {
    let (identity_name, value_name, target, identities) = match element.local_name().as_ref() {
        b"Default" => (
            b"Extension".as_slice(),
            b"ContentType".as_slice(),
            defaults,
            default_identities,
        ),
        b"Override" => (
            b"PartName".as_slice(),
            b"ContentType".as_slice(),
            overrides,
            override_identities,
        ),
        _ => return Ok(false),
    };
    let mut identity = None;
    let mut value = None;
    for attribute in element.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == identity_name {
            identity = Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?
                    .into_owned(),
            );
        } else if attribute.key.as_ref() == value_name {
            value = Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?
                    .into_owned(),
            );
        }
    }
    let (Some(identity), Some(value)) = (identity, value) else {
        return Err(OpcError::InvalidContentTypes);
    };
    if !identities.insert(identity.to_ascii_lowercase()) {
        return Err(OpcError::InvalidContentTypes);
    }
    target.insert(identity, value);
    Ok(true)
}

fn deterministic_identity_match<'a>(
    values: &'a HashMap<String, String>,
    identity: &str,
) -> Option<(&'a String, &'a String)> {
    values
        .iter()
        .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(identity))
        .min_by(|(left, _), (right, _)| left.cmp(right))
}

fn validate_identity_map(values: &HashMap<String, String>) -> Result<()> {
    let mut identities = HashSet::with_capacity(values.len());
    for identity in values.keys() {
        if !identities.insert(identity.to_ascii_lowercase()) {
            return Err(OpcError::InvalidContentTypes);
        }
    }
    Ok(())
}
