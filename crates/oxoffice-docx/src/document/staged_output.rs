//! Preparing a staged package and reopening it.

use super::*;

#[cfg(debug_assertions)]
fn assert_same_reopen(reopened: &Result<Document>, through_zip: &Result<Document>) {
    match (reopened, through_zip) {
        (Ok(reopened), Ok(expected)) => {
            assert_eq!(reopened.document, expected.document);
            assert_eq!(reopened.identifiers, expected.identifiers);
            assert!(reopened.package.same_package_as(&expected.package));
            let saved = |document: &Document| {
                let mut copy = document.clone_for_debug_reference();
                copy.to_bytes().map_err(|error| error.to_string())
            };
            assert_eq!(saved(reopened), saved(expected));
        }
        (Err(reopened), Err(expected)) => {
            assert_eq!(reopened.to_string(), expected.to_string());
        }
        (reopened, expected) => panic!(
            "in-memory reopen {} zip {}",
            reopened.as_ref().map(|_| "ok").unwrap_or("err"),
            expected
                .as_ref()
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "ok".to_string())
        ),
    }
}

impl Document {
    fn ensure_referenced_hyperlink_style(&mut self) -> Result<()> {
        if self.styles.get_by_id("Hyperlink").is_some() {
            return Ok(());
        }
        let body = self
            .document
            .body
            .content
            .iter()
            .filter_map(|item| match item {
                BodyContent::Paragraph(paragraph) => Some(WordXmlOwner::Paragraph(paragraph)),
                BodyContent::Table(table) => Some(WordXmlOwner::Table(table)),
                BodyContent::ContentControl(control) => Some(WordXmlOwner::Control(control)),
                BodyContent::RawXml(_) => None,
            })
            .collect();
        let mut referenced = owners_reference_hyperlink_style(body);
        for (rel_id, is_header) in self.header_footer_rel_ids() {
            if !referenced && let Some(story) = self.load_header_footer(&rel_id, is_header) {
                referenced = owners_reference_hyperlink_style(
                    story
                        .paragraphs
                        .iter()
                        .map(WordXmlOwner::Paragraph)
                        .collect(),
                );
            }
        }
        if !referenced {
            referenced = owners_reference_hyperlink_style(
                self.footnotes
                    .footnotes
                    .iter()
                    .flat_map(|note| &note.paragraphs)
                    .map(WordXmlOwner::Paragraph)
                    .collect(),
            );
        }
        if !referenced
            && let Ok(Some(part)) = self.note_part_name(StoryKind::Endnote)
            && let Some(source) = self.package.get_part(&part)
            && let Ok(notes) = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(source)
        {
            referenced = owners_reference_hyperlink_style(
                notes
                    .footnotes
                    .iter()
                    .flat_map(|note| &note.paragraphs)
                    .map(WordXmlOwner::Paragraph)
                    .collect(),
            );
        }
        if referenced {
            self.add_style(
                StyleBuilder::character("Hyperlink", "Hyperlink").run_properties(CT_RPr {
                    color: Some("0563C1".into()),
                    underline: Some(oxoffice_docx_oxml::shared::ST_Underline::Single),
                    ..Default::default()
                }),
            )?;
        }
        Ok(())
    }

    pub(crate) fn prepare_staged_package(&mut self) -> Result<bool> {
        self.prepare_staged_package_in(None)
    }

