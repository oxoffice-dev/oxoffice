//! Checked relationships, part bundles and chart packages.

use super::*;

impl Document {
    /// Make sure `part_name` is reachable from the main document: it needs a
    /// relationship of `rel_type` and a content-type override.
    pub(crate) fn ensure_part_relationship_checked(
        &mut self,
        part_name: &str,
        rel_type: &str,
        content_type: &str,
    ) -> Result<()> {
        let doc_part_name = self.doc_part_name.clone();
        let already_linked =
            self.package
                .get_part_rels(&doc_part_name)
                .is_some_and(|relationships| {
                    relationships.items.iter().any(|relationship| {
                        relationship.rel_type == rel_type
                            && relationship_is_internal(relationship)
                            && OpcPackage::resolve_rel_target(&doc_part_name, &relationship.target)
                                == part_name
                    })
                });
        let pending_id = if already_linked {
            None
        } else {
            Some(
                self.identifiers
                    .reserve_relationship_id_checked(&doc_part_name)?,
            )
        };
        self.add_override_journaled(part_name, content_type);
        self.identifiers.register_content_type_override(part_name);
        if let Some(id) = pending_id {
            // Relationship targets are relative to the source part's directory.
            let target = relative_target(&doc_part_name, part_name);
            self.add_relationship_journaled(Some(&doc_part_name), &id, rel_type, &target);
        }
        Ok(())
    }

    pub(crate) fn reserve_document_part_bundle(
        &mut self,
        existing: Option<&str>,
        preferred: &str,
        rel_type: &str,
        content_type: &str,
    ) -> Result<String> {
        self.identifiers.observe_package_graph(&self.package)?;
        let part_name = match existing {
            Some(part_name) => part_name.to_owned(),
            None => self.identifiers.reserve_preferred_part_name(preferred)?,
        };
        let owner = self.doc_part_name.clone();
        let already_linked = self
            .package
            .get_part_rels(&owner)
            .is_some_and(|relationships| {
                relationships.items.iter().any(|relationship| {
                    relationship.rel_type == rel_type
                        && relationship_is_internal(relationship)
                        && OpcPackage::resolve_rel_target(&owner, &relationship.target) == part_name
                })
            });
        let pending_id = if already_linked {
            None
        } else {
            Some(self.identifiers.reserve_relationship_id_checked(&owner)?)
        };
        self.add_override_journaled(&part_name, content_type);
        self.identifiers.register_content_type_override(&part_name);
        if let Some(id) = pending_id {
            let target = relative_target(&owner, &part_name);
            self.add_relationship_journaled(Some(&owner), &id, rel_type, &target);
        }
        Ok(part_name)
    }

    pub(super) fn reserve_core_properties_bundle(&mut self) -> Result<String> {
        let existing = self.core_properties_part_name.clone();
        let part_name = self.reserve_package_properties_bundle(
            existing.as_deref(),
            DEFAULT_CORE_PROPERTIES_PART,
            rel_types::CORE_PROPERTIES,
            content_types::CORE_PROPERTIES,
        )?;
        self.core_properties_part_name = Some(part_name.clone());
        Ok(part_name)
    }

    pub(super) fn reserve_application_properties_bundle(&mut self) -> Result<String> {
        let existing = self.application_properties_part_name.clone();
        let part_name = self.reserve_package_properties_bundle(
            existing.as_deref(),
            DEFAULT_APP_PROPERTIES_PART,
            rel_types::EXTENDED_PROPERTIES,
            content_types::EXTENDED_PROPERTIES,
        )?;
        self.application_properties_part_name = Some(part_name.clone());
        Ok(part_name)
    }

    pub(super) fn reserve_custom_properties_bundle(&mut self) -> Result<String> {
        let existing = self.custom_properties_part_name.clone();
        let part_name = self.reserve_package_properties_bundle(
            existing.as_deref(),
            "/docProps/custom.xml",
            rel_types::CUSTOM_PROPERTIES,
            content_types::CUSTOM_PROPERTIES,
        )?;
        self.custom_properties_part_name = Some(part_name.clone());
        Ok(part_name)
    }

