//! Custom XML store items: finding a part by item id, reading and normalizing item ids, checking the bytes of a new item.

use super::*;

const CUSTOM_XML_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
const CUSTOM_XML_PROPS_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps";
const CUSTOM_XML_PROPS_NS: &[u8] =
    b"http://schemas.openxmlformats.org/officeDocument/2006/customXml";

pub(crate) fn resolve_custom_xml_part(
    package: &OpcPackage,
    document_part: &str,
    store_item_id: &str,
) -> Result<String> {
    let matches = matching_custom_xml_parts(package, document_part, store_item_id)?;
    match matches.as_slice() {
        [part_name] => Ok(part_name.clone()),
        [] => Err(Error::Other(format!(
            "custom XML store item {store_item_id} was not found"
        ))),
        _ => Err(Error::Other(format!(
            "custom XML store item {store_item_id} is ambiguous"
        ))),
    }
}

pub(crate) fn custom_xml_store_item_count(
    package: &OpcPackage,
    document_part: &str,
    store_item_id: &str,
) -> Result<usize> {
    Ok(matching_custom_xml_parts(package, document_part, store_item_id)?.len())
}

fn matching_custom_xml_parts(
    package: &OpcPackage,
    document_part: &str,
    store_item_id: &str,
) -> Result<Vec<String>> {
    let expected = normalize_store_item_id(store_item_id);
    let mut matches = Vec::new();
    let Some(relationships) = package.get_part_rels(document_part) else {
        return Ok(matches);
    };
    for relationship in relationships
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == CUSTOM_XML_REL_TYPE)
    {
        let item_part = OpcPackage::resolve_rel_target(document_part, &relationship.target);
        let Some(properties_relationship) = package
            .get_part_rels(&item_part)
            .and_then(|relationships| relationships.get_by_type(CUSTOM_XML_PROPS_REL_TYPE))
        else {
            continue;
        };
        let properties_part =
            OpcPackage::resolve_rel_target(&item_part, &properties_relationship.target);
        let Some(properties_xml) = package.get_part(&properties_part) else {
            continue;
        };
        if parse_store_item_id(properties_xml)?
            .is_some_and(|found| normalize_store_item_id(&found) == expected)
        {
            matches.push(item_part);
        }
    }
    Ok(matches)
}

pub(crate) fn normalize_store_item_id(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .to_ascii_lowercase()
}

pub(crate) fn parse_store_item_id(xml: &[u8]) -> Result<Option<String>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid custom XML properties: {error}")))?;
        let root_matches = namespace_matches(&namespace, Some(CUSTOM_XML_PROPS_NS));
        let event = event.into_owned();
        match event {
            Event::Start(element) | Event::Empty(element)
                if root_matches
                    && matches_local_name(element.name().as_ref(), b"datastoreItem") =>
            {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!("invalid custom XML properties attribute: {error}"))
                    })?;
                    let (attribute_namespace, local_name) =
                        reader.resolver().resolve_attribute(attribute.key);
                    if namespace_matches(&attribute_namespace, Some(CUSTOM_XML_PROPS_NS))
                        && local_name.as_ref() == b"itemID"
                    {
                        return attribute
                            .decoded_and_normalized_value(
                                XmlVersion::Implicit1_0,
                                element.decoder(),
                            )
                            .map(|value| Some(value.into_owned()))
                            .map_err(|error| {
                                Error::Other(format!(
                                    "invalid custom XML properties item id: {error}"
                                ))
                            });
                    }
                }
                return Ok(None);
            }
            Event::Start(_) | Event::Empty(_) => return Ok(None),
            Event::Text(text) if is_xml_whitespace(text.as_ref()) => {}
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {}
            Event::Eof => return Ok(None),
            _ => return Ok(None),
        }
        buffer.clear();
    }
}

pub(super) fn is_xml_whitespace(bytes: &[u8]) -> bool {
    bytes.iter().all(u8::is_ascii_whitespace)
}

fn namespace_matches(namespace: &ResolveResult<'_>, expected: Option<&[u8]>) -> bool {
    match (namespace, expected) {
        (ResolveResult::Bound(Namespace(actual)), Some(expected)) => *actual == expected,
        (ResolveResult::Unbound, None) => true,
        _ => false,
    }
}

