//! Importing document fragments: parts, styles and numbering, notes and markers, and fresh identities for what they bring.

use super::*;

pub(crate) fn import_document_fragment_content(
    document: &mut Document,
    bytes: &[u8],
    include_final_section_properties: bool,
    policy: FragmentConflictPolicy,
    destination_part: &str,
    source_owned_numbering: bool,
    note_copy_sources: &mut Vec<(StoryKind, String, i32, String, i32)>,
    template: Option<(
        &mut crate::template::TemplateEvaluation<'_, '_>,
        crate::document::TemplateFragmentSources,
    )>,
) -> Result<(
    Vec<u8>,
    Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
)> {
    let mut identity_state = BodyIdentityState::from_documents(std::slice::from_ref(document))?;
    import_fragment_content_with_state(
        document,
        bytes,
        include_final_section_properties,
        true,
        policy,
        destination_part,
        &mut identity_state,
        true,
        source_owned_numbering,
        note_copy_sources,
        template,
    )
    .map(|imported| (imported.xml, imported.source))
}

fn apply_fragment_xml_edits(
    xml: &mut Vec<u8>,
    source: &mut Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    operation: Option<&mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>>,
    mut edits: Vec<FieldSourceEdit>,
) -> Result<()> {
    if edits.is_empty() {
        return Ok(());
    }
    edits.sort_by_key(|edit| edit.start);
    if let Some(parent) = source.as_ref() {
        let operation =
            operation.ok_or_else(|| Error::Other("fragment source lost its operation".into()))?;
        let mut pieces = Vec::new();
        let mut at = 0;
        for edit in &edits {
            if edit.start < at || edit.start > edit.end || edit.end > parent.as_bytes().len() {
                return Err(Error::Other("invalid fragment source edit ranges".into()));
            }
            pieces.push((Some(parent.as_ref()), at..edit.start, &[][..]));
            pieces.push((None, 0..0, edit.replacement.as_slice()));
            at = edit.end;
        }
        pieces.push((Some(parent.as_ref()), at..parent.as_bytes().len(), &[][..]));
        let updated = operation.compose_sources(&pieces)?;
        let retained = operation.repeat_source(&updated)?;
        *xml = operation.finish_source(retained)?;
        *source = Some(Arc::new(updated));
    } else {
        for edit in edits.into_iter().rev() {
            xml.splice(edit.start..edit.end, edit.replacement);
        }
    }
    Ok(())
}

/// Materialize only document-default font components on the imported body.
fn prepare_fragment_body_effective_defaults(fragment: &mut Document) -> Result<()> {
    fn paragraph(
        paragraph: &mut CT_P,
        styles: &CT_Styles,
        style_only: &CT_Styles,
        table: Option<&str>,
    ) {
        let default = styles
            .get_default(StyleType::Paragraph)
            .map(|style| style.style_id.as_str());
        let properties = paragraph.properties.get_or_insert_with(Default::default);
        if properties.style_id.is_none() {
            properties.style_id = default.map(str::to_owned);
        }
        let mut table_properties = CT_RPr::default();
        if let Some(id) = table {
            for style in style::collect_style_chain(id, styles).into_iter().rev() {
                if let Some(properties) = &style.rpr {
                    table_properties.merge_from(properties);
                }
            }
        }
        for run in &mut paragraph.runs {
            let run_style = run
                .properties
                .as_ref()
                .and_then(|properties| properties.style_id.as_deref());
            let inherited = style::resolve_run_properties(
                properties.style_id.as_deref(),
                run_style,
                style_only,
            );
            let mut without_defaults = table_properties.clone();
            without_defaults.merge_from(&inherited);
            let mut resolved = styles
                .doc_defaults
                .as_ref()
                .and_then(|defaults| defaults.rpr.as_ref())
                .cloned()
                .unwrap_or_default();
            resolved.merge_from(&without_defaults);
            let direct = run.properties.get_or_insert_with(Default::default);
            if direct.font_ascii.is_none() && resolved.font_ascii != without_defaults.font_ascii {
                direct.font_ascii = resolved.font_ascii;
            }
            if direct.font_hansi.is_none() && resolved.font_hansi != without_defaults.font_hansi {
                direct.font_hansi = resolved.font_hansi;
            }
            if direct.sz.is_none() && resolved.sz != without_defaults.sz {
                direct.sz = resolved.sz;
            }
            if *direct == CT_RPr::default() {
                run.properties = None;
            }
        }
    }
    fn prepare_table(table: &mut CT_Tbl, styles: &CT_Styles, style_only: &CT_Styles) {
        let table_style = table
            .properties
            .as_ref()
            .and_then(|properties| properties.style_id.as_deref());
        for row in &mut table.rows {
            prepare_row(row, styles, style_only, table_style);
        }
        for (_, _, control) in &mut table.content_controls {
            prepare_control(control, styles, style_only, table_style);
        }
    }
    fn prepare_row(
        row: &mut CT_Row,
        styles: &CT_Styles,
        style_only: &CT_Styles,
        table_style: Option<&str>,
    ) {
        for cell in &mut row.cells {
            prepare_cell(cell, styles, style_only, table_style);
        }
        for (_, _, control) in &mut row.content_controls {
            prepare_control(control, styles, style_only, table_style);
        }
    }
    fn prepare_cell(
        cell: &mut CT_Tc,
        styles: &CT_Styles,
        style_only: &CT_Styles,
        table_style: Option<&str>,
    ) {
        for child in &mut cell.content {
            match child {
                CellContent::Paragraph(p) => paragraph(p, styles, style_only, table_style),
                CellContent::Table(t) => prepare_table(t, styles, style_only),
                CellContent::ContentControl(c) => {
                    prepare_control(c, styles, style_only, table_style)
                }
            }
        }
    }
    fn prepare_control(
        control: &mut CT_Sdt,
        styles: &CT_Styles,
        style_only: &CT_Styles,
        table_style: Option<&str>,
    ) {
        for item in &mut control.content {
            match item {
                SdtContent::Paragraph(p) => paragraph(p, styles, style_only, table_style),
                SdtContent::Table(t) => prepare_table(t, styles, style_only),
                SdtContent::ContentControl(c) => {
                    prepare_control(c, styles, style_only, table_style)
                }
                SdtContent::Row(row) => prepare_row(row, styles, style_only, table_style),
                SdtContent::Cell(cell) => prepare_cell(cell, styles, style_only, table_style),
                SdtContent::Run(_) | SdtContent::RawXml(_) => {}
            }
        }
    }
    let styles = &fragment.styles;
    let mut style_only = CT_Styles::clone(styles);
    style_only.doc_defaults = None;
    for item in &mut fragment.document.body.content {
        match item {
            BodyContent::Paragraph(p) => paragraph(p, styles, &style_only, None),
            BodyContent::Table(t) => prepare_table(t, styles, &style_only),
            BodyContent::ContentControl(control) => {
                prepare_control(control, styles, &style_only, None)
            }
            BodyContent::RawXml(_) => {}
        }
    }
    // Every prepared paragraph now names its source default explicitly. It
    // must not become a second destination default or restyle existing text.
    for style in &mut Arc::make_mut(&mut fragment.styles).styles {
        if style.style_type == StyleType::Paragraph {
            style.is_default = false;
        }
    }
    Ok(())
}

