//! Sections and page setup: their stories, insertion and removal, size, margins and columns.

use super::*;

impl Document {
    /// Whether any document section carries layout formatting.
    ///
    /// This includes the final body section and sections attached to paragraph
    /// properties. Callers that cannot preserve page layout can reject such
    /// documents without treating section XML as ordinary body content.
    pub fn has_section_layout_formatting(&self) -> bool {
        self.section_properties_in_order().any(section_has_layout)
    }

    /// Whether any document section retains XML that the semantic section
    /// reader does not model.
    pub fn has_unmodeled_section_properties(&self) -> bool {
        self.section_properties_in_order()
            .any(|properties| !properties.extra_xml.is_empty() || properties.change.is_some())
    }
}

impl Document {
    pub(super) fn section_properties_in_order(&self) -> impl Iterator<Item = &CT_SectPr> {
        self.document
            .body
            .content
            .iter()
            .filter_map(|content| match content {
                BodyContent::Paragraph(paragraph) => paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.sect_pr.as_ref()),
                BodyContent::Table(_) | BodyContent::ContentControl(_) | BodyContent::RawXml(_) => {
                    None
                }
            })
            .chain(self.document.body.sect_pr.iter())
    }

    // ---- Section/Page setup ----

    /// Return the number of section property owners in document order.
    pub fn section_count(&self) -> usize {
        self.section_properties_in_order().count()
    }

    /// Iterate over every section in document order.
    pub fn sections(&self) -> impl Iterator<Item = SectionRef<'_>> {
        let final_ordinal = self.section_count().checked_sub(1);
        let has_final_body_owner = self.document.body.sect_pr.is_some();
        self.section_properties_in_order()
            .enumerate()
            .map(move |(ordinal, inner)| SectionRef {
                ordinal,
                is_final: has_final_body_owner && Some(ordinal) == final_ordinal,
                inner,
            })
    }

    /// Get a section by its document-order index.
    pub fn section(&self, index: usize) -> Option<SectionRef<'_>> {
        let final_ordinal = self.section_count().checked_sub(1);
        let has_final_body_owner = self.document.body.sect_pr.is_some();
        self.section_properties_in_order()
            .nth(index)
            .map(|inner| SectionRef {
                ordinal: index,
                is_final: has_final_body_owner && Some(index) == final_ordinal,
                inner,
            })
    }

    /// Report explicit invalid or one-sided section page sizes without normalizing source.
    pub fn validate_sections(&self) -> ValidationReport {
        let mut findings = Vec::new();
        for section in self.sections() {
            let properties = section.properties();
            if properties.page_width.is_some_and(|size| size.0 <= 0)
                || properties.page_height.is_some_and(|size| size.0 <= 0)
            {
                findings.push(ValidationFinding {
                    severity: ValidationSeverity::Error,
                    message: format!("section {} has nonpositive page size", section.ordinal()),
                });
            } else if properties.page_width.is_some() != properties.page_height.is_some() {
                findings.push(ValidationFinding {
                    severity: ValidationSeverity::Warning,
                    message: format!(
                        "section {} has incomplete page size: width or height is missing",
                        section.ordinal()
                    ),
                });
            }
        }
        ValidationReport { findings }
    }

    /// Resolve the effective header or footer story for one section variant.
    pub fn section_story(
        &self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<Option<SectionStory>> {
        if section_index >= self.section_count() {
            return Err(Error::Other(format!(
                "section index {section_index} is out of range"
            )));
        }
        let mut effective = None;
        for (source_section, section) in self
            .section_properties_in_order()
            .enumerate()
            .take(section_index + 1)
        {
            let references = match kind {
                HeaderFooterKind::Header => &section.header_refs,
                HeaderFooterKind::Footer => &section.footer_refs,
            };
            if let Some(reference) = self
                .first_usable_header_footer_reference(references, hdr_type, kind.is_header())
                .cloned()
            {
                effective = Some((source_section, reference));
            }
        }
        let Some((source_section, reference)) = effective else {
            return Ok(None);
        };
        let part_name = self
            .header_footer_part_name(&reference.rel_id, kind.is_header())
            .ok_or_else(|| {
                Error::Other(format!(
                    "section story relationship {} is not a valid internal {:?}",
                    reference.rel_id, kind
                ))
            })?;
        Ok(Some(SectionStory {
            story: self.story_id_for_header_footer_part(&part_name, kind)?,
            source_section,
            inherited: source_section != section_index,
        }))
    }

    /// The staged route of [`Self::create_section_story`]: a copy is
    /// flushed, edited, reconciled, saved and reopened. It is the debug
    /// oracle of the in-place route, and the route of a document whose
    /// headers or footers hold comment markers.
    fn create_section_story_staged(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<StoryId> {
        if let Some(story) = self.section_story(section_index, kind, hdr_type)?
            && !story.is_inherited()
        {
            let enabled = match hdr_type {
                HdrFtrType::Default => true,
                HdrFtrType::First => self
                    .section(section_index)
                    .is_some_and(|section| section.properties().title_pg.unwrap_or(false)),
                HdrFtrType::Even => self.even_and_odd_headers(),
            };
            if enabled {
                return Ok(story.story);
            }
            let mut candidate = if hdr_type == HdrFtrType::Even {
                self.settings_mutation_candidate()?
            } else {
                self.clone_for_staging()
            };
            match hdr_type {
                HdrFtrType::First => {
                    candidate
                        .section_mut(section_index)
                        .ok_or_else(|| {
                            Error::Other(format!("section index {section_index} is out of range"))
                        })?
                        .inner
                        .title_pg = Some(true);
                }
                HdrFtrType::Even => candidate
                    .settings
                    .get_or_insert_with(CT_Settings::new)
                    .set_even_and_odd_headers(true)?,
                HdrFtrType::Default => {}
            }
            let reopened = candidate.prepare_and_reopen_staged()?;
            let result = reopened
                .section_story(section_index, kind, hdr_type)?
                .map(|section_story| section_story.story)
                .ok_or_else(|| {
                    Error::Other("existing section story was not published".to_owned())
                })?;
            self.commit_staged_mutation(reopened);
            return Ok(result);
        }
        let mut candidate = if hdr_type == HdrFtrType::Even {
            self.settings_mutation_candidate()?
        } else {
            self.clone_for_staging()
        };
        if hdr_type == HdrFtrType::Even {
            candidate
                .settings
                .get_or_insert_with(CT_Settings::new)
                .set_even_and_odd_headers(true)?;
        }
        candidate.flush_to_package()?;
        let story = candidate.install_empty_section_story(section_index, kind, hdr_type)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let result = reopened
            .section_story(section_index, kind, hdr_type)?
            .map(|section_story| section_story.story)
            .ok_or_else(|| Error::Other("created section story was not published".to_owned()))?;
        debug_assert_eq!(result.part_name(), story.part_name());
        self.commit_staged_mutation(reopened);
        Ok(result)
    }

    /// The staged route of [`Self::link_section_story`].
    fn link_section_story_staged(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
        story: &StoryId,
    ) -> Result<StoryId> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.validate_section_story_target(story, kind)?;
        let relationship_id = candidate.document_story_relationship(kind, story.part_name())?;
        let removed = candidate.set_direct_section_story_reference(
            section_index,
            kind,
            hdr_type,
            Some(relationship_id),
        )?;
        candidate.prune_unreachable_section_stories(&removed)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let result = reopened
            .section_story(section_index, kind, hdr_type)?
            .map(|section_story| section_story.story)
            .ok_or_else(|| Error::Other("linked section story was not published".to_owned()))?;
        self.commit_staged_mutation(reopened);
        Ok(result)
    }

    /// The staged route of [`Self::inherit_section_story`].
    fn inherit_section_story_staged(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let removed =
            candidate.set_direct_section_story_reference(section_index, kind, hdr_type, None)?;
        candidate.prune_unreachable_section_stories(&removed)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Ensure that one section has an explicit story, creating an empty story
    /// when the variant is absent or inherited.
    ///
    /// It edits the live document: the story part, its relationship and
    /// content type through the part journal (`EditUnit::Parts`), the
    /// section's properties (`EditUnit::SectionProperties`) and, for an even
    /// story, the settings (`EditUnit::Settings`).
    pub fn create_section_story(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<StoryId> {
        if let Some(story) = self.section_story(section_index, kind, hdr_type)?
            && !story.is_inherited()
            && self.section_variant_shown(section_index, hdr_type)
        {
            return Ok(story.story);
        }
        if self.header_footer_stories_hold_comment_markers() {
            return self.create_section_story_staged(section_index, kind, hdr_type);
        }
        let mut units = vec![
            crate::EditUnit::Parts,
            crate::EditUnit::SectionProperties {
                index: section_index,
            },
        ];
        if hdr_type == HdrFtrType::Even {
            units.push(crate::EditUnit::Settings);
        }
        self.section_story_edit(
            &units,
            |document| document.create_section_story_in_place(section_index, kind, hdr_type),
            |staged| staged.create_section_story_staged(section_index, kind, hdr_type),
        )
    }

    /// Whether section `section_index` shows its `hdr_type` variant: always
    /// the default one, the first-page one with a different first page, the
    /// even one with even and odd headers.
    fn section_variant_shown(&self, section_index: usize, hdr_type: HdrFtrType) -> bool {
        match hdr_type {
            HdrFtrType::Default => true,
            HdrFtrType::First => self
                .section(section_index)
                .is_some_and(|section| section.properties().title_pg.unwrap_or(false)),
            HdrFtrType::Even => self.even_and_odd_headers(),
        }
    }

    /// Run `edit`, a section story edit, in place on `units`; debug builds
    /// compare it with `staged`, today's staged route, run on a copy.
    pub(super) fn section_story_edit<T: PartialEq + std::fmt::Debug>(
        &mut self,
        units: &[crate::EditUnit],
        edit: impl FnOnce(&mut Self) -> Result<T>,
        #[cfg_attr(not(debug_assertions), allow(unused_variables))] staged: impl FnOnce(
            &mut Self,
        )
            -> Result<T>,
    ) -> Result<T> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.edit_in_place(units, edit);
        #[cfg(debug_assertions)]
        self.assert_in_place_matches_staged(before, &result, staged);
        result
    }

    fn create_section_story_in_place(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<StoryId> {
        let enable_even = |document: &mut Self| -> Result<()> {
            document.reserve_settings_part()?;
            document
                .settings
                .get_or_insert_with(CT_Settings::new)
                .set_even_and_odd_headers(true)?;
            Ok(())
        };
        if let Some(story) = self.section_story(section_index, kind, hdr_type)?
            && !story.is_inherited()
        {
            match hdr_type {
                HdrFtrType::First => {
                    self.section_mut(section_index)
                        .ok_or_else(|| {
                            Error::Other(format!("section index {section_index} is out of range"))
                        })?
                        .inner
                        .title_pg = Some(true);
                }
                HdrFtrType::Even => enable_even(self)?,
                HdrFtrType::Default => {}
            }
            return self
                .section_story(section_index, kind, hdr_type)?
                .map(|section_story| section_story.story)
                .ok_or_else(|| {
                    Error::Other("existing section story was not published".to_owned())
                });
        }
        if hdr_type == HdrFtrType::Even {
            enable_even(self)?;
        }
        let story = self.install_empty_section_story(section_index, kind, hdr_type)?;
        let result = self
            .section_story(section_index, kind, hdr_type)?
            .map(|section_story| section_story.story)
            .ok_or_else(|| Error::Other("created section story was not published".to_owned()))?;
        debug_assert_eq!(result.part_name(), story.part_name());
        Ok(result)
    }

    /// Point one section variant at an existing same-kind story in this document.
    ///
    /// It edits the live document: the main part's relationships and the
    /// story parts nothing reaches any more through the part journal
    /// (`EditUnit::Parts`), and the section's properties
    /// (`EditUnit::SectionProperties`).
    pub fn link_section_story(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
        story: &StoryId,
    ) -> Result<StoryId> {
        if self.header_footer_stories_hold_comment_markers() {
            return self.link_section_story_staged(section_index, kind, hdr_type, story);
        }
        self.section_story_edit(
            &Self::section_story_units(section_index),
            |document| {
                document.validate_section_story_target(story, kind)?;
                let relationship_id =
                    document.document_story_relationship(kind, story.part_name())?;
                let removed = document.set_direct_section_story_reference(
                    section_index,
                    kind,
                    hdr_type,
                    Some(relationship_id),
                )?;
                document.prune_unreachable_section_stories(&removed)?;
                document
                    .section_story(section_index, kind, hdr_type)?
                    .map(|section_story| section_story.story)
                    .ok_or_else(|| {
                        Error::Other("linked section story was not published".to_owned())
                    })
            },
            |staged| staged.link_section_story_staged(section_index, kind, hdr_type, story),
        )
    }

    fn inherit_section_story_body(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<()> {
        let removed =
            self.set_direct_section_story_reference(section_index, kind, hdr_type, None)?;
        self.prune_unreachable_section_stories(&removed)
    }

    /// The units of a section story edit of section `section_index`.
    fn section_story_units(section_index: usize) -> [crate::EditUnit; 2] {
        [
            crate::EditUnit::Parts,
            crate::EditUnit::SectionProperties {
                index: section_index,
            },
        ]
    }

    /// Remove the direct reference so the same variant inherits from the
    /// nearest preceding section, or remains absent when none exists.
    ///
    /// It edits the live document, as [`Self::link_section_story`] does.
    pub fn inherit_section_story(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<()> {
        if self.header_footer_stories_hold_comment_markers() {
            return self.inherit_section_story_staged(section_index, kind, hdr_type);
        }
        self.section_story_edit(
            &Self::section_story_units(section_index),
            |document| document.inherit_section_story_body(section_index, kind, hdr_type),
            |staged| staged.inherit_section_story_staged(section_index, kind, hdr_type),
        )
    }

    /// Clone the effective story into an independent story owned by this section.
    pub fn unlink_section_story(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<StoryId> {
        let effective = self
            .section_story(section_index, kind, hdr_type)?
            .ok_or_else(|| Error::Other("cannot unlink an absent section story".to_owned()))?;
        self.replace_section_story(section_index, kind, hdr_type, effective.story())
    }

    /// Replace one variant with an independent copy of an existing same-kind story.
    ///
    /// It edits the live document: the copied part, its relationships, the
    /// main part's relationships and the story parts nothing reaches any
    /// more through the part journal (`EditUnit::Parts`), the drawing ids
    /// the copy takes through the reservation journal, and the section's
    /// properties (`EditUnit::SectionProperties`).
    pub fn replace_section_story(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
        source: &StoryId,
    ) -> Result<StoryId> {
        if self.header_footer_stories_hold_comment_markers() {
            return self.replace_section_story_staged(section_index, kind, hdr_type, source);
        }
        self.section_story_edit(
            &Self::section_story_units(section_index),
            |document| {
                document.validate_section_story_target(source, kind)?;
                let part_name =
                    document.clone_header_footer_story_part(source.part_name(), kind)?;
                let relationship_id = document.document_story_relationship(kind, &part_name)?;
                let removed = document.set_direct_section_story_reference(
                    section_index,
                    kind,
                    hdr_type,
                    Some(relationship_id),
                )?;
                document.prune_unreachable_section_stories(&removed)?;
                document
                    .section_story(section_index, kind, hdr_type)?
                    .map(|section_story| section_story.story)
                    .ok_or_else(|| {
                        Error::Other("replaced section story was not published".to_owned())
                    })
            },
            |staged| staged.replace_section_story_staged(section_index, kind, hdr_type, source),
        )
    }

    /// Replace one section variant with a distinct empty story.
    ///
    /// It edits the live document, as [`Self::create_section_story`] does.
    pub fn remove_section_story(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<StoryId> {
        if self.header_footer_stories_hold_comment_markers() {
            return self.remove_section_story_staged(section_index, kind, hdr_type);
        }
        self.section_story_edit(
            &Self::section_story_units(section_index),
            |document| {
                document.install_empty_section_story(section_index, kind, hdr_type)?;
                document
                    .section_story(section_index, kind, hdr_type)?
                    .map(|section_story| section_story.story)
                    .ok_or_else(|| Error::Other("empty section story was not published".to_owned()))
            },
            |staged| staged.remove_section_story_staged(section_index, kind, hdr_type),
        )
    }

    /// The staged route of [`Self::replace_section_story`].
    fn replace_section_story_staged(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
        source: &StoryId,
    ) -> Result<StoryId> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.validate_section_story_target(source, kind)?;
        let part_name = candidate.clone_header_footer_story_part(source.part_name(), kind)?;
        let relationship_id = candidate.document_story_relationship(kind, &part_name)?;
        let removed = candidate.set_direct_section_story_reference(
            section_index,
            kind,
            hdr_type,
            Some(relationship_id),
        )?;
        candidate.prune_unreachable_section_stories(&removed)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let result = reopened
            .section_story(section_index, kind, hdr_type)?
            .map(|section_story| section_story.story)
            .ok_or_else(|| Error::Other("replaced section story was not published".to_owned()))?;
        self.commit_staged_mutation(reopened);
        Ok(result)
    }

    /// The staged route of [`Self::remove_section_story`].
    fn remove_section_story_staged(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<StoryId> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.install_empty_section_story(section_index, kind, hdr_type)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let result = reopened
            .section_story(section_index, kind, hdr_type)?
            .map(|section_story| section_story.story)
            .ok_or_else(|| Error::Other("empty section story was not published".to_owned()))?;
        self.commit_staged_mutation(reopened);
        Ok(result)
    }

    fn story_id_for_header_footer_part(
        &self,
        part_name: &str,
        kind: HeaderFooterKind,
    ) -> Result<StoryId> {
        let xml = self
            .package
            .get_part(part_name)
            .ok_or_else(|| Error::Other(format!("section story part {part_name} is missing")))?;
        let story_kind = kind.story_kind();
        let owner = scan_story_owners(xml, story_kind)?
            .into_iter()
            .find(|owner| owner.kind == story_kind)
            .ok_or_else(|| Error::Other(format!("section story part {part_name} has no root")))?;
        Ok(StoryId {
            kind: story_kind,
            part_name: part_name.to_owned(),
            owner_index: owner.owner_index,
            fingerprint: owner.fingerprint,
        })
    }

    fn validate_section_story_target(&self, story: &StoryId, kind: HeaderFooterKind) -> Result<()> {
        if story.kind != kind.story_kind() {
            return Err(StoryError::WrongOwner {
                expected: kind.story_kind(),
                actual: story.kind,
            }
            .into());
        }
        let (_, owner) = self.story_source_and_owner(story)?;
        if owner.kind != kind.story_kind() || owner.owner_index != 0 {
            return Err(Error::Other(
                "section story target must be the physical header or footer root".to_owned(),
            ));
        }
        Ok(())
    }

    fn document_story_relationship(
        &mut self,
        kind: HeaderFooterKind,
        part_name: &str,
    ) -> Result<String> {
        if let Some(id) = self
            .package
            .get_part_rels(&self.doc_part_name)
            .and_then(|relationships| {
                relationships.items.iter().find(|relationship| {
                    relationship.rel_type == kind.relationship_type()
                        && relationship_is_internal(relationship)
                        && part_name_identity(&OpcPackage::resolve_rel_target(
                            &self.doc_part_name,
                            &relationship.target,
                        )) == part_name_identity(part_name)
                })
            })
            .map(|relationship| relationship.id.clone())
        {
            return Ok(id);
        }
        let id = self
            .identifiers
            .reserve_relationship_id_checked(&self.doc_part_name)?;
        let owner = self.doc_part_name.clone();
        let target = relative_target(&owner, part_name);
        self.add_relationship_journaled(Some(&owner), &id, kind.relationship_type(), &target);
        Ok(id)
    }

    fn set_direct_section_story_reference(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
        relationship_id: Option<String>,
    ) -> Result<Vec<HdrFtrRef>> {
        let section = self.section_mut(section_index).ok_or_else(|| {
            Error::Other(format!("section index {section_index} is out of range"))
        })?;
        let references = match kind {
            HeaderFooterKind::Header => &mut section.inner.header_refs,
            HeaderFooterKind::Footer => &mut section.inner.footer_refs,
        };
        let slot = story_reference_slot(references, hdr_type);
        let mut removed = Vec::new();
        references.retain(|reference| {
            if reference.hdr_ftr_type == hdr_type {
                removed.push(reference.clone());
                false
            } else {
                true
            }
        });
        if let Some(rel_id) = relationship_id {
            // A replacement keeps the place of the reference it replaces.
            let mut reference = removed
                .first()
                .cloned()
                .unwrap_or_else(|| HdrFtrRef::new(hdr_type, String::new()));
            reference.rel_id = rel_id;
            references.insert(slot, reference);
            if hdr_type == HdrFtrType::First {
                section.inner.title_pg = Some(true);
            }
        }
        Ok(removed)
    }

    fn install_empty_section_story(
        &mut self,
        section_index: usize,
        kind: HeaderFooterKind,
        hdr_type: HdrFtrType,
    ) -> Result<StoryId> {
        let xml = Self::serialize_hdr_ftr(&CT_HdrFtr::new(), kind.is_header())?;
        let part_name = self.install_header_footer_story_part(xml, None, kind)?;
        let relationship_id = self.document_story_relationship(kind, &part_name)?;
        let removed = self.set_direct_section_story_reference(
            section_index,
            kind,
            hdr_type,
            Some(relationship_id),
        )?;
        self.prune_unreachable_section_stories(&removed)?;
        self.story_id_for_header_footer_part(&part_name, kind)
    }

    fn clone_header_footer_story_part(
        &mut self,
        source_part: &str,
        kind: HeaderFooterKind,
    ) -> Result<String> {
        let xml = self
            .package
            .get_part(source_part)
            .ok_or_else(|| Error::Other(format!("section story part {source_part} is missing")))?;
        if !header_footer_story_has_expected_root(xml, kind.is_header()) {
            return Err(Error::Other(format!(
                "section story part {source_part} has the wrong root"
            )));
        }
        let mut xml = xml.to_vec();
        xml = self
            .identifiers
            .reserve_drawing_ids(|occupied| rewrite_authored_doc_pr_ids(&xml, occupied))?;
        let relationships = self.package.get_part_rels(source_part).cloned();
        self.install_header_footer_story_part(
            xml,
            relationships.as_ref().map(|rels| (source_part, rels)),
            kind,
        )
    }

    fn install_header_footer_story_part(
        &mut self,
        xml: Vec<u8>,
        copied_relationships: Option<(&str, &oxoffice_opc::relationship::Relationships)>,
        kind: HeaderFooterKind,
    ) -> Result<String> {
        let part_name = self
            .identifiers
            .reserve_part_name("/word", kind.part_stem(), "xml")?;
        self.package.set_part(&part_name, xml);
        self.add_override_journaled(&part_name, kind.content_type());
        self.identifiers.register_content_type_override(&part_name);
        self.identifiers.author_story_part(&part_name);

        if let Some((source_part, relationships)) = copied_relationships {
            let mut relationships = relationships.clone();
            for relationship in &mut relationships.items {
                if relationship_is_internal(relationship) {
                    let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
                    relationship.target = relative_target(&part_name, &target);
                }
            }
            let owner = relationship_owner_identity(&part_name);
            let ids = relationships
                .items
                .iter()
                .map(|relationship| relationship.id.clone())
                .collect::<HashSet<_>>();
            self.identifiers
                .replace_story_relationship_sets(&owner, ids);
            // An in-place edit puts the part's relationships back.
            self.package.record_part_rels(&part_name);
            self.package.set_part_rels(&part_name, relationships);
        }
        Ok(part_name)
    }

    /// Get a mutable section by its document-order index.
    pub fn section_mut(&mut self, index: usize) -> Option<Section<'_>> {
        let count = self.section_count();
        let has_final_body_owner = self.document.body.sect_pr.is_some();
        if index >= count {
            return None;
        }
        self.invalidate_layout();
        let document = &mut *self.document;
        let mut remaining = index;
        for content in &mut document.body.content {
            if let BodyContent::Paragraph(paragraph) = content
                && let Some(inner) = paragraph
                    .properties
                    .as_mut()
                    .and_then(|properties| properties.sect_pr.as_mut())
            {
                if remaining == 0 {
                    return Some(Section {
                        ordinal: index,
                        is_final: has_final_body_owner && index + 1 == count,
                        inner,
                    });
                }
                remaining -= 1;
            }
        }
        document.body.sect_pr.as_mut().map(|inner| Section {
            ordinal: index,
            is_final: has_final_body_owner && index + 1 == count,
            inner,
        })
    }

    /// Insert an empty section at a document-order index.
    ///
    /// `index == section_count()` appends a new final section. Larger indices
    /// are rejected without changing the document.
    pub fn insert_section(&mut self, index: usize) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.insert_section_in_place(index)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Remove a section by document-order index and merge its content.
    ///
    /// A non-final section merges into the following section. Removing the
    /// final section promotes the preceding section properties to the final
    /// body owner. The sole section cannot be removed.
    pub fn remove_section(&mut self, index: usize) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        let removed_references = candidate.remove_section_in_place(index)?;
        candidate.prune_unreachable_section_stories(&removed_references)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    pub(super) fn insert_section_in_place(&mut self, index: usize) -> Result<()> {
        let count = self.section_count();
        if index > count {
            return Err(Error::Other(format!(
                "section index {index} is out of bounds for insertion into {count} sections"
            )));
        }
        // Copy resolved geometry only. Relationship owners and other section
        // policies belong to the old section and must not be cloned.
        let previous = self
            .section(index.saturating_sub(1))
            .map(|s| s.properties());
        let mut inserted = empty_section_properties();
        inserted.page_width = Some(previous.and_then(|s| s.page_width).unwrap_or(Twips(12240)));
        inserted.page_height = Some(previous.and_then(|s| s.page_height).unwrap_or(Twips(15840)));
        inserted.orientation = Some(
            previous
                .and_then(|s| s.orientation)
                .unwrap_or(ST_PageOrientation::Portrait),
        );
        inserted.margin_top = Some(previous.and_then(|s| s.margin_top).unwrap_or(Twips(1440)));
        inserted.margin_right = Some(previous.and_then(|s| s.margin_right).unwrap_or(Twips(1440)));
        inserted.margin_bottom = Some(
            previous
                .and_then(|s| s.margin_bottom)
                .unwrap_or(Twips(1440)),
        );
        inserted.margin_left = Some(previous.and_then(|s| s.margin_left).unwrap_or(Twips(1440)));
        inserted.gutter = Some(previous.and_then(|s| s.gutter).unwrap_or(Twips(0)));
        inserted.header_distance = Some(
            previous
                .and_then(|s| s.header_distance)
                .unwrap_or(Twips(720)),
        );
        inserted.footer_distance = Some(
            previous
                .and_then(|s| s.footer_distance)
                .unwrap_or(Twips(720)),
        );
        self.invalidate_layout();
        if count == 0 {
            self.document.body.sect_pr = Some(inserted);
            return Ok(());
        }
        if index == count {
            let previous_final = self.document.body.sect_pr.take().ok_or_else(|| {
                Error::Other("document section ownership changed during insertion".to_owned())
            })?;
            let mut paragraph = CT_P::new();
            paragraph.properties = Some(CT_PPr {
                sect_pr: Some(previous_final),
                ..CT_PPr::default()
            });
            self.document
                .body
                .content
                .push(BodyContent::Paragraph(Box::new(paragraph)));
            self.document.body.sect_pr = Some(inserted);
            return Ok(());
        }

        let insert_at = if index == 0 {
            0
        } else {
            self.paragraph_section_owner_index(index - 1)
                .ok_or_else(|| {
                    Error::Other("document section ownership changed during insertion".to_owned())
                })?
                + 1
        };
        let mut paragraph = CT_P::new();
        paragraph.properties = Some(CT_PPr {
            sect_pr: Some(inserted),
            ..CT_PPr::default()
        });
        self.document
            .body
            .content
            .insert(insert_at, BodyContent::Paragraph(Box::new(paragraph)));
        Ok(())
    }

    fn remove_section_in_place(&mut self, index: usize) -> Result<Vec<HdrFtrRef>> {
        let count = self.section_count();
        if count <= 1 {
            return Err(Error::Other(
                "the sole document section cannot be removed".to_owned(),
            ));
        }
        if index >= count {
            return Err(Error::Other(format!(
                "section index {index} is out of bounds for {count} sections"
            )));
        }
        let removed_section = self
            .section(index)
            .expect("validated section index")
            .properties();
        let removed = removed_section
            .header_refs
            .iter()
            .chain(&removed_section.footer_refs)
            .cloned()
            .collect::<Vec<_>>();
        self.invalidate_layout();

        if index + 1 < count {
            let mut effective_headers: [Option<HdrFtrRef>; 3] = [None, None, None];
            let mut effective_footers: [Option<HdrFtrRef>; 3] = [None, None, None];
            for section in self.section_properties_in_order().take(index + 1) {
                for hdr_type in [HdrFtrType::Default, HdrFtrType::First, HdrFtrType::Even] {
                    let slot = header_type_index(hdr_type);
                    if let Some(reference) = self.first_usable_header_footer_reference(
                        &section.header_refs,
                        hdr_type,
                        true,
                    ) {
                        effective_headers[slot] = Some(reference.clone());
                    }
                    if let Some(reference) = self.first_usable_header_footer_reference(
                        &section.footer_refs,
                        hdr_type,
                        false,
                    ) {
                        effective_footers[slot] = Some(reference.clone());
                    }
                }
            }
            let following = self
                .section(index + 1)
                .expect("every non-final section has a following section");
            let mut following_header_overrides = [false; 3];
            let mut following_footer_overrides = [false; 3];
            for hdr_type in [HdrFtrType::Default, HdrFtrType::First, HdrFtrType::Even] {
                let slot = header_type_index(hdr_type);
                following_header_overrides[slot] = self
                    .first_usable_header_footer_reference(
                        &following.properties().header_refs,
                        hdr_type,
                        true,
                    )
                    .is_some();
                following_footer_overrides[slot] = self
                    .first_usable_header_footer_reference(
                        &following.properties().footer_refs,
                        hdr_type,
                        false,
                    )
                    .is_some();
            }
            let mut following = self
                .section_mut(index + 1)
                .expect("every non-final section has a following section");
            for (slot, inherited) in effective_headers.into_iter().enumerate() {
                if !following_header_overrides[slot]
                    && let Some(inherited) = inherited
                {
                    place_story_reference(
                        &mut following.properties_mut().header_refs,
                        HdrFtrRef::new(inherited.hdr_ftr_type, inherited.rel_id),
                    );
                }
            }
            for (slot, inherited) in effective_footers.into_iter().enumerate() {
                if !following_footer_overrides[slot]
                    && let Some(inherited) = inherited
                {
                    place_story_reference(
                        &mut following.properties_mut().footer_refs,
                        HdrFtrRef::new(inherited.hdr_ftr_type, inherited.rel_id),
                    );
                }
            }
        }

        if index + 1 < count || self.document.body.sect_pr.is_none() {
            let owner_index = self
                .paragraph_section_owner_index(index)
                .expect("every non-final section has a paragraph owner");
            let remove_empty_owner = {
                let BodyContent::Paragraph(paragraph) =
                    &mut self.document.body.content[owner_index]
                else {
                    unreachable!("recorded section owner is not a paragraph")
                };
                let properties = paragraph
                    .properties
                    .as_mut()
                    .expect("a section owner has paragraph properties");
                properties.sect_pr = None;
                if *properties == CT_PPr::default() {
                    paragraph.properties = None;
                }
                *paragraph == Box::default()
            };
            if remove_empty_owner {
                self.document.body.content.remove(owner_index);
            }
        } else {
            let previous_owner = self
                .paragraph_section_owner_index(index - 1)
                .expect("a removable final section has a preceding paragraph owner");
            let (promoted, remove_empty_owner) = {
                let BodyContent::Paragraph(paragraph) =
                    &mut self.document.body.content[previous_owner]
                else {
                    unreachable!("recorded section owner is not a paragraph")
                };
                let properties = paragraph
                    .properties
                    .as_mut()
                    .expect("a section owner has paragraph properties");
                let promoted = properties
                    .sect_pr
                    .take()
                    .expect("recorded section properties disappeared");
                if *properties == CT_PPr::default() {
                    paragraph.properties = None;
                }
                (promoted, *paragraph == Box::default())
            };
            if remove_empty_owner {
                self.document.body.content.remove(previous_owner);
            }
            self.document.body.sect_pr = Some(promoted);
        }
        Ok(removed)
    }

    fn paragraph_section_owner_index(&self, ordinal: usize) -> Option<usize> {
        self.document
            .body
            .content
            .iter()
            .enumerate()
            .filter(|(_, content)| {
                matches!(
                    content,
                    BodyContent::Paragraph(paragraph)
                        if paragraph
                            .properties
                            .as_ref()
                            .is_some_and(|properties| properties.sect_pr.is_some())
                )
            })
            .nth(ordinal)
            .map(|(index, _)| index)
    }

    /// Which main-part relationships to the retired stories the main story
    /// still uses: the retired references and every other main-part
    /// relationship to a retired target, the only ids the prune asks about.
    ///
    /// Only a section names a header or footer relationship, so the typed
    /// section properties answer without serializing the body. A body whose
    /// XML kept verbatim (a raw block, a raw child of a table or a section)
    /// mentions one of the ids is scanned serialized, as every relationship id
    /// of the story; debug builds compare the typed answer with that scan.
    fn main_story_uses_of_retired_stories(
        &self,
        removed: &[HdrFtrRef],
        retired_targets: &HashSet<String>,
    ) -> Result<HashSet<String>> {
        let retired = retired_targets
            .iter()
            .map(|target| part_name_identity(target))
            .collect::<HashSet<_>>();
        let mut asked = removed
            .iter()
            .map(|reference| reference.rel_id.clone())
            .collect::<HashSet<_>>();
        asked.extend(
            self.package
                .get_part_rels(&self.doc_part_name)
                .into_iter()
                .flat_map(|relationships| &relationships.items)
                .filter(|relationship| {
                    relationship_is_internal(relationship)
                        && retired.contains(&part_name_identity(&OpcPackage::resolve_rel_target(
                            &self.doc_part_name,
                            &relationship.target,
                        )))
                })
                .map(|relationship| relationship.id.clone()),
        );
        let typed = walkers::section_story_relationship_ids(
            &self.document.body.content,
            self.document.body.sect_pr.as_ref(),
            &asked,
        )
        .map(|named| {
            let nested = self
                .identifiers
                .authored_nested_story_relationship_ids
                .get(&relationship_owner_identity(&self.doc_part_name));
            asked
                .iter()
                .filter(|id| {
                    named.contains(id.as_str())
                        || nested.is_some_and(|nested| nested.contains(id.as_str()))
                })
                .cloned()
                .collect::<HashSet<_>>()
        });
        #[cfg(not(debug_assertions))]
        if let Some(used) = typed {
            return Ok(used);
        }
        let modeled_ids = self.main_story_relationship_ids();
        let opaque_ids = self.opaque_relationship_ids_from_serialized_story(&modeled_ids)?;
        let scanned = modeled_ids
            .into_iter()
            .chain(opaque_ids)
            .filter(|id| asked.contains(id))
            .collect::<HashSet<_>>();
        #[cfg(debug_assertions)]
        if let Some(used) = typed {
            assert_eq!(
                used, scanned,
                "the typed sections tell another use of the retired stories than the serialized story"
            );
        }
        Ok(scanned)
    }

    fn prune_unreachable_section_stories(&mut self, removed: &[HdrFtrRef]) -> Result<()> {
        // Nothing retired, nothing to prune: the walk of the body's
        // relationship ids below is not needed.
        if removed.is_empty() {
            return Ok(());
        }
        // A retired header or footer part goes, with its relationship, its
        // .rels and its content type, once no modeled, inherited or opaque
        // physical use survives, whether it was authored here or imported:
        // Word drops such a part when it saves, and a dead copy can still hold
        // text the user believes removed. The proof is the loop below: every
        // relationship to the part, from any owner, must be shown unused. If a
        // use survives, the part is left as it is; if a relationship from
        // another part cannot be proved, the edit is refused.
        let retired_targets = removed
            .iter()
            .map(|reference| {
                let relationship = self
                    .package
                    .get_part_rels(&self.doc_part_name)
                    .and_then(|rels| rels.items.iter().find(|rel| rel.id == reference.rel_id))
                    .ok_or_else(|| {
                        Error::Other(format!(
                            "retired section relationship {} is missing",
                            reference.rel_id
                        ))
                    })?;
                if !relationship_is_internal(relationship)
                    || !matches!(
                        relationship.rel_type.as_str(),
                        rel_types::HEADER | rel_types::FOOTER
                    )
                {
                    return Err(Error::Other(
                        "retired section relationship is not an internal story".into(),
                    ));
                }
                Ok(OpcPackage::resolve_rel_target(
                    &self.doc_part_name,
                    &relationship.target,
                ))
            })
            .collect::<Result<HashSet<_>>>()?;
        let active_ids = self.main_story_uses_of_retired_stories(removed, &retired_targets)?;
        let mut unused_targets = HashSet::new();
        for target in retired_targets {
            let identity = part_name_identity(&target);
            let mut used = false;
            for (owner, relationships) in &self.package.part_rels {
                for relationship in &relationships.items {
                    if !relationship_is_internal(relationship)
                        || part_name_identity(&OpcPackage::resolve_rel_target(
                            owner,
                            &relationship.target,
                        )) != identity
                    {
                        continue;
                    }
                    if part_name_identity(owner) == part_name_identity(&self.doc_part_name) {
                        used |= active_ids.contains(relationship.id.as_str());
                    } else {
                        let xml = self.package.get_part(owner).ok_or_else(|| {
                            Error::Other(format!(
                                "cannot prove incoming section-story ownership from missing {owner}"
                            ))
                        })?;
                        if !xml_relationship_ids_in_order(xml)?.contains(&relationship.id) {
                            return Err(Error::Other(format!(
                                "cannot prove opaque incoming section-story relationship {} from {owner}",
                                relationship.id
                            )));
                        }
                        used = true;
                    }
                }
            }
            if !used {
                if let Some(xml) = self.package.get_part(&target) {
                    let omitted = Self::omit_comment_markers(xml)?;
                    if omitted != xml {
                        set_story_source_xml(self, &target, omitted)?;
                    }
                }
                unused_targets.insert(identity);
            }
        }
        let removable_ids = removed
            .iter()
            .map(|reference| reference.rel_id.as_str())
            .filter(|id| !active_ids.contains(*id))
            .collect::<HashSet<_>>();
        let candidates = self
            .package
            .get_part_rels(&self.doc_part_name)
            .into_iter()
            .flat_map(|relationships| &relationships.items)
            .filter(|relationship| {
                // Any other main-part relationship to an unused part was just
                // shown unused too, so it goes with the retired ones.
                let unused_target = || {
                    !active_ids.contains(relationship.id.as_str())
                        && unused_targets.contains(&part_name_identity(
                            &OpcPackage::resolve_rel_target(
                                &self.doc_part_name,
                                &relationship.target,
                            ),
                        ))
                };
                (removable_ids.contains(relationship.id.as_str()) || unused_target())
                    && relationship_is_internal(relationship)
                    && matches!(
                        relationship.rel_type.as_str(),
                        rel_types::HEADER | rel_types::FOOTER
                    )
            })
            .map(|relationship| {
                (
                    relationship.id.clone(),
                    OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target),
                )
            })
            .filter(|(_, target)| {
                self.identifiers.authored_story_parts.contains(target)
                    || unused_targets.contains(&part_name_identity(target))
            })
            .collect::<Vec<_>>();

        for (relationship_id, target) in candidates {
            // Journaled one by one: an undo puts back the removed items, not
            // a copy of the main part's relationships.
            let owner = self.doc_part_name.clone();
            self.package
                .remove_relationships_where(&owner, |relationship| {
                    relationship.id == relationship_id
                });
            self.identifiers
                .retire_authored_story_relationships(&self.doc_part_name, [relationship_id]);
            let target_identity = part_name_identity(&target);
            let still_referenced = self.package.part_rels.iter().any(|(owner, relationships)| {
                relationships.items.iter().any(|relationship| {
                    relationship_is_internal(relationship)
                        && part_name_identity(&OpcPackage::resolve_rel_target(
                            owner,
                            &relationship.target,
                        )) == target_identity
                })
            });
            if still_referenced {
                continue;
            }
            let released = self
                .package
                .get_part_rels(&target)
                .map(|relationships| relationships.items.clone())
                .unwrap_or_default();
            self.clear_authored_story_relationships(&target);
            self.package.remove_part(&target);
            self.package.remove_part_rels(&target);
            self.package.remove_content_type_override(&target);
            self.identifiers.release_story_part(&target);
            self.identifiers.retire_authored_part(&target);
            self.remove_unreachable_targets(&target, released);
        }
        Ok(())
    }

    /// Remove what a dropped part pointed at once no relationship reaches it,
    /// imported or not: a picture, and in turn what only it pointed at. The
    /// proof is the dropped part's: no internal relationship from any owner,
    /// the package included, still names the target.
    fn remove_unreachable_targets(
        &mut self,
        owner: &str,
        relationships: Vec<oxoffice_opc::relationship::Relationship>,
    ) {
        let mut pending = relationships
            .into_iter()
            .map(|relationship| (owner.to_owned(), relationship))
            .collect::<Vec<_>>();
        while let Some((owner, relationship)) = pending.pop() {
            if !relationship_is_internal(&relationship) {
                continue;
            }
            let target = OpcPackage::resolve_rel_target(&owner, &relationship.target);
            if self.package.get_part(&target).is_none() {
                continue;
            }
            let identity = part_name_identity(&target);
            let reached = std::iter::once(("/", &self.package.package_rels))
                .chain(
                    self.package
                        .part_rels
                        .iter()
                        .map(|(source, relationships)| (source.as_str(), relationships)),
                )
                .any(|(source, relationships)| {
                    relationships.items.iter().any(|candidate| {
                        relationship_is_internal(candidate)
                            && part_name_identity(&OpcPackage::resolve_rel_target(
                                source,
                                &candidate.target,
                            )) == identity
                    })
                });
            if reached {
                continue;
            }
            if let Some(relationships) = self.package.get_part_rels(&target) {
                pending.extend(
                    relationships
                        .items
                        .iter()
                        .cloned()
                        .map(|relationship| (target.clone(), relationship)),
                );
            }
            self.package.remove_part(&target);
            self.package.remove_part_rels(&target);
            self.package.remove_content_type_override(&target);
            self.identifiers.retire_authored_part(&target);
        }
    }

    /// Get the section properties (page size, margins).
    pub fn section_properties(&self) -> Option<&CT_SectPr> {
        self.document.body.sect_pr.as_ref()
    }

    /// Get a mutable reference to section properties, creating defaults if needed.
    pub fn section_properties_mut(&mut self) -> &mut CT_SectPr {
        self.invalidate_layout();
        self.document
            .body
            .sect_pr
            .get_or_insert_with(CT_SectPr::default_letter)
    }

    /// Set page size without validation, preserving the legacy infallible behavior.
    pub fn set_page_size(&mut self, width: Length, height: Length) {
        let section = self.section_properties_mut();
        section.page_width = Some(width.as_twips());
        section.page_height = Some(height.as_twips());
    }

    /// Set page orientation to landscape (swaps width and height if needed).
    pub fn set_landscape(&mut self) {
        let ordinal = self.section_count().saturating_sub(1);
        Section {
            ordinal,
            is_final: true,
            inner: self.section_properties_mut(),
        }
        .set_orientation(ST_PageOrientation::Landscape);
    }

    /// Set page orientation to portrait (swaps width and height if needed).
    pub fn set_portrait(&mut self) {
        let ordinal = self.section_count().saturating_sub(1);
        Section {
            ordinal,
            is_final: true,
            inner: self.section_properties_mut(),
        }
        .set_orientation(ST_PageOrientation::Portrait);
    }

    /// Set page margins without validation, preserving legacy behavior.
    pub fn set_margins(&mut self, top: Length, right: Length, bottom: Length, left: Length) {
        let section = self.section_properties_mut();
        section.margin_top = Some(top.as_twips());
        section.margin_right = Some(right.as_twips());
        section.margin_bottom = Some(bottom.as_twips());
        section.margin_left = Some(left.as_twips());
    }

    /// Set equal-width columns without validation, preserving legacy behavior.
    pub fn set_columns(&mut self, num: u32, spacing: Length) {
        self.section_properties_mut().columns = Some(CT_Columns {
            num: Some(num),
            space: Some(spacing.as_twips()),
            equal_width: Some(true),
            sep: None,
            columns: Vec::new(),
        });
    }

    /// Set header and footer distances without validation for compatibility.
    pub fn set_header_footer_distance(&mut self, header: Length, footer: Length) {
        let section = self.section_properties_mut();
        section.header_distance = Some(header.as_twips());
        section.footer_distance = Some(footer.as_twips());
    }

    /// Set the gutter margin without validation, preserving legacy behavior.
    pub fn set_gutter(&mut self, gutter: Length) {
        self.section_properties_mut().gutter = Some(gutter.as_twips());
    }

    /// Enable or disable different first page header/footer.
    pub fn set_different_first_page(&mut self, val: bool) {
        let ordinal = self.section_count().saturating_sub(1);
        Section {
            ordinal,
            is_final: true,
            inner: self.section_properties_mut(),
        }
        .set_different_first_page(val);
    }
}

#[cfg(test)]
mod tests {
    use super::super::headers_footers::tests::{state, with_header_picture};
    use super::*;

    /// A section story edit of both sections that wrote the first section's
    /// properties (held by a body paragraph) and the final one, added a
    /// story part and dropped another, then refused or panicked.
    fn edit_both_sections_then(document: &mut Document, panic: bool) -> Result<()> {
        let units = [
            crate::EditUnit::Parts,
            crate::EditUnit::Settings,
            crate::EditUnit::SectionProperties { index: 0 },
            crate::EditUnit::SectionProperties { index: 1 },
        ];
        document.edit_in_place(&units, |document| {
            document.create_section_story_in_place(
                0,
                HeaderFooterKind::Header,
                HdrFtrType::First,
            )?;
            document.create_section_story_in_place(
                1,
                HeaderFooterKind::Footer,
                HdrFtrType::Even,
            )?;
            document.inherit_section_story_body(
                0,
                HeaderFooterKind::Header,
                HdrFtrType::Default,
            )?;
            assert!(
                document.section(0).unwrap().properties().title_pg == Some(true),
                "the first section's properties changed"
            );
            if panic {
                panic!("the section edit panicked");
            }
            Err(Error::Other("refused".into()))
        })
    }

    #[test]
    fn a_refused_section_story_edit_leaves_the_document_as_it_was() {
        let mut document = with_header_picture();
        let before = state(&document);
        let error = edit_both_sections_then(&mut document, false).unwrap_err();
        assert_eq!(error.to_string(), "refused");
        assert_eq!(state(&document), before);
    }

    #[test]
    fn a_panic_in_a_section_story_edit_leaves_the_document_as_it_was() {
        let mut document = with_header_picture();
        let before = state(&document);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            edit_both_sections_then(&mut document, true)
        }));
        assert!(unwound.is_err());
        assert_eq!(state(&document), before);
    }

    /// The main part's header relationships.
    fn header_relationships(document: &Document) -> usize {
        document
            .package
            .get_part_rels(&document.doc_part_name)
            .into_iter()
            .flat_map(|relationships| &relationships.items)
            .filter(|relationship| relationship.rel_type == rel_types::HEADER)
            .count()
    }

    #[test]
    fn a_refused_prune_of_the_main_part_relationships_is_undone_item_by_item() {
        // The prune journals the removed relationship alone: alone, and
        // before an edit that records the main part's relationships whole,
        // an undo puts them back as they were.
        for record_after in [false, true] {
            let mut document = with_header_picture();
            let before = state(&document);
            let headers = header_relationships(&document);
            let units = [
                crate::EditUnit::Parts,
                crate::EditUnit::Settings,
                crate::EditUnit::SectionProperties { index: 0 },
                crate::EditUnit::SectionProperties { index: 1 },
            ];
            let error = document
                .edit_in_place(&units, |document| {
                    document.inherit_section_story_body(
                        0,
                        HeaderFooterKind::Header,
                        HdrFtrType::Default,
                    )?;
                    assert_eq!(
                        header_relationships(document),
                        headers - 1,
                        "the unreachable header was pruned"
                    );
                    if record_after {
                        // A later edit that records the main part's
                        // relationships whole and adds one.
                        let owner = document.doc_part_name.clone();
                        document.package.record_part_rels(&owner);
                        document
                            .package
                            .get_or_create_part_rels(&owner)
                            .add_external(rel_types::HYPERLINK, "https://example.com/");
                    }
                    Err::<(), _>(Error::Other("refused".into()))
                })
                .unwrap_err();
            assert_eq!(error.to_string(), "refused");
            assert_eq!(state(&document), before, "record after: {record_after}");
        }
    }

    #[test]
    fn a_section_story_call_refused_up_front_leaves_the_document_as_it_was() {
        let mut document = with_header_picture();
        let before = state(&document);
        assert!(
            document
                .create_section_story(7, HeaderFooterKind::Header, HdrFtrType::Default)
                .is_err()
        );
        let header = document
            .section_story(0, HeaderFooterKind::Header, HdrFtrType::Default)
            .unwrap()
            .unwrap()
            .story;
        assert!(
            document
                .link_section_story(1, HeaderFooterKind::Footer, HdrFtrType::Default, &header)
                .is_err()
        );
        assert_eq!(state(&document), before);
    }

    #[test]
    fn section_stories_edited_in_place_save_what_the_staged_routes_save() {
        type Edit = fn(&mut Document, bool) -> Result<()>;
        let edits: [(&str, Edit); 6] = [
            ("create first", |document, staged| {
                let (index, kind, hdr_type) = (0, HeaderFooterKind::Header, HdrFtrType::First);
                if staged {
                    document.create_section_story_staged(index, kind, hdr_type)
                } else {
                    document.create_section_story(index, kind, hdr_type)
                }
                .map(drop)
            }),
            ("create even", |document, staged| {
                let (index, kind, hdr_type) = (1, HeaderFooterKind::Footer, HdrFtrType::Even);
                if staged {
                    document.create_section_story_staged(index, kind, hdr_type)
                } else {
                    document.create_section_story(index, kind, hdr_type)
                }
                .map(drop)
            }),
            ("link", |document, staged| {
                let header = document
                    .section_story(0, HeaderFooterKind::Header, HdrFtrType::Default)?
                    .unwrap()
                    .story;
                let (index, kind, hdr_type) = (1, HeaderFooterKind::Header, HdrFtrType::First);
                if staged {
                    document.link_section_story_staged(index, kind, hdr_type, &header)
                } else {
                    document.link_section_story(index, kind, hdr_type, &header)
                }
                .map(drop)
            }),
            ("unlink", |document, staged| {
                let (index, kind, hdr_type) = (1, HeaderFooterKind::Header, HdrFtrType::Default);
                if staged {
                    let source = document
                        .section_story(index, kind, hdr_type)?
                        .unwrap()
                        .story;
                    document.replace_section_story_staged(index, kind, hdr_type, &source)
                } else {
                    document.unlink_section_story(index, kind, hdr_type)
                }
                .map(drop)
            }),
            ("inherit", |document, staged| {
                let (index, kind, hdr_type) = (0, HeaderFooterKind::Header, HdrFtrType::Default);
                if staged {
                    document.inherit_section_story_staged(index, kind, hdr_type)
                } else {
                    document.inherit_section_story(index, kind, hdr_type)
                }
            }),
            ("remove", |document, staged| {
                let (index, kind, hdr_type) = (0, HeaderFooterKind::Header, HdrFtrType::Default);
                if staged {
                    document.remove_section_story_staged(index, kind, hdr_type)
                } else {
                    document.remove_section_story(index, kind, hdr_type)
                }
                .map(drop)
            }),
        ];
        for (name, edit) in edits {
            let mut document = with_header_picture();
            let mut staged = document.clone_for_staging();
            edit(&mut document, false).unwrap();
            edit(&mut staged, true).unwrap();
            assert_eq!(
                document.to_bytes().unwrap(),
                staged.to_bytes().unwrap(),
                "{name}"
            );
        }
    }
}
