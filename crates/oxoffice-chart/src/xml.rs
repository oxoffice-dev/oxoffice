//! Namespaces, attributes and raw children: chart prefixes, fixed namespaces, foreign elements, shape properties, raw shells and schema order.

use super::*;

pub(super) fn chart_root_prefix(element: &BytesStart<'_>) -> Result<Vec<u8>> {
    let name = element.name();
    let qualified = name.as_ref();
    let prefix = qualified
        .iter()
        .position(|byte| *byte == b':')
        .map(|position| qualified[..position].to_vec())
        .unwrap_or_default();
    let declaration = if prefix.is_empty() {
        b"xmlns".to_vec()
    } else {
        let mut declaration = b"xmlns:".to_vec();
        declaration.extend_from_slice(&prefix);
        declaration
    };
    if let Some(namespace) = attribute_value(element, &declaration)?
        && namespace != C_NS
    {
        return Err(invalid_attribute(
            &element_name(element),
            "namespace",
            namespace,
        ));
    }
    reject_conflicting_prefix(element, b"c", C_NS)?;
    Ok(prefix)
}

pub(super) fn typed_rewrite_bindings(
    element: &BytesStart<'_>,
    inherited: &NamespaceBindings,
) -> Result<NamespaceBindings> {
    chart_root_prefix(element)?;
    if !element_is_in_namespace(element, C_NS, inherited)? {
        return Err(ChartError::UnexpectedElement(element_name(element)));
    }
    let bindings = chart_bindings(inherited, element)?;
    require_fixed_namespace(&bindings, b"c", C_NS, element)?;
    Ok(bindings)
}

pub(super) fn chart_namespace_defaults() -> NamespaceBindings {
    vec![
        (b"c".to_vec(), C_NS.to_owned()),
        (b"a".to_vec(), A_NS.to_owned()),
        (b"r".to_vec(), R_NS.to_owned()),
    ]
}

pub(super) fn chart_bindings(
    inherited: &NamespaceBindings,
    element: &BytesStart<'_>,
) -> Result<NamespaceBindings> {
    let mut bindings = chart_namespace_defaults();
    for (prefix, namespace) in inherited
        .iter()
        .cloned()
        .chain(namespace_bindings(element)?)
    {
        upsert_namespace_binding(&mut bindings, prefix, namespace);
    }
    Ok(bindings)
}

pub(super) fn root_chart_bindings(
    xml: &[u8],
    local: &[u8],
    inherited: &NamespaceBindings,
) -> Result<NamespaceBindings> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) | Event::Empty(element)
                if matches_local_name(element.name().as_ref(), local) =>
            {
                if !element_is_in_namespace(&element, C_NS, inherited)? {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                return chart_bindings(inherited, &element);
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn chart_child_local(
    element: &BytesStart<'_>,
    inherited: &NamespaceBindings,
) -> Result<Option<Vec<u8>>> {
    Ok(element_is_in_namespace(element, C_NS, inherited)?
        .then(|| local_name(element.name().as_ref()).to_vec()))
}

pub(super) fn capture_fixed_root_attributes(
    start: &BytesStart<'_>,
    fixed: &[&str],
) -> Result<(XmlAttributes, XmlAttributes)> {
    let mut attributes = Vec::new();
    let mut namespaces = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        if fixed.contains(&name.as_str()) {
            continue;
        }
        if name == "xmlns" || name.starts_with("xmlns:") {
            namespaces.push((name, value));
        } else {
            attributes.push((name, value));
        }
    }
    Ok((attributes, namespaces))
}

pub(super) fn require_fixed_namespace(
    bindings: &NamespaceBindings,
    prefix: &[u8],
    expected: &str,
    element: &BytesStart<'_>,
) -> Result<()> {
    let actual = bindings
        .iter()
        .find(|(candidate, _)| candidate.as_slice() == prefix)
        .map(|(_, namespace)| namespace.as_str());
    if actual == Some(expected) {
        return Ok(());
    }
    Err(invalid_attribute(
        &element_name(element),
        &format!("xmlns:{}", String::from_utf8_lossy(prefix)),
        actual.unwrap_or("<missing>").to_owned(),
    ))
}

pub(super) fn standalone_namespace_declarations(
    bindings: &NamespaceBindings,
) -> Result<XmlAttributes> {
    bindings
        .iter()
        .filter(|(prefix, _)| prefix != b"c" && prefix != b"a" && prefix != b"r")
        .map(|(prefix, namespace)| {
            let prefix = std::str::from_utf8(prefix).map_err(OxmlError::from)?;
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            Ok((name, namespace.clone()))
        })
        .collect()
}

pub(super) fn capture_fixed_attributes(
    start: &BytesStart<'_>,
    fixed: &[&str],
) -> Result<XmlAttributes> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        if fixed.contains(&name.as_str()) {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        attributes.push((name, value));
    }
    Ok(attributes)
}