pub(super) fn validate_store_item(xml: &[u8]) -> Result<()> {
    if xml.is_empty() {
        return Err(Error::Other("custom XML item is empty".to_owned()));
    }
    let text = std::str::from_utf8(xml)
        .map_err(|error| Error::Other(format!("custom XML item is not XML: {error}")))?;
    oxoffice_core::xml::reject_non_xml_characters("custom XML item is not XML", text)?;
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_comments = true;
    let mut buffer = Vec::new();
    let mut roots = 0usize;
    let mut depth = 0usize;
    let mut declaration_allowed = true;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(start)) => {
                reject_store_attributes(&start)?;
                if depth == 0 {
                    roots += 1;
                    if roots > 1 {
                        return Err(Error::Other("custom XML item is a fragment".to_owned()));
                    }
                }
                depth += 1;
            }
            Ok(Event::Empty(start)) => {
                reject_store_attributes(&start)?;
                if depth == 0 {
                    roots += 1;
                    if roots > 1 {
                        return Err(Error::Other("custom XML item is a fragment".to_owned()));
                    }
                }
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    return Err(Error::Other(
                        "custom XML item is not one element".to_owned(),
                    ));
                }
                depth -= 1;
            }
            Ok(Event::Text(text)) => {
                if text.windows(3).any(|bytes| bytes == b"]]>") {
                    return Err(Error::Other(
                        "custom XML item is not XML: literal CDATA terminator in text".to_owned(),
                    ));
                }
                if depth == 0 && !is_xml_whitespace(text.as_ref()) {
                    return Err(Error::Other("custom XML item is a fragment".to_owned()));
                }
            }
            Ok(Event::CData(_)) => {
                if depth == 0 {
                    return Err(Error::Other("custom XML item is a fragment".to_owned()));
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if depth == 0 {
                    return Err(Error::Other("custom XML item is a fragment".to_owned()));
                }
                decode_one_xml_reference(&reference).map_err(|detail| {
                    Error::Other(format!("custom XML item is not XML: {detail}"))
                })?;
            }
            Ok(Event::Decl(declaration)) => {
                if !declaration_allowed {
                    return Err(Error::Other(
                        "custom XML item is not XML: declaration must be first".to_owned(),
                    ));
                }
                validate_store_declaration(&declaration, text.is_ascii())?;
            }
            Ok(Event::DocType(_)) => {
                return Err(Error::Other(
                    "custom XML item is not XML: DTD declarations are unsupported".to_owned(),
                ));
            }
            Ok(Event::PI(instruction)) => {
                let target = instruction.target();
                if !store_xml_name_is_valid(target) || target.eq_ignore_ascii_case(b"xml") {
                    return Err(Error::Other(
                        "custom XML item is not XML: invalid processing instruction target"
                            .to_owned(),
                    ));
                }
            }
            Ok(Event::Comment(_)) => {} // The reader checks comment syntax.
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(Error::Other(format!("custom XML item is not XML: {error}")));
            }
        }
        declaration_allowed = false;
        buffer.clear();
    }
    if roots != 1 || depth != 0 {
        return Err(Error::Other(
            "custom XML item is not one element".to_owned(),
        ));
    }
    Ok(())
}

fn validate_store_declaration(
    declaration: &quick_xml::events::BytesDecl<'_>,
    ascii_only: bool,
) -> Result<()> {
    let invalid =
        || Error::Other("custom XML item is not XML: invalid XML 1.0 declaration".to_owned());
    let content = std::str::from_utf8(declaration.as_ref()).map_err(|_| invalid())?;
    let start = BytesStart::from_content(content, 3);
    reject_store_attribute_separators(&start)?;
    let mut encoding_seen = false;
    let mut standalone_seen = false;
    let mut version_seen = false;
    for (index, attribute) in start.attributes().enumerate() {
        let attribute = attribute.map_err(|_| invalid())?;
        let value = attribute.value.as_ref();
        match attribute.key.as_ref() {
            b"version" if index == 0 && value == b"1.0" => version_seen = true,
            b"encoding" if version_seen && !encoding_seen && !standalone_seen => {
                if !value.first().is_some_and(u8::is_ascii_alphabetic)
                    || !value.iter().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })
                {
                    return Err(invalid());
                }
                if !value.eq_ignore_ascii_case(b"UTF-8")
                    && !(ascii_only && value.eq_ignore_ascii_case(b"US-ASCII"))
                {
                    return Err(Error::Other(
                        "custom XML item is not XML: encoding is unsupported or contradicts UTF-8 input"
                            .to_owned(),
                    ));
                }
                encoding_seen = true;
            }
            b"standalone"
                if version_seen && !standalone_seen && matches!(value, b"yes" | b"no") =>
            {
                standalone_seen = true;
            }
            _ => return Err(invalid()),
        }
    }
    if !version_seen {
        return Err(invalid());
    }
    Ok(())
}

