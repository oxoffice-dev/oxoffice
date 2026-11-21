//! Canonicalizing identifiers, relationships, drawings and images before a save.

use super::*;

/// Whether every header and footer reference of a section keeps a relationship
/// read with the document.
fn section_references_were_read(section: &CT_SectPr, preserved: &HashSet<String>) -> bool {
    section
        .header_refs
        .iter()
        .chain(&section.footer_refs)
        .all(|reference| preserved.contains(&reference.rel_id))
}

fn bundle_relationship_precedes_story(rel_type: &str) -> bool {
    matches!(rel_type, rel_types::STYLES | rel_types::SETTINGS)
}

#[cfg(debug_assertions)]
fn assert_same_canonicalization(
    document: &Document,
    result: &Result<()>,
    full: &Document,
    full_result: &Result<()>,
) {
    match (result, full_result) {
        (Ok(()), Ok(())) => {}
        (Err(error), Err(full_error)) => assert_eq!(error.to_string(), full_error.to_string()),
        (result, full_result) => panic!(
            "canonicalizing beside the unchanged main part gives {result:?} where the whole \
             canonicalization gives {full_result:?}"
        ),
    }
    assert!(
        document.document == full.document
            && document.document.to_xml().ok() == full.document.to_xml().ok(),
        "canonicalizing beside the unchanged main part changes the model otherwise"
    );
    assert!(
        document.package.same_package_as(&full.package),
        "canonicalizing beside the unchanged main part changes the package otherwise"
    );
    assert!(
        document.identifiers == full.identifiers,
        "canonicalizing beside the unchanged main part changes the identifiers otherwise"
    );
}

impl Document {
    pub(super) fn canonicalize_typed_identifiers(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        self.canonicalize_bookmark_ids(template)?;
        Ok(())
    }

