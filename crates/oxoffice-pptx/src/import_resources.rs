//! Slide import: the layouts, masters, master defaults, fonts, comment authors and main relationships an imported slide brings along.

use super::*;

const CLASSIC_COMMENT_AUTHORS_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/commentAuthors";

impl Presentation {
    pub(super) fn adopt_source_layout(
        &mut self,
        source_package: &OpcPackage,
        layout: &str,
        adopted: &mut HashMap<String, String>,
    ) -> Result<String> {
        if let Some(existing) = adopted.get(layout) {
            return Ok(existing.clone());
        }
        // A layout of the same name comes first, so that a copy within one
        // presentation keeps its own layout even when another is equal.
        let (same_name, others): (Vec<_>, Vec<_>) = self
            .layouts
            .iter()
            .partition(|record| record.part_name == layout);
        for record in same_name.into_iter().chain(others) {
            if formatting_parts_equal(source_package, layout, &self.package, &record.part_name)? {
                adopted.insert(layout.to_owned(), record.part_name.clone());
                return Ok(record.part_name.clone());
            }
        }
        let source_master = related_internal_part(source_package, layout, rel_types::SLIDE_MASTER)?
            .ok_or_else(|| Error::MalformedPart {
                part_name: layout.to_owned(),
                message: "layout has no slide master relationship".to_owned(),
            })?;
        let master = self.adopt_source_master(source_package, &source_master, adopted)?;

        let bytes = required_part(source_package, layout)?.to_vec();
        let copied = CT_SlideLayout::from_xml(&bytes).map_err(|error| Error::MalformedPart {
            part_name: layout.to_owned(),
            message: error.to_string(),
        })?;
        let part_name = fresh_related_part_name(&self.package, layout);
        self.package.set_part(&part_name, bytes);
        self.package
            .content_types
            .add_override(&part_name, content_types::SLIDE_LAYOUT);
        let relationships =
            self.copy_formatting_relationships(source_package, layout, &part_name, Some(&master))?;
        self.package.set_part_rels(&part_name, relationships);

        let id = self.next_master_or_layout_id()?;
        let mut master_relationships = self
            .package
            .get_part_rels(&master)
            .cloned()
            .unwrap_or_default();
        let relationship_id = master_relationships.add(
            rel_types::SLIDE_LAYOUT,
            &relative_part_target(&master, &part_name),
        );
        let master_relationships = master_relationships
            .rebind_for_import(master_relationships.items.clone(), &HashMap::new())?;
        self.package.set_part_rels(&master, master_relationships);
        let malformed = |error: OxmlError| Error::MalformedPart {
            part_name: master.clone(),
            message: error.to_string(),
        };
        let master_xml = required_part(&self.package, &master)?;
        let mut model = CT_SlideMaster::from_xml(master_xml).map_err(malformed)?;
        let master_xml = model
            .append_slide_layout_id(master_xml, id, &relationship_id)
            .map_err(malformed)?;
        self.package.set_part(&master, master_xml);
        self.layouts.push(Arc::new(LayoutRecord {
            part_name: part_name.clone(),
            layout: copied,
            dirty: false,
        }));
        adopted.insert(layout.to_owned(), part_name.clone());
        Ok(part_name)
    }

    /// Returns this presentation's master equal to the source master, or a
    /// copy of it, with a copy of its theme and none of its layouts.
    fn adopt_source_master(
        &mut self,
        source_package: &OpcPackage,
        master: &str,
        adopted: &mut HashMap<String, String>,
    ) -> Result<String> {
        if let Some(existing) = adopted.get(master) {
            return Ok(existing.clone());
        }
        for part_name in self.master_parts() {
            if formatting_parts_equal(source_package, master, &self.package, &part_name)? {
                adopted.insert(master.to_owned(), part_name.clone());
                return Ok(part_name);
            }
        }
        let malformed = |error: OxmlError| Error::MalformedPart {
            part_name: master.to_owned(),
            message: error.to_string(),
        };
        let mut bytes = required_part(source_package, master)?.to_vec();
        let mut model = CT_SlideMaster::from_xml(&bytes).map_err(malformed)?;
        for (_, relationship) in model.slide_layout_ids().map_err(malformed)? {
            bytes = model
                .remove_slide_layout_id(&bytes, &relationship)
                .map_err(malformed)?;
        }
        let part_name = fresh_related_part_name(&self.package, master);
        self.package.set_part(&part_name, bytes);
        self.package
            .content_types
            .add_override(&part_name, content_types::SLIDE_MASTER);
        let relationships =
            self.copy_formatting_relationships(source_package, master, &part_name, None)?;
        self.package.set_part_rels(&part_name, relationships);
        let id = self.next_master_or_layout_id()?;
        let relationship_id = self
            .package
            .get_or_create_part_rels(&self.presentation_part)
            .add(
                rel_types::SLIDE_MASTER,
                &relative_part_target(&self.presentation_part, &part_name),
            );
        self.presentation
            .slide_master_ids
            .push(CT_SlideMasterId::new(id, relationship_id));
        adopted.insert(master.to_owned(), part_name.clone());
        Ok(part_name)
    }

