//! Custom properties from `docProps/custom.xml`.

use std::io::Write;

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::{Reader, Writer, XmlVersion};

use crate::error::{OxmlError, Result};
use crate::raw_xml::{capture_element, capture_empty_element};
use crate::xml::{extra_namespace_declarations, local_name};
use crate::xml_text::read_element_text;

const CUSTOM_PROPERTIES_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties";
const VARIANT_TYPES_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes";

/// A typed value stored in a custom document property.
#[derive(Debug, Clone, PartialEq)]
pub enum CustomPropertyValue {
    /// An ANSI string (`vt:lpstr`).
    Lpstr(String),
    /// A Unicode string (`vt:lpwstr`).
    Lpwstr(String),
    /// A signed 32-bit integer (`vt:i4`).
    I4(i32),
    /// A 64-bit floating-point number (`vt:r8`).
    R8(f64),
    /// A Boolean (`vt:bool`).
    Bool(bool),
    /// An ISO 8601 file time (`vt:filetime`).
    FileTime(String),
    /// An explicitly empty value (`vt:empty`).
    Empty,
    /// An unsupported `vt:*` value preserved as its complete XML subtree.
    Raw(Vec<u8>),
}

impl CustomPropertyValue {
    /// Whether the two values are written the same. Unlike `==`, a NaN
    /// number is the same as itself, and `-0` is not the same as `0`.
    pub fn is_same_value(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::R8(left), Self::R8(right)) => r8_text(*left) == r8_text(*right),
            _ => self == other,
        }
    }
}

/// One property in `docProps/custom.xml`.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomProperty {
    pub fmtid: String,
    pub pid: i32,
    pub name: Option<String>,
    pub value: CustomPropertyValue,
}

/// The ordered custom-property collection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CustomProperties {
    pub properties: Vec<CustomProperty>,
    extra_namespaces: Vec<(String, String)>,
}

impl CustomProperties {
    /// Whether the two collections are written the same: as `==`, except
    /// that values are compared with [`CustomPropertyValue::is_same_value`].
    pub fn is_same_as(&self, other: &Self) -> bool {
        self.extra_namespaces == other.extra_namespaces
            && self.properties.len() == other.properties.len()
            && self
                .properties
                .iter()
                .zip(&other.properties)
                .all(|(left, right)| {
                    left.fmtid == right.fmtid
                        && left.pid == right.pid
                        && left.name == right.name
                        && left.value.is_same_value(&right.value)
                })
    }

    /// Parse a `docProps/custom.xml` part.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        if !custom_property_namespaces_are_valid(xml) {
            return Err(OxmlError::UnexpectedElement(
                "custom properties namespace".to_owned(),
            ));
        }
        let mut reader = Reader::from_reader(xml);
        let mut properties = Self::default();
        let mut root_open = false;
        let mut root_closed = false;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref element)) => {
                    let qualified_name = element.name();
                    let name = local_name(qualified_name.as_ref());
                    if name == b"Properties" {
                        if root_open || root_closed {
                            return Err(OxmlError::UnexpectedElement("Properties".to_owned()));
                        }
                        root_open = true;
                        properties
                            .extra_namespaces
                            .extend(extra_namespace_declarations(element)?);
                    } else if !root_open {
                        return Err(OxmlError::UnexpectedElement(
                            String::from_utf8_lossy(name).into_owned(),
                        ));
                    } else if name == b"property" {
                        properties
                            .properties
                            .push(parse_property(&mut reader, element)?);
                    } else {
                        return Err(OxmlError::UnexpectedElement(
                            String::from_utf8_lossy(name).into_owned(),
                        ));
                    }
                }
                Ok(Event::Empty(ref element)) => {
                    let qualified_name = element.name();
                    let name = local_name(qualified_name.as_ref());
                    if name == b"Properties" {
                        if root_open || root_closed {
                            return Err(OxmlError::UnexpectedElement("Properties".to_owned()));
                        }
                        root_closed = true;
                    } else if !root_open {
                        return Err(OxmlError::UnexpectedElement(
                            String::from_utf8_lossy(name).into_owned(),
                        ));
                    } else if name == b"property" {
                        return Err(OxmlError::MissingElement(
                            "custom property value".to_owned(),
                        ));
                    } else {
                        return Err(OxmlError::UnexpectedElement(
                            String::from_utf8_lossy(name).into_owned(),
                        ));
                    }
                }
                Ok(Event::End(ref element))
                    if local_name(element.name().as_ref()) == b"Properties" =>
                {
                    if !root_open {
                        return Err(OxmlError::UnexpectedElement("Properties".to_owned()));
                    }
                    root_open = false;
                    root_closed = true;
                }
                Ok(Event::Eof) => break,
                Err(error) => return Err(error.into()),
                _ => {}
            }
            buf.clear();
        }

        if root_closed {
            Ok(properties)
        } else {
            Err(OxmlError::MissingElement("Properties root".to_owned()))
        }
    }

    /// Serialize a `docProps/custom.xml` part.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;

        let mut root = BytesStart::new("Properties");
        root.push_attribute(("xmlns", CUSTOM_PROPERTIES_NS));
        root.push_attribute(("xmlns:vt", VARIANT_TYPES_NS));
        for (name, value) in &self.extra_namespaces {
            root.push_attribute((name.as_str(), value.as_str()));
        }
        writer.write_event(Event::Start(root))?;

        for property in &self.properties {
            write_property(&mut writer, property)?;
        }

        writer.write_event(Event::End(BytesEnd::new("Properties")))?;
        Ok(writer.into_inner())
    }
}