fn reject_store_attributes(start: &BytesStart<'_>) -> Result<()> {
    reject_store_name(start.name().as_ref())?;
    reject_store_attribute_separators(start)?;
    for attribute in start.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("custom XML item is not XML: {error}")))?;
        reject_store_name(attribute.key.as_ref())?;
        reject_store_attribute_value(attribute.value.as_ref())?;
    }
    Ok(())
}

fn reject_store_attribute_separators(start: &BytesStart<'_>) -> Result<()> {
    // quick-xml checks quoting and duplicates, but accepts adjacent attributes.
    // Inspect only their separators here. Names and values are checked above.
    let mut quote = None;
    let mut separator_required = true;
    for &byte in start.attributes_raw() {
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
                separator_required = true;
            }
        } else {
            if separator_required {
                if !matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
                    return Err(Error::Other(
                        "custom XML item is not XML: attribute requires whitespace".to_owned(),
                    ));
                }
                separator_required = false;
            }
            if matches!(byte, b'\'' | b'"') {
                quote = Some(byte);
            }
        }
    }
    Ok(())
}

fn reject_store_name(name: &[u8]) -> Result<()> {
    if store_xml_name_is_valid(name) {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "custom XML item is not XML: invalid name {}",
            String::from_utf8_lossy(name)
        )))
    }
}

fn reject_store_attribute_value(value: &[u8]) -> Result<()> {
    if value.contains(&b'<') {
        return Err(Error::Other(
            "custom XML item is not XML: attribute contains a literal less-than sign".to_owned(),
        ));
    }
    let text = std::str::from_utf8(value)
        .map_err(|error| Error::Other(format!("custom XML item is not XML: {error}")))?;
    decode_xml_reference_text(text)
        .map_err(|detail| Error::Other(format!("custom XML item is not XML: {detail}")))?;
    Ok(())
}

/// Decode XML references with quick-xml and then apply the XML 1.0 character rule.
/// Numeric syntax stays in `unescape`, including its lowercase hexadecimal marker.
fn decode_xml_reference_text(raw: &str) -> std::result::Result<std::borrow::Cow<'_, str>, String> {
    let decoded = quick_xml::escape::unescape(raw).map_err(|error| error.to_string())?;
    if decoded
        .chars()
        .all(oxoffice_core::xml::is_xml_1_0_character)
    {
        Ok(decoded)
    } else {
        Err("character reference is not legal in XML 1.0".to_owned())
    }
}

pub(super) fn decode_one_xml_reference(
    reference: &BytesRef<'_>,
) -> std::result::Result<char, String> {
    let body = reference.decode().map_err(|error| error.to_string())?;
    let mut raw = String::with_capacity(body.len() + 2);
    raw.push('&');
    raw.push_str(body.as_ref());
    raw.push(';');
    let decoded = decode_xml_reference_text(&raw)?;
    let mut characters = decoded.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Ok(character),
        _ => Err("character reference is not legal in XML 1.0".to_owned()),
    }
}

fn store_xml_name_is_valid(name: &[u8]) -> bool {
    let Ok(name) = std::str::from_utf8(name) else {
        return false;
    };
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    store_name_start(first) && characters.all(store_name_character)
}

fn store_name_start(character: char) -> bool {
    character == ':'
        || matches!(
            character,
            'A'..='Z'
                | '_'
                | 'a'..='z'
                | '\u{00C0}'..='\u{00D6}'
                | '\u{00D8}'..='\u{00F6}'
                | '\u{00F8}'..='\u{02FF}'
                | '\u{0370}'..='\u{037D}'
                | '\u{037F}'..='\u{1FFF}'
                | '\u{200C}'..='\u{200D}'
                | '\u{2070}'..='\u{218F}'
                | '\u{2C00}'..='\u{2FEF}'
                | '\u{3001}'..='\u{D7FF}'
                | '\u{F900}'..='\u{FDCF}'
                | '\u{FDF0}'..='\u{FFFD}'
                | '\u{10000}'..='\u{EFFFF}'
        )
}

fn store_name_character(character: char) -> bool {
    store_name_start(character)
        || matches!(
            character,
            '-' | '.' | '0'..='9' | '\u{00B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}'
        )
}