    /// Copies the relationships of a source layout, or of a source master
    /// when `master` is `None`, keeping their ids.
    ///
    /// A layout relates to `master`. A master drops its layouts, which
    /// [`Self::adopt_source_layout`] adds one by one, and gets a copy of its
    /// theme. Images go through the media store, tags and legacy drawings
    /// are copied, and any other internal relationship is refused.
    fn copy_formatting_relationships(
        &mut self,
        source_package: &OpcPackage,
        source_part: &str,
        destination_part: &str,
        master: Option<&str>,
    ) -> Result<Relationships> {
        let mut relationships = Relationships::new();
        let mut copies = HashMap::new();
        let source_relationships = source_package
            .get_part_rels(source_part)
            .cloned()
            .unwrap_or_default();
        for relationship in &source_relationships.items {
            let target = if relationship_is_external(relationship) {
                relationship.target.clone()
            } else {
                let resolved = OpcPackage::resolve_rel_target(source_part, &relationship.target);
                let copied = match (relationship.rel_type.as_str(), master) {
                    (rel_types::SLIDE_LAYOUT, None) => continue,
                    (rel_types::SLIDE_MASTER, Some(master)) => master.to_owned(),
                    (rel_types::IMAGE, _) => {
                        preflight_internal_transfer_image(
                            source_package,
                            &resolved,
                            "import slide",
                        )?;
                        let bytes = required_part(source_package, &resolved)?.to_vec();
                        self.media_store
                            .insert(&mut self.package, &bytes, &resolved)
                    }
                    (rel_type @ (TAGS_RELATIONSHIP | VML_DRAWING_RELATIONSHIP), _)
                    | (rel_type @ rel_types::THEME, None) => {
                        preflight_carried_part_graph(
                            source_package,
                            &resolved,
                            rel_type,
                            &mut HashSet::new(),
                        )?;
                        duplicate_part_graph(
                            source_package,
                            &mut self.package,
                            &mut self.media_store,
                            &resolved,
                            &mut copies,
                        )?
                    }
                    (rel_type, _) => {
                        return Err(invalid_presentation_mutation(
                            "import slide",
                            format!(
                                "{source_part} has an internal relationship of type {rel_type}, which is not carried"
                            ),
                        ));
                    }
                };
                relative_part_target(destination_part, &copied)
            };
            relationships.add_with_id(&relationship.id, &relationship.rel_type, &target);
            if let Some(inserted) = relationships.items.last_mut() {
                inserted.target_mode = relationship.target_mode.clone();
            }
        }
        Ok(source_relationships.rebind_for_import(relationships.items, &HashMap::new())?)
    }

    /// The master parts that the presentation's master list names.
    fn master_parts(&self) -> Vec<String> {
        let relationships = self.package.get_part_rels(&self.presentation_part);
        self.presentation
            .slide_master_ids
            .iter()
            .filter_map(|master| {
                relationships
                    .and_then(|relationships| relationships.get_by_id(&master.relationship_id))
                    .filter(|relationship| !relationship_is_external(relationship))
                    .map(|relationship| {
                        OpcPackage::resolve_rel_target(
                            &self.presentation_part,
                            &relationship.target,
                        )
                    })
            })
            .collect()
    }

    /// Returns an id above every master and layout id, which share one space
    /// starting at 2147483648.
    fn next_master_or_layout_id(&self) -> Result<u32> {
        let mut highest = 2_147_483_647_u32;
        for master in &self.presentation.slide_master_ids {
            highest = highest.max(master.id.unwrap_or_default());
        }
        for part_name in self.master_parts() {
            let ids =
                master_layout_ids(required_part(&self.package, &part_name)?).map_err(|error| {
                    Error::MalformedPart {
                        part_name: part_name.clone(),
                        message: error.to_string(),
                    }
                })?;
            for (id, _) in ids.entries {
                highest = highest.max(id);
            }
        }
        highest.checked_add(1).ok_or_else(|| {
            invalid_presentation_mutation(
                "import slide",
                "master and layout ids are exhausted".to_owned(),
            )
        })
    }
}

impl Presentation {
    /// Publishes selected presentation-owned edge metadata in one original source scope.
    pub(super) fn reconcile_imported_main_relationships(
        &mut self,
        source: &Presentation,
        package: &OpcPackage,
        targets: &HashMap<String, String>,
        original_ids: &HashSet<String>,
    ) -> Result<()> {
        let original = package
            .get_part_rels(&source.presentation_part)
            .cloned()
            .unwrap_or_default();
        // Stage newly authored records against their retained physical scope
        // before selected IDs are rebound. This keeps opaque original metadata.
        let original = original.rebind_for_import(original.items.clone(), &HashMap::new())?;
        let destination = self
            .package
            .get_part_rels(&self.presentation_part)
            .cloned()
            .unwrap_or_default();
        let mut ids = original
            .items
            .iter()
            .map(|item| (item.id.clone(), String::new()))
            .collect::<HashMap<_, _>>();
        let mut selected = Vec::new();
        let mut replacing = HashSet::new();
        let mut attributed = HashSet::new();
        for relationship in &original.items {
            if !matches!(
                relationship.rel_type.as_str(),
                rel_types::SLIDE
                    | rel_types::SLIDE_MASTER
                    | rel_types::FONT
                    | rel_types::TABLE_STYLES
                    | rel_types::POWERPOINT_AUTHORS
                    | CLASSIC_COMMENT_AUTHORS_RELATIONSHIP
            ) {
                continue;
            }
            reject_external(&source.presentation_part, relationship)?;
            let source_target =
                OpcPackage::resolve_rel_target(&source.presentation_part, &relationship.target);
            let Some(target) = targets.get(&source_target) else {
                continue;
            };
            let matching = destination
                .items
                .iter()
                .filter(|item| {
                    item.rel_type == relationship.rel_type
                        && !relationship_is_external(item)
                        && OpcPackage::resolve_rel_target(&self.presentation_part, &item.target)
                            == *target
                })
                .collect::<Vec<_>>();
            let item = matching
                .iter()
                .find(|item| !attributed.contains(&item.id))
                .copied()
                .or_else(|| matching.first().copied())
                .ok_or_else(|| {
                    footer_malformed(
                        &self.presentation_part,
                        "imported global dependency has no destination relationship",
                    )
                })?;
            if original_ids.contains(&item.id) || attributed.contains(&item.id) {
                if !original.record_metadata_equal_for_import(
                    &relationship.id,
                    &destination,
                    &item.id,
                )? {
                    return Err(footer_malformed(
                        &self.presentation_part,
                        "conflicting source global relationship metadata",
                    ));
                }
                continue;
            }
            attributed.insert(item.id.clone());
            replacing.insert(item.id.clone());
            ids.insert(relationship.id.clone(), item.id.clone());
            let mut copy = relationship.clone();
            copy.id.clone_from(&item.id);
            copy.target.clone_from(&item.target);
            selected.push(copy);
        }
        let imported = original.rebind_for_import(selected, &ids)?;
        let kept = destination
            .items
            .iter()
            .filter(|item| !replacing.contains(&item.id))
            .cloned()
            .collect();
        let destination = destination.rebind_for_import(kept, &HashMap::new())?;
        let (merged, mappings) = destination.merge_for_import(&imported)?;
        if mappings.iter().any(|(old, new)| old != new) {
            return Err(footer_malformed(
                &self.presentation_part,
                "preallocated global dependency relationship collision",
            ));
        }
        self.package.set_part_rels(&self.presentation_part, merged);
        Ok(())
    }