pub(super) fn import_fragment_content_with_state(
    document: &mut Document,
    bytes: &[u8],
    include_final_section_properties: bool,
    allow_external_relationships: bool,
    policy: FragmentConflictPolicy,
    destination_part: &str,
    identity_state: &mut BodyIdentityState,
    keep_free_names: bool,
    source_owned_numbering: bool,
    note_copy_sources: &mut Vec<(StoryKind, String, i32, String, i32)>,
    mut template: Option<(
        &mut crate::template::TemplateEvaluation<'_, '_>,
        crate::document::TemplateFragmentSources,
    )>,
) -> Result<ImportedFragmentContent> {
    let mut fragment_sources = None;
    let mut fragment = if let Some((evaluation, input)) = template.as_mut() {
        let package = OpcPackage::from_reader(std::io::Cursor::new(bytes))?;
        Document::from_opened_package(
            package,
            true,
            Some(crate::document::ScopedDocumentRead {
                operation: &mut evaluation.operation,
                source: input.main.as_ref(),
                related: Some(&input.related),
            }),
        )
    } else {
        Document::from_bytes(bytes)
    }
    .map_err(|error| Error::Other(format!("invalid document fragment: {error}")))?;
    if let Some((_, input)) = template.as_mut() {
        let input = crate::document::TemplateFragmentSources {
            main: Arc::clone(&input.main),
            related: std::mem::take(&mut input.related),
        };
        fragment_sources = Some(crate::document::TemplateMainSources::from_fragment_read(
            &fragment.package,
            &fragment.doc_part_name,
            &fragment.footnotes,
            input,
        ));
    }
    let mut fragment_xml_source = template.as_ref().map(|(_, input)| Arc::clone(&input.main));
    let mut fragment_xml = if template.is_none() {
        fragment
            .package
            .get_part(&fragment.doc_part_name)
            .ok_or_else(|| Error::Other("document fragment main part is missing".to_owned()))?
            .to_vec()
    } else {
        Vec::new()
    };
    if let Some((evaluation, _)) = template.as_mut() {
        fragment.prepare_staged_template_package(
            &mut evaluation.operation,
            &mut evaluation.lineage,
            fragment_sources
                .as_mut()
                .expect("actual fragment reader companions"),
        )?;
    } else {
        fragment.prepare_staged_package()?;
    }
    // Both whole-document policies read the body from the model, which
    // settles the namespaces the source scoped on its elements.
    let from_model =
        policy.preserves_source_effective_defaults() || policy.keeps_destination_formatting();
    if policy.preserves_source_effective_defaults() {
        prepare_fragment_body_effective_defaults(&mut fragment)?;
    }
    if from_model {
        fragment_xml = fragment.document.to_xml()?;
        fragment
            .package
            .set_part(&fragment.doc_part_name, fragment_xml.clone());
    }
    if let Some((evaluation, _)) = template.as_mut() {
        let retained = evaluation.operation.repeat_source(
            fragment_xml_source
                .as_ref()
                .expect("actual authoritative fragment input"),
        )?;
        fragment_xml = evaluation.operation.finish_source(retained)?;
    }
    if document.glossary_part_name.as_deref() == Some(destination_part) {
        // The prepared physical body selects current note ids, excluding root payload.
        if let Some((evaluation, _)) = template.as_mut() {
            let mut writer = crate::document::TemplateMainWriter {
                operation: &mut evaluation.operation,
                lineage: &evaluation.lineage,
                sources: fragment_sources
                    .as_mut()
                    .expect("fragment omission sources"),
            };
            (fragment_xml, fragment_xml_source) = fragment
                .omit_glossary_selected_body_comments_in(
                    include_final_section_properties,
                    Some(&mut writer),
                )?;
        } else {
            fragment_xml =
                fragment.omit_glossary_selected_body_comments(include_final_section_properties)?;
        }
    }
    let final_section_properties = fragment.document.body.sect_pr.take();
    if include_final_section_properties && let Some(section_properties) = final_section_properties {
        let mut paragraph = CT_P::new();
        paragraph.properties = Some(CT_PPr {
            sect_pr: Some(section_properties),
            ..Default::default()
        });
        fragment
            .document
            .body
            .content
            .push(BodyContent::Paragraph(Box::new(paragraph)));
    }
    let model_xml = if from_model {
        fragment_xml.clone()
    } else if let Some((evaluation, _)) = template.as_mut() {
        let mut writer = crate::document::TemplateMainWriter {
            operation: &mut evaluation.operation,
            lineage: &evaluation.lineage,
            sources: fragment_sources.as_mut().expect("fragment reader writer"),
        };
        let source = writer.document_source(&fragment.document)?;
        writer.operation.finish_source(source)?
    } else {
        fragment.document.to_xml()?
    };
    let dependency_source = if document.glossary_part_name.as_deref() == Some(destination_part) {
        wrap_fragment_companion(&crate::document::package_authoritative_body_fragment(
            &model_xml,
            false,
            &BTreeMap::new(),
        )?)
    } else {
        model_xml
    };
    prune_document_fragment_dependencies(&mut fragment, &dependency_source, true)?;
    let mut fragment_identity_values = body_identity_values(&fragment_xml)?;
    for companion in fragment_dependency_companions(&fragment, &dependency_source)? {
        for id in body_identity_values(&wrap_fragment_companion(&companion))?.comment_ids {
            if !fragment_identity_values.comment_ids.contains(&id) {
                fragment_identity_values.comment_ids.push(id);
            }
        }
    }
    validate_fragment_identity_ownership(&fragment_identity_values)?;
    let companion_comment_ids = &fragment_identity_values.comment_ids[..];
    if source_owned_numbering {
        document.source_owned_numbering_remap(&fragment)?;
    }
    remap_fragment_style_collisions(
        document,
        &mut fragment,
        &mut fragment_xml,
        policy.reuse_styles(),
        policy.keeps_destination_formatting(),
        policy.reuse_numbering(),
        source_owned_numbering,
        template.as_mut().map(|(evaluation, _)| {
            (
                &mut evaluation.operation,
                &mut fragment_xml_source,
                fragment_sources
                    .as_mut()
                    .expect("fragment style source graph"),
            )
        }),
    )?;
    let dependency_xml =
        wrap_fragment_companion(&crate::document::package_authoritative_body_fragment(
            &fragment_xml,
            include_final_section_properties,
            &BTreeMap::new(),
        )?);
    let used_relationships = relationship_ids_in_xml(&dependency_xml)?;
    let source_rels = fragment
        .package
        .get_part_rels(&fragment.doc_part_name)
        .cloned()
        .unwrap_or_default();
    let mut relationship_map = BTreeMap::new();
    let mut comment_relationship_map = BTreeMap::new();
    let mut part_map = BTreeMap::new();
    let mut imports = Vec::new();
    let mut external_imports = Vec::new();
    for relationship_id in used_relationships.iter().cloned() {
        let relationship = source_rels.get_by_id(&relationship_id).ok_or_else(|| {
            Error::Other(format!(
                "rich mail merge fragment relationship {relationship_id} is missing"
            ))
        })?;
        if !crate::document::relationship_is_internal(relationship) {
            if !allow_external_relationships {
                return Err(Error::Other(
                    "rich mail merge fragment has a non-internal relationship".to_owned(),
                ));
            }
            external_imports.push((
                true,
                destination_part.to_owned(),
                relationship_id,
                relationship.clone(),
            ));
            continue;
        }
        let source_part =
            OpcPackage::resolve_rel_target(&fragment.doc_part_name, &relationship.target);
        imports.push((
            true,
            destination_part.to_owned(),
            relationship_id,
            relationship.clone(),
            source_part,
        ));
    }
    let mut binding_stores = Vec::new();
    for store_item_id in fragment_store_item_ids(&dependency_xml)? {
        let source_part = crate::content_control::resolve_custom_xml_part(
            &fragment.package,
            &fragment.doc_part_name,
            &store_item_id,
        )?;
        let relationship = source_rels.items.iter().find(|relationship| {
            relationship.rel_type.ends_with("/customXml")
                && crate::document::relationship_is_internal(relationship)
                && OpcPackage::resolve_rel_target(&fragment.doc_part_name, &relationship.target)
                    == source_part
        }).ok_or_else(|| Error::Other(format!(
            "document fragment custom XML store {store_item_id} has no main-part relationship"
        )))?;
        if !imports
            .iter()
            .any(|(_, _, id, _, _)| id == &relationship.id)
        {
            imports.push((
                true,
                document.doc_part_name.clone(),
                relationship.id.clone(),
                relationship.clone(),
                source_part.clone(),
            ));
        }
        binding_stores.push((store_item_id, source_part));
    }
    if let Some(comment_xml) = Document::fragment_comment_dependency_xml(
        fragment.comments.as_ref(),
        fragment.comments_extended.as_ref(),
        companion_comment_ids,
    )? {
        for store_item_id in fragment_store_item_ids(&comment_xml)? {
            let source_item_part = crate::content_control::resolve_custom_xml_part(
                &fragment.package,
                &fragment.doc_part_name,
                &store_item_id,
            )?;
            if !binding_stores.iter().any(|(id, _)| id == &store_item_id) {
                binding_stores.push((store_item_id, source_item_part));
            }
        }
        let destination_owner = document.ensure_fragment_comment_models_staged()?;
        let source_owner = fragment.comments_part_name.as_ref().ok_or_else(|| {
            Error::Other("document fragment comments part name is missing".to_owned())
        })?;
        let source_rels = fragment
            .package
            .get_part_rels(source_owner)
            .cloned()
            .unwrap_or_default();
        for relationship_id in relationship_ids_in_xml(&comment_xml)? {
            let relationship = source_rels.get_by_id(&relationship_id).ok_or_else(|| {
                Error::Other(format!(
                    "document fragment comment relationship {relationship_id} is missing"
                ))
            })?;
            if !crate::document::relationship_is_internal(relationship) {
                if !allow_external_relationships {
                    return Err(Error::Other(
                        "rich mail merge fragment has a non-internal relationship".to_owned(),
                    ));
                }
                external_imports.push((
                    false,
                    destination_owner.clone(),
                    relationship_id,
                    relationship.clone(),
                ));
                continue;
            }
            let source_part = OpcPackage::resolve_rel_target(source_owner, &relationship.target);
            imports.push((
                false,
                destination_owner.clone(),
                relationship_id,
                relationship.clone(),
                source_part,
            ));
        }
    }
    let mut note_copies: Vec<FragmentNoteCopy> = Vec::new();
    let mut note_relationship_imports = Vec::new();
    let mut note_external_imports = Vec::new();
    let mut pending_notes = fragment_note_references(&dependency_xml)?;
    if let Some(comments) = fragment.comments.as_ref()
        && let Some(xml) = Document::fragment_comment_dependency_xml(
            Some(comments),
            fragment.comments_extended.as_ref(),
            companion_comment_ids,
        )?
    {
        pending_notes.extend(fragment_note_references(&xml)?);
    }
    let mut seen_notes = HashSet::new();
    let mut reserved_note_ids: HashMap<StoryKind, HashSet<i32>> = HashMap::new();
    let mut note_id_maps: HashMap<StoryKind, BTreeMap<String, String>> = HashMap::new();
    let mut note_index = 0;
    while note_index < pending_notes.len() {
        let (kind, old_id) = pending_notes[note_index];
        note_index += 1;
        if !seen_notes.insert((kind, old_id)) {
            continue;
        }
        let (source_part, note_xml, note_source) = fragment.fragment_note_dependency_in(
            kind,
            old_id,
            template.as_mut().map(|(evaluation, _)| {
                (
                    &mut evaluation.operation,
                    fragment_sources
                        .as_ref()
                        .expect("actual fragment note sources"),
                )
            }),
        )?;
        for store_item_id in fragment_store_item_ids(&note_xml)? {
            let source_item_part = crate::content_control::resolve_custom_xml_part(
                &fragment.package,
                &fragment.doc_part_name,
                &store_item_id,
            )?;
            if !binding_stores.iter().any(|(id, _)| id == &store_item_id) {
                binding_stores.push((store_item_id, source_item_part));
            }
        }
        let destination_note_part = if let Some((evaluation, _)) = template.as_mut() {
            let mut writer = crate::document::TemplateMainWriter {
                operation: &mut evaluation.operation,
                lineage: &evaluation.lineage,
                sources: &mut evaluation.sources,
            };
            document.ensure_fragment_note_part_staged_in(kind, Some(&mut writer))?
        } else {
            document.ensure_fragment_note_part_staged(kind)?
        };
        let current_notes = if let Some((evaluation, _)) = template.as_mut() {
            let mut writer = crate::document::TemplateMainWriter {
                operation: &mut evaluation.operation,
                lineage: &evaluation.lineage,
                sources: &mut evaluation.sources,
            };
            let bytes = document
                .package
                .get_part_shared(&destination_note_part)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "document fragment destination note part {destination_note_part} is missing"
                    ))
                })?;
            if let Some(source) = writer.take_related_source(&destination_note_part, bytes)? {
                let notes = writer.operation.source_notes(&source)?;
                writer
                    .sources
                    .put_related_source(&destination_note_part, bytes, source);
                notes
            } else {
                CT_Footnotes::from_xml(bytes)?
            }
        } else {
            CT_Footnotes::from_xml(document.package.get_part(&destination_note_part).ok_or_else(|| Error::Other(format!("document fragment destination note part {destination_note_part} is missing")))?)?
        };
        let source_bytes = fragment
            .package
            .get_part_shared(&source_part)
            .ok_or_else(|| {
                Error::Other(format!(
                    "document fragment source note part {source_part} is missing"
                ))
            })?;
        let source_notes = if let Some((evaluation, _)) = template.as_mut()
            && let Some(source) = fragment_sources
                .as_ref()
                .and_then(|sources| sources.related_source(&source_part, source_bytes))
        {
            evaluation.operation.source_notes(source)?
        } else {
            CT_Footnotes::from_xml(source_bytes)?
        };
        let used = reserved_note_ids.entry(kind).or_insert_with(|| {
            current_notes
                .footnotes
                .iter()
                .chain(&source_notes.footnotes)
                .map(|note| note.id)
                .collect()
        });
        let mut new_id = 2_i32;
        while used.contains(&new_id) {
            new_id = new_id.checked_add(1).ok_or_else(|| {
                Error::Other(format!("document fragment {kind:?} ID range is exhausted"))
            })?;
        }
        used.insert(new_id);
        note_copy_sources.push((
            kind,
            source_part.clone(),
            old_id,
            destination_note_part.clone(),
            new_id,
        ));
        note_id_maps
            .entry(kind)
            .or_default()
            .insert(old_id.to_string(), new_id.to_string());
        for related in fragment_note_references(&note_xml)? {
            if !seen_notes.contains(&related) && !pending_notes.contains(&related) {
                pending_notes.push(related);
            }
        }
        let source_note_rels = fragment
            .package
            .get_part_rels(&source_part)
            .cloned()
            .unwrap_or_default();
        for relationship_id in relationship_ids_in_xml(&note_xml)? {
            if note_relationship_imports.iter().any(
                |(owner, id, _, _): &(
                    String,
                    String,
                    oxoffice_opc::relationship::Relationship,
                    String,
                )| owner == &destination_note_part && id == &relationship_id,
            ) || note_external_imports.iter().any(
                |(owner, id, _): &(String, String, oxoffice_opc::relationship::Relationship)| {
                    owner == &destination_note_part && id == &relationship_id
                },
            ) {
                continue;
            }
            let relationship = source_note_rels
                .get_by_id(&relationship_id)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "document fragment {kind:?} relationship {relationship_id} is missing"
                    ))
                })?
                .clone();
            if crate::document::relationship_is_internal(&relationship) {
                let child = OpcPackage::resolve_rel_target(&source_part, &relationship.target);
                note_relationship_imports.push((
                    destination_note_part.clone(),
                    relationship_id,
                    relationship,
                    child,
                ));
            } else {
                if !allow_external_relationships {
                    return Err(Error::Other(
                        "rich mail merge fragment has a non-internal relationship".to_owned(),
                    ));
                }
                note_external_imports.push((
                    destination_note_part.clone(),
                    relationship_id,
                    relationship,
                ));
            }
        }
        note_copies.push(FragmentNoteCopy {
            kind,
            destination_part: destination_note_part,
            xml: note_xml,
            source: note_source,
            replace_id: None,
        });
    }
    let edits = fragment_note_id_edits(&fragment_xml, &note_id_maps)?;
    apply_fragment_xml_edits(
        &mut fragment_xml,
        &mut fragment_xml_source,
        template
            .as_mut()
            .map(|(evaluation, _)| &mut evaluation.operation),
        edits,
    )?;
    for note in &mut note_copies {
        let edits = fragment_note_id_edits(&note.xml, &note_id_maps)?;
        apply_fragment_xml_edits(
            &mut note.xml,
            &mut note.source,
            template
                .as_mut()
                .map(|(evaluation, _)| &mut evaluation.operation),
            edits,
        )?;
    }
    if let Some(comments) = fragment.comments.as_mut() {
        *comments = CT_Comments::from_xml(&patch_fragment_note_ids(
            &comments.to_xml()?,
            &note_id_maps,
        )?)?;
    }
    for (_, source_part) in &binding_stores {
        if imports.iter().any(|(_, _, _, _, part)| part == source_part) {
            continue;
        }
        let relationship = source_rels
            .items
            .iter()
            .find(|relationship| {
                relationship.rel_type.ends_with("/customXml")
                    && crate::document::relationship_is_internal(relationship)
                    && OpcPackage::resolve_rel_target(&fragment.doc_part_name, &relationship.target)
                        == *source_part
            })
            .ok_or_else(|| {
                Error::Other(format!(
                    "document fragment custom XML store {source_part} has no main-part relationship"
                ))
            })?;
        // Binding stores are document-wide, even when selected content lives in another story.
        imports.push((
            true,
            document.doc_part_name.clone(),
            relationship.id.clone(),
            relationship.clone(),
            source_part.clone(),
        ));
    }
    let mut occupied_store_ids = binding_stores
        .iter()
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    for package in [&fragment.package, &document.package] {
        for (part, xml) in &package.parts {
            if package.content_types.content_type_for(part)
                == Some("application/vnd.openxmlformats-officedocument.customXmlProperties+xml")
                && let Some(id) = crate::content_control::parse_store_item_id(xml)?
            {
                occupied_store_ids.insert(crate::content_control::normalize_store_item_id(&id));
            }
        }
    }
    let mut binding_remaps = Vec::new();
    for (store_item_id, source_item_part) in &binding_stores {
        match crate::content_control::custom_xml_store_item_count(
            &document.package,
            &document.doc_part_name,
            store_item_id,
        )? {
            0 => {}
            1 => {
                let new_id = (1_u64..)
                    .map(|index| format!("{{F2760000-0000-4000-8000-{index:012X}}}"))
                    .find(|candidate| {
                        !occupied_store_ids
                            .contains(&crate::content_control::normalize_store_item_id(candidate))
                    })
                    .ok_or_else(|| {
                        Error::Other(
                            "document fragment custom XML store ID range is exhausted".to_owned(),
                        )
                    })?;
                occupied_store_ids.insert(crate::content_control::normalize_store_item_id(&new_id));
                binding_remaps.push((store_item_id.clone(), source_item_part.clone(), new_id));
            }
            _ => {
                return Err(Error::Other(format!(
                    "document fragment custom XML store {store_item_id} is ambiguous in the destination"
                )));
            }
        }
    }
    let mut remapped_store_properties = HashSet::new();
    for (_, item_part, _) in &binding_remaps {
        if let Some(rels) = fragment.package.get_part_rels(item_part) {
            for relationship in &rels.items {
                if relationship.rel_type.ends_with("/customXmlProps")
                    && crate::document::relationship_is_internal(relationship)
                {
                    remapped_store_properties.insert(OpcPackage::resolve_rel_target(
                        item_part,
                        &relationship.target,
                    ));
                }
            }
        }
    }
    let mut closure = HashSet::new();
    for (_, _, _, _, source_part) in &imports {
        discover_fragment_part_closure(&fragment.package, source_part, &mut closure)?;
    }
    for (_, _, _, source_part) in &note_relationship_imports {
        discover_fragment_part_closure(&fragment.package, source_part, &mut closure)?;
    }
    let mut closure = closure.into_iter().collect::<Vec<_>>();
    closure.sort();
    if !allow_external_relationships
        && closure.iter().any(|part| {
            fragment
                .package
                .get_part_rels(part)
                .is_some_and(|relationships| {
                    relationships.items.iter().any(|relationship| {
                        !crate::document::relationship_is_internal(relationship)
                    })
                })
        })
    {
        return Err(Error::Other(
            "rich mail merge fragment has a non-internal relationship".to_owned(),
        ));
    }

    // Header and footer parts are copied whole, and the list numbers they
    // use are remapped after the copy: one holding a list is never shared
    // with an equal destination part, whose numbers mean other lists. With
    // source-owned numbering the numbers keep their meaning.
    let mut numbered_header_footers = HashSet::new();
    for (selected_owner, _, _, relationship, source_part) in &imports {
        if *selected_owner
            && !source_owned_numbering
            && (relationship.rel_type == rel_types::HEADER
                || relationship.rel_type == rel_types::FOOTER)
        {
            let xml = fragment.package.get_part(source_part).ok_or_else(|| {
                Error::Other(format!("document fragment part {source_part} is missing"))
            })?;
            if word_value_attributes(xml, &[b"numId"])?
                .iter()
                .any(|value| value != "0")
            {
                numbered_header_footers.insert(source_part.clone());
            }
        }
    }
    for source_part in closure {
        let destination_part = if policy.reuse_related_parts()
            && !remapped_store_properties.contains(&source_part)
            && !numbered_header_footers.contains(&source_part)
        {
            equivalent_fragment_leaf_part(&fragment.package, document, &source_part).map_or_else(
                || {
                    document
                        .identifiers
                        .reserve_fragment_part_name(&source_part)
                },
                Ok,
            )?
        } else {
            document
                .identifiers
                .reserve_fragment_part_name(&source_part)?
        };
        part_map.insert(source_part, destination_part);
    }
    for (source_part, destination_part) in &part_map {
        let payload = fragment.package.get_part(source_part).ok_or_else(|| {
            Error::Other(format!(
                "document fragment reachable part {source_part} is missing"
            ))
        })?;
        if let Some(relationships) = fragment.package.get_part_rels(source_part) {
            for relationship in &relationships.items {
                if !crate::document::relationship_is_internal(relationship) {
                    continue;
                }
                let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
                let copied_target = part_map.get(&target).ok_or_else(|| {
                    Error::Other(format!("fragment target {target} was not reserved"))
                })?;
                let new_target = relative_fragment_target(destination_part, copied_target);
                if new_target != relationship.target
                    && !relationship.target.is_empty()
                    && payload
                        .windows(relationship.target.len())
                        .any(|window| window == relationship.target.as_bytes())
                {
                    return Err(Error::Other(format!(
                        "document fragment part {source_part} embeds renamed target {}; opaque payload cannot be rewritten safely",
                        relationship.target
                    )));
                }
            }
        }
        for (old, new) in &part_map {
            if old != new
                && payload
                    .windows(old.len())
                    .any(|window| window == old.as_bytes())
            {
                return Err(Error::Other(format!(
                    "document fragment part {source_part} embeds renamed part name {old}; opaque payload cannot be rewritten safely for {destination_part}"
                )));
            }
        }
    }
    let mut copied = HashSet::new();
    for (selected_owner, destination_owner, relationship_id, relationship, source_part) in imports {
        let destination_part = copy_fragment_part(
            &fragment.package,
            document,
            &source_part,
            &part_map,
            &mut copied,
        )?;
        let target = relative_fragment_target(&destination_owner, &destination_part);
        let destination_id = document
            .add_internal_relationship_checked(
                &destination_owner,
                &relationship.rel_type,
                &target,
            )
            .map_err(|error| {
                Error::Other(format!(
                    "rich mail merge relationship allocation failed for {destination_owner}: {error}"
                ))
            })?;
        if selected_owner {
            if used_relationships.contains(&relationship_id) {
                document.register_pending_relationship(&destination_owner, &destination_id);
            }
            relationship_map.insert(relationship_id, destination_id);
        } else {
            comment_relationship_map.insert(relationship_id, destination_id);
        }
    }
    for (selected_owner, destination_owner, relationship_id, relationship) in external_imports {
        let destination_id = document.add_external_relationship_checked(
            &destination_owner,
            &relationship.rel_type,
            &relationship.target,
        )?;
        if selected_owner {
            document.register_pending_relationship(&destination_owner, &destination_id);
            relationship_map.insert(relationship_id, destination_id);
        } else {
            comment_relationship_map.insert(relationship_id, destination_id);
        }
    }
    let mut note_relationship_maps: HashMap<String, BTreeMap<String, String>> = HashMap::new();
    for (destination_owner, old_id, relationship, source_part) in note_relationship_imports {
        let copied_part = copy_fragment_part(
            &fragment.package,
            document,
            &source_part,
            &part_map,
            &mut copied,
        )?;
        let new_id = document.add_relative_internal_relationship_checked(
            &destination_owner,
            &relationship.rel_type,
            &copied_part,
        )?;
        note_relationship_maps
            .entry(destination_owner)
            .or_default()
            .insert(old_id, new_id);
    }
    for (destination_owner, old_id, relationship) in note_external_imports {
        let new_id = document.add_external_relationship_checked(
            &destination_owner,
            &relationship.rel_type,
            &relationship.target,
        )?;
        note_relationship_maps
            .entry(destination_owner)
            .or_default()
            .insert(old_id, new_id);
    }
    for (store_item_id, source_item_part, new_id) in binding_remaps {
        let item_relationships = fragment.package.get_part_rels(&source_item_part).ok_or_else(|| Error::Other(format!(
                    "document fragment custom XML store {store_item_id} has no item properties relationship"
                )))?;
        let props_relationship = item_relationships.items.iter().find(|relationship| relationship.rel_type.ends_with("/customXmlProps") && crate::document::relationship_is_internal(relationship)).ok_or_else(|| Error::Other(format!(
                    "document fragment custom XML store {store_item_id} has no item properties relationship"
                )))?;
        let source_props =
            OpcPackage::resolve_rel_target(&source_item_part, &props_relationship.target);
        let destination_props = part_map.get(&source_props).ok_or_else(|| {
            Error::Other(format!(
                "document fragment custom XML properties {source_props} were not copied"
            ))
        })?;
        if fragment_store_item_ids(&fragment_xml)?.contains(&store_item_id) {
            let edits = fragment_store_id_edits(
                &fragment_xml,
                &store_item_id,
                &new_id,
                b"dataBinding",
                b"storeItemID",
            )?;
            apply_fragment_xml_edits(
                &mut fragment_xml,
                &mut fragment_xml_source,
                template
                    .as_mut()
                    .map(|(evaluation, _)| &mut evaluation.operation),
                edits,
            )?;
        }
        for note in &mut note_copies {
            if fragment_store_item_ids(&note.xml)?.contains(&store_item_id) {
                let edits = fragment_store_id_edits(
                    &note.xml,
                    &store_item_id,
                    &new_id,
                    b"dataBinding",
                    b"storeItemID",
                )?;
                apply_fragment_xml_edits(
                    &mut note.xml,
                    &mut note.source,
                    template
                        .as_mut()
                        .map(|(evaluation, _)| &mut evaluation.operation),
                    edits,
                )?;
            }
        }
        if let Some(comments) = fragment.comments.as_mut() {
            let comments_xml = comments.to_xml()?;
            if fragment_store_item_ids(&comments_xml)?.contains(&store_item_id) {
                *comments = CT_Comments::from_xml(&replace_fragment_store_id(
                    &comments_xml,
                    &store_item_id,
                    &new_id,
                    b"dataBinding",
                    b"storeItemID",
                )?)?;
            }
        }
        let props_xml = document
            .package
            .get_part(destination_props)
            .ok_or_else(|| {
                Error::Other(format!(
                    "document fragment custom XML properties {destination_props} are missing"
                ))
            })?;
        let patched_props = replace_fragment_store_id(
            props_xml,
            &store_item_id,
            &new_id,
            b"datastoreItem",
            b"itemID",
        )?;
        document.package.set_part(destination_props, patched_props);
    }
    if !relationship_map.is_empty() {
        let edits = relationship_value_edits(&fragment_xml, &relationship_map)?;
        apply_fragment_xml_edits(
            &mut fragment_xml,
            &mut fragment_xml_source,
            template
                .as_mut()
                .map(|(evaluation, _)| &mut evaluation.operation),
            edits,
        )?;
    }
    fragment.document = if let (Some((evaluation, _)), Some(source)) =
        (template.as_mut(), fragment_xml_source.as_ref())
    {
        crate::document::DocumentModel::unproved(
            fragment_sources
                .as_mut()
                .expect("fragment read state")
                .read_document(&mut evaluation.operation, Arc::clone(source))?,
        )
    } else {
        crate::document::DocumentModel::unproved(CT_Document::from_xml(&fragment_xml)?)
    };
    let mut identity_xml = fragment_xml.clone();
    for note in &note_copies {
        validate_fragment_identity_ownership(&body_identity_values(&wrap_fragment_companion(
            &note.xml,
        ))?)?;
        identity_xml.extend_from_slice(&note.xml);
    }
    if let Some(comments) = Document::fragment_comment_dependency_xml(
        fragment.comments.as_ref(),
        fragment.comments_extended.as_ref(),
        companion_comment_ids,
    )? {
        identity_xml.extend(comments);
    }
    let (_, mut identity_remap) = remap_fragment_xml_identities(
        &wrap_fragment_companion(&identity_xml),
        &mut document.identifiers,
        identity_state,
        false,
        keep_free_names,
    )?;
    let patched_comments = Document::fragment_comment_dependency_xml(
        fragment.comments.as_ref(),
        fragment.comments_extended.as_ref(),
        companion_comment_ids,
    )?
    .map(|xml| -> Result<CT_Comments> {
        let xml = patch_relationship_ids(&xml, &comment_relationship_map)?;
        let wrapped =
            patch_body_identity_attributes(&wrap_fragment_companion(&xml), &identity_remap)?;
        let xml = unwrap_fragment_companion(&wrapped);
        Ok(CT_Comments::from_xml(&freshen_fragment_marker_ids(
            document, &xml,
        )?)?)
    })
    .transpose()?;
    let comment_ids = document.import_fragment_comments_staged(
        patched_comments.as_ref().or(fragment.comments.as_ref()),
        fragment.comments_extended.as_ref(),
        companion_comment_ids,
    )?;
    if !comment_ids.is_empty() {
        let comment_remap = BodyIdentityRemap {
            comment_ids: comment_ids.clone(),
            ..Default::default()
        };
        let edits = body_identity_edits(&fragment_xml, &comment_remap)?;
        apply_fragment_xml_edits(
            &mut fragment_xml,
            &mut fragment_xml_source,
            template
                .as_mut()
                .map(|(evaluation, _)| &mut evaluation.operation),
            edits,
        )?;
        for note in &mut note_copies {
            remap_fragment_companion(
                &mut note.xml,
                &mut note.source,
                template
                    .as_mut()
                    .map(|(evaluation, _)| &mut evaluation.operation),
                &comment_remap,
            )?;
        }
        let (mut updated, mut updated_source) = if let Some((evaluation, _)) = template.as_mut() {
            let mut writer = crate::document::TemplateMainWriter {
                operation: &mut evaluation.operation,
                lineage: &evaluation.lineage,
                sources: fragment_sources
                    .as_mut()
                    .expect("fragment canonical writer"),
            };
            let source = Arc::new(writer.document_source(&fragment.document)?);
            let retained = writer.operation.repeat_source(&source)?;
            (writer.operation.finish_source(retained)?, Some(source))
        } else {
            (fragment.document.to_xml()?, None)
        };
        let edits = body_identity_edits(&updated, &comment_remap)?;
        apply_fragment_xml_edits(
            &mut updated,
            &mut updated_source,
            template
                .as_mut()
                .map(|(evaluation, _)| &mut evaluation.operation),
            edits,
        )?;
        fragment.document =
            if let (Some((evaluation, _)), Some(source)) = (template.as_mut(), updated_source) {
                crate::document::DocumentModel::unproved(
                    fragment_sources
                        .as_mut()
                        .expect("fragment canonical read")
                        .read_document(&mut evaluation.operation, source)?,
                )
            } else {
                crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?)
            };
    }
    identity_remap.drop_paragraph_identities = true;
    let edits = crate::document::unique_drawing_id_edits(&fragment_xml)?
        .into_iter()
        .map(|(start, end, replacement)| FieldSourceEdit {
            start,
            end,
            replacement,
        })
        .collect();
    apply_fragment_xml_edits(
        &mut fragment_xml,
        &mut fragment_xml_source,
        template
            .as_mut()
            .map(|(evaluation, _)| &mut evaluation.operation),
        edits,
    )?;
    let edits = body_identity_edits(&fragment_xml, &identity_remap)?;
    apply_fragment_xml_edits(
        &mut fragment_xml,
        &mut fragment_xml_source,
        template
            .as_mut()
            .map(|(evaluation, _)| &mut evaluation.operation),
        edits,
    )?;
    fragment.document = if let (Some((evaluation, _)), Some(source)) =
        (template.as_mut(), fragment_xml_source.as_ref())
    {
        crate::document::DocumentModel::unproved(
            fragment_sources
                .as_mut()
                .expect("fragment read state")
                .read_document(&mut evaluation.operation, Arc::clone(source))?,
        )
    } else {
        crate::document::DocumentModel::unproved(CT_Document::from_xml(&fragment_xml)?)
    };
    let insert_at = document.document.body.content.len();
    let numbering_remap = document.insert_document_fragment_content_staged(
        insert_at,
        &fragment,
        policy.reuse_numbering(),
        source_owned_numbering,
    )?;
    let typed = document.document.body.content.drain(insert_at..).collect();
    let numbering_values = numbering_remap
        .iter()
        .map(|(old, new)| (old.to_string(), new.to_string()))
        .collect::<BTreeMap<_, _>>();
    let edits = word_value_edits(&fragment_xml, &[b"numId"], &numbering_values)?;
    apply_fragment_xml_edits(
        &mut fragment_xml,
        &mut fragment_xml_source,
        template
            .as_mut()
            .map(|(evaluation, _)| &mut evaluation.operation),
        edits,
    )?;
    for source_part in &numbered_header_footers {
        let copied = part_map
            .get(source_part)
            .ok_or_else(|| Error::Other(format!("fragment part {source_part} was not reserved")))?;
        let mut xml = document
            .package
            .get_part(copied)
            .ok_or_else(|| Error::Other(format!("copied fragment part {copied} is missing")))?
            .to_vec();
        patch_word_values(&mut xml, &[b"numId"], &numbering_values)?;
        document.package.set_part(copied, xml);
    }
    if let Some(comments) = document.comments.as_mut() {
        for comment in &mut comments.comments {
            if !comment_ids.values().any(|id| *id == comment.id.to_string()) {
                continue;
            }
            let mut selected = CT_Comments::new();
            selected.comments.push(comment.clone());
            let mut xml = selected.to_xml()?;
            patch_word_values(&mut xml, &[b"numId"], &numbering_values)?;
            *comment = CT_Comments::from_xml(&xml)?.comments.remove(0);
        }
    }
    for mut note in note_copies {
        if let Some(map) = note_relationship_maps.get(&note.destination_part) {
            let edits = relationship_value_edits(&note.xml, map)?;
            apply_fragment_xml_edits(
                &mut note.xml,
                &mut note.source,
                template
                    .as_mut()
                    .map(|(evaluation, _)| &mut evaluation.operation),
                edits,
            )?;
        }
        let edits = word_value_edits(&note.xml, &[b"numId"], &numbering_values)?;
        apply_fragment_xml_edits(
            &mut note.xml,
            &mut note.source,
            template
                .as_mut()
                .map(|(evaluation, _)| &mut evaluation.operation),
            edits,
        )?;
        remap_fragment_companion(
            &mut note.xml,
            &mut note.source,
            template
                .as_mut()
                .map(|(evaluation, _)| &mut evaluation.operation),
            &identity_remap,
        )?;
        let edits = fragment_marker_id_edits(document, &note.xml)?;
        apply_fragment_xml_edits(
            &mut note.xml,
            &mut note.source,
            template
                .as_mut()
                .map(|(evaluation, _)| &mut evaluation.operation),
            edits,
        )?;
        if let (Some((evaluation, _)), Some(source)) = (template.as_mut(), note.source) {
            let mut writer = crate::document::TemplateMainWriter {
                operation: &mut evaluation.operation,
                lineage: &evaluation.lineage,
                sources: &mut evaluation.sources,
            };
            document.publish_fragment_note_staged_in(
                note.kind,
                &note.destination_part,
                note.xml,
                note.replace_id,
                Some((&mut writer, source)),
            )?;
        } else {
            document.publish_fragment_note_staged(
                note.kind,
                &note.destination_part,
                note.xml,
                note.replace_id,
            )?;
        }
    }
    let edits = fragment_marker_id_edits(document, &fragment_xml)?;
    apply_fragment_xml_edits(
        &mut fragment_xml,
        &mut fragment_xml_source,
        template
            .as_mut()
            .map(|(evaluation, _)| &mut evaluation.operation),
        edits,
    )?;
    Ok(ImportedFragmentContent {
        typed,
        xml: fragment_xml,
        source: fragment_xml_source,
    })
}

