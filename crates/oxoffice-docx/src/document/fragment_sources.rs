//! Fragments projected from other parts and glossary entries.

use super::*;

impl Document {
    /// Bind an existing control to a snapshot-checked placeholder entry.
    pub fn bind_building_block_placeholder(
        &mut self,
        control: &ContentLocation,
        entry: &crate::BuildingBlockInfo,
    ) -> Result<()> {
        self.checked_building_block(entry)?;
        if entry.block.gallery.as_deref() != Some("placeholder") {
            return Err(Error::Other(
                "placeholder binding requires the placeholder gallery".to_owned(),
            ));
        }
        if control.item_kind != StoryItemKind::ContentControl
            || control.index_path.len() != 1
            || control.is_end
        {
            return Err(Error::Other(
                "placeholder binding requires an existing content control location".to_owned(),
            ));
        }
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        let (source, owner, _) =
            candidate.fragment_story_source_owner_and_owners(&control.story)?;
        let part = source.part_name.clone();
        let mut xml = source.xml.into_owned();
        let item = scan_story_items(&xml, &owner, None)?
            .get(control.index_path[0])
            .cloned()
            .ok_or_else(|| Error::Other("stale content control index".to_owned()))?;
        if item.kind != StoryItemKind::ContentControl {
            return Err(Error::Other(
                "content control location changed kind".to_owned(),
            ));
        }
        let scope = story_namespace_scope_at(&xml, item.full.start)?;
        let raw = close_content_fragment_namespaces(&xml[item.full.clone()], &scope)?;
        let updated = crate::building_block::bind_placeholder_xml(&raw, entry)?;
        xml.splice(item.full, updated);
        set_story_source_xml(&mut candidate, &part, xml)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    pub(crate) fn preserve_glossary_drawing_ids_staged(&mut self) -> Result<()> {
        if let Some(glossary) = &self.glossary {
            let ids = drawing_ids_in_xml(&glossary.to_xml()?)?;
            self.identifiers.drawing_ids.extend(ids.iter().copied());
            self.identifiers.preserved_drawing_ids.extend(ids);
        }
        Ok(())
    }

    fn fragment_projection_relationships(
        &mut self,
        source_part: &str,
        selected: &[u8],
    ) -> Result<BTreeMap<String, String>> {
        if source_part == self.doc_part_name {
            return Ok(BTreeMap::new());
        }
        let source_rels = self
            .package
            .get_part_rels(source_part)
            .cloned()
            .unwrap_or_default();
        let mut relationship_map = BTreeMap::new();
        for relationship_id in crate::field::relationship_ids_in_xml(selected)? {
            let relationship = source_rels.get_by_id(&relationship_id).ok_or_else(|| {
                Error::Other(format!(
                    "fragment source relationship {relationship_id} is missing from {source_part}"
                ))
            })?;
            let destination_part = self.doc_part_name.clone();
            let new_id = if relationship_is_internal(relationship) {
                let target_part = OpcPackage::resolve_rel_target(source_part, &relationship.target);
                self.add_internal_relationship_checked(
                    &destination_part,
                    &relationship.rel_type,
                    &relative_target(&destination_part, &target_part),
                )?
            } else {
                self.add_external_relationship_checked(
                    &destination_part,
                    &relationship.rel_type,
                    &relationship.target,
                )?
            };
            relationship_map.insert(relationship_id, new_id);
        }
        Ok(relationship_map)
    }

    pub(crate) fn project_fragment_part_content(
        &mut self,
        source_part: &str,
        mut selected: Vec<u8>,
    ) -> Result<()> {
        if source_part != self.doc_part_name {
            let relationships = self.fragment_projection_relationships(source_part, &selected)?;
            selected = crate::field::patch_relationship_ids(&selected, &relationships)?;
        }
        let main_xml = self
            .package
            .get_part(&self.doc_part_name)
            .ok_or_else(|| Error::Other("document fragment main part is missing".to_owned()))?
            .to_vec();
        let main_owner = scan_story_owners(&main_xml, StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("document fragment main body is missing".to_owned()))?;
        let main_items = direct_story_content_items(&main_xml, &main_owner)?;
        let main_end = story_owner_content_end(&main_xml, &main_owner)?;
        let main_start = main_items.first().map_or(main_end, |item| item.full.start);
        let mut updated = main_xml;
        updated.splice(main_start..main_end, selected);
        self.package.set_part(&self.doc_part_name, updated);
        Ok(())
    }

    pub(crate) fn project_fragment_part_source(
        &mut self,
        source_part: &str,
        selected: oxoffice_docx_oxml::document::ScopedWordSource,
        main_source: &oxoffice_docx_oxml::document::ScopedWordSource,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    ) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
        let relationships =
            self.fragment_projection_relationships(source_part, selected.as_bytes())?;
        let selected =
            crate::field::patch_relationship_source(operation, selected, &relationships)?;
        let main_owner = scan_story_owners(main_source.as_bytes(), StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("document fragment main body is missing".into()))?;
        let main_items = direct_story_content_items(main_source.as_bytes(), &main_owner)?;
        let main_end = story_owner_content_end(main_source.as_bytes(), &main_owner)?;
        let main_start = main_items.first().map_or(main_end, |item| item.full.start);
        let source = operation.compose_sources(&[
            (Some(main_source), 0..main_start, &[]),
            (Some(&selected), 0..selected.as_bytes().len(), &[]),
            (
                Some(main_source),
                main_end..main_source.as_bytes().len(),
                &[],
            ),
        ])?;
        let retained = operation.repeat_source(&source)?;
        self.package
            .set_part(&self.doc_part_name, operation.finish_source(retained)?);
        Ok(source)
    }

    pub(super) fn fragment_story_source_owner_and_owners<'a>(
        &'a self,
        story: &StoryId,
    ) -> Result<(StorySource<'a>, StoryOwnerSpan, Vec<StoryOwnerSpan>)> {
        let (canonical, _) = self.story_source_and_owner(story)?;
        self.fragment_story_source_from_canonical(story, &canonical)
    }

    // Reuse a canonical source only within the same immutable candidate phase.
    // The retained physical owner is still independently discovered below.
    pub(super) fn fragment_story_source_from_canonical<'a>(
        &'a self,
        story: &StoryId,
        canonical: &StorySource<'_>,
    ) -> Result<(StorySource<'a>, StoryOwnerSpan, Vec<StoryOwnerSpan>)> {
        let xml = self.package.get_part(&canonical.part_name).ok_or_else(|| {
            Error::Other(format!(
                "document fragment story part {} is missing",
                canonical.part_name
            ))
        })?;
        let owners = scan_story_owners(xml, canonical.root_kind)?;
        let owner = owners
            .iter()
            .find(|owner| owner.kind == story.kind && owner.owner_index == story.owner_index)
            .cloned()
            .ok_or_else(|| StoryError::OwnerNotFound {
                story: story.clone(),
            })?;
        Ok((
            StorySource {
                root_kind: canonical.root_kind,
                part_name: canonical.part_name.clone(),
                xml: Cow::Borrowed(xml),
            },
            owner,
            owners,
        ))
    }
}
