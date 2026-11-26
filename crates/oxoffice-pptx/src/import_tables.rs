//! Slide import: the table styles imported tables need, merged into this presentation's style list.

use super::*;

impl Presentation {
    /// Carries selected classic comments and their required parent records.
    /// Original numeric identities and external scopes are rebound together.
    /// Transfers selected style definitions without changing destination defaults.
    pub(super) fn import_table_style_dependencies(
        &mut self,
        source: &Presentation,
        source_package: &mut OpcPackage,
        parts: &HashSet<String>,
        copies: &mut HashMap<String, String>,
    ) -> Result<HashMap<Option<String>, String>> {
        let mut selected = HashSet::new();
        for part in parts {
            import_table_part(source_package, part, &mut selected, None)?;
        }
        if selected.is_empty() {
            return Ok(HashMap::new());
        }
        let source_part = import_table_style_part(source_package, &source.presentation_part)?;
        let source_xml = source_part
            .as_deref()
            .map(|part| required_part(source_package, part).map(<[u8]>::to_vec))
            .transpose()?;
        let source_styles = source_xml
            .as_deref()
            .map(import_table_styles)
            .transpose()
            .map_err(|error| {
                invalid_presentation_mutation("import table styles", error.to_string())
            })?;
        let destination_part = import_table_style_part(&self.package, &self.presentation_part)?;
        let mut destination_xml = destination_part
            .as_deref()
            .map(|part| required_part(&self.package, part).map(<[u8]>::to_vec))
            .transpose()?
            .unwrap_or_else(|| format!(r#"<a:tblStyleLst xmlns:a="{A_NS}"/>"#).into_bytes());
        let original_table_xml = destination_xml.clone();
        if let Some(source_styles) = &source_styles {
            destination_xml = merge_import_table_root_metadata(&destination_xml, source_styles)
                .map_err(|error| {
                    invalid_presentation_mutation("import table styles", error.to_string())
                })?;
        }
        let destination_styles = import_table_styles(&destination_xml).map_err(|error| {
            invalid_presentation_mutation("import table styles", error.to_string())
        })?;
        let mut used = destination_styles
            .styles
            .iter()
            .map(|(id, _)| id.to_ascii_uppercase())
            .collect::<HashSet<_>>();
        let mut mapping = HashMap::new();
        let mut appended = Vec::new();
        let source_relationships = source_part
            .as_deref()
            .and_then(|part| source_package.get_part_rels(part))
            .cloned()
            .unwrap_or_default();
        let destination_owner = destination_part
            .clone()
            .unwrap_or_else(|| fresh_related_part_name(&self.package, "/ppt/tableStyles1.xml"));
        let mut destination_relationships = self
            .package
            .get_part_rels(&destination_owner)
            .cloned()
            .unwrap_or_default();
        let original_resource_ids = destination_relationships
            .items
            .iter()
            .map(|relationship| relationship.id.clone())
            .collect::<HashSet<_>>();
        let mut resources = HashMap::new();
        let mut adopted_styles: HashMap<String, String> = HashMap::new();
        let mut selected = selected.into_iter().collect::<Vec<_>>();
        selected.sort();
        for selection in selected {
            let resolved = source_styles
                .as_ref()
                .and_then(|styles| {
                    selection
                        .as_deref()
                        .filter(|id| {
                            styles
                                .styles
                                .iter()
                                .any(|(candidate, _)| candidate.eq_ignore_ascii_case(id))
                        })
                        .or(styles.default.as_deref())
                })
                .or(selection.as_deref());
            let key = resolved.unwrap_or("").to_ascii_uppercase();
            if let Some(id) = adopted_styles.get(&key) {
                mapping.insert(selection, id.clone());
                continue;
            }
            let definition = source_styles.as_ref().and_then(|styles| {
                resolved.and_then(|id| {
                    styles
                        .styles
                        .iter()
                        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(id))
                })
            });
            let (preferred, mut fragment) = match definition {
                Some((id, xml)) => (
                    id.clone(),
                    close_import_table_style(xml, &source_styles.as_ref().unwrap().namespaces)
                        .map_err(|error| {
                            invalid_presentation_mutation("import table styles", error.to_string())
                        })?,
                ),
                None => {
                    if let Some(id) = resolved {
                        if let Some(style) = oxoffice_drawing::table::CT_TableStyle::builtin(id) {
                            if !used.contains(&id.to_ascii_uppercase()) {
                                let id = id.to_owned();
                                mapping.insert(selection, id);
                                continue;
                            }
                            let mut list = oxoffice_drawing::table::CT_TableStyleList::from_xml(
                                format!(r#"<a:tblStyleLst xmlns:a="{A_NS}"/>"#).as_bytes(),
                            )
                            .map_err(|error| {
                                invalid_presentation_mutation(
                                    "import table styles",
                                    error.to_string(),
                                )
                            })?;
                            list.styles.push(style);
                            let xml = list.to_xml().map_err(|error| {
                                invalid_presentation_mutation(
                                    "import table styles",
                                    error.to_string(),
                                )
                            })?;
                            let builtin = import_table_styles(&xml).map_err(|error| {
                                invalid_presentation_mutation(
                                    "import table styles",
                                    error.to_string(),
                                )
                            })?;
                            let fragment =
                                close_import_table_style(&builtin.styles[0].1, &builtin.namespaces)
                                    .map_err(|error| {
                                        invalid_presentation_mutation(
                                            "import table styles",
                                            error.to_string(),
                                        )
                                    })?;
                            (id.to_owned(), fragment)
                        } else {
                            let id = "{00000000-0000-4000-8000-000000000000}".to_owned();
                            (id.clone(), format!(r#"<a:tblStyle xmlns:a="{A_NS}" styleId="{id}" styleName="Unstyled"/>"#).into_bytes())
                        }
                    } else {
                        let id = "{00000000-0000-4000-8000-000000000000}".to_owned();
                        (id.clone(), format!(r#"<a:tblStyle xmlns:a="{A_NS}" styleId="{id}" styleName="Unstyled"/>"#).into_bytes())
                    }
                }
            };
            let source_scope = ImportTableStyleScope {
                package: source_package,
                part: source_part.as_deref().unwrap_or(""),
                relationships: &source_relationships,
                namespaces: source_styles
                    .as_ref()
                    .map_or(&[], |styles| styles.namespaces.as_slice()),
            };
            let destination_scope = ImportTableStyleScope {
                package: &self.package,
                part: &destination_owner,
                relationships: &destination_relationships,
                namespaces: &destination_styles.namespaces,
            };
            let mut equivalent = None;
            for (id, old) in &destination_styles.styles {
                if import_table_style_equal(&source_scope, &fragment, &destination_scope, old)? {
                    equivalent = Some(id.clone());
                    break;
                }
            }
            if let Some(id) = equivalent {
                adopted_styles.insert(key, id.clone());
                mapping.insert(selection, id);
                continue;
            }
            let ids = relationship_ids(&fragment).map_err(|error| {
                invalid_presentation_mutation("import table styles", error.to_string())
            })?;
            for id in ids {
                if resources.contains_key(&id) {
                    continue;
                }
                let source_scope = ImportTableStyleScope {
                    package: source_package,
                    part: source_part.as_deref().unwrap_or(""),
                    relationships: &source_relationships,
                    namespaces: source_styles
                        .as_ref()
                        .map_or(&[], |styles| styles.namespaces.as_slice()),
                };
                let destination_scope = ImportTableStyleScope {
                    package: &self.package,
                    part: &destination_owner,
                    relationships: &destination_relationships,
                    namespaces: &destination_styles.namespaces,
                };
                let mut reused = None;
                for relationship in &destination_relationships.items {
                    if import_table_resource_equal(
                        &source_scope,
                        &id,
                        &destination_scope,
                        &relationship.id,
                    )? {
                        reused = Some(relationship.id.clone());
                        break;
                    }
                }
                if let Some(reused) = reused {
                    resources.insert(id, reused);
                    continue;
                }
                let relationship = source_relationships.get_by_id(&id).ok_or_else(|| {
                    Error::MissingRelationship {
                        source_part: source_part.clone().unwrap_or_default(),
                        relationship_id: id.clone(),
                    }
                })?;
                let target = if relationship_is_external(relationship) {
                    relationship.target.clone()
                } else {
                    if relationship.rel_type != rel_types::IMAGE {
                        return Err(invalid_presentation_mutation(
                            "import table styles",
                            format!(
                                "{} has unsupported table-style resource {}",
                                source_part.as_deref().unwrap_or(""),
                                relationship.rel_type
                            ),
                        ));
                    }
                    let source_target = OpcPackage::resolve_rel_target(
                        source_part.as_deref().unwrap(),
                        &relationship.target,
                    );
                    preflight_internal_transfer_image(
                        source_package,
                        &source_target,
                        "import table styles",
                    )?;
                    let copied = if let Some(copied) = copies.get(&source_target) {
                        copied.clone()
                    } else {
                        if source_package
                            .get_part_rels(&source_target)
                            .is_some_and(|rels| {
                                rels.items
                                    .iter()
                                    .any(|relationship| !relationship_is_external(relationship))
                            })
                        {
                            return Err(invalid_presentation_mutation(
                                "import table styles",
                                format!(
                                    "{source_target} has unsupported internal image relationships"
                                ),
                            ));
                        }
                        let copied = fresh_related_part_name(&self.package, &source_target);
                        self.package.set_part(
                            &copied,
                            required_part(source_package, &source_target)?.to_vec(),
                        );
                        if let Some(content_type) = source_package
                            .content_types
                            .content_type_for(&source_target)
                        {
                            self.package
                                .content_types
                                .add_override(&copied, content_type);
                        }
                        if let Some(relationships) = source_package.get_part_rels(&source_target) {
                            self.package.set_part_rels(&copied, relationships.clone());
                        }
                        copies.insert(source_target, copied.clone());
                        copied
                    };
                    relative_part_target(&destination_owner, &copied)
                };
                let new_id = if relationship_is_external(relationship) {
                    destination_relationships.add_external(&relationship.rel_type, &target)
                } else {
                    destination_relationships.add(&relationship.rel_type, &target)
                };
                resources.insert(id, new_id);
            }
            fragment = rewrite_exact_rel_ids(&fragment, &resources).map_err(|error| {
                invalid_presentation_mutation("import table styles", error.to_string())
            })?;
            let same = destination_styles.styles.iter().find(|(_, old)| {
                set_import_style_id(old, &preferred).is_ok_and(|old| old == fragment)
            });
            let id = if let Some((id, _)) = same {
                id.clone()
            } else {
                let id = if used.insert(preferred.to_ascii_uppercase()) {
                    preferred
                } else {
                    let id = (1_u64..)
                        .map(|index| format!("{{00000000-0000-4000-8001-{index:012X}}}"))
                        .find(|id| !used.contains(id))
                        .expect("finite style identities leave an unused identifier");
                    used.insert(id.clone());
                    id
                };
                fragment = set_import_style_id(&fragment, &id).map_err(|error| {
                    invalid_presentation_mutation("import table styles", error.to_string())
                })?;
                appended.extend_from_slice(&fragment);
                id
            };
            adopted_styles.insert(key, id.clone());
            mapping.insert(selection, id);
        }
        let changed_metadata = original_table_xml != destination_xml;
        if !appended.is_empty() || changed_metadata {
            if destination_styles.empty {
                let start = destination_styles.start;
                let attributes_end = start
                    + oxoffice_core::xml_element::start_tag(&destination_xml[start..])
                        .map_err(|error| {
                            invalid_presentation_mutation("import table styles", error.to_string())
                        })?
                        .attributes_end;
                let mut xml = destination_xml[..attributes_end].to_vec();
                xml.push(b'>');
                xml.extend_from_slice(&appended);
                xml.extend_from_slice(format!("</{}>", destination_styles.name).as_bytes());
                xml.extend_from_slice(&destination_xml[destination_styles.head_end..]);
                destination_xml = xml;
            } else {
                destination_xml.splice(
                    destination_styles.insert..destination_styles.insert,
                    appended,
                );
            }
            self.package.set_part(&destination_owner, destination_xml);
            self.package.content_types.add_override(
                &destination_owner,
                "application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml",
            );
            if destination_part.is_none() {
                self.package
                    .part_rels
                    .entry(self.presentation_part.clone())
                    .or_default()
                    .add(
                        rel_types::TABLE_STYLES,
                        &relative_part_target(&self.presentation_part, &destination_owner),
                    );
            }
        }
        if !resources.is_empty() {
            let imported_items = source_relationships
                .items
                .iter()
                .filter_map(|relationship| {
                    resources
                        .get(&relationship.id)
                        .filter(|id| !original_resource_ids.contains(*id))
                        .map(|new_id| {
                            let mut copied = relationship.clone();
                            copied.id.clone_from(new_id);
                            copied.target = destination_relationships
                                .get_by_id(new_id)
                                .unwrap()
                                .target
                                .clone();
                            copied
                        })
                })
                .collect();
            let imported = source_relationships.rebind_for_import(imported_items, &resources)?;
            let existing = self
                .package
                .get_part_rels(&destination_owner)
                .cloned()
                .unwrap_or_default();
            let (merged, ids) = existing.merge_for_import(&imported)?;
            if ids.iter().any(|(old, new)| old != new) {
                return Err(invalid_presentation_mutation(
                    "import table styles",
                    "preallocated table resource relationship collision".into(),
                ));
            }
            self.package.set_part_rels(&destination_owner, merged);
        }
        for part in parts {
            let bytes =
                import_table_part(source_package, part, &mut HashSet::new(), Some(&mapping))?;
            source_package.set_part(part, bytes);
        }
        Ok(mapping)
    }
}

/// Raw direct style definitions retain producer spelling and opaque descendants.
pub(super) struct ImportTableStyleAttribute {
    uri: String,
    local: String,
    value: String,
    raw: Vec<u8>,
}

pub(super) fn import_table_styles(xml: &[u8]) -> std::result::Result<ImportTableStyles, OxmlError> {
    let origin = usize::from(xml.starts_with(b"\xef\xbb\xbf")) * 3;
    let mut reader = NsReader::from_reader(&xml[origin..]);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut pending = None;
    let mut extension_start = None;
    let mut result = None;
    let mut ids = HashSet::new();
    loop {
        let begin = origin + reader.buffer_position() as usize;
        let decoder = reader.decoder();
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let uri = import_namespace_uri(namespace, decoder)?;
        let end = origin + reader.buffer_position() as usize;
        let empty = matches!(event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                if depth == 0 {
                    if result.is_some()
                        || uri != A_NS
                        || start.local_name().as_ref() != b"tblStyleLst"
                    {
                        return Err(OxmlError::MissingElement("a:tblStyleLst".into()));
                    }
                    let mut default = None;
                    let mut namespaces = Vec::new();
                    let mut metadata = Vec::new();
                    let content: &[u8] = start.as_ref();
                    for attribute in start.attributes() {
                        let attribute = attribute?;
                        let key = std::str::from_utf8(attribute.key.as_ref())
                            .map_err(|error| OxmlError::InvalidValue(error.to_string()))?;
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?
                            .into_owned();
                        if key == "def" {
                            default = Some(value);
                        } else if key == "xmlns" {
                            namespaces.push((String::new(), value));
                        } else if let Some(prefix) = key.strip_prefix("xmlns:") {
                            namespaces.push((prefix.to_owned(), value));
                        } else {
                            let (namespace, local) =
                                reader.resolver().resolve_attribute(attribute.key);
                            let uri = import_namespace_uri(namespace, decoder)?;
                            let from = attribute.key.as_ref().as_ptr() as usize
                                - content.as_ptr() as usize;
                            let to = attribute.value.as_ref().as_ptr() as usize
                                - content.as_ptr() as usize
                                + attribute.value.len()
                                + 1;
                            metadata.push(ImportTableStyleAttribute {
                                uri,
                                local: String::from_utf8_lossy(local.as_ref()).into_owned(),
                                value,
                                raw: content[from..to].to_vec(),
                            });
                        }
                    }
                    result = Some(ImportTableStyles {
                        start: begin,
                        metadata,
                        opaque: Vec::new(),
                        default,
                        styles: Vec::new(),
                        namespaces,
                        close: end,
                        insert: end,
                        head_end: end,
                        name: String::from_utf8_lossy(start.name().as_ref()).into_owned(),
                        empty,
                    });
                } else if depth == 1 && uri == A_NS && start.local_name().as_ref() == b"tblStyle" {
                    let mut id = None;
                    for attribute in start.attributes() {
                        let attribute = attribute?;
                        if attribute.key.as_ref() == b"styleId" {
                            id = Some(
                                attribute
                                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?
                                    .into_owned(),
                            );
                        }
                    }
                    let id =
                        id.ok_or_else(|| OxmlError::MissingElement("a:tblStyle/@styleId".into()))?;
                    if !ids.insert(id.to_ascii_uppercase()) {
                        return Err(OxmlError::InvalidValue("duplicate table style id".into()));
                    }
                    if empty {
                        result
                            .as_mut()
                            .unwrap()
                            .styles
                            .push((id, xml[begin..end].to_vec()));
                    } else {
                        pending = Some((Some(id), begin));
                    }
                } else if depth == 1 {
                    if uri == A_NS && start.local_name().as_ref() == b"extLst" {
                        extension_start.get_or_insert(begin);
                    }
                    if empty {
                        result
                            .as_mut()
                            .unwrap()
                            .opaque
                            .push(xml[begin..end].to_vec());
                    } else {
                        pending = Some((None, begin));
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            XmlEvent::End(_) => {
                if depth == 2
                    && let Some((id, begin)) = pending.take()
                {
                    if let Some(id) = id {
                        result
                            .as_mut()
                            .unwrap()
                            .styles
                            .push((id, xml[begin..end].to_vec()));
                    } else {
                        result
                            .as_mut()
                            .unwrap()
                            .opaque
                            .push(xml[begin..end].to_vec());
                    }
                }
                if depth == 1 {
                    result.as_mut().unwrap().close = begin;
                }
                depth = depth.saturating_sub(1);
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut result = result.ok_or_else(|| OxmlError::MissingElement("a:tblStyleLst".into()))?;
    result.insert = extension_start.unwrap_or(result.close);
    Ok(result)
}

pub(super) fn close_import_table_style(
    xml: &[u8],
    namespaces: &[(String, String)],
) -> std::result::Result<Vec<u8>, OxmlError> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml);
    let event = reader.read_event()?;
    let empty = matches!(event, XmlEvent::Empty(_));
    let mut start = match event {
        XmlEvent::Start(start) | XmlEvent::Empty(start) => start.into_owned(),
        _ => return Err(OxmlError::MissingElement("a:tblStyle".into())),
    };
    let local = start
        .attributes()
        .map(|attribute| attribute.map(|attribute| attribute.key.as_ref().to_vec()))
        .collect::<std::result::Result<HashSet<_>, _>>()?;
    for (prefix, value) in namespaces {
        let key = if prefix.is_empty() {
            "xmlns".to_owned()
        } else {
            format!("xmlns:{prefix}")
        };
        if !local.contains(key.as_bytes()) {
            let escaped = quick_xml::escape::escape(value);
            start.push_attribute((key.as_str(), escaped.as_ref()));
        }
    }
    // An absent source default namespace is itself an ownership fact.
    if !namespaces.iter().any(|(prefix, _)| prefix.is_empty())
        && !local.contains(b"xmlns".as_slice())
    {
        start.push_attribute(("xmlns", ""));
    }
    let mut writer = XmlWriter::new(Vec::new());
    writer.write_event(if empty {
        XmlEvent::Empty(start)
    } else {
        XmlEvent::Start(start)
    })?;
    let mut closed = writer.into_inner();
    closed.extend_from_slice(&xml[reader.buffer_position() as usize..]);
    Ok(closed)
}

fn set_import_style_id(xml: &[u8], id: &str) -> std::result::Result<Vec<u8>, OxmlError> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml);
    let event = reader.read_event()?;
    let start = match event {
        XmlEvent::Start(start) | XmlEvent::Empty(start) => start,
        _ => return Err(OxmlError::MissingElement("a:tblStyle".into())),
    };
    let content: &[u8] = start.as_ref();
    for attribute in start.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == b"styleId" {
            let offset = attribute.value.as_ref().as_ptr() as usize - content.as_ptr() as usize + 1;
            let mut changed = xml[..offset].to_vec();
            changed.extend_from_slice(id.as_bytes());
            changed.extend_from_slice(&xml[offset + attribute.value.len()..]);
            return Ok(changed);
        }
    }
    Err(OxmlError::MissingElement("a:tblStyle/@styleId".into()))
}

fn merge_import_table_root_metadata(
    destination: &[u8],
    source: &ImportTableStyles,
) -> std::result::Result<Vec<u8>, OxmlError> {
    let original = import_table_styles(destination)?;
    let mut attributes = Vec::new();
    let mut namespace_attributes = Vec::new();
    for attribute in &source.metadata {
        if let Some(old) = original
            .metadata
            .iter()
            .find(|old| old.uri == attribute.uri && old.local == attribute.local)
        {
            if old.value != attribute.value {
                return Err(OxmlError::InvalidValue(
                    "conflicting table style root metadata".into(),
                ));
            }
            continue;
        }
        // Root metadata can contain producer-defined namespace-valued strings.
        // A new root binding cannot override another destination authority.
        for (prefix, uri) in &source.namespaces {
            if let Some((_, old)) = original.namespaces.iter().find(|(old, _)| old == prefix) {
                if old != uri {
                    return Err(OxmlError::InvalidValue(
                        "conflicting table style root namespace context".into(),
                    ));
                }
            } else {
                let key = if prefix.is_empty() {
                    "xmlns".to_owned()
                } else {
                    format!("xmlns:{prefix}")
                };
                if !namespace_attributes.iter().any(|(old, _)| old == &key) {
                    namespace_attributes.push((key, uri.clone()));
                }
            }
        }
        attributes.push(attribute.raw.clone());
    }
    let origin = usize::from(destination.starts_with(b"\xef\xbb\xbf")) * 3;
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(&destination[origin..]);
    let (start, begin) = loop {
        let begin = origin + reader.buffer_position() as usize;
        if let XmlEvent::Start(start) | XmlEvent::Empty(start) = reader.read_event()? {
            break (start.into_owned(), begin);
        }
    };
    let mut start = start;
    for (key, value) in namespace_attributes {
        let value = quick_xml::escape::escape(&value);
        start.push_attribute((key.as_str(), value.as_ref()));
    }
    let mut writer = XmlWriter::new(Vec::new());
    writer.write_event(XmlEvent::Start(start))?;
    let head = writer.into_inner();
    // The rewritten head, then the root's own close and content as written.
    let head_end = oxoffice_core::xml_element::start_tag(&head)?.attributes_end;
    let close =
        begin + oxoffice_core::xml_element::start_tag(&destination[begin..])?.attributes_end;
    let mut changed = destination[..begin].to_vec();
    changed.extend_from_slice(&head[..head_end]);
    for raw in attributes {
        changed.push(b' ');
        changed.extend_from_slice(&raw);
    }
    changed.extend_from_slice(&destination[close..]);
    let scope = import_table_styles(&changed)?;
    let mut opaque = Vec::new();
    for raw in &source.opaque {
        let closed = close_import_table_style(raw, &source.namespaces)?;
        if !scope.opaque.contains(&closed) {
            opaque.extend(closed);
        }
    }
    if !opaque.is_empty() {
        if scope.empty {
            let mut result = changed[..scope.start].to_vec();
            result.extend(
                oxoffice_core::xml_element::replace_content(&changed[scope.start..], &opaque)?.0,
            );
            changed = result;
        } else {
            changed.splice(scope.close..scope.close, opaque);
        }
    }
    Ok(changed)
}

fn imported_table_style_ids(children: &[ShapeTreeChild], ids: &mut HashSet<Option<String>>) {
    let mut pending = children.iter().collect::<Vec<_>>();
    while let Some(child) = pending.pop() {
        match child {
            ShapeTreeChild::GraphicFrame(frame) => {
                if let GraphicDataPayload::Table(table) = frame.graphic_data.payload() {
                    ids.insert(
                        table
                            .properties
                            .as_ref()
                            .and_then(|properties| properties.style_id.clone()),
                    );
                }
            }
            ShapeTreeChild::GroupShape(group) => pending.extend(group.children.iter()),
            _ => {}
        }
    }
}

fn remap_imported_table_styles(
    children: &mut [ShapeTreeChild],
    ids: &HashMap<Option<String>, String>,
) {
    let mut pending = children.iter_mut().collect::<Vec<_>>();
    while let Some(child) = pending.pop() {
        match child {
            ShapeTreeChild::GraphicFrame(frame) => {
                if let Some(table) = frame.graphic_data.table_mut() {
                    let old = table
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.style_id.clone());
                    if let Some(id) = ids.get(&old) {
                        table_properties_mut(table).style_id = Some(id.clone());
                    }
                }
            }
            ShapeTreeChild::GroupShape(group) => pending.extend(group.children.iter_mut()),
            _ => {}
        }
    }
}

fn import_table_part(
    package: &OpcPackage,
    part: &str,
    selected: &mut HashSet<Option<String>>,
    mapping: Option<&HashMap<Option<String>, String>>,
) -> Result<Vec<u8>> {
    let xml = required_part(package, part)?;
    let mut local = HashSet::new();
    let result = match package.content_types.content_type_for(part) {
        Some(content_types::SLIDE) => {
            let mut model = CT_Slide::from_xml(xml)
                .map_err(|error| footer_malformed(part, error.to_string()))?;
            imported_table_style_ids(&model.common_slide_data.shape_tree.children, &mut local);
            if let Some(mapping) = mapping {
                remap_imported_table_styles(
                    &mut model.common_slide_data.shape_tree.children,
                    mapping,
                );
            }
            model.to_xml()
        }
        Some(content_types::SLIDE_LAYOUT) => {
            let mut model = CT_SlideLayout::from_xml(xml)
                .map_err(|error| footer_malformed(part, error.to_string()))?;
            imported_table_style_ids(&model.common_slide_data.shape_tree.children, &mut local);
            if let Some(mapping) = mapping {
                remap_imported_table_styles(
                    &mut model.common_slide_data.shape_tree.children,
                    mapping,
                );
            }
            model.to_xml()
        }
        Some(content_types::SLIDE_MASTER) => {
            let mut model = CT_SlideMaster::from_xml(xml)
                .map_err(|error| footer_malformed(part, error.to_string()))?;
            imported_table_style_ids(&model.common_slide_data.shape_tree.children, &mut local);
            if let Some(mapping) = mapping {
                remap_imported_table_styles(
                    &mut model.common_slide_data.shape_tree.children,
                    mapping,
                );
            }
            model.to_xml()
        }
        Some(content_types::NOTES_SLIDE) => {
            let mut model = CT_NotesSlide::from_xml(xml)
                .map_err(|error| footer_malformed(part, error.to_string()))?;
            imported_table_style_ids(&model.common_slide_data.shape_tree.children, &mut local);
            if let Some(mapping) = mapping {
                remap_imported_table_styles(
                    &mut model.common_slide_data.shape_tree.children,
                    mapping,
                );
            }
            model.to_xml()
        }
        _ => return Ok(xml.to_vec()),
    };
    selected.extend(local.iter().cloned());
    if mapping.is_none_or(|mapping| {
        local
            .iter()
            .all(|id| mapping.get(id).is_none_or(|new| id.as_ref() == Some(new)))
    }) {
        return Ok(xml.to_vec());
    }
    result.map_err(|error| footer_malformed(part, error.to_string()))
}

struct ImportTableStyleScope<'a> {
    package: &'a OpcPackage,
    part: &'a str,
    relationships: &'a Relationships,
    namespaces: &'a [(String, String)],
}

fn import_table_resource_equal(
    source: &ImportTableStyleScope<'_>,
    source_id: &str,
    destination: &ImportTableStyleScope<'_>,
    destination_id: &str,
) -> Result<bool> {
    let Some(left) = source.relationships.get_by_id(source_id) else {
        return Ok(false);
    };
    let Some(right) = destination.relationships.get_by_id(destination_id) else {
        return Ok(false);
    };
    if left.rel_type != right.rel_type
        || relationship_is_external(left) != relationship_is_external(right)
        || !source.relationships.record_metadata_equal_for_import(
            source_id,
            destination.relationships,
            destination_id,
        )?
    {
        return Ok(false);
    }
    if relationship_is_external(left) {
        return Ok(left.target == right.target);
    }
    if left.rel_type != rel_types::IMAGE {
        return Ok(false);
    }
    let a = OpcPackage::resolve_rel_target(source.part, &left.target);
    let b = OpcPackage::resolve_rel_target(destination.part, &right.target);
    preflight_internal_transfer_image(source.package, &a, "import table styles")?;
    if source.package.get_part_rels(&a).is_some_and(|rels| {
        rels.items
            .iter()
            .any(|relationship| !relationship_is_external(relationship))
    }) {
        return Err(invalid_presentation_mutation(
            "import table styles",
            format!("{a} has unsupported internal image relationships"),
        ));
    }
    Ok(
        source.package.get_part(&a) == destination.package.get_part(&b)
            && source.package.content_types.content_type_for(&a)
                == destination.package.content_types.content_type_for(&b)
            && source
                .package
                .get_part_rels(&a)
                .map(Relationships::to_xml)
                .transpose()?
                == destination
                    .package
                    .get_part_rels(&b)
                    .map(Relationships::to_xml)
                    .transpose()?,
    )
}

fn import_table_style_equal(
    source: &ImportTableStyleScope<'_>,
    left: &[u8],
    destination: &ImportTableStyleScope<'_>,
    right: &[u8],
) -> Result<bool> {
    let malformed =
        |error: OxmlError| invalid_presentation_mutation("import table styles", error.to_string());
    let left = close_import_table_style(left, source.namespaces).map_err(malformed)?;
    let right = close_import_table_style(right, destination.namespaces).map_err(malformed)?;
    let left_ids = relationship_ids(&left).map_err(malformed)?;
    let right_ids = relationship_ids(&right).map_err(malformed)?;
    if left_ids.len() != right_ids.len() {
        return Ok(false);
    }
    let mut aliases = HashMap::new();
    let mut inverse = HashMap::new();
    for (left, right) in left_ids.iter().zip(&right_ids) {
        if aliases.get(left).is_some_and(|old| old != right)
            || inverse.get(right).is_some_and(|old| old != left)
            || !import_table_resource_equal(source, left, destination, right)?
        {
            return Ok(false);
        }
        aliases.insert(left.clone(), right.clone());
        inverse.insert(right.clone(), left.clone());
    }
    let left = rewrite_exact_rel_ids(&left, &aliases).map_err(malformed)?;
    let neutral = "{00000000-0000-4000-8000-000000000000}";
    Ok(set_import_style_id(&left, neutral).map_err(malformed)?
        == set_import_style_id(&right, neutral).map_err(malformed)?)
}

pub(super) fn import_table_style_part(package: &OpcPackage, owner: &str) -> Result<Option<String>> {
    let count = package.get_part_rels(owner).map_or(0, |rels| {
        rels.get_all_by_type(rel_types::TABLE_STYLES).len()
    });
    if count > 1 {
        return Err(Error::DuplicateRelationships {
            source_part: owner.to_owned(),
            relationship_type: rel_types::TABLE_STYLES,
            count,
        });
    }
    related_internal_part(package, owner, rel_types::TABLE_STYLES)
}