fn equivalent_fragment_leaf_part(
    source: &OpcPackage,
    destination: &Document,
    source_part: &str,
) -> Option<String> {
    if source
        .get_part_rels(source_part)
        .is_some_and(|relationships| !relationships.items.is_empty())
    {
        return None;
    }
    let source_bytes = source.get_part(source_part)?;
    let source_content_type = source.content_types.content_type_for(source_part)?;
    destination
        .package
        .parts
        .iter()
        .find_map(|(part_name, bytes)| {
            (bytes.as_slice() == source_bytes
                && destination
                    .package
                    .get_part_rels(part_name)
                    .is_none_or(|relationships| relationships.items.is_empty())
                && destination
                    .package
                    .content_types
                    .content_type_for(part_name)
                    == Some(source_content_type))
            .then(|| part_name.clone())
        })
}

fn prune_document_fragment_dependencies(
    fragment: &mut Document,
    selected: &[u8],
    include_companions: bool,
) -> Result<()> {
    // A repeated style ID resolves to its first definition before the
    // dependency closure. A later definition is neither a dependency nor
    // compared with the destination style.
    let mut defined = HashSet::new();
    std::sync::Arc::make_mut(&mut fragment.styles)
        .styles
        .retain(|style| defined.insert(style.style_id.clone()));
    let mut body_xml = selected.to_vec();
    for companion in if include_companions {
        fragment_dependency_companions(fragment, selected)?
    } else {
        Vec::new()
    } {
        body_xml.extend(companion);
    }
    let mut used_numbering = word_value_attributes(&body_xml, &[b"numId"])?
        .into_iter()
        .filter_map(|value| value.parse::<u32>().ok())
        .collect::<HashSet<_>>();
    let mut used_styles = word_value_attributes(&body_xml, &[b"pStyle", b"rStyle", b"tblStyle"])?;
    // The header and footer parts the section properties reach are copied
    // whole: the styles and lists they use come along like the body's.
    for part in fragment_header_footer_parts(fragment, selected)? {
        let xml = fragment
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other(format!("document fragment part {part} is missing")))?;
        used_numbering.extend(
            word_value_attributes(xml, &[b"numId"])?
                .into_iter()
                .filter_map(|value| value.parse::<u32>().ok()),
        );
        used_styles.extend(word_value_attributes(
            xml,
            &[b"pStyle", b"rStyle", b"tblStyle"],
        )?);
    }
    used_styles.extend(
        fragment
            .styles
            .styles
            .iter()
            .filter(|style| style.is_default)
            .map(|style| style.style_id.clone()),
    );
    loop {
        let before = (used_styles.len(), used_numbering.len());
        for style in &fragment.styles.styles {
            if !used_styles.contains(&style.style_id) {
                continue;
            }
            used_styles.extend(
                [
                    style.based_on.as_ref(),
                    style.next_style.as_ref(),
                    style.linked_style.as_ref(),
                ]
                .into_iter()
                .flatten()
                .cloned(),
            );
        }
        let mut retained_styles = oxoffice_docx_oxml::styles::CT_Styles::clone(&fragment.styles);
        retained_styles
            .styles
            .retain(|style| used_styles.contains(&style.style_id));
        used_numbering.extend(
            word_value_attributes(&retained_styles.to_xml()?, &[b"numId"])?
                .into_iter()
                .filter_map(|value| value.parse::<u32>().ok()),
        );
        if let Some(numbering) = &fragment.numbering {
            let mut retained_numbering = numbering.clone();
            retained_numbering
                .nums
                .retain(|instance| used_numbering.contains(&instance.num_id));
            let used_abstract = retained_numbering
                .nums
                .iter()
                .map(|instance| instance.abstract_num_id)
                .collect::<HashSet<_>>();
            retained_numbering
                .abstract_nums
                .retain(|definition| used_abstract.contains(&definition.abstract_num_id));
            used_styles.extend(word_value_attributes(
                &retained_numbering.to_xml()?,
                &[b"pStyle", b"rStyle", b"styleLink", b"numStyleLink"],
            )?);
        }
        if (used_styles.len(), used_numbering.len()) == before {
            break;
        }
    }
    std::sync::Arc::make_mut(&mut fragment.styles)
        .styles
        .retain(|style| used_styles.contains(&style.style_id));
    if let Some(numbering) = fragment.numbering.as_mut() {
        numbering
            .nums
            .retain(|instance| used_numbering.contains(&instance.num_id));
        let used_abstract = numbering
            .nums
            .iter()
            .map(|instance| instance.abstract_num_id)
            .collect::<HashSet<_>>();
        numbering
            .abstract_nums
            .retain(|definition| used_abstract.contains(&definition.abstract_num_id));
        if numbering.nums.is_empty() {
            fragment.numbering = None;
        }
    }
    Ok(())
}