    /// Copies active stored programs required by selected text and theme graphs.
    /// Face collisions are checked before publishing the staged presentation.
    pub(super) fn import_embedded_font_dependencies(
        &mut self,
        source: &Presentation,
        source_package: &OpcPackage,
        names: &HashSet<String>,
        copies: &mut HashMap<String, String>,
    ) -> Result<()> {
        let source_active = source
            .presentation
            .embed_true_type_fonts()
            .map_err(font_model_error)?
            == Some(true);
        let destination_active = self
            .presentation
            .embed_true_type_fonts()
            .map_err(font_model_error)?
            == Some(true);
        if !source_active {
            if destination_active
                && self
                    .presentation
                    .embedded_fonts()
                    .any(|entry| names.contains(&entry.typeface().to_lowercase()))
            {
                return Err(Error::InvalidFontEmbedding("inactive source font appearance conflicts with destination active stored program".into()));
            }
            return Ok(());
        }
        if !destination_active && self.presentation.embedded_fonts().next().is_some() {
            return Err(Error::InvalidFontEmbedding(
                "activating imported fonts would activate unrelated inactive destination programs"
                    .into(),
            ));
        }
        let source_entries = source
            .presentation
            .embedded_fonts()
            .filter(|entry| names.contains(&entry.typeface().to_lowercase()))
            .cloned()
            .collect::<Vec<_>>();
        let source_relationships = source_package
            .get_part_rels(&source.presentation_part)
            .cloned()
            .unwrap_or_default();
        let destination_relationships = self
            .package
            .get_part_rels(&self.presentation_part)
            .cloned()
            .unwrap_or_default();
        let mut reserved = destination_relationships.clone();
        let mut new_relationships = Vec::new();
        let mut source_ids: HashMap<String, String> = HashMap::new();
        let mut transferred = false;
        let mut source_model = source.presentation.clone();
        for entry in source_entries {
            source_model
                .embedded_font_mut(entry.typeface())
                .map_err(font_model_error)?;
            let matching = self
                .presentation
                .embedded_fonts()
                .filter(|old| old.typeface().to_lowercase() == entry.typeface().to_lowercase())
                .cloned()
                .collect::<Vec<_>>();
            if matching.len() > 1 {
                return Err(Error::InvalidFontEmbedding(format!(
                    "ambiguous destination imported font {}",
                    entry.typeface()
                )));
            }
            let existing = matching.first().cloned();
            let mut copy = existing.clone().unwrap_or_else(|| entry.clone());
            for (kind, id) in entry.faces() {
                let source_part = source.checked_font_target(id)?;
                let bytes = required_part(source_package, &source_part)?;
                if let Some(owned) = source_package
                    .get_part_rels(&source_part)
                    .and_then(|rels| rels.items.iter().find(|rel| !relationship_is_external(rel)))
                {
                    return Err(Error::InvalidFontEmbedding(format!(
                        "{source_part} has unsupported font-data relationship {}",
                        owned.rel_type
                    )));
                }
                if let Some(existing_id) = existing.as_ref().and_then(|entry| entry.face(kind)) {
                    let destination_part = self.checked_font_target(existing_id)?;
                    if !source_relationships.record_metadata_equal_for_import(
                        id,
                        &destination_relationships,
                        existing_id,
                    )? || self.package.get_part(&destination_part) != Some(bytes)
                        || self
                            .package
                            .get_part_rels(&destination_part)
                            .map(Relationships::to_xml)
                            .transpose()?
                            != source_package
                                .get_part_rels(&source_part)
                                .map(Relationships::to_xml)
                                .transpose()?
                    {
                        return Err(Error::InvalidFontEmbedding(format!(
                            "imported font {} face {kind:?} conflicts with destination stored program",
                            entry.typeface()
                        )));
                    }
                    copies.insert(source_part.clone(), destination_part);
                    copy.import_face(&entry, kind, existing_id)
                        .map_err(font_model_error)?;
                    transferred = true;
                    continue;
                }
                let destination_id = if let Some(id) = source_ids.get(id) {
                    id.clone()
                } else {
                    let copied = if let Some(part) = copies.get(&source_part) {
                        part.clone()
                    } else {
                        let part = fresh_related_part_name(&self.package, &source_part);
                        self.package.set_part(&part, bytes.to_vec());
                        self.package
                            .content_types
                            .add_override(&part, EMBEDDED_FONT_CONTENT_TYPE);
                        if let Some(relationships) = source_package.get_part_rels(&source_part) {
                            self.package.set_part_rels(&part, relationships.clone());
                        }
                        copies.insert(source_part.clone(), part.clone());
                        part
                    };
                    let destination_id = reserved.add(
                        rel_types::FONT,
                        &relative_part_target(&self.presentation_part, &copied),
                    );
                    let mut relationship = source_relationships
                        .get_by_id(id)
                        .expect("checked source font relationship")
                        .clone();
                    relationship.id.clone_from(&destination_id);
                    relationship.target = relative_part_target(&self.presentation_part, &copied);
                    new_relationships.push(relationship);
                    source_ids.insert(id.to_owned(), destination_id.clone());
                    destination_id
                };
                if existing.is_some() {
                    copy.import_face(&entry, kind, &destination_id)
                        .map_err(font_model_error)?;
                } else {
                    copy.set_face(kind, &destination_id)
                        .map_err(font_model_error)?;
                }
                transferred = true;
            }
            if copy != existing.clone().unwrap_or_else(|| entry.clone()) || existing.is_none() {
                if let Some(old) = existing {
                    *self
                        .presentation
                        .embedded_font_mut(old.typeface())
                        .map_err(font_model_error)?
                        .expect("matched font entry") = copy;
                } else {
                    self.presentation
                        .push_embedded_font(copy)
                        .map_err(font_model_error)?;
                }
            }
        }
        if !new_relationships.is_empty() {
            let relationships =
                destination_relationships.rebind_for_import(reserved.items, &HashMap::new())?;
            self.package
                .set_part_rels(&self.presentation_part, relationships);
        }
        if transferred {
            if self
                .presentation
                .embed_true_type_fonts()
                .map_err(font_model_error)?
                == Some(false)
            {
                return Err(Error::InvalidFontEmbedding(
                    "destination disables retaining imported embedded fonts".into(),
                ));
            }
            self.presentation.set_embed_true_type_fonts(Some(true));
        }
        Ok(())
    }

