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

/// Whether an element of `xml` has a `val` attribute, whatever its prefix,
/// quotes, spacing or character references, whose value is `value`.
fn part_has_val(xml: &[u8], value: &str) -> bool {
    let mut reader = quick_xml::Reader::from_reader(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                let named = element.attributes().flatten().any(|attribute| {
                    oxoffice_core::xml::local_name(attribute.key.as_ref()) == b"val"
                        && oxoffice_core::xml::attribute_text(&attribute)
                            .is_ok_and(|text| text == value)
                });
                if named {
                    return true;
                }
            }
            Ok(Event::Eof) | Err(_) => return false,
            Ok(_) => {}
        }
    }
}

impl Document {
    /// Whether a word part the package holds now, as read or as last saved,
    /// names the style `id` as a value: the producer's own reference.
    fn stored_story_parts_name_style(&self, id: &str) -> bool {
        self.package.parts.iter().any(|(name, bytes)| {
            name.starts_with("/word/")
                && name.ends_with(".xml")
                && !name.ends_with("/styles.xml")
                && part_has_val(bytes, id)
        })
    }

    /// Add the Hyperlink style and the footnote mark style when a run of
    /// any story names one the document does not define, as `add_hyperlink`
    /// and [`crate::Paragraph::add_footnote_ref`] do. The stories are
    /// scanned once for both.
    ///
    /// On a localized file, a run that names one of those built-in ids the
    /// styles part does not define takes the id of the style carrying its
    /// built-in name (`Lienhypertexte` for `Hyperlink` in French), and no
    /// second style of that name is added: the session's runs of the body
    /// and of the notes are renamed here, the other stories' as they are
    /// edited.
    fn ensure_referenced_character_styles(&mut self) -> Result<()> {
        self.rename_localized_run_styles();
        let mut missing = Vec::new();
        if self.styles.get_by_id("Hyperlink").is_none()
            && built_in_character_style_id(&self.styles, "Hyperlink", "Hyperlink").is_none()
        {
            missing.push((
                "Hyperlink",
                StyleBuilder::character("Hyperlink", "Hyperlink").run_properties(CT_RPr {
                    color: Some("0563C1".into()),
                    underline: Some(oxoffice_docx_oxml::shared::ST_Underline::Single),
                    ..Default::default()
                }),
            ));
        }
        // Without a note, no footnote reference names the mark style. A file
        // that already names it without defining it (Google Docs does, in its
        // notes) keeps its styles as written: only a reference this session
        // added brings the style.
        if built_in_character_style_id(&self.styles, "FootnoteReference", "footnote reference")
            .is_none()
            && self
                .footnotes
                .footnotes
                .iter()
                .any(|note| note.note_type == oxoffice_docx_oxml::footnotes::NoteType::Normal)
            && !self.stored_story_parts_name_style("FootnoteReference")
        {
            missing.push((
                "FootnoteReference",
                note_reference_style(StoryKind::Footnote, &self.styles),
            ));
        }
        if missing.is_empty() {
            return Ok(());
        }
        let wanted = missing.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let found = vec![Cell::new(false); wanted.len()];
        let done = |found: &[Cell<bool>]| found.iter().all(Cell::get);
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
        owners_reference_styles(body, &wanted, &found);
        for (rel_id, is_header) in self.header_footer_rel_ids() {
            if !done(&found)
                && let Some(story) = self.load_header_footer(&rel_id, is_header)
            {
                owners_reference_styles(
                    story
                        .paragraphs
                        .iter()
                        .map(WordXmlOwner::Paragraph)
                        .collect(),
                    &wanted,
                    &found,
                );
            }
        }
        if !done(&found) {
            owners_reference_styles(
                self.footnotes
                    .footnotes
                    .iter()
                    .flat_map(|note| &note.paragraphs)
                    .map(WordXmlOwner::Paragraph)
                    .collect(),
                &wanted,
                &found,
            );
        }
        if !done(&found)
            && let Ok(Some(part)) = self.note_part_name(StoryKind::Endnote)
            && let Some(source) = self.package.get_part(&part)
            && let Ok(notes) = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(source)
        {
            owners_reference_styles(
                notes
                    .footnotes
                    .iter()
                    .flat_map(|note| &note.paragraphs)
                    .map(WordXmlOwner::Paragraph)
                    .collect(),
                &wanted,
                &found,
            );
        }
        for ((_, style), found) in missing.into_iter().zip(found) {
            if found.get() {
                self.add_style(style)?;
            }
        }
        Ok(())
    }