    fn reserve_package_properties_bundle(
        &mut self,
        existing: Option<&str>,
        preferred: &str,
        rel_type: &str,
        content_type: &str,
    ) -> Result<String> {
        self.identifiers.observe_package_graph(&self.package)?;
        let part_name = match existing {
            Some(part_name) => part_name.to_owned(),
            None => self.identifiers.reserve_preferred_part_name(preferred)?,
        };
        let already_linked = self.package.package_rels.items.iter().any(|relationship| {
            relationship.rel_type == rel_type
                && relationship_is_internal(relationship)
                && OpcPackage::resolve_rel_target("/", &relationship.target) == part_name
        });
        let pending_id = if already_linked {
            None
        } else {
            Some(self.identifiers.reserve_relationship_id_checked("/")?)
        };
        self.add_override_journaled(&part_name, content_type);
        self.identifiers.register_content_type_override(&part_name);
        if let Some(id) = pending_id {
            let target = part_name.strip_prefix('/').unwrap_or(&part_name).to_owned();
            self.add_relationship_journaled(None, &id, rel_type, &target);
        }
        Ok(part_name)
    }

    pub(crate) fn add_internal_relationship_checked(
        &mut self,
        owner: &str,
        rel_type: &str,
        target: &str,
    ) -> Result<String> {
        let id = self.identifiers.reserve_relationship_id_checked(owner)?;
        self.add_relationship_journaled(Some(owner), &id, rel_type, target);
        Ok(id)
    }

    pub(crate) fn add_relative_internal_relationship_checked(
        &mut self,
        owner: &str,
        rel_type: &str,
        target_part: &str,
    ) -> Result<String> {
        let id = self.add_internal_relationship_checked(
            owner,
            rel_type,
            &relative_target(owner, target_part),
        )?;
        if owner != self.doc_part_name {
            self.identifiers.author_story_relationship(owner, &id);
        }
        Ok(id)
    }

    pub(super) fn add_image_relationship_checked(
        &mut self,
        owner: &str,
        image_data: &[u8],
        filename: &str,
    ) -> Result<String> {
        let (part_name, format) = self.reserve_image_part(image_data, filename)?;
        self.install_reserved_image_part(&part_name, image_data, format);
        self.add_relative_internal_relationship_checked(owner, rel_types::IMAGE, &part_name)
    }

    /// Adds an external relationship to `target`, refusing a target that
    /// holds a character XML 1.0 cannot carry.
    pub(crate) fn add_external_relationship_checked(
        &mut self,
        owner: &str,
        rel_type: &str,
        target: &str,
    ) -> Result<String> {
        oxoffice_core::xml::reject_non_xml_characters("external relationship target", target)?;
        let id = self.identifiers.reserve_relationship_id_checked(owner)?;
        // An in-place edit takes the edge back through the reservation
        // journal, without a copy of the owner's relationships.
        self.add_external_relationship_journaled(owner, &id, rel_type, target);
        if owner != self.doc_part_name {
            self.identifiers.author_story_relationship(owner, &id);
        }
        Ok(id)
    }

    pub(crate) fn add_html_image_relationship_to_story(
        &mut self,
        story: &StoryId,
        image_data: &[u8],
        filename: &str,
    ) -> Result<String> {
        self.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let relationship_id = self.add_image_relationship_checked(&owner, image_data, filename)?;
        if owner == self.doc_part_name && story.kind != StoryKind::Body {
            self.identifiers
                .register_nested_story_relationship(story, relationship_id.clone());
        }
        Ok(relationship_id)
    }

    pub(crate) fn add_html_hyperlink_relationship_to_story(
        &mut self,
        story: &StoryId,
        url: &str,
    ) -> Result<String> {
        self.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let relationship_id =
            self.add_external_relationship_checked(&owner, rel_types::HYPERLINK, url)?;
        if owner == self.doc_part_name && story.kind != StoryKind::Body {
            self.identifiers
                .register_nested_story_relationship(story, relationship_id.clone());
        }
        Ok(relationship_id)
    }

    pub(crate) fn reserve_html_drawing_id(&mut self, story: &StoryId) -> Result<u32> {
        self.story_source_and_owner(story)?;
        self.identifiers.reserve_drawing_id()
    }