fn custom_property_namespaces_are_valid(xml: &[u8]) -> bool {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut saw_root = false;
    loop {
        let Ok((namespace, event)) = reader.read_resolved_event_into(&mut buffer) else {
            return false;
        };
        let is_start = matches!(event, Event::Start(_));
        if let Event::Start(ref element) | Event::Empty(ref element) = event {
            let valid = match depth {
                0 => {
                    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == CUSTOM_PROPERTIES_NS.as_bytes())
                        && local_name(element.name().as_ref()) == b"Properties"
                }
                1 => {
                    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == CUSTOM_PROPERTIES_NS.as_bytes())
                        && local_name(element.name().as_ref()) == b"property"
                        && property_attributes_are_unqualified(&reader, element)
                }
                2 => {
                    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == VARIANT_TYPES_NS.as_bytes())
                }
                _ => true,
            };
            if !valid {
                return false;
            }
            saw_root = true;
            if is_start {
                let Some(next_depth) = depth.checked_add(1) else {
                    return false;
                };
                depth = next_depth;
            }
        } else if matches!(event, Event::End(_)) {
            depth = depth.saturating_sub(1);
        } else if matches!(event, Event::Eof) {
            return saw_root && depth == 0;
        }
        buffer.clear();
    }
}

fn property_attributes_are_unqualified(reader: &NsReader<&[u8]>, element: &BytesStart<'_>) -> bool {
    element.attributes().all(|attribute| {
        let Ok(attribute) = attribute else {
            return false;
        };
        let name = attribute.key.as_ref();
        (name == b"xmlns" || name.starts_with(b"xmlns:"))
            || matches!(
                reader.resolver().resolve_attribute(attribute.key).0,
                ResolveResult::Unbound
            )
    })
}

fn parse_property(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>) -> Result<CustomProperty> {
    let fmtid = required_attribute(start, b"fmtid")?;
    let pid_text = required_attribute(start, b"pid")?;
    let pid = pid_text.parse().map_err(|_| {
        OxmlError::InvalidValue(format!(
            "custom property pid must be an integer, got {pid_text:?}"
        ))
    })?;
    let name = optional_attribute(start, b"name")?;
    let mut value = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref element)) => {
                if value.is_some() {
                    return Err(OxmlError::InvalidValue(
                        "custom property has more than one value".to_owned(),
                    ));
                }
                value = Some(parse_value_element(reader, element)?);
            }
            Ok(Event::Empty(ref element)) => {
                if value.is_some() {
                    return Err(OxmlError::InvalidValue(
                        "custom property has more than one value".to_owned(),
                    ));
                }
                value = Some(parse_empty_value(element)?);
            }
            Ok(Event::End(ref element)) if local_name(element.name().as_ref()) == b"property" => {
                break;
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(error.into()),
            _ => {}
        }
        buf.clear();
    }

    Ok(CustomProperty {
        fmtid,
        pid,
        name,
        value: value
            .ok_or_else(|| OxmlError::MissingElement("custom property value".to_owned()))?,
    })
}