    pub(super) fn import_classic_comment_graphs(
        &mut self,
        source_slides: &[Arc<SlideRecord>],
        source_presentation_part: &str,
        source_package: &OpcPackage,
        indexes: &[usize],
        copies: &mut HashMap<String, String>,
    ) -> Result<()> {
        let mut source_parts = Vec::new();
        let mut seen = HashSet::new();
        for &index in indexes {
            let part = &source_slides[index].part_name;
            let Some(relationships) = source_package.get_part_rels(part) else {
                continue;
            };
            let comments = relationships.get_all_by_type(rel_types::COMMENTS);
            if comments.len() > 1 {
                return Err(Error::DuplicateRelationships {
                    source_part: part.clone(),
                    relationship_type: rel_types::COMMENTS,
                    count: comments.len(),
                });
            }
            for relationship in comments {
                reject_external(part, relationship)?;
                let comment_part = OpcPackage::resolve_rel_target(part, &relationship.target);
                if seen.insert(comment_part.clone()) {
                    source_parts.push((
                        comment_part.clone(),
                        required_part(source_package, &comment_part)?.to_vec(),
                    ));
                }
            }
        }
        if source_parts.is_empty() {
            return Ok(());
        }
        let mut all_parts = Vec::new();
        let mut seen = HashSet::new();
        for record in source_slides {
            if let Some(relationships) = source_package.get_part_rels(&record.part_name) {
                for relationship in relationships.get_all_by_type(rel_types::COMMENTS) {
                    reject_external(&record.part_name, relationship)?;
                    let part =
                        OpcPackage::resolve_rel_target(&record.part_name, &relationship.target);
                    if seen.insert(part.clone()) {
                        all_parts
                            .push((part.clone(), required_part(source_package, &part)?.to_vec()));
                    }
                }
            }
        }
        let dependencies =
            classic_comment_dependencies(&source_parts, &all_parts).map_err(|error| {
                invalid_presentation_mutation("import classic comments", error.to_string())
            })?;
        let mut imported_relationships = HashMap::new();
        for (part, xml) in &mut source_parts {
            let mut relationships = source_package
                .get_part_rels(part)
                .cloned()
                .unwrap_or_default();
            let mut scopes: HashMap<String, HashMap<String, String>> = HashMap::new();
            for dependency in dependencies.get(part).into_iter().flatten() {
                let ids = if let Some(ids) = scopes.get(&dependency.source_part) {
                    ids.clone()
                } else {
                    let origin = source_package
                        .get_part_rels(&dependency.source_part)
                        .cloned()
                        .unwrap_or_default();
                    if origin
                        .items
                        .iter()
                        .any(|relationship| !relationship_is_external(relationship))
                    {
                        return Err(invalid_presentation_mutation(
                            "import classic comments",
                            format!(
                                "{} parent scope has unsupported internal relationships",
                                dependency.source_part
                            ),
                        ));
                    }
                    let (merged, ids) = relationships.merge_for_import(&origin)?;
                    relationships = merged;
                    scopes.insert(dependency.source_part.clone(), ids.clone());
                    ids
                };
                let parent = rewrite_exact_rel_ids(&dependency.xml, &ids).map_err(|error| {
                    invalid_presentation_mutation("import classic comments", error.to_string())
                })?;
                *xml = append_classic_comment_dependency(xml, &parent).map_err(|error| {
                    invalid_presentation_mutation("import classic comments", error.to_string())
                })?;
            }
            imported_relationships.insert(part.clone(), relationships);
        }
        for (package, owner) in [
            (source_package, source_presentation_part),
            (&self.package, self.presentation_part.as_str()),
        ] {
            let count = package.get_part_rels(owner).map_or(0, |relationships| {
                relationships
                    .get_all_by_type(CLASSIC_COMMENT_AUTHORS_RELATIONSHIP)
                    .len()
            });
            if count > 1 {
                return Err(Error::DuplicateRelationships {
                    source_part: owner.to_owned(),
                    relationship_type: CLASSIC_COMMENT_AUTHORS_RELATIONSHIP,
                    count,
                });
            }
        }
        let source_authors = related_internal_part(
            source_package,
            source_presentation_part,
            CLASSIC_COMMENT_AUTHORS_RELATIONSHIP,
        )?
        .ok_or_else(|| Error::MalformedPart {
            part_name: source_presentation_part.to_owned(),
            message: "classic comments have no commentAuthors relationship".into(),
        })?;
        let destination_authors = related_internal_part(
            &self.package,
            &self.presentation_part,
            CLASSIC_COMMENT_AUTHORS_RELATIONSHIP,
        )?;
        let mut destination_parts = Vec::new();
        let mut seen = HashSet::new();
        for record in &self.slides {
            if let Some(relationships) = self.package.get_part_rels(&record.part_name) {
                for relationship in relationships.get_all_by_type(rel_types::COMMENTS) {
                    reject_external(&record.part_name, relationship)?;
                    let part =
                        OpcPackage::resolve_rel_target(&record.part_name, &relationship.target);
                    if seen.insert(part.clone()) {
                        destination_parts.push(required_part(&self.package, &part)?.to_vec());
                    }
                }
            }
        }
        let imported = import_classic_comments(
            required_part(source_package, &source_authors)?,
            destination_authors
                .as_deref()
                .map(|part| required_part(&self.package, part))
                .transpose()?,
            &source_parts,
            &destination_parts,
        )
        .map_err(|error| Error::MalformedPart {
            part_name: source_authors.clone(),
            message: error.to_string(),
        })?;
        let author_part = destination_authors
            .unwrap_or_else(|| fresh_related_part_name(&self.package, &source_authors));
        copies.insert(source_authors.clone(), author_part.clone());
        self.package.set_part(&author_part, imported.authors_xml);
        let author_type = source_package
            .content_types
            .content_type_for(&source_authors)
            .ok_or_else(|| Error::MalformedPart {
                part_name: source_authors.clone(),
                message: "classic authors have no content type".into(),
            })?;
        self.package
            .content_types
            .add_override(&author_part, author_type);
        let relationships = self
            .package
            .get_or_create_part_rels(&self.presentation_part);
        if relationships
            .get_by_type(CLASSIC_COMMENT_AUTHORS_RELATIONSHIP)
            .is_none()
        {
            relationships.add(
                CLASSIC_COMMENT_AUTHORS_RELATIONSHIP,
                &relative_part_target(&self.presentation_part, &author_part),
            );
        }
        for (source_part, _) in source_parts {
            let part = fresh_related_part_name(&self.package, &source_part);
            let content_type = source_package
                .content_types
                .content_type_for(&source_part)
                .ok_or_else(|| Error::MalformedPart {
                    part_name: source_part.clone(),
                    message: "classic comments have no content type".into(),
                })?;
            self.package
                .set_part(&part, imported.comment_parts[&source_part].clone());
            self.package.content_types.add_override(&part, content_type);
            if source_package.get_part_rels(&source_part).is_some()
                || dependencies.contains_key(&source_part)
            {
                self.package.set_part_rels(
                    &part,
                    imported_relationships
                        .remove(&source_part)
                        .expect("selected classic scope prepared"),
                );
            }
            copies.insert(source_part, part);
        }
        Ok(())
    }