fn word_value_attributes(xml: &[u8], elements: &[&[u8]]) -> Result<HashSet<String>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut values = HashSet::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("invalid document fragment dependency XML: {error}"))
        })? {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace)
                    && elements.contains(&local.as_ref())
                    && let Some((_, value)) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"val",
                        AttributeNamespace::Word,
                    )?
                {
                    values.insert(value);
                }
            }
            Event::Eof => return Ok(values),
            _ => {}
        }
        buffer.clear();
    }
}

// The final direct owned numId is modeled. Earlier duplicates and Word-looking
// descendants below opaque owners remain exact. Only the writer's redundant
// canonical Word closure on the selected pPr, numPr and numId is comparison
// bookkeeping.
fn fragment_style_numbering_projection(
    style: &oxoffice_docx_oxml::styles::CT_Style,
    mapped_num_id: Option<u32>,
) -> Result<Vec<u8>> {
    let mut writer = quick_xml::Writer::new(Vec::new());
    style.to_xml(&mut writer)?;
    let xml = writer.into_inner();
    let prefix =
        b"<w:fragment xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">";
    let mut wrapped = prefix.to_vec();
    wrapped.extend_from_slice(&xml);
    wrapped.extend_from_slice(b"</w:fragment>");
    let mut reader = NsReader::from_reader(wrapped.as_slice());
    let mut buffer = Vec::new();
    struct ProjectionFrame {
        owned: bool,
        word_prefix: bool,
        closure: Option<(usize, usize)>,
    }
    let mut path: Vec<ProjectionFrame> = Vec::new();
    let expected: &[&[u8]] = &[b"fragment", b"style", b"pPr", b"numPr", b"numId"];
    let mut selected = None;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("invalid fragment style numbering XML: {error}"))
        })?;
        let is_start = matches!(&event, Event::Start(_));
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(node) | Event::Empty(node) => {
                let (namespace, local) = reader.resolver().resolve_element(node.name());
                let owned = path.iter().all(|entry| entry.owned)
                    && expected
                        .get(path.len())
                        .is_some_and(|expected| *expected == local.as_ref())
                    && namespace_is_word(&namespace);
                let (w_namespace, _) = reader
                    .resolver()
                    .resolve_element(quick_xml::name::QName(b"w:reference"));
                let w_is_word = namespace_is_word(&w_namespace);
                let mut closure = None;
                if owned && matches!(path.len(), 2..=4)
                    && path.last().is_some_and(|parent| parent.word_prefix)
                    && let Some((name_start, _value_start, value_end)) = attribute_source_span(&wrapped[before..after], b"xmlns:w")
                    && &wrapped[before + name_start..before + value_end + 1]
                        == b"xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\""
                {
                    let mut start = before + name_start;
                    while start > before && wrapped[start - 1].is_ascii_whitespace() { start -= 1; }
                    closure = Some((start - prefix.len(), before + value_end + 1 - prefix.len()));
                }
                if owned && path.len() == 4 {
                    let (key, value) = resolved_element_attribute(
                        &node,
                        reader.resolver(),
                        b"val",
                        AttributeNamespace::Word,
                    )?
                    .ok_or_else(|| {
                        Error::Other("fragment style numbering value is missing".into())
                    })?;
                    let (start, end) = attribute_value_span(&wrapped[before..after], &key)
                        .ok_or_else(|| {
                            Error::Other(
                                "fragment style numbering attribute source was not found".into(),
                            )
                        })?;
                    let mut closures = path
                        .iter()
                        .filter_map(|frame| frame.closure)
                        .collect::<Vec<_>>();
                    if let Some(closure) = closure {
                        closures.push(closure);
                    }
                    selected = Some((
                        value,
                        before + start - prefix.len(),
                        before + end - prefix.len(),
                        closures,
                    ));
                }
                if is_start {
                    path.push(ProjectionFrame {
                        owned,
                        word_prefix: w_is_word,
                        closure,
                    });
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut edits = Vec::new();
    if let Some((value, start, end, closures)) = selected {
        if Some(
            value
                .parse::<u32>()
                .map_err(|_| Error::Other("invalid fragment style numbering ID".into()))?,
        ) != style.ppr.as_ref().and_then(|properties| properties.num_id)
        {
            return Err(Error::Other(
                "fragment style numbering projection disagrees with its owned reference".into(),
            ));
        }
        if let Some(id) = mapped_num_id {
            edits.push((start, end, id.to_string().into_bytes()));
        }
        for (start, end) in closures {
            edits.push((start, end, Vec::new()));
        }
    } else if mapped_num_id.is_some() {
        return Err(Error::Other(
            "fragment style has no owned numbering reference".into(),
        ));
    }
    let mut xml = xml;
    edits.sort_by_key(|edit| edit.0);
    for (start, end, replacement) in edits.into_iter().rev() {
        xml.splice(start..end, replacement);
    }
    Ok(xml)
}

fn remap_fragment_style_collisions(
    destination: &Document,
    fragment: &mut Document,
    fragment_xml: &mut Vec<u8>,
    reuse_equivalent: bool,
    keep_destination_styles: bool,
    reuse_equivalent_numbering: bool,
    source_owned_numbering: bool,
    mut source: Option<(
        &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
        &mut Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
        &mut crate::document::TemplateMainSources,
    )>,
) -> Result<bool> {
    let mut replacements = BTreeMap::new();
    let mut kept = HashSet::new();
    let mut used = destination
        .styles
        .styles
        .iter()
        .chain(fragment.styles.styles.iter())
        .map(|style| style.style_id.clone())
        .collect::<HashSet<_>>();
    // A default of its type under an id the destination lacks, such as
    // Word's localized Policepardfaut or a German Standard, takes the
    // destination's default when it keeps the destination's look or is that
    // default under another id, as a type has one default. A different one
    // keeps its look under its own id, no longer a default, as a different
    // default under the destination's id is renamed: the result does not
    // depend on the language of the Word that wrote either file.
    let mut demoted = HashSet::new();
    for style in &fragment.styles.styles {
        if !style.is_default || destination.styles.get_by_id(&style.style_id).is_some() {
            continue;
        }
        let Some(default) = destination.styles.get_default(style.style_type) else {
            continue;
        };
        let mut under_destination_id = style.clone();
        under_destination_id.style_id = default.style_id.clone();
        if keep_destination_styles || *default == under_destination_id {
            replacements.insert(style.style_id.clone(), default.style_id.clone());
        } else {
            demoted.insert(style.style_id.clone());
        }
    }
    // Styles are compared once their references to a mapped default name
    // the destination's: a style based on Policepardfaut equals one based on
    // DefaultParagraphFont.
    let defaults = replacements.clone();
    let with_mapped_defaults = |style: &oxoffice_docx_oxml::styles::CT_Style| {
        let mut style = style.clone();
        for reference in [
            &mut style.based_on,
            &mut style.next_style,
            &mut style.linked_style,
        ] {
            if let Some(target) = reference.as_ref().and_then(|id| defaults.get(id)) {
                *reference = Some(target.clone());
            }
        }
        style
    };
    for style in &fragment.styles.styles {
        let Some(existing) = destination.styles.get_by_id(&style.style_id) else {
            continue;
        };
        // The destination's definition wins: the content keeps the id. A
        // style of another type under the id is renamed as usual.
        if keep_destination_styles && existing.style_type == style.style_type {
            kept.insert(style.style_id.clone());
            continue;
        }
        let source_num_id = style.ppr.as_ref().and_then(|properties| properties.num_id);
        let reusable_num_id = source_num_id
            .filter(|_| reuse_equivalent_numbering)
            .and_then(|num_id| {
                if source_owned_numbering {
                    Some(num_id)
                } else {
                    destination.equivalent_numbering_dependency_id(fragment, num_id)
                }
            });
        let numbering_is_reusable = source_num_id.is_none() || reusable_num_id.is_some();
        // Compare the source-owned projection with only the selected numbering
        // reference remapped. Preservation snapshots and generated namespace
        // closures are not new style facts, while opaque XML remains exact.
        let compared = with_mapped_defaults(style);
        let styles_match = if *existing == compared {
            true
        } else if let Some(num_id) = reusable_num_id {
            fragment_style_numbering_projection(existing, None)?
                == fragment_style_numbering_projection(&compared, Some(num_id))?
        } else {
            false
        };
        if styles_match && (reuse_equivalent || style.is_default) && numbering_is_reusable {
            continue;
        }
        let mut ordinal = 1u64;
        loop {
            let candidate = format!("{}Merge{ordinal}", style.style_id);
            if used.insert(candidate.clone()) {
                replacements.insert(style.style_id.clone(), candidate);
                break;
            }
            ordinal = ordinal.checked_add(1).ok_or_else(|| {
                Error::Other("rich mail merge fragment style name space is exhausted".to_owned())
            })?;
        }
    }
    // A renamed style linked to one that is reused would point at the
    // destination's style, which links elsewhere: rename the pair together.
    loop {
        let mut partners = Vec::new();
        for style in &fragment.styles.styles {
            // A default mapped to the destination's own takes its links too.
            let Some(partner) = style.linked_style.as_ref().filter(|_| {
                replacements
                    .get(&style.style_id)
                    .is_some_and(|target| destination.styles.get_by_id(target).is_none())
            }) else {
                continue;
            };
            if !replacements.contains_key(partner)
                && !kept.contains(partner)
                && destination.styles.get_by_id(partner).is_some()
                && !partners.contains(partner)
            {
                partners.push(partner.clone());
            }
        }
        if partners.is_empty() {
            break;
        }
        for partner in partners {
            let mut ordinal = 1u64;
            loop {
                let candidate = format!("{partner}Merge{ordinal}");
                if used.insert(candidate.clone()) {
                    replacements.insert(partner.clone(), candidate);
                    break;
                }
                ordinal = ordinal.checked_add(1).ok_or_else(|| {
                    Error::Other("fragment style name space is exhausted".to_owned())
                })?;
            }
        }
    }
    if !kept.is_empty() {
        // An imported style linked to one the destination keeps would not be
        // linked back, so it comes in unlinked.
        for style in &mut std::sync::Arc::make_mut(&mut fragment.styles).styles {
            if kept.contains(&style.style_id) {
                continue;
            }
            let unreciprocated = style.linked_style.as_ref().is_some_and(|partner| {
                kept.contains(partner)
                    && destination
                        .styles
                        .get_by_id(partner)
                        .and_then(|existing| existing.linked_style.as_deref())
                        != Some(style.style_id.as_str())
            });
            if unreciprocated {
                style.linked_style = None;
            }
        }
    }
    if replacements.is_empty() && demoted.is_empty() {
        return Ok(false);
    }
    for style in &mut std::sync::Arc::make_mut(&mut fragment.styles).styles {
        if demoted.contains(&style.style_id) {
            style.is_default = false;
        }
        if let Some(replacement) = replacements.get(&style.style_id) {
            style.style_id = replacement.clone();
            // A renamed copy is not the destination default: the destination
            // keeps its own, and a source default under another id maps to it.
            style.is_default = false;
        }
        if let Some(replacement) = style
            .based_on
            .as_ref()
            .and_then(|style_id| replacements.get(style_id))
        {
            style.based_on = Some(replacement.clone());
        }
        if let Some(replacement) = style
            .next_style
            .as_ref()
            .and_then(|style_id| replacements.get(style_id))
        {
            style.next_style = Some(replacement.clone());
        }
        if let Some(replacement) = style
            .linked_style
            .as_ref()
            .and_then(|style_id| replacements.get(style_id))
        {
            style.linked_style = Some(replacement.clone());
        }
        for (_, raw) in &mut style.extra_xml {
            patch_word_values(raw, &[b"link"], &replacements)?;
        }
    }
    if let Some(numbering) = &mut fragment.numbering {
        let mut xml = numbering.to_xml()?;
        patch_word_values(
            &mut xml,
            &[b"styleLink", b"numStyleLink", b"pStyle"],
            &replacements,
        )?;
        *numbering = oxoffice_docx_oxml::numbering::CT_Numbering::from_xml(&xml)?;
    }
    // One pass with the whole map: a replacement can also be a key, as when a
    // source default maps to the destination's Standard and the source's own
    // Standard is renamed StandardMerge1.
    const STYLE_REFERENCES: [&[u8]; 3] = [b"pStyle", b"rStyle", b"tblStyle"];
    if let Some((operation, source, _)) = source.as_mut() {
        let edits = word_value_edits(fragment_xml, &STYLE_REFERENCES, &replacements)?;
        apply_fragment_xml_edits(fragment_xml, source, Some(operation), edits)?;
    } else {
        patch_word_values(fragment_xml, &STYLE_REFERENCES, &replacements)?;
    }
    for kind in [StoryKind::Footnote, StoryKind::Endnote] {
        let stories = fragment.stories()?;
        if let Some(story) = stories.iter().find(|story| story.kind() == kind) {
            let part = story.part_name().to_owned();
            let storage = fragment
                .package
                .get_part_shared(&part)
                .ok_or_else(|| Error::Other(format!("fragment note part {part} is missing")))?;
            let mut note_source = source
                .as_ref()
                .and_then(|(_, _, sources)| sources.related_source(&part, storage))
                .cloned();
            let mut xml = if let (Some((operation, _, _)), Some(parent)) =
                (source.as_mut(), note_source.as_ref())
            {
                let retained = operation.repeat_source(parent)?;
                operation.finish_source(retained)?
            } else {
                storage.as_ref().clone()
            };
            let edits = word_value_edits(&xml, &STYLE_REFERENCES, &replacements)?;
            let changed = !edits.is_empty();
            apply_fragment_xml_edits(
                &mut xml,
                &mut note_source,
                source.as_mut().map(|(operation, _, _)| &mut **operation),
                edits,
            )?;
            if changed {
                fragment.package.set_part(&part, xml);
                if let (Some((_, _, sources)), Some(parent)) = (source.as_mut(), note_source) {
                    sources.put_related_source(
                        &part,
                        fragment
                            .package
                            .get_part_shared(&part)
                            .expect("actual style-remapped note part"),
                        parent,
                    );
                }
            }
        }
    }
    if let Some(comments) = fragment.comments.as_mut() {
        let mut xml = comments.to_xml()?;
        patch_word_values(&mut xml, &STYLE_REFERENCES, &replacements)?;
        *comments = CT_Comments::from_xml(&xml)?;
    }
    // Header and footer parts are copied whole: they take the renames here,
    // in one pass, since a replacement can also be a key.
    let header_footer_parts = fragment
        .package
        .get_part_rels(&fragment.doc_part_name)
        .map(|relationships| {
            relationships
                .items
                .iter()
                .filter(|relationship| {
                    (relationship.rel_type == rel_types::HEADER
                        || relationship.rel_type == rel_types::FOOTER)
                        && crate::document::relationship_is_internal(relationship)
                })
                .map(|relationship| {
                    OpcPackage::resolve_rel_target(&fragment.doc_part_name, &relationship.target)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for part in header_footer_parts {
        let Some(original) = fragment.package.get_part(&part) else {
            continue;
        };
        let mut xml = original.to_vec();
        patch_word_values(
            &mut xml,
            &[b"pStyle", b"rStyle", b"tblStyle"],
            &replacements,
        )?;
        if xml != original {
            fragment.package.set_part(&part, xml);
        }
    }
    let styles_part = fragment
        .package
        .get_part_rels(&fragment.doc_part_name)
        .and_then(|relationships| {
            relationships.items.iter().find(|relationship| {
                relationship.rel_type == rel_types::STYLES
                    && crate::document::relationship_is_internal(relationship)
            })
        })
        .map(|relationship| {
            OpcPackage::resolve_rel_target(&fragment.doc_part_name, &relationship.target)
        })
        .ok_or_else(|| Error::Other("rich mail merge fragment has no styles part".to_owned()))?;
    fragment
        .package
        .set_part(&styles_part, fragment.styles.to_xml()?);
    if let Some(numbering) = &fragment.numbering {
        let numbering_part = fragment
            .package
            .get_part_rels(&fragment.doc_part_name)
            .and_then(|relationships| {
                relationships.items.iter().find(|relationship| {
                    relationship.rel_type == rel_types::NUMBERING
                        && crate::document::relationship_is_internal(relationship)
                })
            })
            .map(|relationship| {
                OpcPackage::resolve_rel_target(&fragment.doc_part_name, &relationship.target)
            })
            .ok_or_else(|| {
                Error::Other("rich mail merge fragment has no numbering part".to_owned())
            })?;
        fragment
            .package
            .set_part(&numbering_part, numbering.to_xml()?);
    }
    Ok(true)
}

fn patch_word_values(
    xml: &mut Vec<u8>,
    elements: &[&[u8]],
    replacements: &BTreeMap<String, String>,
) -> Result<()> {
    for edit in word_value_edits(xml, elements, replacements)?
        .into_iter()
        .rev()
    {
        xml.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(())
}

fn word_value_edits(
    xml: &[u8],
    elements: &[&[u8]],
    replacements: &BTreeMap<String, String>,
) -> Result<Vec<FieldSourceEdit>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut edits = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "invalid rich mail merge style reference XML: {error}"
            ))
        })?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(node) | Event::Empty(node) => {
                let (namespace, local) = reader.resolver().resolve_element(node.name());
                if namespace_is_word(&namespace)
                    && elements.contains(&local.as_ref())
                    && let Some((key, value)) = resolved_element_attribute(
                        &node,
                        reader.resolver(),
                        b"val",
                        AttributeNamespace::Word,
                    )?
                    && let Some(new) = replacements.get(&value)
                {
                    let Some((relative_start, relative_end)) =
                        attribute_value_span(&xml[before..after], &key)
                    else {
                        return Err(Error::Other(
                            "rich mail merge style reference attribute source was not found"
                                .to_owned(),
                        ));
                    };
                    edits.push(FieldSourceEdit {
                        start: before + relative_start,
                        end: before + relative_end,
                        replacement: new.as_bytes().to_vec(),
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    edits.sort_by_key(|edit| edit.start);
    Ok(edits)
}

const FRAGMENT_COMPANION_OPEN: &[u8] = b"<f276:document xmlns:f276=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><f276:body>";
const FRAGMENT_COMPANION_CLOSE: &[u8] = b"</f276:body></f276:document>";

// Companion roots are namespace-complete, so the existing body identity scanner can
// inspect them without changing their own namespace bindings.
fn wrap_fragment_companion(xml: &[u8]) -> Vec<u8> {
    let mut wrapped = FRAGMENT_COMPANION_OPEN.to_vec();
    wrapped.extend_from_slice(xml);
    wrapped.extend_from_slice(FRAGMENT_COMPANION_CLOSE);
    wrapped
}

fn unwrap_fragment_companion(xml: &[u8]) -> Vec<u8> {
    xml[FRAGMENT_COMPANION_OPEN.len()..xml.len() - FRAGMENT_COMPANION_CLOSE.len()].to_vec()
}

fn remap_fragment_companion(
    xml: &mut Vec<u8>,
    source: &mut Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    operation: Option<&mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>>,
    remap: &BodyIdentityRemap,
) -> Result<()> {
    if let (Some(parent), Some(operation)) = (source.as_ref(), operation) {
        let wrapped = operation.compose_sources(&[
            (None, 0..0, FRAGMENT_COMPANION_OPEN),
            (Some(parent.as_ref()), 0..parent.as_bytes().len(), &[]),
            (None, 0..0, FRAGMENT_COMPANION_CLOSE),
        ])?;
        let edits = body_identity_edits(wrapped.as_bytes(), remap)?;
        if edits.is_empty() {
            return Ok(());
        }
        let updated = crate::document::apply_template_source_edits(
            operation,
            wrapped,
            edits
                .into_iter()
                .map(|edit| (edit.start, edit.end, edit.replacement))
                .collect(),
        )?;
        let selected = operation.compose_sources(&[(
            Some(&updated),
            FRAGMENT_COMPANION_OPEN.len()
                ..updated.as_bytes().len() - FRAGMENT_COMPANION_CLOSE.len(),
            &[],
        )])?;
        let retained = operation.repeat_source(&selected)?;
        *xml = operation.finish_source(retained)?;
        *source = Some(Arc::new(selected));
    } else {
        *xml = unwrap_fragment_companion(&patch_body_identity_attributes(
            &wrap_fragment_companion(xml),
            remap,
        )?);
    }
    Ok(())
}

/// The header and footer parts the section properties in `selected` reach.
fn fragment_header_footer_parts(fragment: &Document, selected: &[u8]) -> Result<Vec<String>> {
    let Some(relationships) = fragment.package.get_part_rels(&fragment.doc_part_name) else {
        return Ok(Vec::new());
    };
    let mut parts = Vec::new();
    for id in relationship_ids_in_xml(selected)? {
        if let Some(relationship) = relationships.get_by_id(&id)
            && (relationship.rel_type == rel_types::HEADER
                || relationship.rel_type == rel_types::FOOTER)
            && crate::document::relationship_is_internal(relationship)
        {
            let part =
                OpcPackage::resolve_rel_target(&fragment.doc_part_name, &relationship.target);
            if !parts.contains(&part) {
                parts.push(part);
            }
        }
    }
    Ok(parts)
}

fn fragment_dependency_companions(fragment: &Document, selected: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut pending = vec![selected.to_vec()];
    let mut companions = Vec::new();
    let mut seen_notes = HashSet::new();
    let mut seen_comments = HashSet::new();
    let mut index = 0;
    while index < pending.len() {
        let xml = pending[index].clone();
        index += 1;
        for (kind, id) in fragment_note_references(&xml)? {
            if seen_notes.insert((kind, id)) {
                let (_, note) = fragment.fragment_note_dependency(kind, id)?;
                pending.push(note.clone());
                companions.push(note);
            }
        }
        let ids = body_identity_values(&wrap_fragment_companion(&xml))?
            .comment_ids
            .into_iter()
            .filter(|id| seen_comments.insert(id.clone()))
            .collect::<Vec<_>>();
        if let Some(comments) = Document::fragment_comment_dependency_xml(
            fragment.comments.as_ref(),
            fragment.comments_extended.as_ref(),
            &ids,
        )? {
            pending.push(comments.clone());
            companions.push(comments);
        }
    }
    Ok(companions)
}

pub(super) fn patch_fragment_note_ids(
    xml: &[u8],
    maps: &HashMap<StoryKind, BTreeMap<String, String>>,
) -> Result<Vec<u8>> {
    let edits = fragment_note_id_edits(xml, maps)?;
    let mut updated = xml.to_vec();
    for edit in edits.into_iter().rev() {
        updated.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(updated)
}

fn fragment_note_id_edits(
    xml: &[u8],
    maps: &HashMap<StoryKind, BTreeMap<String, String>>,
) -> Result<Vec<FieldSourceEdit>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut edits = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid fragment note XML: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace) {
                    let kind = match local.as_ref() {
                        b"footnote" | b"footnoteReference" => Some(StoryKind::Footnote),
                        b"endnote" | b"endnoteReference" => Some(StoryKind::Endnote),
                        _ => None,
                    };
                    if let Some(map) = kind.and_then(|kind| maps.get(&kind)) {
                        add_identity_attribute_edit(
                            xml,
                            before,
                            after,
                            &element,
                            reader.resolver(),
                            b"id",
                            AttributeNamespace::Word,
                            map,
                            &mut edits,
                        )?;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(edits)
}

fn fragment_marker_family(local: &[u8]) -> Option<(&'static str, u8)> {
    match local {
        b"permStart" => Some(("permission", 0)),
        b"permEnd" => Some(("permission", 1)),
        b"moveFromRangeStart" => Some(("move-from", 0)),
        b"moveFromRangeEnd" => Some(("move-from", 1)),
        b"moveToRangeStart" => Some(("move-to", 0)),
        b"moveToRangeEnd" => Some(("move-to", 1)),
        b"customXmlInsRangeStart" => Some(("custom-ins", 0)),
        b"customXmlInsRangeEnd" => Some(("custom-ins", 1)),
        b"customXmlDelRangeStart" => Some(("custom-del", 0)),
        b"customXmlDelRangeEnd" => Some(("custom-del", 1)),
        b"customXmlMoveFromRangeStart" => Some(("custom-move-from", 0)),
        b"customXmlMoveFromRangeEnd" => Some(("custom-move-from", 1)),
        b"customXmlMoveToRangeStart" => Some(("custom-move-to", 0)),
        b"customXmlMoveToRangeEnd" => Some(("custom-move-to", 1)),
        b"ins" | b"del" | b"moveFrom" | b"moveTo" | b"pPrChange" | b"rPrChange"
        | b"tblPrChange" | b"trPrChange" | b"tcPrChange" | b"sectPrChange" | b"tblGridChange"
        | b"numberingChange" => Some(("revision", 2)),
        _ => None,
    }
}

fn freshen_fragment_marker_ids(document: &Document, xml: &[u8]) -> Result<Vec<u8>> {
    let edits = fragment_marker_id_edits(document, xml)?;
    let mut updated = xml.to_vec();
    for edit in edits.into_iter().rev() {
        updated.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(updated)
}

fn fragment_marker_id_edits(document: &Document, xml: &[u8]) -> Result<Vec<FieldSourceEdit>> {
    let mut occupied = HashSet::new();
    // All Word IDs form a conservative occupied set. This also avoids aliases
    // between the separately scoped annotation families.
    for bytes in document.package.parts.values() {
        let mut reader = NsReader::from_reader(bytes.as_slice());
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(element) | Event::Empty(element)) => {
                    if let Ok(Some((_, value))) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                    ) && let Ok(id) = value.parse::<i32>()
                    {
                        occupied.insert(id);
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut maps = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut endpoints = BTreeMap::<(String, String), [usize; 2]>::new();
    let mut pending = Vec::new();
    let mut next = 0_i32;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid fragment annotation XML: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace)
                    && let Some((family, endpoint)) = fragment_marker_family(local.as_ref())
                {
                    let (_, old) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                    )?
                    .ok_or_else(|| {
                        Error::Other(format!("document fragment {family} annotation has no ID"))
                    })?;
                    if endpoint < 2 {
                        endpoints
                            .entry((family.to_owned(), old.clone()))
                            .or_default()[usize::from(endpoint)] += 1;
                    }
                    let map = maps.entry(family.to_owned()).or_default();
                    if let std::collections::btree_map::Entry::Vacant(entry) = map.entry(old) {
                        while occupied.contains(&next) {
                            next = next.checked_add(1).ok_or_else(|| {
                                Error::Other(
                                    "document fragment annotation ID range is exhausted".to_owned(),
                                )
                            })?;
                        }
                        occupied.insert(next);
                        entry.insert(next.to_string());
                    }
                    let mut edits = Vec::new();
                    add_identity_attribute_edit(
                        xml,
                        before,
                        after,
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                        map,
                        &mut edits,
                    )?;
                    pending.extend(edits);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if let Some(((family, id), _)) = endpoints.iter().find(|(_, counts)| **counts != [1, 1]) {
        return Err(Error::Other(format!(
            "document fragment {family} range {id} has incomplete or ambiguous ownership"
        )));
    }
    Ok(pending)
}

fn fragment_store_item_ids(xml: &[u8]) -> Result<BTreeSet<String>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut ids = BTreeSet::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("invalid document fragment binding XML: {error}"))
        })? {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace)
                    && local.as_ref() == b"dataBinding"
                    && let Some((_, value)) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"storeItemID",
                        AttributeNamespace::Word,
                    )?
                {
                    ids.insert(crate::content_control::normalize_store_item_id(&value));
                }
            }
            Event::Eof => return Ok(ids),
            _ => {}
        }
        buffer.clear();
    }
}