    pub(crate) fn prepare_staged_template_package<'a>(
        &mut self,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'a>,
        lineage: &mut crate::template::TemplateWordLineage<'a>,
        sources: &mut TemplateMainSources,
    ) -> Result<bool> {
        let result = self.prepare_staged_package_in(Some((operation, lineage, sources)));
        lineage.finish_preparation();
        result
    }

    fn prepare_staged_package_in<'a>(
        &mut self,
        mut template: Option<(
            &mut oxoffice_docx_oxml::document::ScopedWordOperation<'a>,
            &mut crate::template::TemplateWordLineage<'a>,
            &mut TemplateMainSources,
        )>,
    ) -> Result<bool> {
        let normalized = self.prepare_current_field_models(template.as_mut().map(
            |(operation, lineage, sources)| {
                (
                    &mut **operation,
                    &mut **lineage,
                    sources.reader_source.as_deref(),
                    sources.comments_reader_source.as_deref(),
                )
            },
        ))?;
        if let Some((_, lineage, _)) = &mut template {
            lineage.finish_preparation();
        }
        self.ensure_referenced_hyperlink_style()?;
        self.preflight_flush_required_bundles()?;
        if let Some((operation, lineage, sources)) = template.as_mut() {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.canonicalize_typed_identifiers(Some(&mut writer))?;
        } else {
            self.canonicalize_typed_identifiers(None)?;
        }
        let retained_signature_invalidated =
            self.retained_package_signature_would_be_invalidated()?;
        if let Some((operation, lineage, sources)) = template.as_mut() {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.flush_dirty_related_story_models_in(Some(&mut writer))?;
        } else {
            self.flush_dirty_related_story_models()?;
        }
        if let Some((operation, lineage, sources)) = template.as_mut() {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.canonicalize_drawing_ids(Some(&mut writer))?;
        } else {
            self.canonicalize_drawing_ids(None)?;
        }
        if let Some((operation, lineage, sources)) = template.as_mut() {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.refresh_related_story_caches_in(Some(&mut writer))?;
        } else {
            self.refresh_related_story_caches()?;
        }
        self.package_signatures_invalidated |= retained_signature_invalidated;
        let result = if let Some((operation, lineage, sources)) = template {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.flush_to_package_in(Some(&mut writer))
        } else {
            self.flush_to_package()
        };
        result.map(|()| normalized)
    }

    /// Stage every public output. A comments model that no longer matches its
    /// part is written back even when no mutation marked it dirty. An
    /// unchanged comments part keeps its producer bytes, as styles and the
    /// other modelled parts do, so a save without a comment edit neither
    /// rewrites it nor invalidates a package signature over it.
    pub(crate) fn prepare_staged_output(&mut self) -> Result<bool> {
        if let (Some(comments), Some(part_name)) = (&self.comments, &self.comments_part_name) {
            self.comments_dirty |= self
                .package
                .get_part(part_name)
                .and_then(|xml| oxoffice_docx_oxml::comments::CT_Comments::from_xml(xml).ok())
                .as_ref()
                != Some(comments);
        }
        self.prepare_staged_package()
    }

    pub(crate) fn prepare_and_reopen_staged(mut self) -> Result<Self> {
        self.prepare_staged_package()?;
        self.reopen_prepared_staged()
    }

    pub(crate) fn reopen_prepared_staged(self) -> Result<Self> {
        self.reopen_prepared_staged_with_limits(PackageReadLimits::UNBOUNDED)
    }

    pub(crate) fn reopen_prepared_staged_with_limits(
        self,
        limits: PackageReadLimits,
    ) -> Result<Self> {
        let in_memory = limits == PackageReadLimits::UNBOUNDED && self.read_package().is_some();
        #[cfg(debug_assertions)]
        let through_zip = in_memory.then(|| self.clone_for_debug_reference());
        let reopened = self.reopen_staged(in_memory, limits);
        #[cfg(debug_assertions)]
        if let Some(copy) = through_zip {
            let expected = copy.reopen_staged(false, limits);
            assert_same_reopen(&reopened, &expected);
        }
        reopened
    }

    fn install_carried_body_scans(&self, mut carried: MainBodyScans) {
        // Writer eligibility is checked against the current model and frame
        // when offered. It is independent of the stricter address-scan carry.
        // A changed owner span must not discard that current writer memo.
        {
            let mut scans = self
                .main_body_scans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The opening may already have qualified a newer writer record.
            if scans.serialized.is_none() {
                scans.serialized = carried.serialized.clone();
            }
        }
        if carried.frame.is_none() {
            return;
        }
        let Ok((xml, content, children)) = self.main_part_with_body_spans() else {
            return;
        };
        if carried.head.as_slice() != &xml[..content.start]
            || carried.tail.as_slice() != &xml[content.end..]
        {
            return;
        }
        if carried.blocks.len() != children.len()
            || carried
                .blocks
                .iter()
                .zip(&children)
                .any(|(scan, range)| scan.bytes.as_slice() != &xml[range.clone()])
        {
            carried.fingerprint = None;
        }
        // main_story_layout checks the current frame and each aligned child's
        // exact bytes and item coverage before it consumes these read facts.
        // Keep the newly qualified writer memo independently of those facts.
        let mut scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        carried.serialized = scans.serialized.take();
        *scans = carried;
    }

    fn reopen_staged(self, in_memory: bool, limits: PackageReadLimits) -> Result<Self> {
        let carried_scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let carried_blocks = self.main_part_blocks.clone();
        let provenance = self.identifiers.clone();
        let custom_properties_owned = self.custom_properties_owned;
        let settings_owned = self.settings_owned;
        let owned_settings = settings_owned.then(|| self.settings.clone());
        let web_settings_owned = self.web_settings_owned;
        let owned_web_settings = web_settings_owned.then(|| self.web_settings.clone());
        let comments_owned = self.comments_owned;
        let comments_extended_owned = self.comments_extended_owned;
        let embedded_invalidated_signatures = self.embedded_invalidated_signatures.clone();
        let freed_image_relationships = self.freed_image_relationships.clone();
        let package_signatures_invalidated = self.package_signatures_invalidated;
        let source_path = self.source_path.clone();
        let opened = self.parsed.as_deref().filter(|_| in_memory);
        let mut reopened = match opened {
            Some(opened) => Self::from_package_reusing(
                self.package.reread(&opened.package)?,
                true,
                Some((&provenance, opened)),
                carried_blocks.as_ref(),
                carried_scans.serialized.as_ref(),
                self.styles_part_cache.as_ref(),
                None,
            )?,
            None => {
                let mut output = std::io::Cursor::new(Vec::new());
                self.package.write_to(&mut output)?;
                Self::from_bytes_with_limits(output.get_ref(), limits)?
            }
        };
        reopened
            .identifiers
            .reject_new_repeated_drawing_ids(&provenance)?;
        reopened
            .identifiers
            .reject_new_repeated_bookmark_ids(&provenance)?;
        reopened.identifiers.reconcile_provenance(&provenance);
        reopened.restore_live_numbering(
            &self.published_abstract_by_live,
            &self.published_instance_by_live,
            &provenance,
        )?;
        reopened.custom_properties_owned = custom_properties_owned;
        reopened.settings_owned = settings_owned;
        if let Some(settings) = owned_settings {
            reopened.settings = settings;
        }
        reopened.web_settings_owned = web_settings_owned;
        if let Some(web_settings) = owned_web_settings {
            reopened.web_settings = web_settings;
        }
        reopened.comments_owned = comments_owned;
        reopened.comments_extended_owned = comments_extended_owned;
        reopened.embedded_invalidated_signatures = embedded_invalidated_signatures;
        reopened.freed_image_relationships = freed_image_relationships;
        reopened.package_signatures_invalidated = package_signatures_invalidated;
        reopened.source_path = source_path;
        reopened.install_carried_body_scans(carried_scans);
        reopened.install_carried_blocks(carried_blocks);
        Ok(reopened)
    }

    fn restore_live_numbering(
        &mut self,
        abstract_map: &HashMap<u32, u32>,
        instance_map: &HashMap<u32, u32>,
        provenance: &DocumentIdentifiers,
    ) -> Result<()> {
        if abstract_map.is_empty() && instance_map.is_empty() {
            return Ok(());
        }
        let numbering = self
            .numbering
            .as_mut()
            .ok_or_else(|| Error::Other("published numbering model is missing".into()))?;
        for published in abstract_map.values() {
            if !numbering
                .abstract_nums
                .iter()
                .any(|definition| definition.abstract_num_id == *published)
            {
                return Err(Error::Other(
                    "published abstract numbering ID is missing".into(),
                ));
            }
        }
        for published in instance_map.values() {
            if !numbering
                .nums
                .iter()
                .any(|instance| instance.num_id == *published)
            {
                return Err(Error::Other(
                    "published numbering instance ID is missing".into(),
                ));
            }
        }
        let abstract_inverse = abstract_map
            .iter()
            .map(|(live, published)| (*published, *live))
            .collect::<HashMap<_, _>>();
        let instance_inverse = instance_map
            .iter()
            .map(|(live, published)| (*published, *live))
            .collect::<HashMap<_, _>>();
        let published_ids = numbering
            .nums
            .iter()
            .map(|instance| instance.num_id)
            .collect::<HashSet<_>>();
        let mut missing_body_id = false;
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            if let Some(id) = paragraph
                .properties
                .as_mut()
                .and_then(|properties| properties.num_id.as_mut())
                && let Some(live) = instance_inverse.get(id)
            {
                missing_body_id |= !published_ids.contains(id);
                *id = *live;
            }
        });
        if missing_body_id {
            return Err(Error::Other(
                "published body numbering ID is missing".into(),
            ));
        }
        for definition in &mut numbering.abstract_nums {
            if let Some(live) = abstract_inverse.get(&definition.abstract_num_id) {
                definition.abstract_num_id = *live;
            }
        }
        for instance in &mut numbering.nums {
            if let Some(live) = instance_inverse.get(&instance.num_id) {
                instance.num_id = *live;
            }
            if let Some(live) = abstract_inverse.get(&instance.abstract_num_id) {
                instance.abstract_num_id = *live;
            }
        }
        self.identifiers.abstract_numbering_ids = numbering
            .abstract_nums
            .iter()
            .map(|definition| definition.abstract_num_id)
            .collect();
        self.identifiers.numbering_instance_ids = numbering
            .nums
            .iter()
            .map(|instance| instance.num_id)
            .collect();
        self.identifiers.preserved_abstract_numbering_ids =
            provenance.preserved_abstract_numbering_ids.clone();
        self.identifiers.preserved_numbering_instance_ids =
            provenance.preserved_numbering_instance_ids.clone();
        self.published_abstract_by_live = abstract_map.clone();
        self.published_instance_by_live = instance_map.clone();
        Ok(())
    }
}