pub(super) fn parse_chart_shape_properties(
    raw: &[u8],
    namespaces: &NamespaceBindings,
    slot: &mut Option<CT_ShapeProperties>,
) -> Result<()> {
    reject_conflicting_prefix_in_xml(raw, b"a", A_NS)?;
    let inherited = namespaces
        .iter()
        .map(|(prefix, uri)| {
            Ok((
                std::str::from_utf8(prefix)
                    .map_err(OxmlError::from)?
                    .to_owned(),
                uri.clone(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let properties = CT_ShapeProperties::from_xml_with_inherited_namespaces(raw, &inherited)?;
    let mut writer = Writer::new(Vec::new());
    properties.write_xml_as(&mut writer, "c:spPr")?;
    reject_rewritten_foreign_elements(raw, &writer.into_inner(), namespaces, b"spPr")?;
    set_once(slot, properties, "c:spPr")
}

pub(super) fn parse_raw_shell(
    xml: &[u8],
    local: &[u8],
    tag: &str,
) -> Result<(XmlAttributes, OrderedRawChildren)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), local) => {
                if local != b"legend" {
                    reject_conflicting_prefix(&element, b"c", C_NS)?;
                }
                let raw_attributes = capture_attributes(&element)?;
                let mut raw_children = OrderedRawChildren::default();
                let mut child_buffer = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut child_buffer)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            raw_children.push(0, capture_element(&mut reader, &child)?)
                        }
                        Event::Empty(child) => raw_children.push(0, capture_empty_element(&child)?),
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(0, capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), local) => break,
                        Event::Eof => return Err(missing_end(tag)),
                        _ => {}
                    }
                    child_buffer.clear();
                }
                return Ok((raw_attributes, raw_children));
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), local) => {
                if local != b"legend" {
                    reject_conflicting_prefix(&element, b"c", C_NS)?;
                }
                return Ok((capture_attributes(&element)?, OrderedRawChildren::default()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => return Err(ChartError::MissingElement(tag.to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn write_raw_shell<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    raw_attributes: &XmlAttributes,
    raw_children: &OrderedRawChildren,
) -> Result<()> {
    let mut start = BytesStart::new(tag);
    push_attributes(&mut start, raw_attributes);
    if raw_children.is_empty() {
        writer
            .write_event(Event::Empty(start))
            .map_err(OxmlError::from)?;
        return Ok(());
    }
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, raw_children.at(0))?;
    writer
        .write_event(Event::End(BytesEnd::new(tag)))
        .map_err(OxmlError::from)?;
    Ok(())
}

pub(super) fn raw_children_in_schema_order(
    raw_children: &OrderedRawChildren,
    final_boundary: usize,
) -> OrderedRawChildren {
    let mut ordered = OrderedRawChildren::default();
    for boundary in 0..=final_boundary {
        for raw in raw_children.at(boundary) {
            ordered.push(boundary, raw.to_vec());
        }
    }
    ordered
}

pub(super) fn validate_chart_space_namespace(element: &BytesStart<'_>) -> Result<()> {
    let name = element.name();
    let qualified = name.as_ref();
    let declaration = match qualified.iter().position(|byte| *byte == b':') {
        Some(position) => {
            let mut declaration = b"xmlns:".to_vec();
            declaration.extend_from_slice(&qualified[..position]);
            declaration
        }
        None => b"xmlns".to_vec(),
    };
    let namespace = attribute_value(element, &declaration)?
        .ok_or_else(|| invalid_attribute("chartSpace", "namespace", "<missing>".to_owned()))?;
    if namespace != C_NS {
        return Err(invalid_attribute("chartSpace", "namespace", namespace));
    }
    reject_conflicting_prefix(element, b"c", C_NS)?;
    reject_conflicting_prefix(element, b"a", A_NS)?;
    reject_conflicting_prefix(element, b"r", R_NS)
}

pub(super) fn reject_conflicting_prefix_in_xml(
    xml: &[u8],
    prefix: &[u8],
    namespace: &str,
) -> Result<()> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) | Event::Empty(element) => {
                return reject_conflicting_prefix(&element, prefix, namespace);
            }
            Event::Eof => return Err(ChartError::MissingElement("XML root".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn reject_rewritten_foreign_elements(
    original: &[u8],
    written: &[u8],
    inherited: &NamespaceBindings,
    root_local: &[u8],
) -> Result<()> {
    let original = foreign_element_regions(original, inherited, root_local)?;
    let mut written_namespaces = inherited.clone();
    written_namespaces.retain(|(prefix, _)| prefix != b"a");
    written_namespaces.push((b"a".to_vec(), A_NS.to_owned()));
    let written = foreign_element_regions(written, &written_namespaces, root_local)?;
    if original != written {
        return Err(invalid_attribute(
            &String::from_utf8_lossy(root_local),
            "namespace",
            "foreign DrawingML content would be converted to typed content".to_owned(),
        ));
    }
    Ok(())
}

fn foreign_element_regions(
    xml: &[u8],
    inherited: &NamespaceBindings,
    root_local: &[u8],
) -> Result<Vec<Vec<u8>>> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    let root_namespaces = loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), root_local) => {
                break bindings_with_local(inherited, &element)?;
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), root_local) => {
                return Ok(Vec::new());
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(
                    String::from_utf8_lossy(root_local).into_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    };

    let mut namespace_stack = vec![root_namespaces];
    let mut foreign = Vec::new();
    let mut child_buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut child_buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) => {
                let namespaces = namespace_stack.last().expect("root namespace binding");
                if element_is_in_namespace(&element, A_NS, namespaces)? {
                    namespace_stack.push(bindings_with_local(namespaces, &element)?);
                } else {
                    foreign.push(capture_element(&mut reader, &element)?);
                }
            }
            Event::Empty(element) => {
                let namespaces = namespace_stack.last().expect("root namespace binding");
                if !element_is_in_namespace(&element, A_NS, namespaces)? {
                    foreign.push(capture_empty_element(&element)?);
                }
            }
            Event::End(element) if matches_local_name(element.name().as_ref(), root_local) => {
                return Ok(foreign);
            }
            Event::End(_) => {
                namespace_stack.pop();
            }
            Event::Eof => {
                return Err(missing_end(&String::from_utf8_lossy(root_local)));
            }
            _ => {}
        }
        child_buffer.clear();
    }
}