    fn canonicalize_bookmark_ids(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let mut semantic = Vec::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            for marker in &paragraph.bookmark_markers {
                if marker.is_start()
                    && let Some(id) = marker.id()
                    && self.identifiers.authored_bookmark_ids.contains(&id)
                    && !semantic.contains(&id)
                {
                    semantic.push(id);
                }
            }
        });
        let mut occupied = self.identifiers.preserved_bookmark_ids.clone();
        let mut remap = HashMap::new();
        for old in semantic {
            remap.insert(old, reserve_lowest_i32(&mut occupied, 0, "bookmark")?);
        }
        if !remap.is_empty() {
            let mut field_owned = false;
            visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
                if paragraph.bookmark_markers.iter().any(|marker| {
                    marker
                        .id()
                        .is_some_and(|id| remap.get(&id).is_some_and(|new| *new != id))
                }) && paragraph.runs().iter().any(|run| {
                    run.content.iter().any(|content| {
                        matches!(content, RunContent::Field(field) if field.legacy_form.is_some())
                    })
                }) {
                    field_owned = true;
                }
            });
            if field_owned {
                // An external fragment can make a native form bookmark authored.
                // Its start is inside a retained field source, so patch both
                // physical markers together through the existing whole path.
                let remap = remap
                    .iter()
                    .map(|(old, new)| (old.to_string(), new.to_string()))
                    .collect();
                if let Some(template) = template.as_deref_mut() {
                    let source = template.document_source(&self.document)?;
                    let source =
                        crate::field::patch_bookmark_source(template.operation, source, &remap)?;
                    self.document = crate::document::DocumentModel::unproved(
                        template
                            .sources
                            .read_document(template.operation, Arc::new(source))?,
                    );
                } else {
                    let updated =
                        crate::field::patch_bookmark_ids(&self.document.to_xml()?, &remap)?;
                    self.document =
                        crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
                }
            } else {
                visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
                    let _ = paragraph.remap_authored_bookmark_ids(&remap);
                });
            }
            for old in remap.keys() {
                self.identifiers.authored_bookmark_ids.remove(old);
            }
            self.identifiers
                .authored_bookmark_ids
                .extend(remap.values());
        }
        self.canonicalize_toc_bookmark_ids(template)?;
        Ok(())
    }

    fn canonicalize_toc_bookmark_ids(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let mut semantic = Vec::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            for marker in &paragraph.bookmark_markers {
                if marker.is_start()
                    && let Some(id) = marker.id()
                    && self.identifiers.authored_toc_bookmark_ids.contains(&id)
                    && !semantic.contains(&id)
                {
                    semantic.push(id);
                }
            }
        });
        if semantic.is_empty() {
            return Ok(());
        }
        let mut occupied = self.identifiers.bookmark_ids.clone();
        for id in &self.identifiers.authored_toc_bookmark_ids {
            occupied.remove(id);
        }
        let mut next = occupied.iter().copied().max().unwrap_or(0).checked_add(1);
        let mut remap = BTreeMap::new();
        for old in semantic {
            let new = next.ok_or_else(|| {
                Error::Other("table of contents exhausted the bookmark ID range".to_owned())
            })?;
            occupied.insert(new);
            next = new.checked_add(1);
            if old != new {
                remap.insert(old.to_string(), new.to_string());
            }
        }
        if remap.is_empty() {
            return Ok(());
        }
        if let Some(template) = template {
            let source = template.document_source(&self.document)?;
            let source = crate::field::patch_bookmark_source(template.operation, source, &remap)?;
            self.document = crate::document::DocumentModel::unproved(
                template
                    .sources
                    .read_document(template.operation, Arc::new(source))?,
            );
        } else {
            let xml = self.document.to_xml()?;
            let updated = crate::field::patch_bookmark_ids(&xml, &remap)?;
            self.document =
                crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
        }
        for (old, new) in remap {
            let old = old.parse::<i32>().expect("bookmark remap key is numeric");
            let new = new.parse::<i32>().expect("bookmark remap value is numeric");
            self.identifiers.bookmark_ids.remove(&old);
            self.identifiers.bookmark_ids.insert(new);
            self.identifiers.preserved_bookmark_ids.remove(&old);
            self.identifiers.preserved_bookmark_ids.insert(new);
            self.identifiers.authored_toc_bookmark_ids.remove(&old);
            self.identifiers.authored_toc_bookmark_ids.insert(new);
        }
        Ok(())
    }

    pub(super) fn canonicalize_numbering_ids(
        &mut self,
    ) -> Result<(HashMap<u32, u32>, HashMap<u32, u32>)> {
        let Some(numbering) = self.numbering.as_ref() else {
            return Ok((HashMap::new(), HashMap::new()));
        };
        let mut semantic_nums = Vec::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            if let Some(id) = paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.num_id)
                && !self
                    .identifiers
                    .preserved_numbering_instance_ids
                    .contains(&id)
                && self.identifiers.numbering_instance_ids.contains(&id)
                && !semantic_nums.contains(&id)
            {
                semantic_nums.push(id);
            }
        });
        for instance in &numbering.nums {
            if !self
                .identifiers
                .preserved_numbering_instance_ids
                .contains(&instance.num_id)
                && self
                    .identifiers
                    .numbering_instance_ids
                    .contains(&instance.num_id)
                && !semantic_nums.contains(&instance.num_id)
            {
                semantic_nums.push(instance.num_id);
            }
        }
        let abstract_by_num = numbering
            .nums
            .iter()
            .map(|instance| (instance.num_id, instance.abstract_num_id))
            .collect::<HashMap<_, _>>();
        // Identifiers that content names and the save does not renumber stay
        // taken: those of the source, and those naming a list the document
        // does not define, so that no renumbered list takes one of them.
        let mut occupied_nums = self.identifiers.preserved_numbering_instance_ids.clone();
        occupied_nums.extend(
            self.content_numbering_instance_ids()
                .into_iter()
                .filter(|id| !semantic_nums.contains(id)),
        );
        let mut occupied_abstract = self.identifiers.preserved_abstract_numbering_ids.clone();
        let referenced_abstract = abstract_by_num.values().copied().collect::<HashSet<_>>();
        occupied_abstract.extend(
            numbering
                .abstract_nums
                .iter()
                .map(|definition| definition.abstract_num_id)
                .filter(|id| !referenced_abstract.contains(id)),
        );
        let mut num_remap = HashMap::new();
        let mut abstract_remap = HashMap::new();
        for old_num in semantic_nums {
            num_remap.insert(
                old_num,
                reserve_u32_or_lowest_free(&mut occupied_nums, 1, "numbering instance")?,
            );
            if let Some(old_abstract) = abstract_by_num.get(&old_num)
                && !self
                    .identifiers
                    .preserved_abstract_numbering_ids
                    .contains(old_abstract)
                && !abstract_remap.contains_key(old_abstract)
            {
                abstract_remap.insert(
                    *old_abstract,
                    reserve_u32(&mut occupied_abstract, 0, "abstract numbering")?,
                );
            }
        }
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            if let Some(id) = paragraph
                .properties
                .as_mut()
                .and_then(|properties| properties.num_id.as_mut())
                && let Some(updated) = num_remap.get(id)
            {
                *id = *updated;
            }
        });
        let numbering = self.numbering.as_mut().expect("checked above");
        for definition in &mut numbering.abstract_nums {
            if let Some(updated) = abstract_remap.get(&definition.abstract_num_id) {
                definition.abstract_num_id = *updated;
            }
        }
        for instance in &mut numbering.nums {
            if let Some(updated) = num_remap.get(&instance.num_id) {
                instance.num_id = *updated;
            }
            if let Some(updated) = abstract_remap.get(&instance.abstract_num_id) {
                instance.abstract_num_id = *updated;
            }
        }
        if self.identifiers.preserved_abstract_numbering_ids.is_empty()
            && self.identifiers.preserved_numbering_instance_ids.is_empty()
        {
            numbering
                .abstract_nums
                .sort_by_key(|definition| definition.abstract_num_id);
            numbering.nums.sort_by_key(|instance| instance.num_id);
        }
        Ok((abstract_remap, num_remap))
    }

    pub(super) fn opaque_relationship_ids_from_serialized_story(
        &self,
        modeled_relationships: &[String],
    ) -> Result<Vec<String>> {
        // Without modeled candidates there is nothing to mask. An unchanged
        // typed story can inventory its exact retained source, including opaque
        // property-root attributes that canonical serialization cannot replay.
        // Dirty stories and modeled relationships still need the checked mask.
        // The serialization shortcut still requires the opened model. Namespace
        // owners follow the opened part bytes after that model changes.
        let source_kept = self.kept_main_part();
        let model_kept = source_kept
            .clone()
            .filter(|_| self.opened_main_still_serializes());
        if modeled_relationships.is_empty()
            && let Some(source) = self.package.get_part(&self.doc_part_name)
            && {
                let matches = if model_kept.is_some() {
                    true
                } else if let Some(parsed) = source_kept.as_deref().and_then(Parsed::main_document)
                {
                    // The live model may differ, but these retained source
                    // bytes still have the exact parse from their opening.
                    parsed == &*self.document
                } else if let Some(proved) = self
                    .main_part_blocks
                    .as_ref()
                    .and_then(|blocks| blocks.proved_document_matches(source, &self.document))
                {
                    #[cfg(debug_assertions)]
                    {
                        let fresh = CT_Document::from_xml(source).ok();
                        assert_eq!(
                            fresh.as_ref() == Some(&*self.document),
                            proved,
                            "the proved main part disagrees with a fresh parse"
                        );
                    }
                    proved
                } else {
                    CT_Document::from_xml(source).ok().as_ref() == Some(&*self.document)
                };
                #[cfg(debug_assertions)]
                if let Some(kept) = model_kept.as_deref() {
                    {
                        assert_kept_main_document_without_initializing(
                            Some(kept),
                            Some(source),
                            &self.document,
                        );
                    }
                }
                matches
            }
        {
            return relationship_ids_by_blocks(source, self.main_part_blocks.as_ref(), &[]);
        }
        let scanned;
        let nested_namespace_owners = match source_kept
            .as_deref()
            .and_then(Parsed::nested_namespace_owners)
        {
            Some(owners) => {
                #[cfg(debug_assertions)]
                {
                    let source = self.package.get_part(&self.doc_part_name);
                    {
                        assert_eq!(
                            format!("{owners:?}"),
                            format!(
                                "{:?}",
                                source
                                    .map(nested_modeled_namespace_owners)
                                    .transpose()
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default()
                            ),
                            "the kept nested namespace owners differ from a scan"
                        );
                    }
                }
                owners
            }
            None => {
                let nested_source = self.package.get_part(&self.doc_part_name);
                let proved_owners = nested_source.and_then(|xml| {
                    self.main_part_blocks
                        .as_ref()
                        .and_then(|blocks| blocks.retained_nested_owners(xml))
                });
                if let Some(owners) = proved_owners {
                    #[cfg(debug_assertions)]
                    {
                        let fresh = nested_source
                            .map(nested_modeled_namespace_owners)
                            .transpose()
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        assert_eq!(
                            format!("{owners:?}"),
                            format!("{fresh:?}"),
                            "the proved nested owners differ from a scan"
                        );
                    }
                    scanned = owners;
                    &scanned
                } else {
                    scanned = nested_source
                        .map(nested_modeled_namespace_owners)
                        .transpose()?
                        .unwrap_or_default();
                    &scanned
                }
            }
        };
        let serialize = |document: &CT_Document,
                         blocked: Result<(Vec<u8>, Option<MainPartBlocks>)>,
                         query_source: Option<&MainPartBlocks>|
         -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
            match blocked {
                Ok((xml, record)) => {
                    let spans = record.as_ref().and_then(|record| {
                        query_source
                            .and_then(|source| record.masked_query_namespace_spans(&xml, source))
                            .or_else(|| record.proved_namespace_spans(&xml))
                    });
                    let replayed = replay_nested_namespace_declarations_with_spans(
                        &xml,
                        nested_namespace_owners,
                        spans.as_deref(),
                    )?;
                    Ok(relationship_xml_from_replayed(replayed, Ok((xml, record))))
                }
                Err(_) => {
                    // A failed optional writer proof does not replace the
                    // authoritative full writer or its original error.
                    let xml = document.to_xml()?;
                    let spans = self
                        .main_part_blocks
                        .as_ref()
                        .and_then(|blocks| blocks.proved_namespace_spans(&xml));
                    Ok((
                        replay_nested_namespace_declarations_with_spans(
                            &xml,
                            nested_namespace_owners,
                            spans.as_deref(),
                        )?,
                        None,
                    ))
                }
            }
        };
        let owners_empty = nested_namespace_owners.is_empty();
        if modeled_relationships.is_empty() {
            if owners_empty {
                let (current_xml, current_blocks) = self.canonical_source()?;
                return relationship_ids_by_blocks(&current_xml, current_blocks.as_ref(), &[]);
            }
            let (current_xml, current_blocks) =
                serialize(&self.document, self.canonical_source(), None)?;
            return relationship_ids_by_blocks(&current_xml, current_blocks.as_ref(), &[]);
        }

        let (current_xml, current_blocks) = if owners_empty {
            self.canonical_source()?
        } else {
            serialize(&self.document, self.canonical_source(), None)?
        };
        let current_relationships =
            relationship_ids_by_blocks(&current_xml, current_blocks.as_ref(), &[])?;
        let mut masked = self.document.clone();
        let mut masked_values = HashSet::new();
        let mut remap = HashMap::new();
        for (index, relationship_id) in modeled_relationships.iter().enumerate() {
            let mut ordinal = index;
            let sentinel = loop {
                let candidate = format!("rdocx-modeled-relationship-{ordinal}");
                if !current_relationships.contains(&candidate)
                    && masked_values.insert(candidate.clone())
                {
                    break candidate;
                }
                ordinal = ordinal
                    .checked_add(modeled_relationships.len().max(1))
                    .ok_or_else(|| {
                        Error::Other("modeled relationship mask range is exhausted".to_owned())
                    })?;
            };
            remap.insert(relationship_id.clone(), sentinel);
        }

        visit_authored_drawings_mut(&mut masked.body.content, &mut |drawing| {
            if let Some(mut drawing) = drawing
                .inline
                .as_mut()
                .map(DrawingMut::Inline)
                .or_else(|| drawing.anchor.as_mut().map(DrawingMut::Anchor))
            {
                drawing.remap_relationship(&remap);
            }
        });
        visit_body_paragraphs_mut(&mut masked.body.content, &mut |paragraph| {
            for hyperlink in &mut paragraph.hyperlinks {
                if let Some(id) = &mut hyperlink.rel_id
                    && let Some(updated) = remap.get(id)
                {
                    id.clone_from(updated);
                }
            }
            if let Some(section) = paragraph
                .properties
                .as_mut()
                .and_then(|properties| properties.sect_pr.as_mut())
            {
                for reference in section
                    .header_refs
                    .iter_mut()
                    .chain(&mut section.footer_refs)
                {
                    if let Some(updated) = remap.get(&reference.rel_id) {
                        reference.rel_id.clone_from(updated);
                    }
                }
            }
        });
        if let Some(section) = &mut masked.body.sect_pr {
            for reference in section
                .header_refs
                .iter_mut()
                .chain(&mut section.footer_refs)
            {
                if let Some(updated) = remap.get(&reference.rel_id) {
                    reference.rel_id.clone_from(updated);
                }
            }
        }

        let (masked_xml, masked_blocks) = if owners_empty {
            Self::canonical_source_from(&masked, current_blocks.as_ref())?
        } else {
            serialize(
                &masked,
                Self::canonical_source_from(&masked, current_blocks.as_ref()),
                current_blocks.as_ref(),
            )?
        };
        let masked_relationships = masked_blocks.as_ref().and_then(|masked| {
            current_blocks
                .as_ref()
                .and_then(|current| masked.masked_query_relationship_ids(&masked_xml, current))
        });
        let masked_relationships = match masked_relationships {
            Some(ids) => ids,
            None => relationship_ids_by_blocks(&masked_xml, masked_blocks.as_ref(), &[])?,
        };
        Ok(masked_relationships
            .into_iter()
            .filter(|id| !masked_values.contains(id) && current_relationships.contains(id))
            .collect())
    }

    /// Keep a main-part relationship id from staging renumbering until the
    /// main story references it or the id leaves the relationship registry.
    pub(crate) fn register_pending_relationship(&mut self, owner: &str, id: &str) {
        if owner == self.doc_part_name {
            self.identifiers
                .pending_relationship_ids
                .entry(relationship_owner_identity(owner))
                .or_default()
                .insert(id.to_owned());
        }
    }

    pub(super) fn canonicalize_drawing_ids(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let Some(kept) = self.unchanged_main_part() else {
            return self.canonicalize_all_drawing_ids(template);
        };
        let unchanged = kept
            .unchanged_main_part
            .get()
            .and_then(Option::as_ref)
            .expect("checked unchanged main part");
        #[cfg(debug_assertions)]
        let mut full = self.clone_for_debug_reference();
        let result = self.canonicalize_beside_unchanged_main_part(unchanged, template);
        #[cfg(debug_assertions)]
        {
            let _proof_off = CanonicalBlockProofOff::enter();
            let full_result = full.canonicalize_all_drawing_ids(None);
            assert_same_canonicalization(self, &result, &full, &full_result);
        }
        result
    }

    pub(super) fn opened_main_still_serializes(&self) -> bool {
        let Some(kept) = self.kept_main_part() else {
            return false;
        };
        let Some(expected) = kept.serialization_proof.as_deref() else {
            return false;
        };
        if kept.root_namespace_declarations != self.root_namespace_declarations
            || kept.body_namespace_declarations != self.body_namespace_declarations
            || kept.body_namespace_bindings != self.body_namespace_bindings
            || kept.main_document() != Some(&*self.document)
        {
            return false;
        }
        self.serialized_main_part_matches(expected).unwrap_or(false)
    }

    fn unchanged_main_part(&self) -> Option<Arc<Parsed>> {
        if !self.opened_main_still_serializes() {
            return None;
        }
        let kept = self.kept_main_part()?;
        let owner_identity = relationship_owner_identity(&self.doc_part_name);
        if self
            .identifiers
            .pending_relationship_ids
            .get(&owner_identity)
            .is_some_and(|ids| !ids.is_empty())
        {
            return None;
        }
        if self
            .identifiers
            .authored_nested_story_relationship_ids
            .get(&owner_identity)
            .is_some_and(|ids| !ids.is_empty())
        {
            return None;
        }
        let preserved = self
            .identifiers
            .preserved_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        if self
            .package
            .get_part_rels(&self.doc_part_name)
            .is_some_and(|relationships| {
                !relationships
                    .items
                    .iter()
                    .all(|relationship| preserved.contains(&relationship.id))
            })
        {
            return None;
        }
        if let Some(section) = &self.document.body.sect_pr
            && !section_references_were_read(section, &preserved)
        {
            return None;
        }
        let mut paragraph_sections_ok = true;
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            if let Some(section) = paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
                && !section_references_were_read(section, &preserved)
            {
                paragraph_sections_ok = false;
            }
        });
        if !paragraph_sections_ok {
            return None;
        }
        let mut authored_drawing = false;
        visit_authored_drawings(&self.document.body.content, &mut |_| {
            authored_drawing = true;
        });
        if authored_drawing {
            return None;
        }
        let unchanged = kept
            .unchanged_main_part
            .get_or_init(|| self.unchanged_main_part_checks().ok())
            .as_ref()?;
        if unchanged.body_namespace_bindings != self.body_namespace_bindings
            || self.serialized_main_part_matches(&unchanged.xml).ok() != Some(true)
        {
            return None;
        }
        Some(kept)
    }

    fn unchanged_main_part_checks(&self) -> Result<UnchangedMainPart> {
        let mut modeled_story_relationships = Vec::new();
        collect_relationship_ids(
            &self.document.body.content,
            &mut modeled_story_relationships,
        );
        if let Some(section) = &self.document.body.sect_pr {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if !modeled_story_relationships.contains(&reference.rel_id) {
                    modeled_story_relationships.push(reference.rel_id.clone());
                }
            }
        }
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            if let Some(section) = paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
            {
                for reference in section.header_refs.iter().chain(&section.footer_refs) {
                    if !modeled_story_relationships.contains(&reference.rel_id) {
                        modeled_story_relationships.push(reference.rel_id.clone());
                    }
                }
            }
        });
        self.opaque_relationship_ids_from_serialized_story(&modeled_story_relationships)?;
        let (xml, blocks) = self.canonical_source()?;
        relationship_ids_by_blocks(&xml, blocks.as_ref(), &self.body_namespace_bindings)?;
        authored_drawing_slots_by_blocks(&xml, blocks.as_ref(), &[], &HashSet::new())?;
        let drawing_ids = drawing_ids_by_blocks(&xml, blocks.as_ref());
        relationship_ids_by_blocks(&xml, blocks.as_ref(), &[])?;
        Ok(UnchangedMainPart {
            xml,
            body_namespace_bindings: self.body_namespace_bindings.clone(),
            drawing_ids,
        })
    }

    fn canonicalize_beside_unchanged_main_part(
        &mut self,
        unchanged: &UnchangedMainPart,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let owner_identity = relationship_owner_identity(&self.doc_part_name);
        self.identifiers
            .authored_nested_story_relationship_ids
            .remove(&owner_identity);
        let related_parts = self.related_story_parts();
        let mut occupied_drawings = self.identifiers.preserved_drawing_ids.clone();
        if let Some(live_ids) = &unchanged.drawing_ids {
            occupied_drawings.extend(live_ids);
        }
        for part in &related_parts {
            if !self.identifiers.authored_story_parts.contains(part)
                && let Some(xml) = self.package.get_part(part)
                && let Ok(live_ids) = drawing_ids_in_xml(xml)
            {
                occupied_drawings.extend(live_ids);
            }
        }
        self.identifiers
            .authored_nested_story_drawing_ids
            .remove(&owner_identity);
        self.canonicalize_related_story_relationships(
            &related_parts,
            &mut occupied_drawings,
            template,
        )?;
        self.identifiers.drawing_ids = occupied_drawings;
        self.canonicalize_image_parts(true)
    }

    fn canonicalize_all_drawing_ids(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let owner = self.doc_part_name.clone();
        let owner_identity = relationship_owner_identity(&owner);
        // Story picture insertion keeps current producer XML. Patch its
        // canonical relationship and drawing IDs into that retained source.
        let retained_main_xml = self
            .package
            .get_part_shared(&owner)
            .filter(|xml| {
                let authored = self
                    .identifiers
                    .authored_nested_story_relationship_ids
                    .get(&owner_identity)
                    .is_some_and(|ids| !ids.is_empty());
                if !authored {
                    return false;
                }
                let kept = self.kept_main_part();
                let matches = if let Some(parsed) = self
                    .main_part_blocks
                    .as_ref()
                    .and_then(|blocks| blocks.proved_document_matches(xml, &self.document))
                {
                    parsed
                } else if let Some(parsed) = kept.as_deref().and_then(Parsed::main_document) {
                    parsed == &*self.document
                } else {
                    CT_Document::from_xml(xml).ok().as_ref() == Some(&*self.document)
                };
                matches
                    && self
                        .serialized_main_part()
                        .ok()
                        .as_ref()
                        .map(|(xml, _)| xml.as_slice())
                        != Some(xml.as_slice())
            })
            .map(Arc::clone);
        let retained_main_source =
            if let (Some(bytes), Some(writer)) = (&retained_main_xml, template.as_deref_mut()) {
                if let Some((stored, source)) = &writer.sources.published_main
                    && Arc::ptr_eq(stored, bytes)
                {
                    Some(Arc::clone(source))
                } else {
                    writer
                        .lineage
                        .original_part_source(writer.operation, &owner, bytes)?
                        .map(Arc::new)
                }
            } else {
                None
            };
        let preserved_relationships = self
            .identifiers
            .preserved_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let authored_bundle_relationships = self
            .identifiers
            .authored_bundle_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let mut modeled_story_relationships = Vec::new();
        collect_relationship_ids(
            &self.document.body.content,
            &mut modeled_story_relationships,
        );
        if let Some(section) = &self.document.body.sect_pr {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if !modeled_story_relationships.contains(&reference.rel_id) {
                    modeled_story_relationships.push(reference.rel_id.clone());
                }
            }
        }
        let nested_relationships = self
            .identifiers
            .authored_nested_story_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let mut opaque_relationships =
            self.opaque_relationship_ids_from_serialized_story(&modeled_story_relationships)?;
        opaque_relationships.retain(|id| !nested_relationships.contains(id));
        let mut story_relationships = self.main_story_relationship_ids();
        let relationships = self
            .package
            .get_part_rels(&owner)
            .map(|relationships| relationships.items.clone())
            .unwrap_or_default();
        // Pending XML or a caller still names these ids. Renumbering them
        // would point that name at another relationship. A main-story
        // reference, or disappearance from the registry, ends the hold.
        // Owner-part loss alone does not.
        if let Some(pending) = self
            .identifiers
            .pending_relationship_ids
            .get_mut(&owner_identity)
        {
            pending.retain(|id| {
                !modeled_story_relationships.contains(id)
                    && !opaque_relationships.contains(id)
                    && relationships
                        .iter()
                        .any(|relationship| relationship.id == *id)
            });
            opaque_relationships.extend(pending.iter().cloned());
        }
        story_relationships.retain(|id| {
            !preserved_relationships.contains(id)
                && relationships
                    .iter()
                    .any(|relationship| relationship.id == *id)
        });
        let mut bundle_relationships = relationships
            .iter()
            .filter_map(|relationship| {
                (!preserved_relationships.contains(&relationship.id)
                    && authored_bundle_relationships.contains(&relationship.id)
                    && relationship_is_internal(relationship))
                .then(|| {
                    bundle_relationship_order(&relationship.rel_type).map(|order| {
                        (
                            order,
                            relationship.target.as_str(),
                            relationship.id.as_str(),
                            bundle_relationship_precedes_story(&relationship.rel_type),
                        )
                    })
                })
                .flatten()
            })
            .collect::<Vec<_>>();
        bundle_relationships.sort_unstable();
        let mut semantic_relationships = bundle_relationships
            .iter()
            .filter(|(_, _, _, precedes_story)| *precedes_story)
            .map(|(_, _, id, _)| (*id).to_owned())
            .collect::<Vec<_>>();
        semantic_relationships.extend(story_relationships);
        if let Some(section) = &self.document.body.sect_pr {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if !preserved_relationships.contains(&reference.rel_id)
                    && !semantic_relationships.contains(&reference.rel_id)
                {
                    semantic_relationships.push(reference.rel_id.clone());
                }
            }
        }
        let mut unreferenced_relationships = relationships
            .iter()
            .filter(|relationship| {
                !preserved_relationships.contains(&relationship.id)
                    && !semantic_relationships.contains(&relationship.id)
                    && !authored_bundle_relationships.contains(&relationship.id)
                    && !opaque_relationships.contains(&relationship.id)
            })
            .map(|relationship| {
                let internal = relationship_is_internal(relationship);
                let target = if internal {
                    part_name_identity(&OpcPackage::resolve_rel_target(
                        &owner,
                        &relationship.target,
                    ))
                } else {
                    relationship.target.clone()
                };
                (
                    !internal,
                    relationship.rel_type.as_str(),
                    target,
                    relationship.target_mode.as_deref().unwrap_or(""),
                    relationship.id.as_str(),
                )
            })
            .collect::<Vec<_>>();
        unreferenced_relationships.sort_unstable();
        semantic_relationships.extend(
            unreferenced_relationships
                .into_iter()
                .map(|(_, _, _, _, id)| id.to_owned()),
        );
        semantic_relationships.extend(
            bundle_relationships
                .into_iter()
                .filter(|(_, _, _, precedes_story)| !precedes_story)
                .map(|(_, _, id, _)| id.to_owned()),
        );

        let mut occupied_relationships = preserved_relationships.clone();
        occupied_relationships.extend(opaque_relationships);
        occupied_relationships.extend(
            relationships
                .iter()
                .filter(|relationship| !semantic_relationships.contains(&relationship.id))
                .map(|relationship| relationship.id.clone()),
        );
        let mut relationship_cursor = if semantic_relationships.is_empty() {
            0
        } else {
            preserved_relationships
                .iter()
                .filter_map(|id| id.strip_prefix("rId")?.parse::<u32>().ok())
                .max()
                .map_or(Ok(0), |maximum| {
                    maximum.checked_add(1).ok_or_else(|| {
                        Error::Other("relationship id range is exhausted".to_owned())
                    })
                })?
        };
        let mut relationship_remap = HashMap::new();
        for old in &semantic_relationships {
            let updated = reserve_relationship_from_cursor(
                &mut occupied_relationships,
                &mut relationship_cursor,
            )?;
            relationship_remap.insert(old.clone(), updated);
        }
        if let Some(relationships) = self.package.get_part_rels_mut(&owner) {
            for relationship in &mut relationships.items {
                if let Some(updated) = relationship_remap.get(&relationship.id) {
                    relationship.id.clone_from(updated);
                }
            }
            let mut authored = Vec::new();
            relationships.items.retain(|relationship| {
                if preserved_relationships.contains(&relationship.id) {
                    true
                } else {
                    authored.push(relationship.clone());
                    false
                }
            });
            authored.sort_by_key(|relationship| {
                relationship
                    .id
                    .strip_prefix("rId")
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(u32::MAX)
            });
            relationships.items.extend(authored);
        }
        if !relationship_remap.is_empty() {
            if let Some(occupied) = self.identifiers.relationship_ids.get_mut(&owner_identity) {
                for old in relationship_remap.keys() {
                    occupied.remove(old);
                }
                occupied.extend(relationship_remap.values().cloned());
            }
            if let Some(authored) = self
                .identifiers
                .authored_story_relationship_ids
                .get_mut(&owner_identity)
            {
                let remapped = authored
                    .iter()
                    .map(|id| relationship_remap.get(id).unwrap_or(id).clone())
                    .collect();
                *authored = remapped;
            }
            if let Some(authored) = self
                .identifiers
                .authored_bundle_relationship_ids
                .get_mut(&owner_identity)
            {
                let remapped = authored
                    .iter()
                    .map(|id| relationship_remap.get(id).unwrap_or(id).clone())
                    .collect();
                *authored = remapped;
            }
            if let Some(provisional) = self
                .identifiers
                .provisional_relationship_ids
                .get_mut(&owner_identity)
            {
                provisional.retain(|id| !relationship_remap.contains_key(id));
                if provisional.is_empty() {
                    self.identifiers
                        .provisional_relationship_ids
                        .remove(&owner_identity);
                }
            }
        }
        let registered_nested_drawing_ids = self
            .identifiers
            .authored_nested_story_drawing_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let mut modeled_drawing_ids = Vec::new();
        visit_authored_drawings(&self.document.body.content, &mut |drawing| {
            if let Some(inline) = &drawing.inline
                && !registered_nested_drawing_ids.contains(&inline.doc_pr_id)
            {
                modeled_drawing_ids.push(inline.doc_pr_id);
            }
            if let Some(anchor) = &drawing.anchor
                && !registered_nested_drawing_ids.contains(&anchor.doc_pr_id)
            {
                modeled_drawing_ids.push(anchor.doc_pr_id);
            }
        });
        let has_live_nested_relationships = self.remap_authored_nested_story_relationships(
            &owner_identity,
            &nested_relationships,
            &relationship_remap,
            template.as_deref_mut(),
        )?;
        if has_live_nested_relationships && let Some(writer) = template.as_deref_mut() {
            writer.sources.model_lineage_valid = false;
        }
        if !has_live_nested_relationships {
            visit_authored_drawings_mut(&mut self.document.body.content, &mut |drawing| {
                if let Some(mut drawing) = drawing
                    .inline
                    .as_mut()
                    .map(DrawingMut::Inline)
                    .or_else(|| drawing.anchor.as_mut().map(DrawingMut::Anchor))
                {
                    drawing.remap_relationship(&relationship_remap);
                }
            });
            visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
                for hyperlink in &mut paragraph.hyperlinks {
                    if let Some(id) = &mut hyperlink.rel_id
                        && let Some(updated) = relationship_remap.get(id)
                    {
                        id.clone_from(updated);
                    }
                }
                if let Some(section) = paragraph
                    .properties
                    .as_mut()
                    .and_then(|properties| properties.sect_pr.as_mut())
                {
                    for reference in section
                        .header_refs
                        .iter_mut()
                        .chain(&mut section.footer_refs)
                    {
                        if let Some(updated) = relationship_remap.get(&reference.rel_id) {
                            reference.rel_id.clone_from(updated);
                        }
                    }
                }
            });
            if let Some(section) = &mut self.document.body.sect_pr {
                for reference in section
                    .header_refs
                    .iter_mut()
                    .chain(&mut section.footer_refs)
                {
                    if let Some(updated) = relationship_remap.get(&reference.rel_id) {
                        reference.rel_id.clone_from(updated);
                    }
                }
            }
        }
        let related_parts = self.related_story_parts();
        let live_nested_relationships = self
            .identifiers
            .authored_nested_story_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let nested_image_relationship_ids = self
            .package
            .get_part_rels(&owner)
            .into_iter()
            .flat_map(|relationships| &relationships.items)
            .filter(|relationship| {
                relationship.rel_type == rel_types::IMAGE
                    && live_nested_relationships.contains(&relationship.id)
            })
            .map(|relationship| relationship.id.clone())
            .collect::<HashSet<_>>();
        let (current_document_xml, blocks) =
            if modeled_drawing_ids.is_empty() && nested_image_relationship_ids.is_empty() {
                self.canonical_source()?
            } else {
                self.canonical_source_in(template.as_deref_mut())?
            };
        let authored_drawing_slots = authored_drawing_slots_by_blocks(
            &current_document_xml,
            blocks.as_ref(),
            &modeled_drawing_ids,
            &nested_image_relationship_ids,
        )?;
        let mut occupied_drawings = self.identifiers.preserved_drawing_ids.clone();
        if let Some(live_ids) = drawing_ids_by_blocks(&current_document_xml, blocks.as_ref()) {
            occupied_drawings.extend(live_ids);
        }
        for (_, _, id) in &authored_drawing_slots {
            occupied_drawings.remove(id);
        }
        for part in &related_parts {
            if !self.identifiers.authored_story_parts.contains(part)
                && let Some(xml) = self.package.get_part(part)
                && let Ok(live_ids) = drawing_ids_in_xml(xml)
            {
                occupied_drawings.extend(live_ids);
            }
        }
        let mut drawing_edits = Vec::with_capacity(authored_drawing_slots.len());
        let mut canonical_drawing_ids = Vec::with_capacity(authored_drawing_slots.len());
        let mut canonical_nested_drawing_ids = HashSet::new();
        for (slot, span, old_id) in authored_drawing_slots {
            let id = reserve_u32(&mut occupied_drawings, 1, "drawing")?;
            if matches!(slot, AuthoredMainDrawing::Nested) {
                canonical_nested_drawing_ids.insert(id);
            }
            canonical_drawing_ids.push((old_id, id));
            drawing_edits.push((span.start, span.end, id.to_string().into_bytes()));
        }
        let drawing_bytes_unchanged = drawing_edits.iter().all(|(start, end, value)| {
            current_document_xml.get(*start..*end) == Some(value.as_slice())
        });
        let drawing_noop = !drawing_edits.is_empty()
            && drawing_bytes_unchanged
            && offered_canonical_blocks(blocks.as_ref()).is_some_and(|record| {
                record.current_source_model_matches(&current_document_xml, &self.document)
            });
        if !drawing_edits.is_empty() && !drawing_noop {
            if let Some(writer) = template.as_deref_mut() {
                let source = writer.sources.source.take().ok_or_else(|| {
                    Error::Other("main drawing canonicalization lost its produced source".into())
                })?;
                let source =
                    apply_borrowed_template_source_edits(writer.operation, &source, drawing_edits)?;
                self.document = crate::document::DocumentModel::unproved(
                    writer
                        .sources
                        .read_document(writer.operation, Arc::new(source))?,
                );
                self.main_part_blocks = None;
                self.main_body_scans
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .serialized = None;
            } else {
                let mut updated = current_document_xml;
                drawing_edits.sort_by_key(|(start, _, _)| *start);
                for (start, end, replacement) in drawing_edits.into_iter().rev() {
                    updated.splice(start..end, replacement);
                }
                let updated = Arc::new(updated);
                if let Some((parsed, mut record)) = offered_canonical_blocks(blocks.as_ref())
                    .and_then(|blocks| {
                        self.source_blocks_in(blocks.part.as_deref()?, blocks.clone(), &updated)
                    })
                {
                    self.document = crate::document::DocumentModel::unproved(parsed);
                    if let Some(writer) = template.as_deref_mut() {
                        writer.sources.model_lineage_valid = false;
                    }
                    // This is a current writer memo, not a published package part.
                    record.part = None;
                    self.main_body_scans
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .serialized = Some(record);
                } else {
                    self.document =
                        crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
                    if let Some(writer) = template.as_deref_mut() {
                        writer.sources.model_lineage_valid = false;
                    }
                }
            }
        }
        if let Some(retained) = retained_main_xml {
            if let Some(writer) = template.as_deref_mut() {
                let source = retained_main_source.ok_or_else(|| {
                    Error::Other("retained main publication has no actual source handoff".into())
                })?;
                let edits = relationship_remap_edits(source.as_bytes(), &relationship_remap, &[])?;
                let source =
                    apply_borrowed_template_source_edits(writer.operation, &source, edits)?;
                let slots = authored_drawing_slots_in_source_order(
                    source.as_bytes(),
                    &modeled_drawing_ids,
                    &nested_image_relationship_ids,
                )?;
                if slots.len() == canonical_drawing_ids.len()
                    && slots
                        .iter()
                        .zip(&canonical_drawing_ids)
                        .all(|((_, _, old), (expected, _))| old == expected)
                {
                    let edits = slots
                        .into_iter()
                        .zip(&canonical_drawing_ids)
                        .map(|((_, span, _), (_, id))| {
                            (span.start, span.end, id.to_string().into_bytes())
                        })
                        .collect();
                    let source = apply_template_source_edits(writer.operation, source, edits)?;
                    let retained = writer.operation.repeat_source(&source)?;
                    let bytes = writer.operation.finish_source(retained)?;
                    self.document = crate::document::DocumentModel::unproved(
                        writer
                            .sources
                            .read_document(writer.operation, Arc::new(source))?,
                    );
                    self.package.set_part(&owner, bytes);
                    let bytes = self
                        .package
                        .get_part_shared(&owner)
                        .expect("actual retained main publication");
                    writer.sources.published_main = Some((
                        Arc::clone(bytes),
                        Arc::clone(
                            writer
                                .sources
                                .reader_source
                                .as_ref()
                                .expect("actual retained main reader"),
                        ),
                    ));
                    self.main_part_blocks = None;
                }
            } else {
                if let Some(writer) = template.as_deref_mut() {
                    writer.sources.model_lineage_valid = false;
                }
                let fast = self.package_main_part_blocks().and_then(|record| {
                    record.canonical_retained(
                        &relationship_remap,
                        &modeled_drawing_ids,
                        &nested_image_relationship_ids,
                        &canonical_drawing_ids,
                        &mut self.document,
                    )
                });
                match fast {
                    Some(Some((xml, record))) => {
                        self.package.set_part(&owner, xml);
                        self.main_part_blocks = Some(record);
                    }
                    Some(None) => {}
                    None => {
                        if let Some((xml, document)) = canonical_retained_main_part(
                            &retained,
                            &relationship_remap,
                            &modeled_drawing_ids,
                            &nested_image_relationship_ids,
                            &canonical_drawing_ids,
                        )? {
                            self.document = crate::document::DocumentModel::unproved(document);
                            self.package.set_part(&owner, xml);
                            self.main_part_blocks = None;
                        }
                    }
                }
            }
        }
        if canonical_nested_drawing_ids.is_empty() {
            self.identifiers
                .authored_nested_story_drawing_ids
                .remove(&owner_identity);
        } else {
            self.identifiers
                .authored_nested_story_drawing_ids
                .insert(owner_identity.clone(), canonical_nested_drawing_ids);
        }
        self.canonicalize_related_story_relationships(
            &related_parts,
            &mut occupied_drawings,
            template,
        )?;
        self.identifiers.drawing_ids = occupied_drawings;
        self.canonicalize_image_parts(false)?;
        Ok(())
    }

    fn active_header_footer_parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        for (relationship_id, is_header) in self.header_footer_rel_ids() {
            if let Some(part) = self.header_footer_part_name(&relationship_id, is_header)
                && !parts.contains(&part)
            {
                parts.push(part);
            }
        }
        parts
    }

    fn remap_authored_nested_story_relationships(
        &mut self,
        physical_owner: &str,
        registered: &HashSet<String>,
        relationship_remap: &HashMap<String, String>,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<bool> {
        let (xml, blocks) = self.canonical_source()?;
        let occurrences =
            relationship_ids_by_blocks(&xml, blocks.as_ref(), &self.body_namespace_bindings)?;
        let live = registered
            .iter()
            .filter(|id| occurrences.contains(id))
            .cloned()
            .collect::<HashSet<_>>();
        if !relationship_remap.is_empty() && !live.is_empty() {
            if let Some(writer) = template {
                let source = writer.document_source(&self.document)?;
                let edits = relationship_remap_edits(
                    source.as_bytes(),
                    relationship_remap,
                    &self.body_namespace_bindings,
                )?;
                let source = apply_template_source_edits(writer.operation, source, edits)?;
                self.document = crate::document::DocumentModel::unproved(
                    writer
                        .sources
                        .read_document(writer.operation, Arc::new(source))?,
                );
                self.main_part_blocks = None;
                self.main_body_scans
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .serialized = None;
            } else {
                let fast = offered_canonical_blocks(blocks.as_ref()).and_then(|record| {
                    let edits = record.relationship_remap_edits(
                        relationship_remap,
                        &self.body_namespace_bindings,
                    )?;
                    let mut edited = record.edited(&edits)?;
                    let qualified_xml = edited.qualify_document_children()?;
                    let reconciled = edits.is_empty()
                        && edited.reconcile_readback(&qualified_xml, &mut self.document);
                    if reconciled {
                        return Some((None, edited));
                    }
                    // Original phase2, exactly once after original mandatory phase1.
                    let document = edited.proved_document(&qualified_xml)?;
                    document.validate_resource_limits().ok()?;
                    Some((Some(document), edited))
                });
                if let Some((document, mut record)) = fast {
                    if let Some(document) = document {
                        self.document = crate::document::DocumentModel::unproved(document);
                    }
                    record.part = None;
                    self.main_body_scans
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .serialized = Some(record);
                } else {
                    let updated = remap_xml_relationship_ids_with_bindings(
                        &xml,
                        relationship_remap,
                        &self.body_namespace_bindings,
                    )?;
                    self.document =
                        crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
                }
            }
        }
        if live.is_empty() {
            self.identifiers
                .authored_nested_story_relationship_ids
                .remove(physical_owner);
        } else {
            let remapped = live
                .iter()
                .map(|id| relationship_remap.get(id).unwrap_or(id).clone())
                .collect();
            self.identifiers
                .authored_nested_story_relationship_ids
                .insert(physical_owner.to_owned(), remapped);
        }
        Ok(!live.is_empty())
    }

    pub(super) fn related_story_parts(&self) -> Vec<String> {
        let mut parts = self.active_header_footer_parts();
        if let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) {
            for relationship in &relationships.items {
                if matches!(
                    relationship.rel_type.as_str(),
                    rel_types::FOOTNOTES | rel_types::ENDNOTES | rel_types::COMMENTS
                ) && relationship_is_internal(relationship)
                {
                    let part =
                        OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
                    if self.package.contains_part(&part) && !parts.contains(&part) {
                        parts.push(part);
                    }
                }
            }
        }
        parts
    }

    pub(super) fn main_story_relationship_ids(&self) -> Vec<String> {
        let mut modeled = Vec::new();
        collect_relationship_ids(&self.document.body.content, &mut modeled);
        if let Some(section) = &self.document.body.sect_pr {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if !modeled.contains(&reference.rel_id) {
                    modeled.push(reference.rel_id.clone());
                }
            }
        }
        let owner = relationship_owner_identity(&self.doc_part_name);
        let nested = self
            .identifiers
            .authored_nested_story_relationship_ids
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        let semantic = modeled
            .iter()
            .cloned()
            .chain(nested.iter().cloned())
            .collect::<HashSet<_>>();
        if let Ok((xml, blocks)) = self.canonical_source()
            && let Some(ordered) = {
                if let Some(all) = offered_canonical_blocks(blocks.as_ref())
                    .and_then(|blocks| blocks.proved_relationship_ids(&xml, &[]))
                {
                    let mut seen = HashSet::new();
                    let ordered = all
                        .into_iter()
                        .filter(|id| semantic.contains(id) && seen.insert(id.clone()))
                        .collect::<Vec<_>>();
                    (ordered.len() == semantic.len()).then_some(ordered)
                } else {
                    authored_relationship_ids_in_source_order(&xml, &semantic)
                }
            }
        {
            return ordered;
        }
        let mut nested = nested.into_iter().collect::<Vec<_>>();
        nested.sort();
        for id in nested {
            if !modeled.contains(&id) {
                modeled.push(id);
            }
        }
        modeled
    }

    fn canonicalize_related_story_relationships(
        &mut self,
        parts: &[String],
        occupied_drawings: &mut HashSet<u32>,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let mut partially_authored_drawing_slots = HashMap::new();
        for owner in parts {
            if self.identifiers.authored_story_parts.contains(owner) {
                continue;
            }
            let owner_identity = relationship_owner_identity(owner);
            let partially_authored = self
                .identifiers
                .authored_story_relationship_ids
                .get(&owner_identity)
                .cloned()
                .unwrap_or_default();
            if partially_authored.is_empty() {
                continue;
            }
            let xml = self
                .package
                .get_part(owner)
                .ok_or_else(|| Error::Other(format!("story part {owner} is missing")))?;
            let authored_image_relationship_ids = self
                .package
                .get_part_rels(owner)
                .into_iter()
                .flat_map(|relationships| &relationships.items)
                .filter(|relationship| {
                    relationship.rel_type == rel_types::IMAGE
                        && partially_authored.contains(&relationship.id)
                })
                .map(|relationship| relationship.id.clone())
                .collect::<HashSet<_>>();
            let drawing_slots =
                authored_drawing_slots_in_source_order(xml, &[], &authored_image_relationship_ids)?
                    .into_iter()
                    .map(|(_, span, id)| (span, id))
                    .collect::<Vec<_>>();
            for (_, id) in &drawing_slots {
                occupied_drawings.remove(id);
            }
            partially_authored_drawing_slots.insert(owner.clone(), drawing_slots);
        }

        for owner in parts {
            let owner_identity = relationship_owner_identity(owner);
            let fully_authored = self.identifiers.authored_story_parts.contains(owner);
            let partially_authored = self
                .identifiers
                .authored_story_relationship_ids
                .get(&owner_identity)
                .cloned()
                .unwrap_or_default();
            if !fully_authored && partially_authored.is_empty() {
                continue;
            }
            let xml = self
                .package
                .get_part_shared(owner)
                .cloned()
                .ok_or_else(|| Error::Other(format!("story part {owner} is missing")))?;
            let referenced = xml_relationship_ids_in_order(&xml)?;
            let preserved = self
                .identifiers
                .preserved_relationship_ids
                .get(&owner_identity)
                .cloned()
                .unwrap_or_default();
            let relationships = self
                .package
                .get_part_rels(owner)
                .map(|relationships| relationships.items.clone())
                .unwrap_or_default();
            let semantic = referenced
                .into_iter()
                .filter(|id| {
                    (fully_authored && !preserved.contains(id) || partially_authored.contains(id))
                        && relationships
                            .iter()
                            .any(|relationship| relationship.id == *id)
                })
                .collect::<Vec<_>>();
            let mut occupied = preserved.clone();
            occupied.extend(
                relationships
                    .iter()
                    .filter(|relationship| !semantic.contains(&relationship.id))
                    .map(|relationship| relationship.id.clone()),
            );
            let mut cursor = if semantic.is_empty() {
                0
            } else {
                preserved
                    .iter()
                    .filter_map(|id| id.strip_prefix("rId")?.parse::<u32>().ok())
                    .max()
                    .map_or(Ok(0), |maximum| {
                        maximum.checked_add(1).ok_or_else(|| {
                            Error::Other("relationship id range is exhausted".to_owned())
                        })
                    })?
            };
            let mut remap = HashMap::new();
            for old in semantic {
                remap.insert(
                    old,
                    reserve_relationship_from_cursor(&mut occupied, &mut cursor)?,
                );
            }
            if let Some(owner_relationships) = self.package.get_part_rels_mut(owner) {
                for relationship in &mut owner_relationships.items {
                    if let Some(updated) = remap.get(&relationship.id) {
                        relationship.id.clone_from(updated);
                    }
                }
                let canonicalized_relationship_ids = if fully_authored {
                    owner_relationships
                        .items
                        .iter()
                        .filter(|relationship| !preserved.contains(&relationship.id))
                        .map(|relationship| relationship.id.clone())
                        .collect::<HashSet<_>>()
                } else {
                    remap.values().cloned().collect::<HashSet<_>>()
                };
                if !canonicalized_relationship_ids.is_empty() {
                    let mut authored = owner_relationships
                        .items
                        .iter()
                        .filter(|relationship| {
                            canonicalized_relationship_ids.contains(&relationship.id)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    authored.sort_by_key(|relationship| {
                        relationship
                            .id
                            .strip_prefix("rId")
                            .and_then(|value| value.parse::<u32>().ok())
                            .unwrap_or(u32::MAX)
                    });
                    let mut authored = authored.into_iter();
                    for relationship in &mut owner_relationships.items {
                        if canonicalized_relationship_ids.contains(&relationship.id) {
                            *relationship = authored
                                .next()
                                .expect("each authored relationship slot has a value");
                        }
                    }
                }
            }
            let mut drawing_edits = Vec::new();
            if !fully_authored {
                for (span, _) in partially_authored_drawing_slots
                    .remove(owner)
                    .unwrap_or_default()
                {
                    let id = reserve_u32(occupied_drawings, 1, "drawing")?;
                    drawing_edits.push((span.start, span.end, id.to_string().into_bytes()));
                }
            }
            let original_storage = Arc::clone(&xml);
            let mut part_changed = false;
            let mut produced_source = None;
            let xml = if let Some(template) = template.as_deref_mut() {
                let source = template.take_related_source(owner, &xml)?.ok_or_else(|| {
                    Error::Other(format!("related canonicalization lost its source: {owner}"))
                })?;
                part_changed |= drawing_edits.iter().any(|(start, end, value)| {
                    source.as_bytes().get(*start..*end) != Some(value.as_slice())
                });
                let source =
                    apply_shared_template_source_edits(template.operation, source, drawing_edits)?;
                let edits = relationship_remap_edits(source.as_bytes(), &remap, &[])?;
                part_changed |= edits.iter().any(|(start, end, value)| {
                    source.as_bytes().get(*start..*end) != Some(value.as_slice())
                });
                let source = apply_shared_template_source_edits(template.operation, source, edits)?;
                let source = if fully_authored {
                    let edits = authored_doc_pr_edits(source.as_bytes(), occupied_drawings)?;
                    part_changed |= edits.iter().any(|(start, end, value)| {
                        source.as_bytes().get(*start..*end) != Some(value.as_slice())
                    });
                    apply_shared_template_source_edits(template.operation, source, edits)?
                } else {
                    source
                };
                let xml = if part_changed {
                    let retained = template.operation.repeat_source(&source)?;
                    template.operation.finish_source(retained)?
                } else {
                    Vec::new()
                };
                produced_source = Some(source);
                xml
            } else {
                let mut xml = xml.as_ref().clone();
                drawing_edits.sort_by_key(|(start, _, _)| *start);
                for (start, end, replacement) in drawing_edits.into_iter().rev() {
                    part_changed |= xml.get(start..end) != Some(replacement.as_slice());
                    xml.splice(start..end, replacement);
                }
                let edits = relationship_remap_edits(&xml, &remap, &[])?;
                for (start, end, replacement) in edits.into_iter().rev() {
                    part_changed |= xml.get(start..end) != Some(replacement.as_slice());
                    xml.splice(start..end, replacement);
                }
                xml
            };
            if let Some(ids) = self
                .identifiers
                .authored_story_relationship_ids
                .get_mut(&owner_identity)
            {
                *ids = ids
                    .drain()
                    .map(|id| remap.get(&id).cloned().unwrap_or(id))
                    .collect();
            }
            let xml = if fully_authored && produced_source.is_none() {
                let edits = authored_doc_pr_edits(&xml, occupied_drawings)?;
                let mut xml = xml;
                for (start, end, replacement) in edits.into_iter().rev() {
                    part_changed |= xml.get(start..end) != Some(replacement.as_slice());
                    xml.splice(start..end, replacement);
                }
                xml
            } else {
                xml
            };
            if part_changed {
                self.package.set_part(owner, xml);
            } else {
                // The actual edit plans made no byte change. Keep the existing
                // immutable producer storage, not a new buffer matched by value.
                drop(xml);
                debug_assert!(
                    self.package
                        .get_part_shared(owner)
                        .is_some_and(|stored| Arc::ptr_eq(stored, &original_storage))
                );
            }
            if let (Some(template), Some(source)) = (template.as_deref_mut(), produced_source) {
                let bytes = self
                    .package
                    .get_part_shared(owner)
                    .expect("actual published related part");
                template.sources.put_related_source(owner, bytes, source);
            }
        }
        Ok(())
    }

    fn canonicalize_image_parts(&mut self, main_checked: bool) -> Result<()> {
        let mut ordered = Vec::new();
        let main_moves_nothing = main_checked
            && self
                .package
                .get_part_rels(&self.doc_part_name)
                .is_none_or(|relationships| {
                    relationships
                        .items
                        .iter()
                        .filter(|relationship| {
                            relationship.rel_type == rel_types::IMAGE
                                && relationship_is_internal(relationship)
                        })
                        .all(|relationship| {
                            self.identifiers
                                .part_is_preserved(&OpcPackage::resolve_rel_target(
                                    &self.doc_part_name,
                                    &relationship.target,
                                ))
                        })
                });
        let main_ids = if main_moves_nothing {
            Vec::new()
        } else {
            {
                let (xml, blocks) = self.canonical_source()?;
                relationship_ids_by_blocks(&xml, blocks.as_ref(), &[])?
            }
        };
        let mut owners = vec![(self.doc_part_name.clone(), main_ids)];
        for owner in self.related_story_parts() {
            let owner_identity = relationship_owner_identity(&owner);
            if self.identifiers.authored_story_parts.contains(&owner)
                || self
                    .identifiers
                    .authored_story_relationship_ids
                    .contains_key(&owner_identity)
            {
                let ids = self
                    .package
                    .get_part(&owner)
                    .map(xml_relationship_ids_in_order)
                    .transpose()?
                    .unwrap_or_default();
                owners.push((owner, ids));
            }
        }
        for (owner, ids) in &owners {
            let Some(relationships) = self.package.get_part_rels(owner) else {
                continue;
            };
            for id in ids {
                // A linked picture's relationship is External: its target is
                // an address, not a part to move.
                if let Some(relationship) = relationships.get_by_id(id)
                    && relationship.rel_type == rel_types::IMAGE
                    && relationship_is_internal(relationship)
                {
                    let part = OpcPackage::resolve_rel_target(owner, &relationship.target);
                    if !self.identifiers.part_is_preserved(&part) && !ordered.contains(&part) {
                        ordered.push(part);
                    }
                }
            }
        }
        let mut occupied = self.identifiers.preserved_part_names.clone();
        occupied.extend(
            self.package
                .parts
                .keys()
                .filter(|part| !ordered.contains(part))
                .map(|part| part_name_identity(part)),
        );
        let mut remap = HashMap::new();
        for old in ordered {
            let extension = old
                .rsplit_once('.')
                .map(|(_, extension)| extension)
                .unwrap_or("bin")
                .to_owned();
            remap.insert(
                old,
                reserve_part_from_set(&mut occupied, "/word/media", "image", &extension)?,
            );
        }
        for (owner, _) in &owners {
            if let Some(relationships) = self.package.get_part_rels_mut(owner) {
                for relationship in &mut relationships.items {
                    if relationship.rel_type == rel_types::IMAGE
                        && relationship_is_internal(relationship)
                    {
                        let old = OpcPackage::resolve_rel_target(owner, &relationship.target);
                        if let Some(updated) = remap.get(&old) {
                            relationship.target = relative_descendant_target(owner, updated);
                        }
                    }
                }
            }
        }
        let mut moved = Vec::new();
        for (old, updated) in remap {
            if old != updated
                && let Some(bytes) = self.package.remove_part(&old)
            {
                moved.push((updated.clone(), bytes));
                if let Some(content_type) = self.package.content_types.remove_override(&old) {
                    self.package
                        .content_types
                        .add_override(&updated, &content_type);
                }
            }
        }
        for (part_name, bytes) in moved {
            self.package.set_part(&part_name, bytes);
        }
        Ok(())
    }
}

fn relative_descendant_target(source_part: &str, target_part: &str) -> String {
    let directory = source_part
        .rfind('/')
        .map_or("/", |position| &source_part[..=position]);
    target_part
        .strip_prefix(directory)
        .unwrap_or(target_part)
        .to_owned()
}
