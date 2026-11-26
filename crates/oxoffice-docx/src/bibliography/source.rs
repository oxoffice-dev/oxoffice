//! Bibliography sources: the ordered source XML, the source collection in its customXml part, and the edits that add, replace or remove a source.

use super::*;

impl SourceXml {
    pub(super) fn parse(xml: &[u8]) -> Result<Self> {
        oxoffice_core::xml::validate_strict_xml_1_0(xml)
            .map_err(|error| bibliography_error(format!("invalid source XML: {error:?}")))?;
        let mut reader = NsReader::from_reader(xml);
        let mut buffer = Vec::new();
        let mut nodes: Vec<SourceXmlNode> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut roots = 0;
        loop {
            let start = reader.buffer_position() as usize;
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| bibliography_error(format!("invalid source XML: {error}")))?;
            let namespace = namespace_value(namespace)?;
            let event = event.into_owned();
            let end = reader.buffer_position() as usize;
            let empty = matches!(event, Event::Empty(_));
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    let name = std::str::from_utf8(element.name().as_ref())
                        .map_err(|_| bibliography_error("element name is not UTF-8"))?
                        .to_owned();
                    let local = std::str::from_utf8(element.local_name().as_ref())
                        .map_err(|_| bibliography_error("element local name is not UTF-8"))?
                        .to_owned();
                    let mut attributes = Vec::new();
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| bibliography_error(error.to_string()))?;
                        if attribute.key.as_ref() == b"xmlns"
                            || attribute.key.as_ref().starts_with(b"xmlns:")
                        {
                            continue;
                        }
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        let namespace = namespace_value(namespace)?;
                        let local = std::str::from_utf8(local.as_ref())
                            .map_err(|_| bibliography_error("attribute name is not UTF-8"))?
                            .to_owned();
                        let value = attribute
                            .decoded_and_normalized_value(
                                XmlVersion::Implicit1_0,
                                element.decoder(),
                            )
                            .map_err(|error| bibliography_error(error.to_string()))?
                            .into_owned();
                        if attributes
                            .iter()
                            .any(|(ns, key, _)| ns == &namespace && key == &local)
                        {
                            return Err(bibliography_error("duplicate expanded attribute"));
                        }
                        attributes.push((namespace, local, value));
                    }
                    let index = nodes.len();
                    if let Some(&parent) = stack.last() {
                        nodes[parent].children.push(index);
                    } else {
                        roots += 1;
                        if roots != 1 {
                            return Err(bibliography_error("multiple XML roots"));
                        }
                    }
                    nodes.push(SourceXmlNode {
                        namespace,
                        local,
                        name,
                        full: start..end,
                        content: end..end,
                        attributes,
                        children: Vec::new(),
                        text: String::new(),
                        opaque_scalar: false,
                        empty,
                    });
                    if !empty {
                        stack.push(index);
                    }
                }
                Event::End(_) => {
                    let index = stack
                        .pop()
                        .ok_or_else(|| bibliography_error("unmatched XML end"))?;
                    nodes[index].content.end = start;
                    nodes[index].full.end = end;
                }
                Event::Text(text) => {
                    let value = text
                        .xml_content(XmlVersion::Implicit1_0)
                        .map_err(|error| bibliography_error(error.to_string()))?;
                    if let Some(&index) = stack.last() {
                        nodes[index].text.push_str(&value);
                    } else if !value.trim().is_empty() {
                        return Err(bibliography_error("text outside XML root"));
                    }
                }
                Event::CData(text) => {
                    let index = *stack
                        .last()
                        .ok_or_else(|| bibliography_error("CDATA outside XML root"))?;
                    nodes[index].opaque_scalar = true;
                    nodes[index]
                        .text
                        .push_str(&oxoffice_core::xml_text::normalize_line_ends(
                            &text
                                .decode()
                                .map_err(|error| bibliography_error(error.to_string()))?,
                        ));
                }
                Event::GeneralRef(reference) => {
                    let index = *stack
                        .last()
                        .ok_or_else(|| bibliography_error("reference outside XML root"))?;
                    let value = reference
                        .decode()
                        .map_err(|error| bibliography_error(error.to_string()))?;
                    let spelling = format!("&{value};");
                    let decoded = quick_xml::escape::unescape(&spelling)
                        .map_err(|error| bibliography_error(error.to_string()))?;
                    nodes[index].text.push_str(&decoded);
                }
                Event::Comment(_) | Event::PI(_) => {
                    if let Some(&index) = stack.last() {
                        nodes[index].opaque_scalar = true;
                    }
                }
                Event::DocType(_) => return Err(bibliography_error("DOCTYPE is unsupported")),
                Event::Decl(_) if start == 0 && roots == 0 => {}
                Event::Decl(_) => return Err(bibliography_error("misplaced XML declaration")),
                Event::Eof => break,
            }
            buffer.clear();
        }
        if roots != 1 || !stack.is_empty() {
            return Err(bibliography_error("incomplete XML root"));
        }
        Ok(Self { nodes })
    }

    fn bibliography_children(&self, index: usize, name: &str) -> Vec<usize> {
        self.nodes[index]
            .children
            .iter()
            .copied()
            .filter(|&child| {
                self.nodes[child].namespace == BIB_NS && self.nodes[child].local == name
            })
            .collect()
    }

    fn scalar(&self, index: usize) -> Result<&str> {
        let node = &self.nodes[index];
        if !node.children.is_empty() {
            return Err(bibliography_error("mixed source scalar content"));
        }
        Ok(&node.text)
    }

    pub(super) fn optional_scalar(&self, index: usize, name: &str) -> Result<Option<String>> {
        let children = self.bibliography_children(index, name);
        match children.as_slice() {
            [] => Ok(None),
            [child] => Ok(Some(self.scalar(*child)?.to_owned())),
            _ => Err(bibliography_error(format!("ambiguous repeated {name}"))),
        }
    }

    fn source_info(&self, index: usize) -> Result<BibliographySourceInfo> {
        let mut diagnostics = Vec::new();
        let mut identity_scalar = |name| match self.optional_scalar(index, name) {
            Ok(value) => value,
            Err(error) => {
                diagnostics.push(error.to_string());
                None
            }
        };
        let tag = identity_scalar("Tag").unwrap_or_default();
        let guid = identity_scalar("Guid");
        let source_type = identity_scalar("SourceType");
        let supported = if diagnostics.is_empty() {
            match self.supported_source(index, &tag, guid.as_deref(), source_type.as_deref(), true)
            {
                Ok(source) => source,
                Err(error) => {
                    diagnostics.push(error.to_string());
                    None
                }
            }
        } else {
            None
        };
        Ok(BibliographySourceInfo {
            tag,
            guid,
            source_type,
            supported,
            diagnostics,
        })
    }

    pub(super) fn supported_source(
        &self,
        index: usize,
        tag: &str,
        guid: Option<&str>,
        kind: Option<&str>,
        project_locale: bool,
    ) -> Result<Option<BibliographySource>> {
        let Some(kind) = kind.and_then(BibliographySourceKind::from_xml_name) else {
            return Ok(None);
        };
        let Some(guid) = guid.filter(|value| !value.trim().is_empty()) else {
            return Ok(None);
        };
        if tag.trim().is_empty() {
            return Ok(None);
        }
        let locale = if project_locale {
            self.optional_scalar(index, "LCID")?
                .map(|value| source_locale_number(&value))
                .transpose()?
        } else {
            None
        };
        let mut contributors = Vec::new();
        let mut properties = Vec::new();
        for &child in &self.nodes[index].children {
            let node = &self.nodes[child];
            if node.namespace != BIB_NS {
                continue;
            }
            if let Some(field) = BibliographySourceField::from_xml_name(&node.local) {
                properties.push(BibliographyProperty {
                    field,
                    value: self.scalar(child)?.to_owned(),
                });
            } else if node.local == "Author" {
                for &role_node in &node.children {
                    let role = &self.nodes[role_node];
                    if role.namespace != BIB_NS {
                        continue;
                    }
                    let Some(role_kind) = BibliographyContributorRole::from_xml_name(&role.local)
                    else {
                        continue;
                    };
                    // Empty optional Author/Performer containers remain raw, not invented empty People.
                    let known = role
                        .children
                        .iter()
                        .copied()
                        .filter(|&child| self.nodes[child].namespace == BIB_NS)
                        .collect::<Vec<_>>();
                    if known.is_empty()
                        && matches!(
                            role_kind,
                            BibliographyContributorRole::Author
                                | BibliographyContributorRole::Performer
                        )
                    {
                        continue;
                    }
                    let [value_node] = known.as_slice() else {
                        return Err(bibliography_error("invalid contributor choice"));
                    };
                    let value_node = *value_node;
                    let value = match self.nodes[value_node].local.as_str() {
                        "Corporate"
                            if matches!(
                                role_kind,
                                BibliographyContributorRole::Author
                                    | BibliographyContributorRole::Performer
                            ) =>
                        {
                            BibliographyAuthor::Corporate(self.scalar(value_node)?.to_owned())
                        }
                        "NameList" => {
                            let people = self.bibliography_children(value_node, "Person");
                            if people.is_empty() {
                                return Err(bibliography_error("NameList requires Person"));
                            }
                            let mut result = Vec::new();
                            for person in people {
                                let mut result_person = BibliographyPerson {
                                    first: Vec::new(),
                                    middle: Vec::new(),
                                    last: Vec::new(),
                                };
                                let mut particle = 0;
                                for &component in &self.nodes[person].children {
                                    let node = &self.nodes[component];
                                    if node.namespace != BIB_NS {
                                        continue;
                                    }
                                    let next = match node.local.as_str() {
                                        "Last" => 0,
                                        "First" => 1,
                                        "Middle" => 2,
                                        _ => {
                                            return Err(bibliography_error(
                                                "unknown standard Person member",
                                            ));
                                        }
                                    };
                                    if next < particle {
                                        return Err(bibliography_error("Person child order"));
                                    }
                                    particle = next;
                                    let destination = match next {
                                        0 => &mut result_person.last,
                                        1 => &mut result_person.first,
                                        _ => &mut result_person.middle,
                                    };
                                    destination.push(self.scalar(component)?.to_owned());
                                }
                                result.push(result_person);
                            }
                            BibliographyAuthor::People(result)
                        }
                        _ => return Err(bibliography_error("invalid contributor value")),
                    };
                    contributors.push(BibliographyContributor {
                        role: role_kind,
                        value,
                    });
                }
            }
        }
        Ok(Some(BibliographySource {
            tag: tag.to_owned(),
            guid: guid.to_owned(),
            kind,
            locale,
            contributors,
            properties,
        }))
    }
}