pub(crate) fn fragment_note_references(xml: &[u8]) -> Result<Vec<(StoryKind, i32)>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut references = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid document fragment note XML: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let kind = if namespace_is_word(&namespace) {
                    match local.as_ref() {
                        b"footnoteReference" => Some(StoryKind::Footnote),
                        b"endnoteReference" => Some(StoryKind::Endnote),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(kind) = kind {
                    let (_, value) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                    )?
                    .ok_or_else(|| {
                        Error::Other(format!("document fragment {kind:?} reference has no ID"))
                    })?;
                    let id = value.parse::<i32>().map_err(|_| {
                        Error::Other(format!(
                            "document fragment {kind:?} reference ID {value} is invalid"
                        ))
                    })?;
                    if !references.contains(&(kind, id)) {
                        references.push((kind, id));
                    }
                }
            }
            Event::Eof => return Ok(references),
            _ => {}
        }
        buffer.clear();
    }
}

fn replace_fragment_store_id(
    xml: &[u8],
    old: &str,
    new: &str,
    element_name: &[u8],
    attribute_name: &[u8],
) -> Result<Vec<u8>> {
    let edits = fragment_store_id_edits(xml, old, new, element_name, attribute_name)?;
    let mut result = xml.to_vec();
    for edit in edits.into_iter().rev() {
        result.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(result)
}

fn fragment_store_id_edits(
    xml: &[u8],
    old: &str,
    new: &str,
    element_name: &[u8],
    attribute_name: &[u8],
) -> Result<Vec<FieldSourceEdit>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut edits = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "invalid document fragment custom XML store: {error}"
            ))
        })?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let expected_namespace = if element_name == b"datastoreItem" {
                    namespace_matches(
                        &namespace,
                        "http://schemas.openxmlformats.org/officeDocument/2006/customXml",
                    )
                } else {
                    namespace_is_word(&namespace)
                };
                if !expected_namespace || local.as_ref() != element_name {
                    continue;
                }
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!(
                            "invalid document fragment custom XML store attribute: {error}"
                        ))
                    })?;
                    let (attribute_namespace, local) =
                        reader.resolver().resolve_attribute(attribute.key);
                    let namespace_matches_expected = if element_name == b"datastoreItem" {
                        namespace_matches(
                            &attribute_namespace,
                            "http://schemas.openxmlformats.org/officeDocument/2006/customXml",
                        )
                    } else {
                        namespace_is_word(&attribute_namespace)
                    };
                    if !namespace_matches_expected || local.as_ref() != attribute_name {
                        continue;
                    }
                    let value = quick_xml::escape::unescape(
                        std::str::from_utf8(&attribute.value).map_err(|error| {
                            Error::Other(format!(
                                "invalid document fragment custom XML store value: {error}"
                            ))
                        })?,
                    )
                    .map_err(|error| {
                        Error::Other(format!(
                            "invalid document fragment custom XML store value: {error}"
                        ))
                    })?;
                    if crate::content_control::normalize_store_item_id(&value)
                        != crate::content_control::normalize_store_item_id(old)
                    {
                        continue;
                    }
                    let (start, end) =
                        attribute_value_span(&xml[before..after], attribute.key.as_ref())
                            .ok_or_else(|| {
                                Error::Other(
                                    "document fragment custom XML store attribute span is missing"
                                        .to_owned(),
                                )
                            })?;
                    edits.push((before + start, before + end));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if edits.is_empty() {
        return Err(Error::Other(format!(
            "document fragment custom XML store {old} cannot be rewritten without corrupting its XML"
        )));
    }
    Ok(edits
        .into_iter()
        .map(|(start, end)| FieldSourceEdit {
            start,
            end,
            replacement: new.as_bytes().to_vec(),
        })
        .collect())
}