    /// Merges modern authors and returns comment/reply and author ID maps.
    pub(super) fn merge_imported_comment_identities(
        &mut self,
        operation: &'static str,
        source_authors: &[CommentAuthor],
        comments: &CommentList,
    ) -> Result<(HashMap<String, String>, HashMap<String, String>)> {
        let mut used = self
            .comment_authors
            .authors
            .iter()
            .map(|author| author.id.to_ascii_uppercase())
            .collect::<HashSet<_>>();
        for record in &self.slides {
            for comment in record
                .comments
                .iter()
                .flat_map(|comments| &comments.comments.comments)
            {
                used.insert(comment.id.to_ascii_uppercase());
                used.extend(
                    comment
                        .replies()
                        .iter()
                        .map(|reply| reply.id.to_ascii_uppercase()),
                );
            }
        }
        let fresh_id = |used: &mut HashSet<String>| {
            let id = (1_u64..)
                .map(|index| format!("{{00000000-0000-4000-8000-{index:012X}}}"))
                .find(|id| !used.contains(id))
                .expect("a finite set of used ids leaves a sequential id free");
            used.insert(id.clone());
            id
        };

        let mut author_ids = HashMap::new();
        let referenced = comments
            .referenced_author_ids()
            .map_err(|error| invalid_presentation_mutation(operation, error.to_string()))?;
        for author_id in referenced {
            if author_ids.contains_key(&author_id) {
                continue;
            }
            let Some(author) = source_authors.iter().find(|author| author.id == author_id) else {
                return Err(invalid_presentation_mutation(
                    operation,
                    format!("modern comment author {author_id} is absent"),
                ));
            };
            // The same author, id included, comes first, so that two equal
            // authors with different ids each keep their own.
            let same = self
                .comment_authors
                .authors
                .iter()
                .find(|existing| *existing == author);
            if let Some(existing) = same.or_else(|| {
                self.comment_authors.authors.iter().find(|existing| {
                    let mut identity = (**existing).clone();
                    identity.id.clone_from(&author.id);
                    &identity == author
                })
            }) {
                author_ids.insert(author_id.clone(), existing.id.clone());
                continue;
            }
            let mut copy = author.clone();
            if used.contains(&author.id.to_ascii_uppercase()) {
                copy.id = fresh_id(&mut used);
            } else {
                used.insert(author.id.to_ascii_uppercase());
            }
            author_ids.insert(author_id.clone(), copy.id.clone());
            self.ensure_comment_authors_part()?;
            self.comment_authors.authors.push(copy);
            self.comment_authors_dirty = true;
        }

        let mut ids = HashMap::new();
        for comment in &comments.comments {
            for id in
                std::iter::once(&comment.id).chain(comment.replies().iter().map(|reply| &reply.id))
            {
                if used.contains(&id.to_ascii_uppercase()) {
                    ids.insert(id.clone(), fresh_id(&mut used));
                } else {
                    used.insert(id.to_ascii_uppercase());
                }
            }
        }
        Ok((ids, author_ids))
    }
}