fn parse_value_element(
    reader: &mut Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<CustomPropertyValue> {
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    match name {
        b"lpstr" => Ok(CustomPropertyValue::Lpstr(read_element_text(
            reader,
            element.name(),
        ))),
        b"lpwstr" => Ok(CustomPropertyValue::Lpwstr(read_element_text(
            reader,
            element.name(),
        ))),
        b"i4" => {
            let text = read_element_text(reader, element.name());
            let value = text.trim().parse().map_err(|_| {
                OxmlError::InvalidValue(format!("vt:i4 must be an integer, got {text:?}"))
            })?;
            Ok(CustomPropertyValue::I4(value))
        }
        b"r8" => {
            let text = read_element_text(reader, element.name());
            Ok(CustomPropertyValue::R8(parse_r8(&text)?))
        }
        b"bool" => {
            let text = read_element_text(reader, element.name());
            Ok(CustomPropertyValue::Bool(parse_bool(&text)?))
        }
        b"filetime" => Ok(CustomPropertyValue::FileTime(read_element_text(
            reader,
            element.name(),
        ))),
        b"empty" => {
            reader.read_to_end_into(element.name(), &mut Vec::new())?;
            Ok(CustomPropertyValue::Empty)
        }
        _ => Ok(CustomPropertyValue::Raw(capture_element(reader, element)?)),
    }
}

fn parse_empty_value(element: &BytesStart<'_>) -> Result<CustomPropertyValue> {
    match local_name(element.name().as_ref()) {
        b"lpstr" => Ok(CustomPropertyValue::Lpstr(String::new())),
        b"lpwstr" => Ok(CustomPropertyValue::Lpwstr(String::new())),
        b"i4" => Err(OxmlError::InvalidValue(
            "vt:i4 must be an integer, got an empty value".to_owned(),
        )),
        b"r8" => Err(OxmlError::InvalidValue(
            "vt:r8 must be a number, got an empty value".to_owned(),
        )),
        b"bool" => Err(OxmlError::InvalidValue(
            "vt:bool must be a Boolean, got an empty value".to_owned(),
        )),
        b"filetime" => Ok(CustomPropertyValue::FileTime(String::new())),
        b"empty" => Ok(CustomPropertyValue::Empty),
        _ => Ok(CustomPropertyValue::Raw(capture_empty_element(element)?)),
    }
}

fn optional_attribute(element: &BytesStart<'_>, expected: &[u8]) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        if local_name(attribute.key.as_ref()) == expected {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn required_attribute(element: &BytesStart<'_>, expected: &[u8]) -> Result<String> {
    optional_attribute(element, expected)?.ok_or_else(|| {
        OxmlError::MissingElement(format!(
            "custom property {} attribute",
            String::from_utf8_lossy(expected)
        ))
    })
}

fn parse_bool(text: &str) -> Result<bool> {
    match text.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(OxmlError::InvalidValue(format!(
            "vt:bool must be a Boolean, got {text:?}"
        ))),
    }
}

fn parse_r8(text: &str) -> Result<f64> {
    match text.trim() {
        "INF" => Ok(f64::INFINITY),
        "-INF" => Ok(f64::NEG_INFINITY),
        "NaN" => Ok(f64::NAN),
        value => value
            .parse()
            .map_err(|_| OxmlError::InvalidValue(format!("vt:r8 must be a number, got {text:?}"))),
    }
}

fn write_property(writer: &mut Writer<Vec<u8>>, property: &CustomProperty) -> Result<()> {
    let mut start = BytesStart::new("property");
    start.push_attribute(("fmtid", property.fmtid.as_str()));
    let pid = property.pid.to_string();
    start.push_attribute(("pid", pid.as_str()));
    if let Some(name) = &property.name {
        crate::xml::push_escaped_attribute(&mut start, "name", name);
    }
    writer.write_event(Event::Start(start))?;
    write_value(writer, &property.value)?;
    writer.write_event(Event::End(BytesEnd::new("property")))?;
    Ok(())
}

fn write_value(writer: &mut Writer<Vec<u8>>, value: &CustomPropertyValue) -> Result<()> {
    match value {
        CustomPropertyValue::Lpstr(value) => write_text(writer, "vt:lpstr", value),
        CustomPropertyValue::Lpwstr(value) => write_text(writer, "vt:lpwstr", value),
        CustomPropertyValue::I4(value) => write_text(writer, "vt:i4", &value.to_string()),
        CustomPropertyValue::R8(value) => write_text(writer, "vt:r8", r8_text(*value).as_ref()),
        CustomPropertyValue::Bool(value) => write_text(writer, "vt:bool", &value.to_string()),
        CustomPropertyValue::FileTime(value) => write_text(writer, "vt:filetime", value),
        CustomPropertyValue::Empty => {
            writer.write_event(Event::Empty(BytesStart::new("vt:empty")))?;
            Ok(())
        }
        CustomPropertyValue::Raw(raw) => {
            writer.get_mut().write_all(raw)?;
            Ok(())
        }
    }
}

fn r8_text(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f64::INFINITY {
        "INF".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-INF".to_owned()
    } else {
        value.to_string()
    }
}

fn write_text(writer: &mut Writer<Vec<u8>>, tag: &str, value: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(tag)))?;
    writer.write_event(Event::Text(BytesText::from_escaped(
        crate::xml::escape_text(value),
    )))?;
    writer.write_event(Event::End(BytesEnd::new(tag)))?;
    Ok(())
}