pub(super) fn internal_target(
    package: &OpcPackage,
    owner: &str,
    relationship: &oxoffice_opc::Relationship,
) -> Result<String> {
    if relationship
        .target_mode
        .as_deref()
        .is_some_and(|mode| mode != "Internal")
        || !crate::embedded::relationship_target_is_normalized_pack_uri(&relationship.target)
    {
        return Err(bibliography_error("invalid internal custom XML edge"));
    }
    let target = OpcPackage::resolve_rel_target(owner, &relationship.target);
    if !package.contains_part(&target) {
        return Err(bibliography_error("dangling custom XML edge"));
    }
    Ok(target)
}

pub(super) fn bibliography_collection(
    package: &OpcPackage,
    owner: &str,
) -> Result<Option<BibliographyCollection>> {
    let Some(relationships) = package.get_part_rels(owner) else {
        return Ok(None);
    };
    let mut result = None;
    let mut seen_parts = HashSet::new();
    let mut seen_properties = HashSet::new();
    let mut seen_store_ids = HashSet::new();
    for relationship in relationships
        .items
        .iter()
        .filter(|edge| edge.rel_type == CUSTOM_XML_REL)
    {
        let part = internal_target(package, owner, relationship)?;
        if !seen_parts.insert(part.to_ascii_lowercase()) {
            return Err(bibliography_error("duplicate custom XML collection edge"));
        }
        let xml = package
            .get_part(&part)
            .ok_or_else(|| bibliography_error("missing custom XML part"))?;
        let parsed = SourceXml::parse(xml)?;
        let root = &parsed.nodes[0];
        let is_bibliography = root.namespace == BIB_NS && root.local == "Sources";
        if is_bibliography && result.is_some() {
            return Err(bibliography_error("ambiguous source collection"));
        }
        if is_bibliography
            && package.content_types.content_type_for(&part) != Some("application/xml")
        {
            return Err(bibliography_error("wrong bibliography source content type"));
        }
        let properties = package
            .get_part_rels(&part)
            .map(|rels| {
                rels.items
                    .iter()
                    .filter(|edge| edge.rel_type == CUSTOM_XML_PROPS_REL)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if properties.len() > 1 {
            return Err(bibliography_error("ambiguous source properties edges"));
        }
        if let Some(properties) = properties.first() {
            let properties_part = internal_target(package, &part, properties)?;
            if !seen_properties.insert(properties_part.to_ascii_lowercase()) {
                return Err(bibliography_error("shared custom XML properties owner"));
            }
            if package.content_types.content_type_for(&properties_part)
                != Some(CUSTOM_XML_PROPS_TYPE)
            {
                return Err(bibliography_error(
                    "wrong custom XML properties content type",
                ));
            }
            let properties_xml = package
                .get_part(&properties_part)
                .ok_or_else(|| bibliography_error("missing properties part"))?;
            SourceXml::parse(properties_xml)?;
            let store_id = crate::content_control::parse_store_item_id(properties_xml)?
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| bibliography_error("missing custom XML store identity"))?;
            if !seen_store_ids.insert(crate::content_control::normalize_store_item_id(&store_id)) {
                return Err(bibliography_error(
                    "conflicting custom XML store identities",
                ));
            }
        }
        if !is_bibliography {
            continue;
        }
        let mut sources = Vec::new();
        for index in parsed.bibliography_children(0, "Source") {
            sources.push((index, parsed.source_info(index)?));
        }
        result = Some(BibliographyCollection {
            part,
            xml: xml.to_vec(),
            parsed,
            sources,
        });
    }
    Ok(result)
}

pub(super) fn validate_source(source: &BibliographySource) -> Result<()> {
    if let Some(locale) = source.locale {
        validate_locale_number(locale)?;
    }
    if source.tag.trim().is_empty() {
        return Err(bibliography_error("empty source tag"));
    }
    let guid = source
        .guid
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .unwrap_or(&source.guid);
    if guid.len() != 36
        || !guid.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err(bibliography_error("invalid source GUID"));
    }
    for value in [&source.tag, &source.guid]
        .into_iter()
        .chain(source.properties.iter().map(|property| &property.value))
    {
        oxoffice_core::xml::reject_non_xml_characters("bibliography source", value)
            .map_err(|error| bibliography_error(error.to_string()))?;
    }
    for contributor in &source.contributors {
        match &contributor.value {
            BibliographyAuthor::Corporate(value) => {
                if !matches!(
                    contributor.role,
                    BibliographyContributorRole::Author | BibliographyContributorRole::Performer
                ) {
                    return Err(bibliography_error(
                        "corporate value is illegal for contributor role",
                    ));
                }
                oxoffice_core::xml::reject_non_xml_characters("corporate contributor", value)
                    .map_err(|error| bibliography_error(error.to_string()))?;
            }
            BibliographyAuthor::People(people) => {
                if people.is_empty() {
                    return Err(bibliography_error("NameList requires at least one Person"));
                }
                for person in people {
                    for value in person
                        .last
                        .iter()
                        .chain(&person.first)
                        .chain(&person.middle)
                    {
                        oxoffice_core::xml::reject_non_xml_characters("bibliography name", value)
                            .map_err(|error| bibliography_error(error.to_string()))?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn source_text_xml(value: &str) -> String {
    quick_xml::escape::escape(value).replace('\r', "&#13;")
}

fn source_element(name: &str, text: &str) -> String {
    format!(
        "<b:{name} xmlns:b=\"{BIB_NS}\">{}</b:{name}>",
        source_text_xml(text)
    )
}

fn contributor_value_xml(value: &BibliographyAuthor) -> String {
    match value {
        BibliographyAuthor::Corporate(value) => source_element("Corporate", value),
        BibliographyAuthor::People(people) => {
            let mut xml = format!("<b:NameList xmlns:b=\"{BIB_NS}\">");
            for person in people {
                xml.push_str("<b:Person>");
                for (name, values) in [
                    ("Last", &person.last),
                    ("First", &person.first),
                    ("Middle", &person.middle),
                ] {
                    for value in values {
                        xml.push_str(&source_element(name, value));
                    }
                }
                xml.push_str("</b:Person>");
            }
            xml.push_str("</b:NameList>");
            xml
        }
    }
}

fn contributor_xml(contributor: &BibliographyContributor) -> String {
    let name = contributor.role.xml_name();
    format!(
        "<b:{name} xmlns:b=\"{BIB_NS}\">{}</b:{name}>",
        contributor_value_xml(&contributor.value)
    )
}

pub(super) fn authored_source_xml(source: &BibliographySource) -> String {
    let mut xml = format!("<b:Source xmlns:b=\"{BIB_NS}\">");
    xml.push_str(&source_element("Tag", &source.tag));
    xml.push_str(&source_element("SourceType", source.kind.xml_name()));
    xml.push_str(&source_element("Guid", &source.guid));
    if let Some(locale) = source.locale {
        xml.push_str(&source_element("LCID", &locale.to_string()));
    }
    if !source.contributors.is_empty() {
        xml.push_str("<b:Author>");
        for contributor in &source.contributors {
            xml.push_str(&contributor_xml(contributor));
        }
        xml.push_str("</b:Author>");
    }
    for property in &source.properties {
        xml.push_str(&source_element(property.field.xml_name(), &property.value));
    }
    xml.push_str("</b:Source>");
    xml
}

pub(super) fn unique_collection_identities(collection: &BibliographyCollection) -> Result<()> {
    let mut tags = HashSet::new();
    let mut guids = HashSet::new();
    for (index, source) in &collection.sources {
        for name in ["Tag", "Guid", "SourceType", "LCID"] {
            if collection.parsed.bibliography_children(*index, name).len() > 1 {
                return Err(bibliography_error(format!(
                    "ambiguous repeated source identity {name}"
                )));
            }
        }
        if !source.tag.is_empty() && !tags.insert(source.tag.as_str()) {
            return Err(bibliography_error("duplicate source tag"));
        }
        if let Some(guid) = &source.guid
            && !guids.insert(crate::content_control::normalize_store_item_id(guid))
        {
            return Err(bibliography_error("duplicate source GUID"));
        }
    }
    Ok(())
}

pub(super) fn apply_source_edits(
    xml: &[u8],
    mut edits: Vec<(Range<usize>, Vec<u8>)>,
) -> Result<Vec<u8>> {
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let mut output = Vec::new();
    let mut cursor = 0;
    for (range, replacement) in edits {
        if range.start < cursor || range.end < range.start || range.end > xml.len() {
            return Err(bibliography_error("overlapping or stale source edits"));
        }
        output.extend_from_slice(&xml[cursor..range.start]);
        output.extend_from_slice(&replacement);
        cursor = range.end;
    }
    output.extend_from_slice(&xml[cursor..]);
    Ok(output)
}

pub(super) fn append_to_source_node(
    xml: &[u8],
    node: &SourceXmlNode,
    content: &[u8],
) -> Result<(Range<usize>, Vec<u8>)> {
    if node.empty {
        let tag = oxoffice_core::xml_element::start_tag(&xml[node.full.start..])?;
        let mut replacement = xml[node.full.start..node.full.start + tag.attributes_end].to_vec();
        replacement.push(b'>');
        replacement.extend_from_slice(content);
        replacement.extend_from_slice(format!("</{}>", node.name).as_bytes());
        Ok((node.full.clone(), replacement))
    } else {
        Ok((node.content.end..node.content.end, content.to_vec()))
    }
}

fn source_scalar_edit(
    xml: &[u8],
    parsed: &SourceXml,
    index: usize,
    value: Option<&str>,
) -> Result<Option<(Range<usize>, Vec<u8>)>> {
    let node = &parsed.nodes[index];
    if value == Some(parsed.scalar(index)?) {
        return Ok(None);
    }
    if node.opaque_scalar || !node.children.is_empty() {
        return Err(bibliography_error(
            "replacement would discard producer scalar metadata",
        ));
    }
    if let Some(value) = value {
        if node.empty {
            return append_to_source_node(xml, node, source_text_xml(value).as_bytes()).map(Some);
        }
        Ok(Some((
            node.content.clone(),
            source_text_xml(value).as_bytes().to_vec(),
        )))
    } else {
        if !node.attributes.is_empty() {
            return Err(bibliography_error(
                "removal would discard producer attributes",
            ));
        }
        Ok(Some((node.full.clone(), Vec::new())))
    }
}

fn source_name_edits(node: &SourceXmlNode, local: &str) -> Vec<(Range<usize>, Vec<u8>)> {
    let name = node
        .name
        .rsplit_once(':')
        .map(|(prefix, _)| format!("{prefix}:{local}"))
        .unwrap_or_else(|| local.to_owned())
        .into_bytes();
    let mut edits = vec![(
        node.full.start + 1..node.full.start + 1 + node.name.len(),
        name.clone(),
    )];
    if !node.empty {
        edits.push((
            node.content.end + 2..node.content.end + 2 + node.name.len(),
            name,
        ));
    }
    edits
}

pub(super) fn replace_source_xml(
    collection: &BibliographyCollection,
    index: usize,
    source: &BibliographySource,
) -> Result<Vec<u8>> {
    let parsed = &collection.parsed;
    let previous = parsed
        .source_info(index)?
        .supported
        .ok_or_else(|| bibliography_error("source has unsupported or ambiguous modeled data"))?;
    if previous == *source {
        return Ok(collection.xml.clone());
    }
    let mut edits = Vec::new();
    let mut appended = String::new();
    for (name, desired) in [
        ("SourceType", Some(source.kind.xml_name().to_owned())),
        ("LCID", source.locale.map(|value| value.to_string())),
    ] {
        let children = parsed.bibliography_children(index, name);
        match children.as_slice() {
            [] => {
                if let Some(value) = desired {
                    appended.push_str(&source_element(name, &value));
                }
            }
            [child] => {
                if let Some(edit) =
                    source_scalar_edit(&collection.xml, parsed, *child, desired.as_deref())?
                {
                    edits.push(edit);
                }
            }
            _ => return Err(bibliography_error("ambiguous source identity")),
        }
    }
    let mut property_index = 0;
    for &child in &parsed.nodes[index].children {
        let node = &parsed.nodes[child];
        if node.namespace != BIB_NS {
            continue;
        }
        let Some(field) = BibliographySourceField::from_xml_name(&node.local) else {
            continue;
        };
        let replacement = source.properties.get(property_index);
        property_index += 1;
        if let Some(property) = replacement
            && property.field != field
        {
            if !node.attributes.is_empty() || node.opaque_scalar || !node.children.is_empty() {
                return Err(bibliography_error(
                    "property identity change would discard producer metadata",
                ));
            }
            if node.empty {
                let tag =
                    oxoffice_core::xml_element::start_tag(&collection.xml[node.full.start..])?;
                let mut updated = node
                    .name
                    .rsplit_once(':')
                    .map(|(prefix, _)| format!("{prefix}:{}", property.field.xml_name()))
                    .unwrap_or_else(|| property.field.xml_name().to_owned());
                updated = format!(
                    "<{updated}{}>{}</{updated}>",
                    std::str::from_utf8(
                        &collection.xml[node.full.start + node.name.len() + 1
                            ..node.full.start + tag.attributes_end]
                    )
                    .map_err(|_| bibliography_error("invalid property XML"))?,
                    source_text_xml(&property.value)
                );
                edits.push((node.full.clone(), updated.into_bytes()));
                continue;
            }
            edits.extend(source_name_edits(node, property.field.xml_name()));
        }
        if let Some(edit) = source_scalar_edit(
            &collection.xml,
            parsed,
            child,
            replacement.map(|property| property.value.as_str()),
        )? {
            edits.push(edit);
        }
    }
    for property in source.properties.iter().skip(property_index) {
        appended.push_str(&source_element(property.field.xml_name(), &property.value));
    }
    if source.contributors != previous.contributors {
        // Contributor containers can be interleaved with scalar properties.
        // Patch an owned role in place and retain each surrounding raw Author.
        let mut previous_index = 0;
        let mut replacement_index = 0;
        for author in parsed.bibliography_children(index, "Author") {
            for &role in &parsed.nodes[author].children {
                let node = &parsed.nodes[role];
                if node.namespace != BIB_NS {
                    continue;
                }
                let Some(role_kind) = BibliographyContributorRole::from_xml_name(&node.local)
                else {
                    continue;
                };
                if node.children.is_empty()
                    && matches!(
                        role_kind,
                        BibliographyContributorRole::Author
                            | BibliographyContributorRole::Performer
                    )
                {
                    continue;
                }
                let old = previous
                    .contributors
                    .get(previous_index)
                    .ok_or_else(|| bibliography_error("stale contributor inventory"))?;
                previous_index += 1;
                let new = source.contributors.get(replacement_index);
                replacement_index += usize::from(new.is_some());
                if new == Some(old) {
                    continue;
                }
                let mut pending = vec![role];
                while let Some(index) = pending.pop() {
                    let node = &parsed.nodes[index];
                    if node.namespace != BIB_NS || !node.attributes.is_empty() || node.opaque_scalar
                    {
                        return Err(bibliography_error(
                            "replacement would discard contributor producer metadata",
                        ));
                    }
                    pending.extend(node.children.iter().copied());
                }
                if let Some(new) = new {
                    if new.role != role_kind {
                        edits.extend(source_name_edits(node, new.role.xml_name()));
                    }
                    edits.push((
                        node.content.clone(),
                        contributor_value_xml(&new.value).into_bytes(),
                    ));
                } else {
                    edits.push((node.full.clone(), Vec::new()));
                }
            }
        }
        if replacement_index < source.contributors.len() {
            appended.push_str(&format!("<b:Author xmlns:b=\"{BIB_NS}\">"));
            for contributor in &source.contributors[replacement_index..] {
                appended.push_str(&contributor_xml(contributor));
            }
            appended.push_str("</b:Author>");
        }
    }
    if !appended.is_empty() {
        edits.push(append_to_source_node(
            &collection.xml,
            &parsed.nodes[index],
            appended.as_bytes(),
        )?);
    }
    apply_source_edits(&collection.xml, edits)
}

pub(crate) struct StagedCustomXmlStore {
    pub(crate) part_name: String,
    /// Read by the debug proof of the in-place store addition.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub(crate) item_id: String,
}

/// Allocate one custom XML item, its properties part, both relationships and
/// both content-type overrides. Bibliography and the public store method share
/// this sequence so an empty schema list keeps the bibliography properties bytes.
pub(crate) fn stage_custom_xml_store(
    candidate: &mut Document,
    item_xml: &[u8],
    schema_uris: &[&str],
) -> Result<StagedCustomXmlStore> {
    if schema_uris.iter().any(|uri| uri.is_empty()) {
        return Err(Error::Other("custom XML schema URI is empty".to_owned()));
    }
    candidate
        .identifiers
        .observe_package_graph(&candidate.package)?;
    let part = candidate
        .identifiers
        .reserve_preferred_part_name("/customXml/item1.xml")?;
    candidate
        .package
        .content_types
        .add_override(&part, "application/xml");
    let owner = candidate.doc_part_name.clone();
    candidate.add_relative_internal_relationship_checked(&owner, CUSTOM_XML_REL, &part)?;
    let properties = candidate
        .identifiers
        .reserve_preferred_part_name("/customXml/itemProps1.xml")?;
    let identity = free_store_identity(&candidate.package)?;
    candidate
        .package
        .set_part(&properties, datastore_properties(&identity, schema_uris));
    candidate
        .package
        .content_types
        .add_override(&properties, CUSTOM_XML_PROPS_TYPE);
    candidate.add_relative_internal_relationship_checked(
        &part,
        CUSTOM_XML_PROPS_REL,
        &properties,
    )?;
    candidate.package.set_part(&part, item_xml.to_vec());
    Ok(StagedCustomXmlStore {
        part_name: part,
        item_id: identity,
    })
}

/// Add one custom XML item and its properties part to `document` in place,
/// with the part names, relationship ids and identity
/// [`stage_custom_xml_store`] gives a staged copy. Every check and
/// reservation runs first; the writes after them cannot fail. The caller
/// holds the identifiers journal open, which undoes the reservations of a
/// refusal.
pub(crate) fn add_custom_xml_store_in_place(
    document: &mut Document,
    item_xml: &[u8],
    schema_uris: &[&str],
) -> Result<String> {
    document
        .identifiers
        .observe_package_graph(&document.package)?;
    let part = document
        .identifiers
        .reserve_preferred_part_name("/customXml/item1.xml")?;
    let owner = document.doc_part_name.clone();
    let item_relationship = document
        .identifiers
        .reserve_relationship_id_checked(&owner)?;
    let properties = document
        .identifiers
        .reserve_preferred_part_name("/customXml/itemProps1.xml")?;
    let identity = free_store_identity(&document.package)?;
    let properties_relationship = document
        .identifiers
        .reserve_relationship_id_checked(&part)?;

    document.add_override_journaled(&part, "application/xml");
    document.add_relationship_journaled(
        Some(&owner),
        &item_relationship,
        CUSTOM_XML_REL,
        &crate::document::relative_target(&owner, &part),
    );
    // Recorded before the first write: a panic after it is not undone.
    document.identifiers.record_written_to_package();
    document
        .package
        .set_part(&properties, datastore_properties(&identity, schema_uris));
    document.add_override_journaled(&properties, CUSTOM_XML_PROPS_TYPE);
    document.add_relationship_journaled(
        Some(&part),
        &properties_relationship,
        CUSTOM_XML_PROPS_REL,
        &crate::document::relative_target(&part, &properties),
    );
    document.package.set_part(&part, item_xml.to_vec());
    Ok(identity)
}

/// The first store identity of the generated series that no custom XML
/// properties part of the package holds. It reads the properties parts the
/// package relates, not the body.
fn free_store_identity(package: &OpcPackage) -> Result<String> {
    let mut occupied = HashSet::new();
    for (name, relationships) in &package.part_rels {
        for relationship in relationships
            .items
            .iter()
            .filter(|edge| edge.rel_type == CUSTOM_XML_PROPS_REL)
        {
            let target = internal_target(package, name, relationship)?;
            let xml = package
                .get_part(&target)
                .ok_or_else(|| bibliography_error("missing custom XML properties"))?;
            if let Some(identity) = crate::content_control::parse_store_item_id(xml)? {
                occupied.insert(crate::content_control::normalize_store_item_id(&identity));
            }
        }
    }
    (0u64..=0xffffffffffff)
        .map(|index| format!("{{00000000-0000-4000-8000-{index:012X}}}"))
        .find(|value| !occupied.contains(&crate::content_control::normalize_store_item_id(value)))
        .ok_or_else(|| bibliography_error("custom XML store identity space exhausted"))
}

fn datastore_properties(identity: &str, schema_uris: &[&str]) -> Vec<u8> {
    let refs = if schema_uris.is_empty() {
        "<ds:schemaRefs/>".to_owned()
    } else {
        let mut body = String::from("<ds:schemaRefs>");
        for uri in schema_uris {
            body.push_str("<ds:schemaRef ds:uri=\"");
            body.push_str(&xml_attribute(uri));
            body.push_str("\"/>");
        }
        body.push_str("</ds:schemaRefs>");
        body
    };
    format!("<ds:datastoreItem xmlns:ds=\"http://schemas.openxmlformats.org/officeDocument/2006/customXml\" ds:itemID=\"{identity}\">{refs}</ds:datastoreItem>")
        .into_bytes()
}

fn xml_attribute(value: &str) -> String {
    oxoffice_core::xml::escape_attribute_value(value).into_owned()
}

pub(super) fn create_collection(candidate: &mut Document) -> Result<String> {
    Ok(stage_custom_xml_store(
        candidate,
        format!("<b:Sources xmlns:b=\"{BIB_NS}\"></b:Sources>").as_bytes(),
        &[],
    )?
    .part_name)
}

pub(super) fn publish_source_collection(
    document: &mut Document,
    candidate: Document,
) -> Result<()> {
    let collection = bibliography_collection(&candidate.package, &candidate.doc_part_name)?
        .ok_or_else(|| bibliography_error("staged collection disappeared"))?;
    unique_collection_identities(&collection)?;
    let mut candidate = candidate.reopen_prepared_staged()?;
    candidate.invalidate_layout();
    document.commit_staged_mutation(candidate);
    Ok(())
}