    /// Give the body's and the notes model's runs that name a built-in
    /// style id the styles part does not define the localized id, as
    /// [`localized_run_style_ids`] finds it. The body is scanned first, so
    /// that a body with nothing to rename keeps its opened model.
    fn rename_localized_run_styles(&mut self) {
        let renames = localized_run_style_ids(&self.styles);
        if renames.is_empty() {
            return;
        }
        let mut named = false;
        walkers::visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            named |= names_renamed_run_style(paragraph, &renames);
        });
        if named {
            walkers::visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
                rename_run_styles(paragraph, &renames)
            });
        }
        if self
            .footnotes
            .footnotes
            .iter()
            .flat_map(|note| &note.paragraphs)
            .any(|paragraph| names_renamed_run_style(paragraph, &renames))
        {
            for paragraph in self
                .footnotes
                .footnotes
                .iter_mut()
                .flat_map(|note| &mut note.paragraphs)
            {
                rename_run_styles(paragraph, &renames);
            }
            self.footnotes_dirty = true;
        }
    }

    /// The id of the Hyperlink character style: the style with its built-in
    /// name, else `Hyperlink`, which the save adds when it is missing.
    pub(crate) fn hyperlink_style_id(&self) -> String {
        built_in_character_style_id(&self.styles, "Hyperlink", "Hyperlink")
            .unwrap_or_else(|| "Hyperlink".to_owned())
    }

    /// Give the runs of `paragraph`, about to join a story that the save
    /// does not rename, the localized ids [`Self::rename_localized_run_styles`]
    /// gives the body's.
    pub(crate) fn localize_run_styles(&self, paragraph: &mut CT_P) {
        rename_run_styles(paragraph, &localized_run_style_ids(&self.styles));
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
        self.allocate_handle_drawing_ids()?;
        #[cfg(debug_assertions)]
        let (before, registry) = (IdentifierSummary::of(self), self.identifiers.clone());
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
        self.ensure_referenced_character_styles()?;
        self.preflight_flush_required_bundles()?;
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
        self.order_authored_relationships();
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
        #[cfg(debug_assertions)]
        if result.is_ok() {
            IdentifierSummary::of(self).assert_kept_from(&before, &registry);
        }
        result.map(|()| normalized)
    }

    /// Stage every public output. A comments model that no longer matches its
    /// part is written back even when no mutation marked it dirty. An
    /// unchanged comments part keeps its producer bytes, as styles and the
    /// other modelled parts do, so a save without a comment edit neither
    /// rewrites it nor invalidates a package signature over it.
    pub(crate) fn prepare_staged_output(&mut self) -> Result<bool> {
        #[cfg(debug_assertions)]
        let (live_identifiers, live_package) = (self.identifiers.clone(), self.package.clone());
        if let (Some(comments), Some(part_name)) = (&self.comments, &self.comments_part_name) {
            self.comments_dirty |= self
                .package
                .get_part(part_name)
                .and_then(|xml| oxoffice_docx_oxml::comments::CT_Comments::from_xml(xml).ok())
                .as_ref()
                != Some(comments);
        }
        let normalized = self.prepare_staged_package()?;
        #[cfg(debug_assertions)]
        IdentifierSummary::of_package(self).assert_reads_back(&IdentifierSummary::of(self));
        // Content `remove_content_at` freed leaves the written copy only: a
        // copy that released some is not installed, so the live document can
        // still take the removed fragment back.
        let released = self.release_freed_relationships()?;
        #[cfg(debug_assertions)]
        self.assert_saved_package_reopens(&live_identifiers, &live_package);
        Ok(normalized && !released)
    }

    /// Debug proof that the package a save writes reopens, reads the ids it
    /// holds (checked by [`Self::reopen_staged`]), adds no repeated drawing
    /// or bookmark id, and holds no id the live document has not reserved,
    /// so that no later insert can take one again: `live_identifiers` and
    /// `live_package` are the document's before the save prepared its copy.
    /// A package the write refuses is the save's error, not this proof's.
    #[cfg(debug_assertions)]
    fn assert_saved_package_reopens(
        &self,
        live_identifiers: &DocumentIdentifiers,
        live_package: &OpcPackage,
    ) {
        let copy = self.clone_for_debug_reference();
        if copy
            .package
            .write_to(&mut std::io::Cursor::new(Vec::new()))
            .is_err()
        {
            return;
        }
        match copy.reopen_staged(false, PackageReadLimits::UNBOUNDED) {
            Ok(reopened) => live_identifiers.assert_covers(
                &reopened.identifiers,
                &self.identifiers,
                live_package,
            ),
            Err(error) => panic!("a save writes a package that does not reopen: {error}"),
        }
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
        #[cfg(debug_assertions)]
        let prepared = IdentifierSummary::of_package(&self);
        let custom_properties_owned = self.custom_properties_owned;
        let settings_owned = self.settings_owned;
        let owned_settings = settings_owned.then(|| self.settings.clone());
        let web_settings_owned = self.web_settings_owned;
        let owned_web_settings = web_settings_owned.then(|| self.web_settings.clone());
        let comments_owned = self.comments_owned;
        let comments_extended_owned = self.comments_extended_owned;
        let embedded_invalidated_signatures = self.embedded_invalidated_signatures.clone();
        let freed_relationships = self.freed_relationships.clone();
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
        #[cfg(debug_assertions)]
        {
            IdentifierSummary::of(&reopened).assert_reads_back(&prepared);
        }
        reopened.identifiers.reconcile_provenance(&provenance);
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
        reopened.freed_relationships = freed_relationships;
        reopened.package_signatures_invalidated = package_signatures_invalidated;
        reopened.source_path = source_path;
        reopened.install_carried_body_scans(carried_scans);
        reopened.install_carried_blocks(carried_blocks);
        Ok(reopened)
    }
}