pub(crate) fn relationship_ids_in_xml(xml: &[u8]) -> Result<Vec<String>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut ids = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("invalid rich mail merge relationship XML: {error}"))
        })?;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!(
                            "invalid rich mail merge relationship attribute: {error}"
                        ))
                    })?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    if namespace_matches(&namespace, R_NS)
                        && matches!(local.as_ref(), b"id" | b"embed" | b"link")
                    {
                        let raw = std::str::from_utf8(&attribute.value).map_err(|error| {
                            Error::Other(format!(
                                "invalid rich mail merge relationship id: {error}"
                            ))
                        })?;
                        let id = quick_xml::escape::unescape(raw)
                            .map_err(|error| {
                                Error::Other(format!(
                                    "invalid rich mail merge relationship id: {error}"
                                ))
                            })?
                            .into_owned();
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                }
            }
            Event::Eof => return Ok(ids),
            _ => {}
        }
        buffer.clear();
    }
}

pub(crate) fn patch_relationship_ids(
    xml: &[u8],
    replacements: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let edits = relationship_value_edits(xml, replacements)?;
    let mut updated = xml.to_vec();
    for edit in edits.into_iter().rev() {
        updated.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(updated)
}

pub(crate) fn patch_relationship_source(
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    source: oxoffice_docx_oxml::document::ScopedWordSource,
    replacements: &BTreeMap<String, String>,
) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
    let edits = relationship_value_edits(source.as_bytes(), replacements)?;
    crate::document::apply_template_source_edits(
        operation,
        source,
        edits
            .into_iter()
            .map(|edit| (edit.start, edit.end, edit.replacement))
            .collect(),
    )
}

fn relationship_value_edits(
    xml: &[u8],
    replacements: &BTreeMap<String, String>,
) -> Result<Vec<FieldSourceEdit>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut edits = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("invalid rich mail merge relationship XML: {error}"))
        })?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!(
                            "invalid rich mail merge relationship attribute: {error}"
                        ))
                    })?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    if !namespace_matches(&namespace, R_NS)
                        || !matches!(local.as_ref(), b"id" | b"embed" | b"link")
                    {
                        continue;
                    }
                    let raw = std::str::from_utf8(&attribute.value).map_err(|error| {
                        Error::Other(format!("invalid rich mail merge relationship id: {error}"))
                    })?;
                    let old = quick_xml::escape::unescape(raw).map_err(|error| {
                        Error::Other(format!("invalid rich mail merge relationship id: {error}"))
                    })?;
                    let Some(replacement) = replacements.get(old.as_ref()) else {
                        continue;
                    };
                    let Some((relative_start, relative_end)) =
                        attribute_value_span(&xml[before..after], attribute.key.as_ref())
                    else {
                        return Err(Error::Other(
                            "rich mail merge relationship attribute source was not found"
                                .to_owned(),
                        ));
                    };
                    edits.push(FieldSourceEdit {
                        start: before + relative_start,
                        end: before + relative_end,
                        replacement: replacement.as_bytes().to_vec(),
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    edits.sort_by_key(|edit| edit.start);
    Ok(edits)
}

pub(crate) fn copy_fragment_part(
    source: &OpcPackage,
    destination: &mut Document,
    source_part: &str,
    part_map: &BTreeMap<String, String>,
    copied: &mut HashSet<String>,
) -> Result<String> {
    let destination_part = part_map
        .get(source_part)
        .or_else(|| {
            part_map.iter().find_map(|(source, destination)| {
                source
                    .eq_ignore_ascii_case(source_part)
                    .then_some(destination)
            })
        })
        .cloned()
        .ok_or_else(|| {
            Error::Other(format!(
                "rich mail merge fragment part {source_part} was not preallocated"
            ))
        })?;
    if !copied.insert(destination_part.to_ascii_lowercase()) {
        return Ok(destination_part);
    }
    let bytes = source.get_part(source_part).ok_or_else(|| {
        Error::Other(format!(
            "rich mail merge fragment part {source_part} is missing"
        ))
    })?;
    destination
        .package
        .set_part(&destination_part, bytes.to_vec());
    destination
        .identifiers
        .preserve_fragment_part(&destination_part);
    if let Some(content_type) = source.content_types.content_type_for(source_part) {
        destination
            .package
            .content_types
            .add_override(&destination_part, content_type);
        destination
            .identifiers
            .register_content_type_override(&destination_part);
    }
    if let Some(relationships) = source.get_part_rels(source_part) {
        let mut relationships = relationships.items.clone();
        relationships.sort_by(|left, right| left.id.cmp(&right.id));
        for relationship in relationships {
            if !crate::document::relationship_is_internal(&relationship) {
                destination
                    .package
                    .get_or_create_part_rels(&destination_part)
                    .items
                    .push(relationship);
                continue;
            }
            let child_source = OpcPackage::resolve_rel_target(source_part, &relationship.target);
            let child_destination =
                copy_fragment_part(source, destination, &child_source, part_map, copied)?;
            let target = relative_fragment_target(&destination_part, &child_destination);
            let destination_id = destination
                .identifiers
                .reserve_requested_relationship_id_checked(
                    &destination_part,
                    &relationship.id,
                )
                .map_err(|error| {
                    Error::Other(format!(
                        "rich mail merge relationship allocation failed for {destination_part}: {error}"
                    ))
                })?;
            destination
                .package
                .get_or_create_part_rels(&destination_part)
                .add_with_id(&destination_id, &relationship.rel_type, &target);
        }
    }
    Ok(destination_part)
}

pub(super) fn discover_fragment_part_closure(
    source: &OpcPackage,
    source_part: &str,
    discovered: &mut HashSet<String>,
) -> Result<()> {
    if source
        .content_types
        .content_type_for(source_part)
        .is_some_and(|content_type| {
            content_type.starts_with("application/vnd.openxmlformats-package.digital-signature")
        })
    {
        return Err(Error::Other(format!(
            "document fragment integrity-bound signature part {source_part} cannot remain valid after remapping"
        )));
    }
    if !discovered.insert(source_part.to_owned()) {
        return Ok(());
    }
    source.get_part(source_part).ok_or_else(|| {
        Error::Other(format!(
            "rich mail merge fragment part {source_part} is missing"
        ))
    })?;
    if let Some(relationships) = source.get_part_rels(source_part) {
        for relationship in &relationships.items {
            if !crate::document::relationship_is_internal(relationship) {
                continue;
            }
            let child_source = OpcPackage::resolve_rel_target(source_part, &relationship.target);
            discover_fragment_part_closure(source, &child_source, discovered)?;
        }
    }
    Ok(())
}

fn relative_fragment_target(source_part: &str, target_part: &str) -> String {
    let directory = source_part
        .rfind('/')
        .map_or("/", |position| &source_part[..=position]);
    target_part
        .strip_prefix(directory)
        .filter(|target| !target.contains('/'))
        .unwrap_or(target_part)
        .to_owned()
}

/// Merge-local identities that are outside the document allocator scope.
///
/// Bookmark and `wp:docPr` ids are reserved from `DocumentIdentifiers` by
/// `remap_body_identities`. Content-control and non-visual drawing ids have
/// distinct OOXML scopes and remain local to the rich-merge combiner.
pub(super) struct BodyIdentityState {
    used_content_control_ids: HashSet<u32>,
    used_non_visual_drawing_ids: HashSet<u32>,
    used_names: HashSet<String>,
    /// `used_names` lowercased: a fresh name avoids every bookmark and
    /// reference name, in any case, the glossary's included.
    folded_names: HashSet<String>,
    /// The lowercased names of the bookmarks of the destination's stories
    /// (not its glossary), and those given during this import: an imported
    /// bookmark keeps its name while it is not among them, as Word compares
    /// bookmark names without regard to case.
    folded_bookmark_names: HashSet<String>,
    next_content_control_id: u32,
    next_non_visual_drawing_id: u32,
    next_name: u32,
}

impl BodyIdentityState {
    pub(super) fn from_documents(documents: &[Document]) -> Result<Self> {
        let mut state = Self {
            used_content_control_ids: HashSet::new(),
            used_non_visual_drawing_ids: HashSet::new(),
            used_names: HashSet::new(),
            folded_names: HashSet::new(),
            folded_bookmark_names: HashSet::new(),
            next_content_control_id: 1,
            next_non_visual_drawing_id: 1,
            next_name: 1,
        };
        for document in documents {
            let xml = document.document.to_xml()?;
            let values = body_identity_values(&xml)?;
            state.used_content_control_ids.extend(
                values
                    .content_control_ids
                    .iter()
                    .filter_map(|value| value.parse::<u32>().ok()),
            );
            state.used_non_visual_drawing_ids.extend(
                values
                    .non_visual_drawing_ids
                    .iter()
                    .filter_map(|value| value.parse::<u32>().ok()),
            );
            state.use_bookmark_names(values.bookmark_names);
            state.use_names(values.reference_names);
            for (part_name, bytes) in &document.package.parts {
                if let Ok(values) = body_identity_values(&wrap_fragment_companion(bytes)) {
                    state.used_content_control_ids.extend(
                        values
                            .content_control_ids
                            .iter()
                            .filter_map(|value| value.parse::<u32>().ok()),
                    );
                    state.used_non_visual_drawing_ids.extend(
                        values
                            .non_visual_drawing_ids
                            .iter()
                            .filter_map(|value| value.parse::<u32>().ok()),
                    );
                    if part_name.starts_with("/word/glossary/") {
                        state.use_names(values.bookmark_names);
                    } else {
                        state.use_bookmark_names(values.bookmark_names);
                    }
                    state.use_names(values.reference_names);
                }
            }
        }
        Ok(state)
    }

    fn allocate_content_control_id(&mut self) -> Result<String> {
        allocate_merge_local_id(
            &mut self.used_content_control_ids,
            &mut self.next_content_control_id,
            "content-control",
        )
    }

    fn allocate_non_visual_drawing_id(&mut self) -> Result<String> {
        allocate_merge_local_id(
            &mut self.used_non_visual_drawing_ids,
            &mut self.next_non_visual_drawing_id,
            "non-visual drawing",
        )
    }

    fn use_names(&mut self, names: impl IntoIterator<Item = String>) {
        for name in names {
            self.folded_names.insert(name.to_lowercase());
            self.used_names.insert(name);
        }
    }

    fn use_bookmark_names(&mut self, names: impl IntoIterator<Item = String>) {
        for name in names {
            self.folded_bookmark_names.insert(name.to_lowercase());
            self.folded_names.insert(name.to_lowercase());
            self.used_names.insert(name);
        }
    }

    fn allocate_name(&mut self) -> Result<String> {
        loop {
            let candidate = format!("MailMerge{}", self.next_name);
            self.next_name = self.next_name.checked_add(1).ok_or_else(|| {
                Error::Other("mail merge exhausted the bookmark name range".to_owned())
            })?;
            if !self.folded_names.contains(&candidate.to_lowercase())
                && self.used_names.insert(candidate.clone())
            {
                self.folded_names.insert(candidate.to_lowercase());
                self.folded_bookmark_names.insert(candidate.to_lowercase());
                return Ok(candidate);
            }
        }
    }

    /// Keep `name` for an imported bookmark when no bookmark of the
    /// destination's stories, nor one given earlier in this import, has it in
    /// any case. A reference to it in the destination then reaches the
    /// imported bookmark, as when Word pastes one.
    fn keep_free_name(&mut self, name: &str) -> bool {
        let folded = name.to_lowercase();
        if !self.folded_bookmark_names.insert(folded.clone()) {
            return false;
        }
        self.folded_names.insert(folded);
        self.used_names.insert(name.to_owned());
        true
    }
}

fn allocate_merge_local_id(
    occupied: &mut HashSet<u32>,
    cursor: &mut u32,
    category: &str,
) -> Result<String> {
    loop {
        let candidate = *cursor;
        *cursor = cursor.checked_add(1).ok_or_else(|| {
            Error::Other(format!(
                "mail merge exhausted the {category} identity range"
            ))
        })?;
        if occupied.insert(candidate) {
            return Ok(candidate.to_string());
        }
    }
}

pub(super) fn remap_body_identities(
    document: &mut Document,
    identifiers: &mut DocumentIdentifiers,
    state: &mut BodyIdentityState,
) -> Result<BodyIdentityRemap> {
    let (updated, remap) = remap_fragment_xml_identities(
        &document.document.to_xml()?,
        identifiers,
        state,
        true,
        false,
    )?;
    document.document = crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
    Ok(remap)
}

/// `keep_free_names` keeps a bookmark's name while it is free in the
/// destination, as an import does; a merge record renames every bookmark.
fn remap_fragment_xml_identities(
    source: &[u8],
    identifiers: &mut DocumentIdentifiers,
    state: &mut BodyIdentityState,
    drop_paragraph_identities: bool,
    keep_free_names: bool,
) -> Result<(Vec<u8>, BodyIdentityRemap)> {
    // The drawing remap is keyed by value, so a source that repeats a
    // `wp:docPr/@id` is first made unique and every copy gets its own id.
    let xml = uniquify_drawing_ids_in_xml(source)?;
    let values = body_identity_values(&xml)?;
    let mut remap = BodyIdentityRemap {
        drop_paragraph_identities,
        ..Default::default()
    };
    for value in values.bookmark_ids {
        if let std::collections::btree_map::Entry::Vacant(entry) = remap.bookmark_ids.entry(value) {
            entry.insert(identifiers.reserve_bookmark_id()?.to_string());
        }
    }
    for value in values.content_control_ids {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            remap.content_control_ids.entry(value)
        {
            entry.insert(state.allocate_content_control_id()?);
        }
    }
    for value in values.drawing_ids {
        if let std::collections::btree_map::Entry::Vacant(entry) = remap.drawing_ids.entry(value) {
            entry.insert(identifiers.reserve_drawing_id()?.to_string());
        }
    }
    for value in values.non_visual_drawing_ids {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            remap.non_visual_drawing_ids.entry(value)
        {
            entry.insert(state.allocate_non_visual_drawing_id()?);
        }
    }
    // Free names are kept first, so a fresh name never takes one that a
    // later bookmark of the source keeps.
    let mut renamed = Vec::new();
    for value in values.bookmark_names {
        if remap.bookmark_names.contains_key(&value) {
            continue;
        }
        if keep_free_names && state.keep_free_name(&value) {
            remap.bookmark_names.insert(value.clone(), value);
        } else if !renamed.contains(&value) {
            renamed.push(value);
        }
    }
    for value in renamed {
        let name = state.allocate_name()?;
        remap.bookmark_names.insert(value, name);
    }
    let updated = patch_body_identity_attributes(&xml, &remap)?;
    Ok((updated, remap))
}

pub(super) fn body_identity_values(xml: &[u8]) -> Result<BodyIdentityValues> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut body_depth = None;
    let mut sdt_properties_depth = None;
    let mut values = BodyIdentityValues::default();
    let mut complex = Vec::<ComplexInstruction>::new();
    let mut in_instruction_text = false;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid document identity XML: {error}")))?;
        let word = match &event {
            Event::Start(element) | Event::Empty(element) => {
                namespace_is_word(&reader.resolver().resolve_element(element.name()).0)
            }
            Event::End(element) => {
                namespace_is_word(&reader.resolver().resolve_element(element.name()).0)
            }
            _ => false,
        };
        match event {
            Event::Start(element) => {
                if body_depth.is_none()
                    && word
                    && matches_local_name(element.name().as_ref(), b"body")
                {
                    body_depth = Some(depth);
                } else if body_depth.is_some() {
                    collect_body_identity_values(
                        &element,
                        reader.resolver(),
                        sdt_properties_depth.is_some(),
                        &mut values,
                    )?;
                    if collect_body_reference_values(
                        &element,
                        reader.resolver(),
                        &mut complex,
                        &mut values.reference_names,
                    )? {
                        in_instruction_text = true;
                    }
                    if word && matches_local_name(element.name().as_ref(), b"sdtPr") {
                        sdt_properties_depth = Some(depth);
                    }
                }
                depth += 1;
            }
            Event::Empty(element) if body_depth.is_some() => {
                collect_body_identity_values(
                    &element,
                    reader.resolver(),
                    sdt_properties_depth.is_some(),
                    &mut values,
                )?;
                collect_body_reference_values(
                    &element,
                    reader.resolver(),
                    &mut complex,
                    &mut values.reference_names,
                )?;
            }
            Event::Text(text) if body_depth.is_some() && in_instruction_text => {
                append_complex_instruction_text(
                    &text.decode().map_err(|error| {
                        Error::Other(format!("invalid field instruction text: {error}"))
                    })?,
                    &mut complex,
                )?;
            }
            Event::CData(text) if body_depth.is_some() && in_instruction_text => {
                if let Some(instruction) = complex.last_mut()
                    && instruction.collecting
                {
                    instruction.text.push_str(&text.decode().map_err(|error| {
                        Error::Other(format!("invalid field instruction text: {error}"))
                    })?);
                }
            }
            Event::End(element) => {
                if word && matches_local_name(element.name().as_ref(), b"instrText") {
                    in_instruction_text = false;
                }
                depth = depth.saturating_sub(1);
                if sdt_properties_depth == Some(depth) {
                    sdt_properties_depth = None;
                }
                if body_depth == Some(depth) {
                    body_depth = None;
                }
            }
            Event::Eof => return Ok(values),
            _ => {}
        }
        buffer.clear();
    }
}