/// Returns whether a source layout or master equals a destination one, so
/// that a keep-source-formatting import may reuse it.
///
/// Two parts are equal when their bytes are equal and they own the same
/// relationship ids, with the same types and external targets, and internal
/// targets equal by this same rule. A master is compared without its
/// `p:sldLayoutIdLst` and its layout relationships, since a copied master
/// only lists the layouts imported so far. The rule is deliberately strict.
/// An equal-looking part saved by another producer, or with other
/// relationship ids, is not equal and gets copied.
fn formatting_parts_equal(
    source_package: &OpcPackage,
    source_part: &str,
    destination_package: &OpcPackage,
    destination_part: &str,
) -> Result<bool> {
    fn equal(
        source: &OpcPackage,
        left: &str,
        destination: &OpcPackage,
        right: &str,
        visited: &mut HashSet<(String, String)>,
    ) -> Result<bool> {
        if !visited.insert((left.to_owned(), right.to_owned())) {
            return Ok(true);
        }
        if source.content_types.content_type_for(left)
            != destination.content_types.content_type_for(right)
        {
            return Ok(false);
        }
        let master =
            source.content_types.content_type_for(left) == Some(content_types::SLIDE_MASTER);
        let (Some(a), Some(b)) = (source.get_part(left), destination.get_part(right)) else {
            return Ok(false);
        };
        let a = if master {
            replace_master_layout_ids(a, &[])
                .map_err(|error| footer_malformed(left, error.to_string()))?
        } else {
            a.to_vec()
        };
        let b = if master {
            replace_master_layout_ids(b, &[])
                .map_err(|error| footer_malformed(right, error.to_string()))?
        } else {
            b.to_vec()
        };
        let edges = |package: &OpcPackage, part: &str| {
            package
                .get_part_rels(part)
                .map(|r| {
                    r.items
                        .iter()
                        .filter(|r| !(master && r.rel_type == rel_types::SLIDE_LAYOUT))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let a_edges = edges(source, left);
        let b_edges = edges(destination, right);
        if a_edges.len() != b_edges.len() {
            return Ok(false);
        }
        let source_scope = source.get_part_rels(left).cloned().unwrap_or_default();
        let destination_scope = destination
            .get_part_rels(right)
            .cloned()
            .unwrap_or_default();
        if a_edges.is_empty() {
            return Ok(a == b
                && source_scope
                    .rebind_for_import(a_edges, &HashMap::new())?
                    .to_xml()?
                    == destination_scope
                        .rebind_for_import(b_edges, &HashMap::new())?
                        .to_xml()?);
        }
        let a_ids =
            relationship_ids(&a).map_err(|error| footer_malformed(left, error.to_string()))?;
        let b_ids =
            relationship_ids(&b).map_err(|error| footer_malformed(right, error.to_string()))?;
        if a_ids.len() != b_ids.len() {
            return Ok(false);
        }
        let mut aliases = HashMap::new();
        let mut inverse = HashMap::new();
        for (a, b) in a_ids.iter().zip(&b_ids) {
            if aliases.get(a).is_some_and(|old| old != b)
                || inverse.get(b).is_some_and(|old| old != a)
            {
                return Ok(false);
            }
            aliases.insert(a.clone(), b.clone());
            inverse.insert(b.clone(), a.clone());
        }
        if rewrite_exact_rel_ids(&a, &aliases)
            .map_err(|error| footer_malformed(left, error.to_string()))?
            != b
        {
            return Ok(false);
        }
        let mut used = HashSet::new();
        let mut normalized_edges = Vec::with_capacity(a_edges.len());
        let mut normalized_ids = HashMap::new();
        for edge in a_edges {
            let mut matched = false;
            for candidate in &b_edges {
                if used.contains(&candidate.id)
                    || aliases.get(&edge.id).is_some_and(|id| id != &candidate.id)
                    || inverse.get(&candidate.id).is_some_and(|id| id != &edge.id)
                    || edge.rel_type != candidate.rel_type
                    || edge.target_mode != candidate.target_mode
                {
                    continue;
                }
                let mut trial = visited.clone();
                let same = if relationship_is_external(&edge) {
                    edge.target == candidate.target
                } else {
                    equal(
                        source,
                        &OpcPackage::resolve_rel_target(left, &edge.target),
                        destination,
                        &OpcPackage::resolve_rel_target(right, &candidate.target),
                        &mut trial,
                    )?
                };
                if same {
                    *visited = trial;
                    used.insert(candidate.id.clone());
                    normalized_ids.insert(edge.id.clone(), candidate.id.clone());
                    let mut normalized = edge.clone();
                    normalized.id = candidate.id.clone();
                    normalized.target = candidate.target.clone();
                    normalized_edges.push(normalized);
                    matched = true;
                    break;
                }
            }
            if !matched {
                return Ok(false);
            }
        }
        Ok(source_scope
            .rebind_for_import(normalized_edges, &normalized_ids)?
            .to_xml()?
            == destination_scope
                .rebind_for_import(b_edges, &HashMap::new())?
                .to_xml()?)
    }
    equal(
        source_package,
        source_part,
        destination_package,
        destination_part,
        &mut HashSet::new(),
    )
}

/// The `p:sldLayoutIdLst` of one slide master.
struct MasterLayoutIds {
    /// The byte range of the list, or the empty range after `p:clrMap` where
    /// a list belongs when the master has none.
    range: std::ops::Range<usize>,
    /// The `(id, relationship id)` of each `p:sldLayoutId`.
    entries: Vec<(u32, String)>,
}

fn master_layout_ids(xml: &[u8]) -> std::result::Result<MasterLayoutIds, OxmlError> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut list_start = None;
    let mut list = None;
    let mut after_color_map = None;
    let mut entries = Vec::new();
    loop {
        let start_offset = reader.buffer_position() as usize;
        let (is_p, event) = {
            let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
            (xml_is(&namespace, P_NS), event.into_owned())
        };
        let end_offset = reader.buffer_position() as usize;
        match event {
            XmlEvent::Start(start) => {
                depth += 1;
                if depth == 2 && is_p && start.local_name().as_ref() == b"sldLayoutIdLst" {
                    list_start = Some(start_offset);
                }
            }
            XmlEvent::Empty(start) => {
                let local = start.local_name();
                if depth == 1 && is_p && local.as_ref() == b"sldLayoutIdLst" {
                    list = Some(start_offset..end_offset);
                } else if depth == 1 && is_p && local.as_ref() == b"clrMap" {
                    after_color_map = Some(end_offset);
                } else if depth == 2
                    && list_start.is_some()
                    && is_p
                    && local.as_ref() == b"sldLayoutId"
                {
                    let mut id = None;
                    let mut relationship_id = None;
                    for attribute in start.attributes().with_checks(false) {
                        let attribute = attribute?;
                        let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
                            .into_owned();
                        if name.as_ref() != b"id" {
                            continue;
                        }
                        if xml_is(&namespace, R_NS) {
                            relationship_id = Some(value);
                        } else if matches!(namespace, XmlResolveResult::Unbound) {
                            id = value.parse::<u32>().ok();
                        }
                    }
                    if let (Some(id), Some(relationship_id)) = (id, relationship_id) {
                        entries.push((id, relationship_id));
                    }
                }
            }
            XmlEvent::End(end) => {
                if depth == 2 && is_p && end.local_name().as_ref() == b"sldLayoutIdLst" {
                    list = list_start.take().map(|start| start..end_offset);
                } else if depth == 2 && is_p && end.local_name().as_ref() == b"clrMap" {
                    after_color_map = Some(end_offset);
                }
                depth = depth.saturating_sub(1);
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let range = match (list, after_color_map) {
        (Some(range), _) => range,
        (None, Some(offset)) => offset..offset,
        (None, None) => return Err(OxmlError::MissingElement("p:clrMap".to_owned())),
    };
    Ok(MasterLayoutIds { range, entries })
}

/// Replaces the layout list of a slide master with `entries`, or removes it
/// when `entries` is empty.
fn replace_master_layout_ids(
    xml: &[u8],
    entries: &[(u32, String)],
) -> std::result::Result<Vec<u8>, OxmlError> {
    let MasterLayoutIds { range, .. } = master_layout_ids(xml)?;
    let mut replaced = xml[..range.start].to_vec();
    if !entries.is_empty() {
        // The root of a master written by PowerPoint binds both prefixes,
        // and any other root gets them declared on the list itself.
        let root_start = xml
            .windows(2)
            .position(|pair| pair[0] == b'<' && pair[1].is_ascii_alphabetic())
            .unwrap_or_default();
        let root_end = xml[root_start..]
            .iter()
            .position(|byte| *byte == b'>')
            .map_or(xml.len(), |end| root_start + end + 1);
        let root = &xml[root_start..root_end];
        let mut declarations = String::new();
        for (prefix, namespace) in [("p", P_NS), ("r", R_NS)] {
            let declaration = format!(" xmlns:{prefix}=\"{namespace}\"");
            if !root
                .windows(declaration.len())
                .any(|window| window == declaration.as_bytes())
            {
                declarations.push_str(&declaration);
            }
        }
        replaced.extend_from_slice(format!("<p:sldLayoutIdLst{declarations}>").as_bytes());
        for (id, relationship_id) in entries {
            replaced.extend_from_slice(
                format!(r#"<p:sldLayoutId id="{id}" r:id="{relationship_id}"/>"#).as_bytes(),
            );
        }
        replaced.extend_from_slice(b"</p:sldLayoutIdLst>");
    }
    replaced.extend_from_slice(&xml[range.end..]);
    Ok(replaced)
}

/// Moves selected presentation-owned default resources into a source master scope.
pub(super) fn stage_source_master_defaults(
    source: &Presentation,
    package: &mut OpcPackage,
    master_part: &str,
) -> Result<()> {
    let Some(defaults) = source.presentation.default_text_style.as_ref() else {
        return Ok(());
    };
    let xml = defaults
        .to_xml()
        .map_err(|error| footer_malformed(master_part, error.to_string()))?;
    let xml = transfer_imported_property_resources(
        package,
        &source.presentation_part,
        master_part,
        &xml,
    )?;
    let mapped_defaults = CT_TextListStyle::from_xml(&xml)
        .map_err(|error| footer_malformed(master_part, error.to_string()))?;
    let mut master = CT_SlideMaster::from_xml(required_part(package, master_part)?)
        .map_err(|error| footer_malformed(master_part, error.to_string()))?;
    master
        .inherit_presentation_text_defaults(&mapped_defaults)
        .map_err(|error| footer_malformed(master_part, error.to_string()))?;
    package.set_part(
        master_part,
        master
            .to_xml()
            .map_err(|error| footer_malformed(master_part, error.to_string()))?,
    );
    Ok(())
}

pub(super) fn transfer_imported_property_resources(
    package: &mut OpcPackage,
    source_part: &str,
    destination_part: &str,
    xml: &[u8],
) -> Result<Vec<u8>> {
    let ids: HashSet<_> = relationship_ids(xml)
        .map_err(|error| footer_malformed(source_part, error.to_string()))?
        .into_iter()
        .collect();
    if ids.is_empty() {
        return Ok(xml.to_vec());
    }
    let original = package
        .get_part_rels(source_part)
        .cloned()
        .unwrap_or_default();
    let mut selected = Vec::with_capacity(ids.len());

    for id in ids {
        let relationship = original
            .get_by_id(&id)
            .ok_or_else(|| Error::MissingRelationship {
                source_part: source_part.to_owned(),
                relationship_id: id.clone(),
            })?;
        let mut transferred = relationship.clone();
        if !relationship_is_external(relationship) {
            transferred.target = relative_part_target(
                destination_part,
                &OpcPackage::resolve_rel_target(source_part, &relationship.target),
            );
        }
        selected.push(transferred);
    }
    let scope = original.rebind_for_import(selected, &HashMap::new())?;
    let existing = package
        .get_part_rels(destination_part)
        .cloned()
        .unwrap_or_default();
    let (merged, mapping) = existing.merge_for_import(&scope)?;
    let rewritten = rewrite_exact_rel_ids(xml, &mapping)
        .map_err(|error| footer_malformed(source_part, error.to_string()))?;
    package.set_part_rels(destination_part, merged);
    Ok(rewritten)
}

/// Finds qualified rendered character and theme font descriptions in selected XML.
pub(super) fn imported_typefaces(
    package: &OpcPackage,
    parts: &HashSet<String>,
) -> Result<HashSet<String>> {
    let mut names = HashSet::new();
    for part in parts {
        let Some(xml) = package.get_part(part) else {
            continue;
        };
        if !package
            .content_types
            .content_type_for(part)
            .is_some_and(|content_type| {
                content_type.ends_with("+xml") || content_type == "application/xml"
            })
        {
            continue;
        }
        let mut reader = NsReader::from_reader(xml);
        let mut buffer = Vec::new();
        let mut path: Vec<(String, Vec<u8>, bool)> = Vec::new();
        loop {
            let decoder = reader.decoder();
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| footer_malformed(part, error.to_string()))?;
            let uri = match namespace {
                XmlResolveResult::Bound(uri) => {
                    let attribute = quick_xml::events::attributes::Attribute {
                        key: quick_xml::name::QName(b"xmlns"),
                        value: std::borrow::Cow::Borrowed(uri.as_ref()),
                    };
                    attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                        .map_err(|error| footer_malformed(part, error.to_string()))?
                        .into_owned()
                }
                _ => String::new(),
            };
            let empty = matches!(event, XmlEvent::Empty(_));
            match event {
                XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                    let active = path.last().is_none_or(|(_, _, active)| *active)
                        && matches!(
                            uri.as_str(),
                            P_NS | A_NS
                                | "http://schemas.openxmlformats.org/drawingml/2006/chart"
                                | "http://schemas.openxmlformats.org/drawingml/2006/chartDrawing"
                                | "http://schemas.microsoft.com/office/drawing/2014/chartex"
                                | "http://schemas.microsoft.com/office/powerpoint/2018/8/main"
                                | "http://schemas.openxmlformats.org/markup-compatibility/2006"
                        )
                        && start.local_name().as_ref() != b"extLst";
                    let parent_font = path.last().is_some_and(|(uri, name, _)| {
                        uri == A_NS
                            && matches!(
                                name.as_slice(),
                                b"rPr" | b"defRPr" | b"endParaRPr" | b"majorFont" | b"minorFont"
                            )
                    });
                    if active
                        && parent_font
                        && uri == A_NS
                        && matches!(
                            start.local_name().as_ref(),
                            b"latin" | b"ea" | b"cs" | b"font"
                        )
                    {
                        for attribute in start.attributes() {
                            let attribute = attribute
                                .map_err(|error| footer_malformed(part, error.to_string()))?;
                            if attribute.key.as_ref() == b"typeface" {
                                let name = attribute
                                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                                    .map_err(|error| footer_malformed(part, error.to_string()))?;
                                if !name.is_empty() {
                                    names.insert(name.to_lowercase());
                                }
                            }
                        }
                    }
                    if !empty {
                        path.push((uri, start.local_name().as_ref().to_vec(), active));
                    }
                }
                XmlEvent::End(_) => {
                    path.pop();
                }
                XmlEvent::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    Ok(names)
}
