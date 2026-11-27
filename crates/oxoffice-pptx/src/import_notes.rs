//! Slide import: the notes master metadata, appearance, styles, theme and text an imported slide's notes keep.

use super::*;

// Compare retained common extensions under the same global-master owner kind.
// Resource-dependent or changed opaque metadata has no proven local equivalent.
fn notes_common_extension_metadata(xml: &[u8], part: &str) -> Result<Vec<Vec<u8>>> {
    let mut reader = NsReader::from_reader(xml);
    let mut roles: Vec<(String, Vec<u8>)> = Vec::new();
    let mut scopes = vec![std::collections::BTreeMap::<String, String>::new()];
    let mut metadata = Vec::new();
    loop {
        let begin = reader.buffer_position() as usize;
        let decoder = reader.decoder();
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| footer_malformed(part, error.to_string()))?;
        let uri = import_namespace_uri(namespace, decoder)
            .map_err(|error| footer_malformed(part, error.to_string()))?;
        match event {
            event @ (XmlEvent::Start(_) | XmlEvent::Empty(_)) => {
                let empty = matches!(&event, XmlEvent::Empty(_));
                let start = match &event {
                    XmlEvent::Start(start) | XmlEvent::Empty(start) => start,
                    _ => unreachable!(),
                };
                let local = start.local_name().as_ref().to_vec();
                let mut scope = scopes.last().unwrap().clone();
                for attr in start.attributes() {
                    let attr = attr.map_err(|error| footer_malformed(part, error.to_string()))?;
                    let name = attr.key.as_ref();
                    let prefix = if name == b"xmlns" {
                        Some("")
                    } else {
                        name.strip_prefix(b"xmlns:")
                            .map(std::str::from_utf8)
                            .transpose()
                            .map_err(|error| footer_malformed(part, error.to_string()))?
                    };
                    if let Some(prefix) = prefix {
                        scope.insert(
                            prefix.into(),
                            attr.decoded_and_normalized_value(
                                XmlVersion::Implicit1_0,
                                start.decoder(),
                            )
                            .map_err(|error| footer_malformed(part, error.to_string()))?
                            .into_owned(),
                        );
                    }
                }
                if roles.len() == 2
                    && roles
                        .last()
                        .is_some_and(|(uri, local)| uri == P_NS && local == b"cSld")
                    && uri == P_NS
                    && local == b"extLst"
                {
                    if !empty {
                        reader
                            .read_to_end(start.name())
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                    }
                    let raw = &xml[begin..reader.buffer_position() as usize];
                    let context = scope.into_iter().collect::<Vec<_>>();
                    let raw = close_import_table_style(raw, &context)
                        .map_err(|error| footer_malformed(part, error.to_string()))?;
                    if !relationship_ids(&raw)
                        .map_err(|error| footer_malformed(part, error.to_string()))?
                        .is_empty()
                    {
                        return Err(footer_malformed(
                            part,
                            "unrepresentable resource-dependent notes-master common metadata",
                        ));
                    }
                    metadata.push(raw);
                    continue;
                }
                if !empty {
                    roles.push((uri, local));
                    scopes.push(scope);
                }
            }
            XmlEvent::End(_) => {
                roles.pop();
                scopes.pop();
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    Ok(metadata)
}

/// Whether a `p:extLst` holds nothing but a `p14:creationId`, the identity
/// PowerPoint gives a part. It names the source notes master, which is not
/// copied, and draws nothing, so its loss is not a loss of appearance.
fn holds_only_a_creation_id(raw: &[u8]) -> bool {
    const CREATION_ID_EXTENSION: &[u8] = b"{BB962C8B-B14F-4D97-AF65-F5344CB8AC3E}";
    let mut reader = NsReader::from_reader(raw);
    let mut path: Vec<Vec<u8>> = Vec::new();
    let mut extensions = 0;
    loop {
        let Ok((namespace, event)) = reader.read_resolved_event() else {
            return false;
        };
        match event {
            event @ (XmlEvent::Start(_) | XmlEvent::Empty(_)) => {
                let (XmlEvent::Start(start) | XmlEvent::Empty(start)) = &event else {
                    unreachable!()
                };
                let local = start.local_name().as_ref().to_vec();
                let expected = match path.len() {
                    0 => xml_is(&namespace, P_NS) && local == b"extLst",
                    1 => {
                        extensions += 1;
                        xml_is(&namespace, P_NS)
                            && local == b"ext"
                            && start.attributes().flatten().any(|attribute| {
                                attribute.key.as_ref() == b"uri"
                                    && oxoffice_core::xml::attribute_text(&attribute)
                                        .is_ok_and(|uri| uri.as_bytes() == CREATION_ID_EXTENSION)
                            })
                    }
                    2 => {
                        xml_is(
                            &namespace,
                            "http://schemas.microsoft.com/office/powerpoint/2010/main",
                        ) && local == b"creationId"
                    }
                    _ => false,
                };
                if !expected {
                    return false;
                }
                if matches!(event, XmlEvent::Start(_)) {
                    path.push(local);
                }
            }
            XmlEvent::End(_) => {
                path.pop();
            }
            XmlEvent::Eof => return extensions == 1,
            _ => {}
        }
    }
}

// Notes-master owner metadata cannot be moved blindly to a notes-slide owner.
// Admit only the explicitly materialized surface and schema scalars. Unknown
// authority outside that surface is refused before any staged graph publication.
fn admit_source_notes_master_metadata(
    xml: &[u8],
    destination_xml: &[u8],
    part: &str,
) -> Result<()> {
    let source_metadata = notes_common_extension_metadata(xml, part)?;
    let destination_metadata = notes_common_extension_metadata(destination_xml, part)?;
    if source_metadata
        .iter()
        .any(|raw| !holds_only_a_creation_id(raw) && !destination_metadata.contains(raw))
    {
        return Err(footer_malformed(
            part,
            "unrepresentable source notes-master common extension authority",
        ));
    }
    let mut reader = NsReader::from_reader(xml);
    let mut roles: Vec<(String, Vec<u8>)> = Vec::new();
    loop {
        let decoder = reader.decoder();
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| footer_malformed(part, error.to_string()))?;
        let uri = import_namespace_uri(namespace, decoder)
            .map_err(|error| footer_malformed(part, error.to_string()))?;
        match event {
            event @ (XmlEvent::Start(_) | XmlEvent::Empty(_)) => {
                let empty = matches!(&event, XmlEvent::Empty(_));
                let start = match &event {
                    XmlEvent::Start(start) | XmlEvent::Empty(start) => start,
                    _ => unreachable!(),
                };
                let local = start.local_name().as_ref().to_vec();
                let depth = roles.len();
                let owner = roles
                    .last()
                    .map(|(uri, local)| (uri.as_str(), local.as_slice()));
                if depth == 1
                    && !(uri == P_NS
                        && matches!(
                            local.as_slice(),
                            b"cSld" | b"clrMap" | b"hf" | b"notesStyle"
                        ))
                    || depth == 2
                        && owner == Some((P_NS, b"cSld"))
                        && !(uri == P_NS
                            && matches!(local.as_slice(), b"bg" | b"spTree" | b"extLst"))
                    || depth == 2 && matches!(owner, Some((P_NS, b"hf" | b"clrMap")))
                    || depth == 2
                        && owner == Some((P_NS, b"notesStyle"))
                        && !(uri == A_NS
                            && (local == b"defPPr"
                                || matches!(
                                    local.as_slice(),
                                    b"lvl1pPr"
                                        | b"lvl2pPr"
                                        | b"lvl3pPr"
                                        | b"lvl4pPr"
                                        | b"lvl5pPr"
                                        | b"lvl6pPr"
                                        | b"lvl7pPr"
                                        | b"lvl8pPr"
                                        | b"lvl9pPr"
                                )))
                {
                    return Err(footer_malformed(
                        part,
                        "unrepresentable source notes-master owner metadata",
                    ));
                }
                if depth <= 1 {
                    for attr in start.attributes() {
                        let attr =
                            attr.map_err(|error| footer_malformed(part, error.to_string()))?;
                        let key = attr.key.as_ref();
                        if key == b"xmlns" || key.starts_with(b"xmlns:") {
                            continue;
                        }
                        let supported = uri == P_NS
                            && match local.as_slice() {
                                b"cSld" => key == b"name",
                                b"hf" => matches!(key, b"hdr" | b"ftr" | b"dt" | b"sldNum"),
                                b"clrMap" => matches!(
                                    key,
                                    b"accent1"
                                        | b"accent2"
                                        | b"accent3"
                                        | b"accent4"
                                        | b"accent5"
                                        | b"accent6"
                                        | b"bg1"
                                        | b"bg2"
                                        | b"tx1"
                                        | b"tx2"
                                        | b"hlink"
                                        | b"folHlink"
                                ),
                                _ => false,
                            };
                        if !supported {
                            return Err(footer_malformed(
                                part,
                                "unrepresentable source notes-master owner attribute",
                            ));
                        }
                    }
                }
                if !empty {
                    roles.push((uri, local));
                }
            }
            XmlEvent::End(_) => {
                roles.pop();
            }
            XmlEvent::Comment(_) | XmlEvent::PI(_) | XmlEvent::CData(_) if roles.len() <= 2 => {
                return Err(footer_malformed(
                    part,
                    "unrepresentable source notes-master owner event metadata",
                ));
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// Prepares local source notes appearance under the destination's one global master.
pub(super) fn stage_source_notes_appearance(
    source: &Presentation,
    package: &mut OpcPackage,
    notes_part: &str,
    destination: &Presentation,
) -> Result<()> {
    let presentation_defaults = if let Some(defaults) = &source.presentation.default_text_style {
        let xml = defaults
            .to_xml()
            .map_err(|error| footer_malformed(notes_part, error.to_string()))?;
        let rewritten = transfer_imported_property_resources(
            package,
            &source.presentation_part,
            notes_part,
            &xml,
        )?;
        Some(
            CT_TextListStyle::from_xml(&rewritten)
                .map_err(|error| footer_malformed(notes_part, error.to_string()))?,
        )
    } else {
        None
    };
    let note_relationships = package
        .get_part_rels(notes_part)
        .cloned()
        .unwrap_or_default();
    let masters = note_relationships.get_all_by_type(rel_types::NOTES_MASTER);
    if masters.len() != 1 || relationship_is_external(masters[0]) {
        return Err(footer_malformed(
            notes_part,
            "source notes require one internal notes-master relationship",
        ));
    }
    let master_relationship = masters[0];
    let master_part = OpcPackage::resolve_rel_target(notes_part, &master_relationship.target);
    let master_xml = required_part(package, &master_part)?.to_vec();
    let destination_master_part = destination
        .notes_master_part
        .as_deref()
        .ok_or_else(|| footer_malformed(notes_part, "destination notes master is absent"))?;
    admit_source_notes_master_metadata(
        &master_xml,
        required_part(&destination.package, destination_master_part)?,
        &master_part,
    )?;
    let master_relationships = package
        .get_part_rels(&master_part)
        .cloned()
        .unwrap_or_default();
    let ids: HashSet<_> = relationship_ids(&master_xml)
        .map_err(|error| footer_malformed(&master_part, error.to_string()))?
        .into_iter()
        .collect();
    let mut resources = Vec::new();

    for id in ids {
        let relationship =
            master_relationships
                .get_by_id(&id)
                .ok_or_else(|| Error::MissingRelationship {
                    source_part: master_part.clone(),
                    relationship_id: id.clone(),
                })?;
        let mut transferred = relationship.clone();
        if !relationship_is_external(relationship) {
            transferred.target = relative_part_target(
                notes_part,
                &OpcPackage::resolve_rel_target(&master_part, &relationship.target),
            );
        }
        resources.push(transferred);
    }
    let scope = master_relationships.rebind_for_import(resources, &HashMap::new())?;
    let (relationships, mapping) = note_relationships.merge_for_import(&scope)?;
    let rewritten = rewrite_exact_rel_ids(&master_xml, &mapping)
        .map_err(|error| footer_malformed(&master_part, error.to_string()))?;
    let mut master = CT_NotesMaster::from_xml(&rewritten)
        .map_err(|error| footer_malformed(&master_part, error.to_string()))?;
    let mut note = CT_NotesSlide::from_xml(required_part(package, notes_part)?)
        .map_err(|error| footer_malformed(notes_part, error.to_string()))?;
    let source_flags = master.header_footer.as_ref();
    let destination_flags = destination
        .notes_master
        .as_ref()
        .and_then(|master| master.header_footer.as_ref());
    for enabled in [
        CT_HeaderFooter::header_enabled,
        CT_HeaderFooter::footer_enabled,
        CT_HeaderFooter::date_time_enabled,
        CT_HeaderFooter::slide_number_enabled,
    ] {
        if source_flags.is_none_or(enabled) != destination_flags.is_none_or(enabled) {
            return Err(footer_malformed(
                notes_part,
                "unrepresentable source notes header/footer authority under destination global master",
            ));
        }
    }
    if note.common_slide_data.name.is_none() {
        note.common_slide_data.name = master.common_slide_data.name.clone();
    }
    let theme_relationships = master_relationships.get_all_by_type(rel_types::THEME);
    if theme_relationships.len() != 1 || relationship_is_external(theme_relationships[0]) {
        return Err(footer_malformed(
            &master_part,
            "source notes master must own one internal theme",
        ));
    }
    let theme_part = OpcPackage::resolve_rel_target(&master_part, &theme_relationships[0].target);
    let original_theme = required_part(package, &theme_part)?.to_vec();
    package.set_part_rels(notes_part, relationships);
    let theme_xml =
        transfer_imported_property_resources(package, &theme_part, notes_part, &original_theme)?;
    let theme = CT_OfficeStyleSheet::from_xml(&theme_xml)
        .map_err(|error| footer_malformed(&theme_part, error.to_string()))?;
    let defaults = match (&master.notes_style, &presentation_defaults) {
        (Some(local), Some(inherited)) => Some(
            local
                .with_inherited_defaults(inherited)
                .map_err(|error| footer_malformed(notes_part, error.to_string()))?,
        ),
        (Some(local), None) => Some(local.clone()),
        (None, Some(inherited)) => Some(inherited.clone()),
        (None, None) => None,
    };
    if let Some(defaults) = &defaults {
        materialize_imported_notes_text(
            &mut master.common_slide_data.shape_tree.children,
            defaults,
            notes_part,
        )?;
        materialize_imported_notes_text(
            &mut note.common_slide_data.shape_tree.children,
            defaults,
            notes_part,
        )?;
    }
    materialize_source_notes_styles(
        &mut master.common_slide_data.shape_tree.children,
        &theme_xml,
        &theme,
        notes_part,
    )?;
    materialize_source_notes_styles(
        &mut note.common_slide_data.shape_tree.children,
        &theme_xml,
        &theme,
        notes_part,
    )?;
    note.common_slide_data
        .shape_tree
        .materialize_notes_master_surface(&master.common_slide_data.shape_tree)
        .map_err(|error| footer_malformed(notes_part, error.to_string()))?;
    if note.common_slide_data.background.is_none() {
        note.common_slide_data.background = master.common_slide_data.background.clone();
    }
    if let Some(background) = note.common_slide_data.background.as_mut() {
        if let oxoffice_pptx_oxml::slide_parts::BackgroundRendering::Reference { index, color } =
            background.rendering().clone()
        {
            let fill = if index == 0 {
                Fill::from_xml(b"<a:noFill/>")
                    .map_err(|error| footer_malformed(notes_part, error.to_string()))?
            } else {
                let (list, index) = if index >= 1000 {
                    ("bgFillStyleLst", index - 1000)
                } else {
                    ("fillStyleLst", index)
                };
                source_notes_matrix_property(&theme_xml, list, index, color.as_ref(), notes_part)?
                    .fill
                    .ok_or_else(|| {
                        footer_malformed(
                            notes_part,
                            "source background style has no representable fill",
                        )
                    })?
            };
            background
                .replace_fill(fill)
                .map_err(|error| footer_malformed(notes_part, error.to_string()))?;
        }
    } else {
        note.common_slide_data.background = Some(
            oxoffice_pptx_oxml::slide_parts::CT_Background::from_fill(
                Fill::from_xml(b"<a:noFill/>")
                    .map_err(|error| footer_malformed(notes_part, error.to_string()))?,
            )
            .map_err(|error| footer_malformed(notes_part, error.to_string()))?,
        );
    }
    note.set_show_master_shapes(Some(false));
    let xml = note
        .to_xml()
        .map_err(|error| footer_malformed(notes_part, error.to_string()))?;
    let color_map = match note
        .color_map_override
        .as_ref()
        .map(|override_map| &override_map.kind)
    {
        Some(ColorMapOverrideKind::Override(map)) => map,
        _ => &master.color_map,
    };
    let destination_master = destination
        .notes_master_part
        .as_deref()
        .ok_or_else(|| footer_malformed(notes_part, "destination notes master is absent"))?;
    let destination_theme_part =
        related_internal_part(&destination.package, destination_master, rel_types::THEME)?
            .ok_or_else(|| {
                footer_malformed(destination_master, "destination notes theme is absent")
            })?;
    let destination_theme = CT_OfficeStyleSheet::from_xml(required_part(
        &destination.package,
        &destination_theme_part,
    )?)
    .map_err(|error| footer_malformed(&destination_theme_part, error.to_string()))?;
    let xml = freeze_source_notes_theme(&xml, &theme, &destination_theme, color_map, notes_part)?;
    CT_NotesSlide::from_xml(&xml)
        .map_err(|error| footer_malformed(notes_part, error.to_string()))?;
    package.set_part(notes_part, xml);
    Ok(())
}

// Reads one actual selected source format-scheme property in its physical scope.
fn source_notes_matrix_property(
    theme_xml: &[u8],
    list: &str,
    index: u32,
    reference: Option<&ColorChoice>,
    part: &str,
) -> Result<oxoffice_drawing::shape_props::CT_ShapeProperties> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(theme_xml);
    let mut scopes = vec![Vec::<(String, String)>::new()];
    let mut roles = Vec::<String>::new();
    let mut selected = 0u32;
    loop {
        match reader
            .read_event()
            .map_err(|error| footer_malformed(part, error.to_string()))?
        {
            event @ (XmlEvent::Start(_) | XmlEvent::Empty(_)) => {
                let empty = matches!(&event, XmlEvent::Empty(_));
                let start = match &event {
                    XmlEvent::Start(start) | XmlEvent::Empty(start) => start,
                    _ => unreachable!(),
                };
                let mut scope = scopes.last().unwrap().clone();
                for attribute in start.attributes() {
                    let attribute =
                        attribute.map_err(|error| footer_malformed(part, error.to_string()))?;
                    let key = attribute.key.as_ref();
                    let prefix = if key == b"xmlns" {
                        Some("")
                    } else {
                        key.strip_prefix(b"xmlns:")
                            .map(std::str::from_utf8)
                            .transpose()
                            .map_err(|error| footer_malformed(part, error.to_string()))?
                    };
                    if let Some(prefix) = prefix {
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                            .map_err(|error| footer_malformed(part, error.to_string()))?
                            .into_owned();
                        if let Some((_, old)) = scope.iter_mut().find(|(key, _)| key == prefix) {
                            *old = value;
                        } else {
                            scope.push((prefix.to_owned(), value));
                        }
                    }
                }
                let name = start.name();
                let name = std::str::from_utf8(name.as_ref())
                    .map_err(|error| footer_malformed(part, error.to_string()))?;
                let (prefix, local) = name.split_once(':').unwrap_or(("", name));
                let owned = scope.iter().any(|(key, uri)| key == prefix && uri == A_NS);
                if owned && roles == ["theme", "themeElements", "fmtScheme", list] {
                    selected += 1;
                    if selected == index {
                        let raw = if empty {
                            oxoffice_core::raw_xml::capture_empty(start, reader.empty_writing())
                        } else {
                            oxoffice_core::raw_xml::capture_element(&mut reader, start)
                        }
                        .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let raw = close_import_table_style(&raw, &scope)
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let children = if list == "effectStyleLst" {
                            let mut elements =
                                oxoffice_core::xml_reader::Reader::from_reader(raw.as_slice());
                            let first = elements
                                .read_event()
                                .map_err(|error| footer_malformed(part, error.to_string()))?;
                            let root = match first {
                                XmlEvent::Start(root) => root,
                                XmlEvent::Empty(_) => {
                                    return Err(footer_malformed(
                                        part,
                                        "source effect style is empty",
                                    ));
                                }
                                _ => unreachable!(),
                            };
                            for attr in root.attributes() {
                                let attr = attr
                                    .map_err(|error| footer_malformed(part, error.to_string()))?;
                                if attr.key.as_ref() != b"xmlns"
                                    && !attr.key.as_ref().starts_with(b"xmlns:")
                                {
                                    return Err(footer_malformed(
                                        part,
                                        "unrepresentable source effect-style owner attribute",
                                    ));
                                }
                            }
                            let mut children = Vec::new();
                            loop {
                                match elements
                                    .read_event()
                                    .map_err(|error| footer_malformed(part, error.to_string()))?
                                {
                                    XmlEvent::Start(child) => children.extend_from_slice(
                                        &oxoffice_core::raw_xml::capture_element(
                                            &mut elements,
                                            &child,
                                        )
                                        .map_err(
                                            |error| footer_malformed(part, error.to_string()),
                                        )?,
                                    ),
                                    XmlEvent::Empty(child) => children.extend_from_slice(
                                        &oxoffice_core::raw_xml::capture_empty(
                                            &child,
                                            reader.empty_writing(),
                                        )
                                        .map_err(
                                            |error| footer_malformed(part, error.to_string()),
                                        )?,
                                    ),
                                    XmlEvent::End(_) | XmlEvent::Eof => break,
                                    XmlEvent::Text(text)
                                        if text.iter().all(u8::is_ascii_whitespace) => {}
                                    _ => {
                                        return Err(footer_malformed(
                                            part,
                                            "unrepresentable source effect-style owner payload",
                                        ));
                                    }
                                }
                            }
                            children
                        } else {
                            raw
                        };
                        let wrapper = [b"<spPr>".as_slice(), &children, b"</spPr>"].concat();
                        let defaults = oxoffice_drawing::shape_props::CT_ShapeProperties::from_xml_with_inherited_namespaces(&wrapper, &scope)
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let closed = oxoffice_drawing::shape_props::CT_ShapeProperties::default()
                            .with_inherited_defaults(&defaults)
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let projected = closed
                            .to_xml()
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let projected =
                            close_import_table_style(&projected, &[("a".into(), A_NS.into())])
                                .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let xml = substitute_notes_placeholder_color(&projected, reference, part)?;
                        return oxoffice_drawing::shape_props::CT_ShapeProperties::from_xml(&xml)
                            .map_err(|error| footer_malformed(part, error.to_string()));
                    }
                }
                if !empty {
                    scopes.push(scope);
                    roles.push(if owned {
                        local.to_owned()
                    } else {
                        String::new()
                    });
                }
            }
            XmlEvent::End(_) => {
                scopes.pop();
                roles.pop();
            }
            XmlEvent::Eof => {
                return Err(footer_malformed(
                    part,
                    format!("source {list} style index {index} is out of range"),
                ));
            }
            _ => {}
        }
    }
}

fn substitute_notes_placeholder_color(
    xml: &[u8],
    reference: Option<&ColorChoice>,
    part: &str,
) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(xml);
    let mut edits = Vec::new();
    let mut scopes = Vec::<bool>::new();
    loop {
        let begin = reader.buffer_position() as usize;
        let decoder = reader.decoder();
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| footer_malformed(part, error.to_string()))?;
        let uri = import_namespace_uri(namespace, decoder)
            .map_err(|error| footer_malformed(part, error.to_string()))?;
        let empty = matches!(event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                let modeled = scopes.last().copied().unwrap_or(true)
                    && notes_theme_modeled_element(&uri, start.local_name().as_ref());
                if !empty {
                    scopes.push(modeled);
                }
                if !modeled || uri != A_NS || start.local_name().as_ref() != b"schemeClr" {
                    continue;
                }
                let attributes = start
                    .attributes()
                    .map(|attribute| attribute.map_err(OxmlError::from))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|error| footer_malformed(part, error.to_string()))?;
                let placeholder = attributes.iter().any(|attribute| {
                    attribute.key.as_ref() == b"val"
                        && oxoffice_core::xml::attribute_text(attribute)
                            .is_ok_and(|value| value == "phClr")
                });
                if !placeholder {
                    continue;
                }
                let mut reference = reference.cloned().ok_or_else(|| {
                    footer_malformed(
                        part,
                        "source style placeholder color has no actual reference",
                    )
                })?;
                if matches!(&reference, ColorChoice::Scheme { value, .. } if value == "phClr") {
                    return Err(footer_malformed(
                        part,
                        "source style reference has unresolved placeholder color",
                    ));
                }
                for attribute in attributes {
                    let key = attribute.key.as_ref();
                    if key != b"val" && key != b"xmlns" && !key.starts_with(b"xmlns:") {
                        return Err(footer_malformed(
                            part,
                            "unrepresentable source placeholder-color attribute",
                        ));
                    }
                }
                let raw = if empty {
                    oxoffice_core::raw_xml::capture_empty(&start, reader.empty_writing())
                } else {
                    // NsReader exposes the same slice reader to the owning colour parser.
                    let rest = &xml[begin..];
                    let mut raw_reader = oxoffice_core::xml_reader::Reader::from_reader(rest);
                    let XmlEvent::Start(root) = raw_reader
                        .read_event()
                        .map_err(|error| footer_malformed(part, error.to_string()))?
                    else {
                        unreachable!()
                    };
                    oxoffice_core::raw_xml::capture_element(&mut raw_reader, &root)
                }
                .map_err(|error| footer_malformed(part, error.to_string()))?;
                let mut color_reader =
                    oxoffice_core::xml_reader::Reader::from_reader(raw.as_slice());
                let source = match color_reader
                    .read_event()
                    .map_err(|error| footer_malformed(part, error.to_string()))?
                {
                    XmlEvent::Start(start) => ColorChoice::from_xml(&mut color_reader, &start),
                    XmlEvent::Empty(start) => ColorChoice::from_empty_xml(&start),
                    _ => unreachable!(),
                }
                .map_err(|error| footer_malformed(part, error.to_string()))?;
                if (0..=source.transforms().len())
                    .any(|slot| source.raw_children().at(slot).next().is_some())
                {
                    return Err(footer_malformed(
                        part,
                        "unrepresentable opaque source placeholder-color payload",
                    ));
                }
                let transforms = match &mut reference {
                    ColorChoice::Srgb { transforms, .. }
                    | ColorChoice::Scheme { transforms, .. }
                    | ColorChoice::System { transforms, .. }
                    | ColorChoice::Preset { transforms, .. } => transforms,
                };
                transforms.extend_from_slice(source.transforms());
                let mut writer = XmlWriter::new(Vec::new());
                reference
                    .to_xml(&mut writer)
                    .map_err(|error| footer_malformed(part, error.to_string()))?;
                edits.push((begin..begin + raw.len(), writer.into_inner()));
            }
            XmlEvent::End(_) => {
                scopes.pop();
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    let mut result = xml.to_vec();
    let mut limit = xml.len();
    for (range, replacement) in edits.into_iter().rev() {
        if range.end > limit {
            return Err(footer_malformed(
                part,
                "overlapping source placeholder-color surface",
            ));
        }
        limit = range.start;
        result.splice(range, replacement);
    }
    Ok(result)
}

fn materialize_source_notes_styles(
    children: &mut [ShapeTreeChild],
    theme_xml: &[u8],
    theme: &CT_OfficeStyleSheet,
    part: &str,
) -> Result<()> {
    for child in children {
        match child {
            ShapeTreeChild::Shape(shape) => {
                *shape.shape_properties = shape
                    .shape_properties
                    .with_inherited_defaults(
                        &oxoffice_drawing::shape_props::CT_ShapeProperties::default(),
                    )
                    .map_err(|error| footer_malformed(part, error.to_string()))?;
                let Some(style) = shape
                    .style_for_materialization()
                    .map_err(|error| footer_malformed(part, error.to_string()))?
                else {
                    continue;
                };
                for (list, index, color) in [
                    (
                        "fillStyleLst",
                        style.fill_reference.index,
                        style.fill_reference.color.as_ref(),
                    ),
                    (
                        "lnStyleLst",
                        style.line_reference.index,
                        style.line_reference.color.as_ref(),
                    ),
                    (
                        "effectStyleLst",
                        style.effect_reference.index,
                        style.effect_reference.color.as_ref(),
                    ),
                ] {
                    if index == 0 {
                        continue;
                    }
                    let (list, index) = if list == "fillStyleLst" && index >= 1000 {
                        ("bgFillStyleLst", index - 1000)
                    } else {
                        (list, index)
                    };
                    let defaults =
                        source_notes_matrix_property(theme_xml, list, index, color, part)?;
                    if list == "lnStyleLst"
                        && let (Some(local), Some(parent)) =
                            (shape.shape_properties.line.as_mut(), defaults.line.as_ref())
                    {
                        if (0..=8).any(|slot| parent.raw_children().at(slot).next().is_some()) {
                            return Err(footer_malformed(
                                part,
                                "unrepresentable opaque source line-style overlay",
                            ));
                        }
                        let raw = parent
                            .to_xml()
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let mut owned =
                            oxoffice_core::xml_reader::Reader::from_reader(raw.as_slice());
                        let root = match owned
                            .read_event()
                            .map_err(|error| footer_malformed(part, error.to_string()))?
                        {
                            XmlEvent::Start(root) | XmlEvent::Empty(root) => root,
                            _ => unreachable!(),
                        };
                        for attr in root.attributes() {
                            let attr =
                                attr.map_err(|error| footer_malformed(part, error.to_string()))?;
                            let key = attr.key.as_ref();
                            if !matches!(key, b"w" | b"cap" | b"cmpd" | b"algn" | b"xmlns")
                                && !key.starts_with(b"xmlns:")
                            {
                                return Err(footer_malformed(
                                    part,
                                    "unrepresentable opaque source line-style owner attribute",
                                ));
                            }
                        }
                        if local.width.is_none() {
                            local.width = parent.width;
                        }
                        if local.cap.is_none() {
                            local.cap = parent.cap;
                        }
                        if local.fill.is_none() {
                            local.fill = parent.fill.clone();
                        }
                        if local.dash.is_none() {
                            local.dash = parent.dash.clone();
                        }
                        if local.join.is_none() {
                            local.join = parent.join.clone();
                        }
                        if local.head_end.is_none() {
                            local.head_end = parent.head_end.clone();
                        }
                        if local.tail_end.is_none() {
                            local.tail_end = parent.tail_end.clone();
                        }
                        let local_xml = local
                            .to_xml()
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let mut local_reader =
                            oxoffice_core::xml_reader::Reader::from_reader(local_xml.as_slice());
                        let (mut local_start, empty) = match local_reader
                            .read_event()
                            .map_err(|error| footer_malformed(part, error.to_string()))?
                        {
                            XmlEvent::Start(start) => (start.into_owned(), false),
                            XmlEvent::Empty(start) => (start.into_owned(), true),
                            _ => unreachable!(),
                        };
                        let own = local_start
                            .attributes()
                            .map(|attr| attr.map(|attr| attr.key.as_ref().to_vec()))
                            .collect::<std::result::Result<HashSet<_>, _>>()
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        for attr in root.attributes() {
                            let attr =
                                attr.map_err(|error| footer_malformed(part, error.to_string()))?;
                            let key = attr.key.as_ref();
                            if matches!(key, b"cmpd" | b"algn") && !own.contains(key) {
                                let value = attr
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        root.decoder(),
                                    )
                                    .map_err(|error| footer_malformed(part, error.to_string()))?;
                                let value = quick_xml::escape::escape(value.as_ref());
                                local_start.push_attribute((key, value.as_bytes()));
                            }
                        }
                        let mut writer = XmlWriter::new(Vec::new());
                        writer
                            .write_event(if empty {
                                XmlEvent::Empty(local_start)
                            } else {
                                XmlEvent::Start(local_start)
                            })
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        writer.get_mut().extend_from_slice(
                            &local_xml[local_reader.buffer_position() as usize..],
                        );
                        *local = oxoffice_drawing::line::CT_LineProperties::from_xml(
                            &writer.into_inner(),
                        )
                        .map_err(|error| footer_malformed(part, error.to_string()))?;
                        // Both admitted line scopes are now physically closed in
                        // the staged local owner. Capture that completed edit,
                        // rather than comparing it against the earlier snapshot.
                        *shape.shape_properties =
                            oxoffice_drawing::shape_props::CT_ShapeProperties::from_xml(
                                &shape
                                    .shape_properties
                                    .to_xml()
                                    .map_err(|error| footer_malformed(part, error.to_string()))?,
                            )
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                    }
                    *shape.shape_properties = shape
                        .shape_properties
                        .with_inherited_defaults(&defaults)
                        .map_err(|error| footer_malformed(part, error.to_string()))?;
                }
                if let Some(body) = shape.text_body.as_mut() {
                    let own_list = body.list_style().cloned().unwrap_or_default();
                    let collection = match style.font_reference.index {
                        oxoffice_drawing::style_ref::FontCollectionIndex::Major => {
                            Some(&theme.theme_elements.font_scheme.major_font)
                        }
                        oxoffice_drawing::style_ref::FontCollectionIndex::Minor => {
                            Some(&theme.theme_elements.font_scheme.minor_font)
                        }
                        oxoffice_drawing::style_ref::FontCollectionIndex::None => None,
                    };
                    let mut inherited = CT_TextCharacterProperties::default();
                    if let Some(collection) = collection {
                        inherited.latin = Some(
                            TextFont::new(collection.latin.typeface.clone())
                                .map_err(|error| footer_malformed(part, error.to_string()))?,
                        );
                        if !collection.east_asian.typeface.is_empty() {
                            inherited.east_asian = Some(
                                TextFont::new(collection.east_asian.typeface.clone())
                                    .map_err(|error| footer_malformed(part, error.to_string()))?,
                            );
                        }
                        if !collection.complex_script.typeface.is_empty() {
                            inherited.complex_script = Some(
                                TextFont::new(collection.complex_script.typeface.clone())
                                    .map_err(|error| footer_malformed(part, error.to_string()))?,
                            );
                        }
                    }
                    if let Some(color) = style.font_reference.color.clone() {
                        let mut writer = XmlWriter::new(Vec::new());
                        color
                            .to_xml(&mut writer)
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let raw = [
                            b"<a:solidFill>".as_slice(),
                            writer.get_ref(),
                            b"</a:solidFill>",
                        ]
                        .concat();
                        inherited.fill = Some(
                            Fill::from_xml(&raw)
                                .map_err(|error| footer_malformed(part, error.to_string()))?,
                        );
                    }
                    for index in 0..body.paragraph_count() {
                        let paragraph = body.paragraph_mut(index).unwrap();
                        let mut inherited = inherited.clone();
                        if let Some(local) = own_list
                            .default_paragraph_properties
                            .as_ref()
                            .and_then(|properties| properties.default_run_properties.as_ref())
                        {
                            inherited = local
                                .with_inherited_defaults(&inherited)
                                .map_err(|error| footer_malformed(part, error.to_string()))?;
                        }
                        let level = paragraph
                            .properties
                            .as_ref()
                            .and_then(|properties| properties.level)
                            .unwrap_or(0)
                            + 1;
                        if let Some(local) = own_list
                            .level(usize::from(level))
                            .and_then(|properties| properties.default_run_properties.as_ref())
                        {
                            inherited = local
                                .with_inherited_defaults(&inherited)
                                .map_err(|error| footer_malformed(part, error.to_string()))?;
                        }
                        let properties = paragraph.properties.get_or_insert_with(Default::default);
                        let local = properties
                            .default_run_properties
                            .clone()
                            .unwrap_or_default();
                        properties.default_run_properties = Some(
                            local
                                .with_inherited_defaults(&inherited)
                                .map_err(|error| footer_malformed(part, error.to_string()))?,
                        );
                        for run in &mut paragraph.runs {
                            let properties = match run {
                                TextRun::Run(run) => &mut run.properties,
                                TextRun::Break(line_break) => &mut line_break.properties,
                                TextRun::Field(field) => &mut field.run_properties,
                            };
                            *properties = Some(
                                properties
                                    .clone()
                                    .unwrap_or_default()
                                    .with_inherited_defaults(&inherited)
                                    .map_err(|error| footer_malformed(part, error.to_string()))?,
                            );
                        }
                        paragraph.end_properties = Some(
                            paragraph
                                .end_properties
                                .clone()
                                .unwrap_or_default()
                                .with_inherited_defaults(&inherited)
                                .map_err(|error| footer_malformed(part, error.to_string()))?,
                        );
                    }
                }
                shape
                    .materialize_style_references()
                    .map_err(|error| footer_malformed(part, error.to_string()))?;
            }
            ShapeTreeChild::GroupShape(group) => {
                materialize_source_notes_styles(&mut group.children, theme_xml, theme, part)?
            }
            _ => {}
        }
    }
    Ok(())
}

fn notes_theme_modeled_element(uri: &str, local: &[u8]) -> bool {
    match uri {
        P_NS => matches!(
            local,
            b"notes"
                | b"cSld"
                | b"spTree"
                | b"nvGrpSpPr"
                | b"cNvPr"
                | b"cNvGrpSpPr"
                | b"nvPr"
                | b"ph"
                | b"grpSpPr"
                | b"sp"
                | b"nvSpPr"
                | b"cNvSpPr"
                | b"spPr"
                | b"txBody"
                | b"style"
                | b"pic"
                | b"nvPicPr"
                | b"cNvPicPr"
                | b"blipFill"
                | b"grpSp"
                | b"cxnSp"
                | b"nvCxnSpPr"
                | b"cNvCxnSpPr"
                | b"graphicFrame"
                | b"nvGraphicFramePr"
                | b"cNvGraphicFramePr"
                | b"xfrm"
                | b"bg"
                | b"bgPr"
                | b"bgRef"
                | b"clrMapOvr"
                | b"masterClrMapping"
                | b"overrideClrMapping"
        ),
        A_NS => matches!(
            local,
            b"spPr"
                | b"xfrm"
                | b"off"
                | b"ext"
                | b"chOff"
                | b"chExt"
                | b"prstGeom"
                | b"custGeom"
                | b"avLst"
                | b"gdLst"
                | b"gd"
                | b"ahLst"
                | b"cxnLst"
                | b"rect"
                | b"pathLst"
                | b"path"
                | b"moveTo"
                | b"lnTo"
                | b"pt"
                | b"arcTo"
                | b"quadBezTo"
                | b"cubicBezTo"
                | b"close"
                | b"noFill"
                | b"solidFill"
                | b"gradFill"
                | b"pattFill"
                | b"blipFill"
                | b"blip"
                | b"srcRect"
                | b"stretch"
                | b"tile"
                | b"fillRect"
                | b"gsLst"
                | b"gs"
                | b"lin"
                | b"fillToRect"
                | b"tileRect"
                | b"fgClr"
                | b"bgClr"
                | b"ln"
                | b"prstDash"
                | b"custDash"
                | b"ds"
                | b"round"
                | b"bevel"
                | b"miter"
                | b"headEnd"
                | b"tailEnd"
                | b"effectLst"
                | b"blur"
                | b"glow"
                | b"innerShdw"
                | b"outerShdw"
                | b"prstShdw"
                | b"reflection"
                | b"softEdge"
                | b"fillOverlay"
                | b"lnRef"
                | b"fillRef"
                | b"effectRef"
                | b"fontRef"
                | b"srgbClr"
                | b"schemeClr"
                | b"sysClr"
                | b"prstClr"
                | b"tint"
                | b"shade"
                | b"comp"
                | b"inv"
                | b"gray"
                | b"alpha"
                | b"alphaOff"
                | b"alphaMod"
                | b"hue"
                | b"hueOff"
                | b"hueMod"
                | b"sat"
                | b"satOff"
                | b"satMod"
                | b"lum"
                | b"lumOff"
                | b"lumMod"
                | b"red"
                | b"redOff"
                | b"redMod"
                | b"green"
                | b"greenOff"
                | b"greenMod"
                | b"blue"
                | b"blueOff"
                | b"blueMod"
                | b"gamma"
                | b"invGamma"
                | b"bodyPr"
                | b"lstStyle"
                | b"defPPr"
                | b"lvl1pPr"
                | b"lvl2pPr"
                | b"lvl3pPr"
                | b"lvl4pPr"
                | b"lvl5pPr"
                | b"lvl6pPr"
                | b"lvl7pPr"
                | b"lvl8pPr"
                | b"lvl9pPr"
                | b"p"
                | b"pPr"
                | b"defRPr"
                | b"r"
                | b"rPr"
                | b"br"
                | b"fld"
                | b"t"
                | b"endParaRPr"
                | b"latin"
                | b"ea"
                | b"cs"
                | b"sym"
                | b"buFont"
                | b"buClr"
                | b"buSzPct"
                | b"buSzPts"
                | b"buChar"
                | b"buBlip"
                | b"buAutoNum"
                | b"buNone"
                | b"highlight"
                | b"tabLst"
                | b"tab"
                | b"graphic"
                | b"graphicData"
                | b"tbl"
                | b"tblPr"
                | b"tblGrid"
                | b"gridCol"
                | b"tr"
                | b"tc"
                | b"tcPr"
                | b"txBody"
                | b"lnL"
                | b"lnR"
                | b"lnT"
                | b"lnB"
                | b"lnTlToBr"
                | b"lnBlToTr"
        ),
        _ => false,
    }
}

/// Freezes only modeled notes colour and typeface references, retaining all other bytes.
fn freeze_source_notes_theme(
    xml: &[u8],
    theme: &CT_OfficeStyleSheet,
    destination_theme: &CT_OfficeStyleSheet,
    color_map: &ColorMap,
    part: &str,
) -> Result<Vec<u8>> {
    let scheme = &theme.theme_elements.color_scheme;
    let mut colors = Vec::new();
    for (slot, color) in scheme.iter() {
        let value = oxoffice_drawing::color::resolve_color(color, &ColorMap::default(), &[])
            .map_err(|error| footer_malformed(part, error.to_string()))?;
        if value.alpha != 255 {
            return Err(footer_malformed(
                part,
                "source notes theme has a non-opaque scheme base",
            ));
        }
        colors.push((
            slot.as_str(),
            oxoffice_drawing::color::RgbColor::new(value.red, value.green, value.blue),
        ));
    }
    let mut reader = NsReader::from_reader(xml);
    let mut stack: Vec<(bool, Option<Vec<u8>>)> = Vec::new();
    let mut edits = Vec::new();
    loop {
        let begin = reader.buffer_position() as usize;
        let decoder = reader.decoder();
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| footer_malformed(part, error.to_string()))?;
        let uri = import_namespace_uri(namespace, decoder)
            .map_err(|error| footer_malformed(part, error.to_string()))?;
        let empty = matches!(event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(element) | XmlEvent::Empty(element) => {
                let modeled = stack.last().is_none_or(|(modeled, _)| *modeled)
                    && notes_theme_modeled_element(&uri, element.local_name().as_ref());
                let local = element.local_name();
                let mut replacement_name = None;
                if modeled
                    && uri == A_NS
                    && (local.as_ref() == b"schemeClr"
                        || matches!(local.as_ref(), b"latin" | b"ea" | b"cs" | b"buFont"))
                {
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| footer_malformed(part, error.to_string()))?;
                        let key = attribute.key.as_ref();
                        let value = attribute
                            .decoded_and_normalized_value(
                                XmlVersion::Implicit1_0,
                                element.decoder(),
                            )
                            .map_err(|error| footer_malformed(part, error.to_string()))?;
                        let replacement = if key == b"val" && local.as_ref() == b"schemeClr" {
                            let choice = ColorChoice::Scheme {
                                value: value.into_owned(),
                                transforms: Vec::new(),
                                raw_children: Default::default(),
                            };
                            let color =
                                oxoffice_drawing::color::resolve_color(&choice, color_map, &colors)
                                    .map_err(|error| footer_malformed(part, error.to_string()))?;
                            let (red, green, blue) = (color.red, color.green, color.blue);
                            let name = element.name().as_ref().to_vec();
                            let prefix = name
                                .iter()
                                .rposition(|byte| *byte == b':')
                                .map_or(&b""[..], |offset| &name[..=offset]);
                            let mut renamed = prefix.to_vec();
                            renamed.extend_from_slice(b"srgbClr");
                            edits.push((begin + 1..begin + 1 + name.len(), renamed.clone()));
                            replacement_name = Some(renamed);
                            Some(format!("{red:02X}{green:02X}{blue:02X}"))
                        } else if key == b"typeface" {
                            let fonts = &theme.theme_elements.font_scheme;
                            let face = match value.as_ref() {
                                "+mj-lt" => Some(&fonts.major_font.latin.typeface),
                                "+mn-lt" => Some(&fonts.minor_font.latin.typeface),
                                "+mj-ea" => Some(&fonts.major_font.east_asian.typeface),
                                "+mn-ea" => Some(&fonts.minor_font.east_asian.typeface),
                                "+mj-cs" => Some(&fonts.major_font.complex_script.typeface),
                                "+mn-cs" => Some(&fonts.minor_font.complex_script.typeface),
                                _ => None,
                            };
                            if face.is_some_and(|face| face.is_empty()) {
                                let destination_fonts =
                                    &destination_theme.theme_elements.font_scheme;
                                let (source, destination) = if value.starts_with("+mj-") {
                                    (&fonts.major_font, &destination_fonts.major_font)
                                } else {
                                    (&fonts.minor_font, &destination_fonts.minor_font)
                                };
                                // An empty East Asian or complex-script face picks a font
                                // per script from the theme's supplemental list, which a
                                // run cannot hold: the token stays and follows the
                                // destination theme, as notes follow the one notes master
                                // of a deck in PowerPoint. An empty Latin face has no such
                                // list and is kept only under an equal font collection.
                                let automatic_script =
                                    value.ends_with("-ea") || value.ends_with("-cs");
                                if !automatic_script && source != destination {
                                    return Err(footer_malformed(
                                        part,
                                        "unrepresentable automatic source notes script-font inheritance",
                                    ));
                                }
                                None
                            } else {
                                face.cloned()
                            }
                        } else {
                            None
                        };
                        if let Some(replacement) = replacement {
                            let raw_start: &[u8] = element.as_ref();
                            let offset = attribute.value.as_ref().as_ptr() as usize
                                - raw_start.as_ptr() as usize
                                + begin
                                + 1;
                            edits.push((
                                offset..offset + attribute.value.len(),
                                quick_xml::escape::escape(&replacement).as_bytes().to_vec(),
                            ));
                        }
                    }
                }
                if !empty {
                    stack.push((modeled, replacement_name));
                }
            }
            XmlEvent::End(end_element) => {
                if let Some((_, Some(name))) = stack.pop() {
                    edits.push((
                        begin + 2..begin + 2 + end_element.name().as_ref().len(),
                        name,
                    ));
                }
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut result = xml.to_vec();
    let mut next = xml.len();
    for (range, replacement) in edits.into_iter().rev() {
        if range.end > next {
            return Err(footer_malformed(
                part,
                "overlapping source notes theme edits",
            ));
        }
        next = range.start;
        result.splice(range, replacement);
    }
    Ok(result)
}

fn materialize_imported_notes_text(
    children: &mut [ShapeTreeChild],
    defaults: &CT_TextListStyle,
    part: &str,
) -> Result<()> {
    for child in children {
        match child {
            ShapeTreeChild::Shape(shape) => {
                let Some(body) = &mut shape.text_body else {
                    continue;
                };
                let local = body.list_style().cloned().unwrap_or_default();
                let effective = local
                    .with_inherited_defaults(defaults)
                    .map_err(|error| footer_malformed(part, error.to_string()))?;
                for index in 0..body.paragraphs().len() {
                    let paragraph = body.paragraph_mut(index).expect("existing notes paragraph");
                    let level = usize::from(
                        paragraph
                            .properties
                            .as_ref()
                            .and_then(|properties| properties.level)
                            .unwrap_or(0),
                    ) + 1;
                    if let Some(inherited) = effective
                        .level(level)
                        .or(effective.default_paragraph_properties.as_deref())
                    {
                        let local = paragraph.properties.clone().unwrap_or_default();
                        paragraph.properties = Some(
                            local
                                .with_inherited_defaults(inherited)
                                .map_err(|error| footer_malformed(part, error.to_string()))?,
                        );
                    }
                    if let Some(inherited) = paragraph
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.default_run_properties.as_ref())
                    {
                        for run in &mut paragraph.runs {
                            let properties = match run {
                                TextRun::Run(run) => &mut run.properties,
                                TextRun::Break(line_break) => &mut line_break.properties,
                                TextRun::Field(field) => &mut field.run_properties,
                            };
                            let local = properties.clone().unwrap_or_default();
                            *properties = Some(
                                local
                                    .with_inherited_defaults(inherited)
                                    .map_err(|error| footer_malformed(part, error.to_string()))?,
                            );
                        }
                        let local = paragraph.end_properties.clone().unwrap_or_default();
                        paragraph.end_properties = Some(
                            local
                                .with_inherited_defaults(inherited)
                                .map_err(|error| footer_malformed(part, error.to_string()))?,
                        );
                    }
                }
            }
            ShapeTreeChild::GroupShape(group) => {
                materialize_imported_notes_text(&mut group.children, defaults, part)?
            }
            _ => {}
        }
    }
    Ok(())
}