    /// Stage a chart, its editable workbook, and the Word drawing that reaches them.
    pub(super) fn add_chart_package(
        &mut self,
        kind: ChartKind,
        data: &ChartData,
        width: Length,
        height: Length,
    ) -> Result<()> {
        if width.to_emu() <= 0 || height.to_emu() <= 0 {
            return Err(Error::Other(
                "chart width and height must be positive".to_owned(),
            ));
        }

        let mut candidate = self.clone_for_staging();
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        candidate.ensure_authored_chart_theme()?;
        let chart_part = candidate
            .identifiers
            .reserve_part_name("/word/charts", "chart", "xml")?;
        let workbook_part =
            candidate
                .identifiers
                .reserve_part_name("/word/embeddings", "Workbook", "xlsx")?;
        let document_part = candidate.doc_part_name.clone();
        let document_relationship_id = candidate
            .add_relative_internal_relationship_checked(
                &document_part,
                rel_types::CHART,
                &chart_part,
            )
            .map_err(|error| {
                Error::Other(format!("chart relationship allocation failed: {error}"))
            })?;
        let workbook_relationship_id = candidate
            .identifiers
            .reserve_relationship_id_checked(&chart_part)
            .map_err(|error| {
                Error::Other(format!(
                    "chart workbook relationship allocation failed: {error}"
                ))
            })?;
        candidate
            .package
            .get_or_create_part_rels(&chart_part)
            .add_with_id(
                &workbook_relationship_id,
                rel_types::PACKAGE,
                &relative_target(&chart_part, &workbook_part),
            );
        let (chart_xml, workbook_bytes) =
            oxoffice_chart::authored_chart_parts(kind, data, &workbook_relationship_id)
                .map_err(|error| Error::Other(format!("invalid chart data: {error}")))?;

        let mut inline =
            CT_Inline::new_chart(&document_relationship_id, width.to_emu(), height.to_emu());
        inline.doc_pr_id = candidate.identifiers.reserve_drawing_id()?;
        let drawing = CT_Drawing::inline(inline);
        let run = CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        };
        let mut paragraph = CT_P::new();
        paragraph.runs.push(run);
        candidate
            .document
            .body
            .content
            .push(BodyContent::Paragraph(Box::new(paragraph)));
        candidate.document.to_xml()?;

        candidate.package.set_part(&chart_part, chart_xml);
        candidate.package.set_part(&workbook_part, workbook_bytes);
        candidate
            .package
            .content_types
            .add_override(&chart_part, content_types::CHART);
        candidate
            .identifiers
            .register_content_type_override(&chart_part);
        candidate
            .package
            .content_types
            .add_override(&workbook_part, content_types::EMBEDDED_WORKBOOK);
        candidate
            .identifiers
            .register_content_type_override(&workbook_part);

        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    fn ensure_authored_chart_theme(&mut self) -> Result<()> {
        let document_part = self.doc_part_name.clone();
        let reusable = self
            .package
            .get_part_rels(&document_part)
            .and_then(|relationships| {
                relationships.items.iter().find(|relationship| {
                    relationship.rel_type == rel_types::THEME
                        && relationship_is_internal(relationship)
                })
            })
            .and_then(|relationship| {
                let part_name =
                    OpcPackage::resolve_rel_target(&document_part, &relationship.target);
                if self.package.content_types.content_type_for(&part_name)
                    != Some(content_types::THEME)
                {
                    return None;
                }
                let bytes = if self.theme_dirty
                    && self.theme_part_name.as_deref() == Some(part_name.as_str())
                {
                    self.theme.as_ref()?.to_xml().ok()?
                } else {
                    self.package.get_part(&part_name)?.to_vec()
                };
                let theme = oxoffice_drawing::theme::CT_OfficeStyleSheet::from_xml(&bytes).ok()?;
                Some((part_name, theme, bytes))
            });
        if let Some((part_name, theme, bytes)) = reusable {
            if self.theme_dirty && self.theme_part_name.as_deref() == Some(part_name.as_str()) {
                self.package.set_part(&part_name, bytes);
            }
            self.theme = Some(theme);
            self.theme_part_name = Some(part_name);
            return Ok(());
        }

        let theme = oxoffice_drawing::theme::CT_OfficeStyleSheet::office_default();
        let theme_xml = theme
            .to_xml()
            .map_err(|error| Error::Other(format!("invalid default Office theme: {error}")))?;
        self.identifiers.observe_package_graph(&self.package)?;
        let theme_part = self
            .identifiers
            .reserve_preferred_part_name(DEFAULT_THEME_PART)?;
        self.package.set_part(&theme_part, theme_xml);
        self.package
            .content_types
            .add_override(&theme_part, content_types::THEME);
        self.identifiers.register_content_type_override(&theme_part);

        let target = relative_target(&document_part, &theme_part);
        let relationships = self.package.get_or_create_part_rels(&document_part);
        if let Some(relationship) = relationships
            .items
            .iter_mut()
            .find(|relationship| relationship.rel_type == rel_types::THEME)
        {
            relationship.target = target;
            relationship.target_mode = None;
        } else {
            let id = self
                .identifiers
                .reserve_relationship_id_checked(&document_part)?;
            relationships.add_with_id(&id, rel_types::THEME, &target);
        }
        self.theme = Some(theme);
        self.theme_part_name = Some(theme_part);
        self.theme_dirty = false;
        Ok(())
    }
}