fn collect_body_identity_values(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    in_sdt_properties: bool,
    values: &mut BodyIdentityValues,
) -> Result<()> {
    let name = element.name();
    let local = local_name(name.as_ref());
    let namespace = resolver.resolve_element(element.name()).0;
    if namespace_is_word(&namespace) && matches!(local, b"bookmarkStart" | b"bookmarkEnd") {
        if let Some((_, value)) =
            resolved_element_attribute(element, resolver, b"id", AttributeNamespace::Word)?
        {
            values.bookmark_ids.push(value.clone());
            values
                .bookmark_events
                .push((value, local == b"bookmarkStart"));
        }
        if local == b"bookmarkStart"
            && let Some((_, value)) =
                resolved_element_attribute(element, resolver, b"name", AttributeNamespace::Word)?
        {
            values.bookmark_names.push(value);
        }
    } else if namespace_is_word(&namespace)
        && matches!(
            local,
            b"commentRangeStart" | b"commentRangeEnd" | b"commentReference"
        )
    {
        if let Some((_, value)) =
            resolved_element_attribute(element, resolver, b"id", AttributeNamespace::Word)?
        {
            let kind = match local {
                b"commentRangeStart" => 0,
                b"commentRangeEnd" => 1,
                b"commentReference" => 2,
                _ => unreachable!("comment identity element was matched"),
            };
            values.comment_ids.push(value.clone());
            values.comment_events.push((value, kind));
        }
    } else if namespace_is_word(&namespace) && in_sdt_properties && local == b"id" {
        if let Some((_, value)) =
            resolved_element_attribute(element, resolver, b"val", AttributeNamespace::Word)?
        {
            values.content_control_ids.push(value);
        }
    } else if namespace_matches(&namespace, WP_NS) && local == b"docPr" {
        if let Some((_, value)) =
            resolved_element_attribute(element, resolver, b"id", AttributeNamespace::Unbound)?
        {
            values.drawing_ids.push(value);
        }
    } else if namespace_is_non_visual_drawing(&namespace)
        && local == b"cNvPr"
        && let Some((_, value)) =
            resolved_element_attribute(element, resolver, b"id", AttributeNamespace::Unbound)?
    {
        values.non_visual_drawing_ids.push(value);
    }
    Ok(())
}

fn collect_body_reference_values(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    complex: &mut Vec<ComplexInstruction>,
    references: &mut Vec<String>,
) -> Result<bool> {
    let (namespace, local) = resolver.resolve_element(element.name());
    if !namespace_is_word(&namespace) {
        return Ok(false);
    }
    if local.as_ref() == b"hyperlink" {
        if let Some((_, anchor)) =
            resolved_element_attribute(element, resolver, b"anchor", AttributeNamespace::Word)?
        {
            references.push(anchor);
        }
    } else if local.as_ref() == b"fldSimple" {
        if let Some((_, instruction)) =
            resolved_element_attribute(element, resolver, b"instr", AttributeNamespace::Word)?
        {
            collect_reference_field_name(&instruction, references);
        }
    } else if local.as_ref() == b"fldChar" {
        match resolved_element_attribute(
            element,
            resolver,
            b"fldCharType",
            AttributeNamespace::Word,
        )?
        .map(|(_, value)| value)
        .as_deref()
        {
            Some("begin") => complex.push(ComplexInstruction {
                text: String::new(),
                collecting: true,
            }),
            Some("separate") => {
                if let Some(instruction) = complex.last_mut() {
                    instruction.collecting = false;
                }
            }
            Some("end") => {
                if let Some(instruction) = complex.pop() {
                    collect_reference_field_name(&instruction.text, references);
                }
            }
            _ => {}
        }
    }
    Ok(local.as_ref() == b"instrText")
}

fn append_complex_instruction_text(text: &str, complex: &mut [ComplexInstruction]) -> Result<()> {
    if let Some(instruction) = complex.last_mut()
        && instruction.collecting
    {
        let unescaped = quick_xml::escape::unescape(text)
            .map_err(|error| Error::Other(format!("invalid field instruction entity: {error}")))?;
        instruction.text.push_str(&unescaped);
    }
    Ok(())
}

fn collect_reference_field_name(instruction: &str, references: &mut Vec<String>) {
    if let Some(name) = reference_field_name(instruction) {
        references.push(name);
    }
}

/// The bookmark a field refers to (see
/// [`oxoffice_docx_oxml::text::bookmark_reference_span`]).
fn reference_field_name(instruction: &str) -> Option<String> {
    oxoffice_docx_oxml::text::bookmark_reference_span(instruction).map(|(_, name)| name)
}

/// The new name of the bookmark `old` names, when an import renamed it.
/// Word compares bookmark names without regard to case, so a reference
/// written in another case follows the rename too.
fn renamed_bookmark<'a>(names: &'a BTreeMap<String, String>, old: &str) -> Option<&'a String> {
    let new = names.get(old).or_else(|| {
        let folded = old.to_lowercase();
        names
            .iter()
            .find(|(name, _)| name.to_lowercase() == folded)
            .map(|(_, new)| new)
    })?;
    (new.to_lowercase() != old.to_lowercase()).then_some(new)
}

#[derive(Clone, Copy)]
pub(super) enum AttributeNamespace {
    Relationship,
    Word,
    Unbound,
    Bound(&'static str),
}

pub(super) fn resolved_element_attribute(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    local: &[u8],
    expected: AttributeNamespace,
) -> Result<Option<(Vec<u8>, String)>> {
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| Error::Other(format!("invalid XML attribute: {error}")))?;
        let (namespace, resolved_local) = resolver.resolve_attribute(attribute.key);
        let namespace_matches = match expected {
            AttributeNamespace::Relationship => namespace_matches(&namespace, R_NS),
            AttributeNamespace::Word => namespace_is_word(&namespace),
            AttributeNamespace::Unbound => matches!(namespace, ResolveResult::Unbound),
            AttributeNamespace::Bound(expected) => namespace_matches(&namespace, expected),
        };
        if namespace_matches && resolved_local.as_ref() == local {
            let raw = std::str::from_utf8(attribute.value.as_ref())
                .map_err(|error| Error::Other(format!("invalid XML attribute value: {error}")))?;
            let decoded = quick_xml::escape::unescape(raw)
                .map_err(|error| Error::Other(format!("invalid XML attribute entity: {error}")))?;
            return Ok(Some((
                attribute.key.as_ref().to_vec(),
                decoded.into_owned(),
            )));
        }
    }
    Ok(None)
}

const WP_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
pub(super) const W14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const PIC_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const DRAWING_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const WPS_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";

pub(super) fn namespace_matches(namespace: &ResolveResult<'_>, expected: &str) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if *uri == expected.as_bytes())
}

fn namespace_is_non_visual_drawing(namespace: &ResolveResult<'_>) -> bool {
    [PIC_NS, DRAWING_NS, WPS_NS]
        .iter()
        .any(|expected| namespace_matches(namespace, expected))
}