pub(super) fn reject_conflicting_prefix(
    element: &BytesStart<'_>,
    prefix: &[u8],
    namespace: &str,
) -> Result<()> {
    let mut key = b"xmlns:".to_vec();
    key.extend_from_slice(prefix);
    if let Some(value) = attribute_value(element, &key)?
        && value != namespace
    {
        return Err(invalid_attribute(
            &element_name(element),
            &String::from_utf8_lossy(&key),
            value,
        ));
    }
    Ok(())
}

pub(super) fn namespace_bindings(element: &BytesStart<'_>) -> Result<NamespaceBindings> {
    let mut bindings = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let key = attribute.key.as_ref();
        let prefix = if key == b"xmlns" {
            Vec::new()
        } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
            prefix.to_vec()
        } else {
            continue;
        };
        let namespace = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        bindings.push((prefix, namespace));
    }
    Ok(bindings)
}

pub(super) fn bindings_with_local(
    inherited: &NamespaceBindings,
    element: &BytesStart<'_>,
) -> Result<NamespaceBindings> {
    let mut bindings = inherited.clone();
    for (prefix, namespace) in namespace_bindings(element)? {
        upsert_namespace_binding(&mut bindings, prefix, namespace);
    }
    Ok(bindings)
}

pub(super) fn upsert_namespace_binding(
    bindings: &mut NamespaceBindings,
    prefix: Vec<u8>,
    namespace: String,
) {
    if let Some((_, current)) = bindings.iter_mut().find(|(current, _)| current == &prefix) {
        *current = namespace;
    } else {
        bindings.push((prefix, namespace));
    }
}

pub(super) fn element_is_in_namespace(
    element: &BytesStart<'_>,
    expected: &str,
    inherited: &NamespaceBindings,
) -> Result<bool> {
    let name = element.name();
    let qualified = name.as_ref();
    let prefix = qualified
        .iter()
        .position(|byte| *byte == b':')
        .map(|position| &qualified[..position])
        .unwrap_or_default();
    let local = namespace_bindings(element)?;
    let namespace = local
        .iter()
        .rev()
        .chain(inherited.iter().rev())
        .find(|(candidate, _)| candidate.as_slice() == prefix)
        .map(|(_, namespace)| namespace.as_str());
    Ok(namespace == Some(expected))
}

pub(super) fn attribute_value(element: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        if attribute.key.as_ref() == name {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .map_err(OxmlError::from)?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

pub(super) fn capture_root_attributes(start: &BytesStart<'_>) -> Result<RootAttributes> {
    let mut attributes = Vec::new();
    let mut namespaces = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        if name == "xmlns:c" || name == "xmlns:a" || name == "xmlns:r" {
            continue;
        }
        if name == "xmlns" || name.starts_with("xmlns:") {
            namespaces.push((name, value));
        } else {
            attributes.push((name, value));
        }
    }
    Ok((attributes, namespaces))
}

pub(super) fn capture_attributes(start: &BytesStart<'_>) -> Result<XmlAttributes> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        attributes.push((
            std::str::from_utf8(attribute.key.as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                .map_err(OxmlError::from)?
                .into_owned(),
        ));
    }
    Ok(attributes)
}

pub(super) fn capture_attributes_except(
    start: &BytesStart<'_>,
    excluded: &[u8],
) -> Result<XmlAttributes> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        if attribute.key.as_ref() == excluded {
            continue;
        }
        attributes.push((
            std::str::from_utf8(attribute.key.as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                .map_err(OxmlError::from)?
                .into_owned(),
        ));
    }
    Ok(attributes)
}

pub(super) fn capture_attributes_excluding(
    start: &BytesStart<'_>,
    excluded: &[&[u8]],
) -> Result<XmlAttributes> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        if excluded.contains(&attribute.key.as_ref()) {
            continue;
        }
        attributes.push((
            std::str::from_utf8(attribute.key.as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                .map_err(OxmlError::from)?
                .into_owned(),
        ));
    }
    Ok(attributes)
}