pub(super) fn patch_body_identity_attributes(
    xml: &[u8],
    remap: &BodyIdentityRemap,
) -> Result<Vec<u8>> {
    let edits = body_identity_edits(xml, remap)?;
    let mut updated = xml.to_vec();
    for edit in edits.into_iter().rev() {
        updated.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(updated)
}

fn body_identity_edits(xml: &[u8], remap: &BodyIdentityRemap) -> Result<Vec<FieldSourceEdit>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut body_depth = None;
    let mut sdt_properties_depth = None;
    let mut edits = Vec::new();
    let mut complex = Vec::<ComplexInstructionEdit>::new();
    let mut in_instruction_text = false;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid document identity XML: {error}")))?;
        let after = reader.buffer_position() as usize;
        let identity_namespace = match &event {
            Event::Start(element) | Event::Empty(element) => BodyIdentityNamespace::from_resolved(
                &reader.resolver().resolve_element(element.name()).0,
            ),
            Event::End(element) => BodyIdentityNamespace::from_resolved(
                &reader.resolver().resolve_element(element.name()).0,
            ),
            _ => BodyIdentityNamespace::default(),
        };
        match event {
            Event::Start(element) => {
                if body_depth.is_none()
                    && identity_namespace.word
                    && matches_local_name(element.name().as_ref(), b"body")
                {
                    body_depth = Some(depth);
                } else if body_depth.is_some() {
                    collect_body_identity_edits(
                        xml,
                        before,
                        after,
                        &element,
                        reader.resolver(),
                        identity_namespace,
                        sdt_properties_depth.is_some(),
                        remap,
                        &mut edits,
                    )?;
                    if collect_body_reference_edits(
                        xml,
                        before,
                        after,
                        &element,
                        reader.resolver(),
                        remap,
                        &mut complex,
                        &mut edits,
                    )? {
                        in_instruction_text = true;
                    }
                    if identity_namespace.word
                        && matches_local_name(element.name().as_ref(), b"sdtPr")
                    {
                        sdt_properties_depth = Some(depth);
                    }
                }
                depth += 1;
            }
            Event::Empty(element) if body_depth.is_some() => {
                collect_body_identity_edits(
                    xml,
                    before,
                    after,
                    &element,
                    reader.resolver(),
                    identity_namespace,
                    sdt_properties_depth.is_some(),
                    remap,
                    &mut edits,
                )?;
                collect_body_reference_edits(
                    xml,
                    before,
                    after,
                    &element,
                    reader.resolver(),
                    remap,
                    &mut complex,
                    &mut edits,
                )?;
            }
            Event::Text(text) if body_depth.is_some() && in_instruction_text => {
                if let Some(instruction) = complex.last_mut()
                    && instruction.collecting
                {
                    let decoded = text.decode().map_err(|error| {
                        Error::Other(format!("invalid field instruction text: {error}"))
                    })?;
                    let decoded = quick_xml::escape::unescape(&decoded).map_err(|error| {
                        Error::Other(format!("invalid field instruction entity: {error}"))
                    })?;
                    instruction.text.push_str(&decoded);
                    instruction
                        .spans
                        .push((before, after, Some(decoded.into_owned())));
                }
            }
            Event::CData(text) if body_depth.is_some() && in_instruction_text => {
                if let Some(instruction) = complex.last_mut()
                    && instruction.collecting
                {
                    let decoded = text.decode().map_err(|error| {
                        Error::Other(format!("invalid field instruction text: {error}"))
                    })?;
                    instruction.text.push_str(&decoded);
                    instruction.spans.push((before, after, None));
                }
            }
            Event::End(element) => {
                if identity_namespace.word
                    && matches_local_name(element.name().as_ref(), b"instrText")
                {
                    in_instruction_text = false;
                }
                depth = depth.saturating_sub(1);
                if sdt_properties_depth == Some(depth) {
                    sdt_properties_depth = None;
                }
                if body_depth == Some(depth) {
                    body_depth = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    edits.sort_by_key(|edit: &FieldSourceEdit| edit.start);
    Ok(edits)
}

pub(crate) fn patch_bookmark_source(
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    source: oxoffice_docx_oxml::document::ScopedWordSource,
    bookmark_ids: &BTreeMap<String, String>,
) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
    let edits = body_identity_edits(
        source.as_bytes(),
        &BodyIdentityRemap {
            bookmark_ids: bookmark_ids.clone(),
            ..Default::default()
        },
    )?;
    if edits.is_empty() {
        return Ok(source);
    }
    let mut chunks = Vec::new();
    let mut at = 0;
    for edit in &edits {
        if at > edit.start {
            return Err(Error::Other("overlapping document identity edits".into()));
        }
        chunks.push((Some(&source), at..edit.start, &[][..]));
        chunks.push((None, 0..0, edit.replacement.as_slice()));
        at = edit.end;
    }
    chunks.push((Some(&source), at..source.as_bytes().len(), &[][..]));
    Ok(operation.compose_sources(&chunks)?)
}

pub(crate) fn patch_bookmark_ids(
    xml: &[u8],
    bookmark_ids: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    patch_body_identity_attributes(
        xml,
        &BodyIdentityRemap {
            bookmark_ids: bookmark_ids.clone(),
            ..Default::default()
        },
    )
}

pub(crate) fn freshen_content_fragment_identities(
    document: &mut Document,
    wrapped_fragment: &[u8],
) -> Result<Vec<u8>> {
    let wrapped_fragment = &uniquify_drawing_ids_in_xml(wrapped_fragment)?;
    let values = body_identity_values(wrapped_fragment)?;
    let mut open_bookmarks = Vec::new();
    let mut seen_bookmarks = HashSet::new();
    for (id, start) in &values.bookmark_events {
        if *start {
            if !seen_bookmarks.insert(id.clone()) {
                return Err(Error::Other(
                    "content fragment bookmark ownership is incomplete or ambiguous".to_owned(),
                ));
            }
            open_bookmarks.push(id.as_str());
        } else if open_bookmarks.pop() != Some(id.as_str()) {
            return Err(Error::Other(
                "content fragment bookmark ownership is incomplete or ambiguous".to_owned(),
            ));
        }
    }
    if !open_bookmarks.is_empty() {
        return Err(Error::Other(
            "content fragment bookmark ownership is incomplete or ambiguous".to_owned(),
        ));
    }
    let mut state = BodyIdentityState::from_documents(std::slice::from_ref(document))?;
    let mut remap = BodyIdentityRemap {
        drop_paragraph_identities: true,
        ..Default::default()
    };
    for value in values.bookmark_ids {
        if let std::collections::btree_map::Entry::Vacant(entry) = remap.bookmark_ids.entry(value) {
            entry.insert(document.identifiers.reserve_bookmark_id()?.to_string());
        }
    }
    for value in values.content_control_ids {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            remap.content_control_ids.entry(value)
        {
            entry.insert(state.allocate_content_control_id()?);
        }
    }
    for value in values.drawing_ids {
        if let std::collections::btree_map::Entry::Vacant(entry) = remap.drawing_ids.entry(value) {
            entry.insert(document.identifiers.reserve_drawing_id()?.to_string());
        }
    }
    for value in values.non_visual_drawing_ids {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            remap.non_visual_drawing_ids.entry(value)
        {
            entry.insert(state.allocate_non_visual_drawing_id()?);
        }
    }
    for value in values.bookmark_names {
        if let std::collections::btree_map::Entry::Vacant(entry) = remap.bookmark_names.entry(value)
        {
            entry.insert(state.allocate_name()?);
        }
    }
    patch_body_identity_attributes(wrapped_fragment, &remap)
}

fn validate_fragment_identity_ownership(values: &BodyIdentityValues) -> Result<()> {
    // Each bookmark starts once and ends after its start. Ranges may cross,
    // as Word writes a caption's _Ref and _Toc bookmarks.
    let mut open_bookmarks = HashSet::new();
    let mut seen_bookmarks = HashSet::new();
    for (id, start) in &values.bookmark_events {
        if *start {
            if !seen_bookmarks.insert(id.as_str()) {
                return Err(Error::Other(
                    "document fragment bookmark ownership is incomplete or ambiguous".to_owned(),
                ));
            }
            open_bookmarks.insert(id.as_str());
        } else if !open_bookmarks.remove(id.as_str()) {
            return Err(Error::Other(
                "document fragment bookmark ownership is incomplete or ambiguous".to_owned(),
            ));
        }
    }
    if !open_bookmarks.is_empty() {
        return Err(Error::Other(
            "document fragment bookmark ownership is incomplete or ambiguous".to_owned(),
        ));
    }

    let mut comment_counts = BTreeMap::<&str, [usize; 3]>::new();
    for (id, kind) in &values.comment_events {
        comment_counts.entry(id).or_default()[usize::from(*kind)] += 1;
    }
    if comment_counts.values().any(|counts| *counts != [1, 1, 1]) {
        return Err(Error::Other(
            "document fragment comment ownership is incomplete or ambiguous".to_owned(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn collect_body_identity_edits(
    xml: &[u8],
    start: usize,
    end: usize,
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    namespace: BodyIdentityNamespace,
    in_sdt_properties: bool,
    remap: &BodyIdentityRemap,
    edits: &mut Vec<FieldSourceEdit>,
) -> Result<()> {
    let name = element.name();
    let local = local_name(name.as_ref());
    if namespace.word && matches!(local, b"bookmarkStart" | b"bookmarkEnd") {
        add_identity_attribute_edit(
            xml,
            start,
            end,
            element,
            resolver,
            b"id",
            AttributeNamespace::Word,
            &remap.bookmark_ids,
            edits,
        )?;
        if local == b"bookmarkStart" {
            add_identity_attribute_edit(
                xml,
                start,
                end,
                element,
                resolver,
                b"name",
                AttributeNamespace::Word,
                &remap.bookmark_names,
                edits,
            )?;
        }
    } else if namespace.word
        && matches!(
            local,
            b"commentRangeStart" | b"commentRangeEnd" | b"commentReference"
        )
    {
        add_identity_attribute_edit(
            xml,
            start,
            end,
            element,
            resolver,
            b"id",
            AttributeNamespace::Word,
            &remap.comment_ids,
            edits,
        )?;
    } else if namespace.word && in_sdt_properties && local == b"id" {
        add_identity_attribute_edit(
            xml,
            start,
            end,
            element,
            resolver,
            b"val",
            AttributeNamespace::Word,
            &remap.content_control_ids,
            edits,
        )?;
    } else if namespace.wordprocessing_drawing && local == b"docPr" {
        add_identity_attribute_edit(
            xml,
            start,
            end,
            element,
            resolver,
            b"id",
            AttributeNamespace::Unbound,
            &remap.drawing_ids,
            edits,
        )?;
    } else if namespace.non_visual_drawing && local == b"cNvPr" {
        add_identity_attribute_edit(
            xml,
            start,
            end,
            element,
            resolver,
            b"id",
            AttributeNamespace::Unbound,
            &remap.non_visual_drawing_ids,
            edits,
        )?;
    } else if remap.drop_paragraph_identities && namespace.word && matches!(local, b"p" | b"tr") {
        for attribute in element.attributes() {
            let attribute = attribute
                .map_err(|error| Error::Other(format!("invalid XML attribute: {error}")))?;
            let (attribute_namespace, attribute_local) = resolver.resolve_attribute(attribute.key);
            if !namespace_matches(&attribute_namespace, W14_NS)
                || !matches!(attribute_local.as_ref(), b"paraId" | b"textId")
            {
                continue;
            }
            let Some((name_start, _, value_end)) =
                attribute_source_span(&xml[start..end], attribute.key.as_ref())
            else {
                return Err(Error::Other(
                    "document identity attribute source was not found".to_owned(),
                ));
            };
            // Remove the whitespace before the name with the attribute.
            let removed_start = xml[start..start + name_start]
                .iter()
                .rposition(|byte| !byte.is_ascii_whitespace())
                .map_or(0, |last| last + 1);
            edits.push(FieldSourceEdit {
                start: start + removed_start,
                end: start + value_end + 1,
                replacement: Vec::new(),
            });
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct BodyIdentityNamespace {
    word: bool,
    wordprocessing_drawing: bool,
    non_visual_drawing: bool,
}

impl BodyIdentityNamespace {
    fn from_resolved(namespace: &ResolveResult<'_>) -> Self {
        Self {
            word: namespace_is_word(namespace),
            wordprocessing_drawing: namespace_matches(namespace, WP_NS),
            non_visual_drawing: namespace_is_non_visual_drawing(namespace),
        }
    }
}

pub(super) fn add_identity_attribute_edit(
    xml: &[u8],
    start: usize,
    end: usize,
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    local: &[u8],
    expected: AttributeNamespace,
    replacements: &BTreeMap<String, String>,
    edits: &mut Vec<FieldSourceEdit>,
) -> Result<()> {
    let Some((key, old)) = resolved_element_attribute(element, resolver, local, expected)? else {
        return Ok(());
    };
    let Some(replacement) = replacements.get(&old) else {
        return Ok(());
    };
    let Some((relative_start, relative_end)) = attribute_value_span(&xml[start..end], &key) else {
        return Err(Error::Other(
            "document identity attribute source was not found".to_owned(),
        ));
    };
    edits.push(FieldSourceEdit {
        start: start + relative_start,
        end: start + relative_end,
        replacement: replacement.as_bytes().to_vec(),
    });
    Ok(())
}

struct ComplexInstructionEdit {
    text: String,
    collecting: bool,
    /// Each source span of the instruction text with its decoded text, or
    /// `None` for a CDATA span, which is not edited.
    spans: Vec<(usize, usize, Option<String>)>,
}

#[allow(clippy::too_many_arguments)]
fn collect_body_reference_edits(
    xml: &[u8],
    start: usize,
    end: usize,
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    remap: &BodyIdentityRemap,
    complex: &mut Vec<ComplexInstructionEdit>,
    edits: &mut Vec<FieldSourceEdit>,
) -> Result<bool> {
    let (namespace, local) = resolver.resolve_element(element.name());
    if !namespace_is_word(&namespace) {
        return Ok(false);
    }
    if local.as_ref() == b"hyperlink" {
        // A link with an address names a place in that address.
        if resolved_element_attribute(element, resolver, b"id", AttributeNamespace::Relationship)?
            .is_none()
            && let Some((key, old)) =
                resolved_element_attribute(element, resolver, b"anchor", AttributeNamespace::Word)?
            && let Some(replacement) = renamed_bookmark(&remap.bookmark_names, &old)
        {
            add_attribute_value_edit(xml, start, end, &key, replacement, true, edits)?;
        }
    } else if local.as_ref() == b"fldSimple" {
        if let Some((key, instruction)) =
            resolved_element_attribute(element, resolver, b"instr", AttributeNamespace::Word)?
            && let Some(updated) = remap_reference_instruction(&instruction, &remap.bookmark_names)
        {
            add_attribute_value_edit(xml, start, end, &key, &updated, true, edits)?;
        }
    } else if local.as_ref() == b"fldChar" {
        match resolved_element_attribute(
            element,
            resolver,
            b"fldCharType",
            AttributeNamespace::Word,
        )?
        .map(|(_, value)| value)
        .as_deref()
        {
            Some("begin") => complex.push(ComplexInstructionEdit {
                text: String::new(),
                collecting: true,
                spans: Vec::new(),
            }),
            Some("separate") => {
                if let Some(instruction) = complex.last_mut() {
                    instruction.collecting = false;
                }
            }
            Some("end") => {
                // Only the source span that holds the whole name is edited, so
                // no text moves around a nested field; a name split across
                // spans, or in CDATA, is left as it is.
                if let Some(instruction) = complex.pop()
                    && let Some((range, replacement)) =
                        renamed_reference_range(&instruction.text, &remap.bookmark_names)
                {
                    let mut offset = 0;
                    for (start, end, decoded) in &instruction.spans {
                        let length = decoded.as_ref().map_or_else(|| usize::MAX / 2, String::len);
                        if let Some(decoded) = decoded
                            && offset <= range.start
                            && range.end <= offset + length
                        {
                            let mut updated = decoded.clone();
                            updated.replace_range(
                                range.start - offset..range.end - offset,
                                &replacement,
                            );
                            edits.push(FieldSourceEdit {
                                start: *start,
                                end: *end,
                                replacement: quick_xml::escape::escape(&updated)
                                    .into_owned()
                                    .into_bytes(),
                            });
                            break;
                        }
                        if decoded.is_none() {
                            break;
                        }
                        offset += length;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(local.as_ref() == b"instrText")
}

fn add_attribute_value_edit(
    xml: &[u8],
    start: usize,
    end: usize,
    key: &[u8],
    replacement: &str,
    escape: bool,
    edits: &mut Vec<FieldSourceEdit>,
) -> Result<()> {
    let Some((relative_start, relative_end)) = attribute_value_span(&xml[start..end], key) else {
        return Err(Error::Other(
            "document identity attribute source was not found".to_owned(),
        ));
    };
    let replacement = if escape {
        quick_xml::escape::escape(replacement)
            .into_owned()
            .into_bytes()
    } else {
        replacement.as_bytes().to_vec()
    };
    edits.push(FieldSourceEdit {
        start: start + relative_start,
        end: start + relative_end,
        replacement,
    });
    Ok(())
}

fn remap_reference_instruction(
    instruction: &str,
    names: &BTreeMap<String, String>,
) -> Option<String> {
    let (range, replacement) = renamed_reference_range(instruction, names)?;
    let mut updated = instruction.to_owned();
    updated.replace_range(range, &replacement);
    Some(updated)
}

/// Where the instruction names a bookmark the import renamed, and its new
/// name: the name's own bytes, never the keyword or another operand.
fn renamed_reference_range(
    instruction: &str,
    names: &BTreeMap<String, String>,
) -> Option<(std::ops::Range<usize>, String)> {
    let (range, old) = oxoffice_docx_oxml::text::bookmark_reference_span(instruction)?;
    let replacement = renamed_bookmark(names, &old)?;
    Some((range, replacement.clone()))
}

pub(crate) fn attribute_value_span(
    element: &[u8],
    attribute_name: &[u8],
) -> Option<(usize, usize)> {
    attribute_source_span(element, attribute_name)
        .map(|(_, value_start, value_end)| (value_start, value_end))
}

/// Return where the name of one attribute starts in a start tag, and where
/// its value starts and ends.
pub(super) fn attribute_source_span(
    element: &[u8],
    attribute_name: &[u8],
) -> Option<(usize, usize, usize)> {
    let mut index = 1usize;
    while index < element.len() && !element[index].is_ascii_whitespace() {
        index += 1;
    }
    while index < element.len() {
        while index < element.len() && element[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= element.len() || matches!(element[index], b'>' | b'/') {
            return None;
        }
        let name_start = index;
        while index < element.len()
            && !element[index].is_ascii_whitespace()
            && element[index] != b'='
        {
            index += 1;
        }
        let name_end = index;
        while index < element.len() && element[index].is_ascii_whitespace() {
            index += 1;
        }
        if element.get(index) != Some(&b'=') {
            return None;
        }
        index += 1;
        while index < element.len() && element[index].is_ascii_whitespace() {
            index += 1;
        }
        let quote = *element.get(index)?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        index += 1;
        let value_start = index;
        while index < element.len() && element[index] != quote {
            index += 1;
        }
        let value_end = index;
        if &element[name_start..name_end] == attribute_name {
            return Some((name_start, value_start, value_end));
        }
        index += 1;
    }
    None
}
