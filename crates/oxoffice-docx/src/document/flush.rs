//! Flushing the models to the package: bundles, numbering, related stories and the main part.

use super::*;

impl Document {
    pub(super) fn preflight_flush_required_bundles(&mut self) -> Result<()> {
        self.reserve_styles_bundle()?;
        if self.numbering.is_some() {
            self.reserve_numbering_bundle()?;
        }
        if self.footnotes_dirty && !self.footnotes.footnotes.is_empty() {
            self.reserve_footnotes_bundle()?;
        }
        if let Some(part_name) = self.comments_part_name.clone()
            && self.comments.is_some()
        {
            self.ensure_part_relationship_checked(
                &part_name,
                rel_types::COMMENTS,
                crate::comments::COMMENTS_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("comments relationship allocation failed: {error}"))
            })?;
        }
        if let Some(part_name) = self.comments_extended_part_name.clone()
            && self.comments_extended.is_some()
        {
            self.ensure_part_relationship_checked(
                &part_name,
                crate::comments::COMMENTS_EXTENDED_REL_TYPE,
                crate::comments::COMMENTS_EXTENDED_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!(
                    "comments-extended relationship allocation failed: {error}"
                ))
            })?;
        }
        if self.core_properties.is_some() {
            self.reserve_core_properties_bundle()?;
        }
        if self.application_properties.is_some() {
            self.reserve_application_properties_bundle()?;
        }
        if self.custom_properties.is_some() {
            self.reserve_custom_properties_bundle()?;
        }
        Ok(())
    }

    fn styles_differ_from_part(&mut self, part: &str) -> Result<bool> {
        let Some(bytes) = self.package.get_part_shared(part).cloned() else {
            return Ok(true);
        };
        if let Some(cache) = self
            .styles_part_cache
            .as_ref()
            .filter(|cache| Arc::ptr_eq(&bytes, &cache.bytes))
        {
            if cache.emitted_model.as_ref().is_some_and(|emitted| {
                Arc::ptr_eq(&self.styles, emitted) || self.styles == *emitted
            }) {
                // The exact writer already succeeded for this model and these
                // bytes. Keep its lexical form rather than adopting a reparse.
                return Ok(false);
            }
            if Arc::ptr_eq(&self.styles, &cache.parsed) || self.styles == cache.parsed {
                if matches!(cache.proof, StylesSerializationProof::Unserializable) {
                    self.styles.to_xml()?;
                }
                self.styles = Arc::clone(&cache.parsed);
                return Ok(false);
            }
            return Ok(true);
        }
        let parsed = Arc::new(CT_Styles::from_xml(&bytes)?);
        let proof = if parsed.to_xml().is_ok() {
            StylesSerializationProof::Serializable
        } else {
            StylesSerializationProof::Unserializable
        };
        if self.styles != parsed {
            return Ok(true);
        }
        if matches!(proof, StylesSerializationProof::Unserializable) {
            self.styles.to_xml()?;
        }
        self.styles = Arc::clone(&parsed);
        self.styles_part_cache = Some(StylesPartCache {
            bytes,
            parsed,
            emitted_model: None,
            proof,
        });
        Ok(false)
    }

    /// Write the main part, then the numbering part. Every id keeps its
    /// value; a numbering the document was read without is written with its
    /// definitions and instances in id order, as the save always wrote it.
    fn flush_numbering_emission(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let in_template = template.is_some();
        self.flush_document_to_package_in(template)?;
        let Some(mut emitted) = self.numbering.clone() else {
            return Ok(());
        };
        if !in_template
            && self.identifiers.preserved_abstract_numbering_ids.is_empty()
            && self.identifiers.preserved_numbering_instance_ids.is_empty()
        {
            emitted
                .abstract_nums
                .sort_by_key(|definition| definition.abstract_num_id);
            emitted.nums.sort_by_key(|instance| instance.num_id);
        }
        let numbering_xml = emitted.to_xml()?;
        let numbering_part_name = self.numbering_part_name.clone();
        let numbering_part = self
            .reserve_document_part_bundle(
                numbering_part_name.as_deref(),
                DEFAULT_NUMBERING_PART,
                rel_types::NUMBERING,
                NUMBERING_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("numbering relationship allocation failed: {error}"))
            })?;
        self.numbering_part_name = Some(numbering_part.clone());
        let numbering_changed = self
            .package
            .get_part(&numbering_part)
            .and_then(|xml| CT_Numbering::from_xml(xml).ok())
            .as_ref()
            != Some(&emitted);
        if numbering_changed {
            self.package.set_part(&numbering_part, numbering_xml);
        }
        Ok(())
    }

    /// Write the in-memory document/styles back into the OPC package parts.
    pub(crate) fn flush_to_package(&mut self) -> Result<()> {
        self.flush_to_package_in(None)
    }

    pub(super) fn flush_to_package_in(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        self.flush_numbering_emission(template.as_deref_mut())?;

        // Unchanged proved styles share their model and source bytes through staging.
        let styles_part_name = self.styles_part_name.clone();
        let styles_part = self
            .reserve_document_part_bundle(
                styles_part_name.as_deref(),
                DEFAULT_STYLES_PART,
                rel_types::STYLES,
                STYLES_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("styles relationship allocation failed: {error}"))
            })?;
        self.styles_part_name = Some(styles_part.clone());
        if self.styles_differ_from_part(&styles_part)? {
            let styles_xml = self.styles.to_xml()?;
            let styles_xml = if let Some(source) = self.package.get_part(&styles_part) {
                retain_styles_root_attributes(source, styles_xml)?
            } else {
                styles_xml
            };
            self.package.set_part(&styles_part, styles_xml);
            let bytes = self
                .package
                .get_part_shared(&styles_part)
                .expect("just installed styles bytes");
            // The cache promises the model an actual reopening would parse.
            // Authored properties can acquire retained namespace context on
            // parse, so the pre-publication model is not that proof.
            let parsed = Arc::new(CT_Styles::from_xml(bytes)?);
            let proof = if parsed.to_xml().is_ok() {
                StylesSerializationProof::Serializable
            } else {
                StylesSerializationProof::Unserializable
            };
            self.styles_part_cache = Some(StylesPartCache {
                bytes: Arc::clone(bytes),
                parsed,
                emitted_model: Some(Arc::clone(&self.styles)),
                proof,
            });
        }

        // Settings are exposed as a read-only projection. Parsed settings
        // retain their complete producer bytes and are written back only to
        // the relationship-resolved part they came from.
        if let (Some(settings), Some(part_name)) = (&self.settings, &self.settings_part_name) {
            let changed = self
                .package
                .get_part(part_name)
                .and_then(|xml| CT_Settings::from_xml(xml).ok())
                .as_ref()
                != Some(settings);
            if changed {
                self.package.set_part(part_name, settings.to_xml()?);
            }
        }

        if let (Some(web_settings), Some(part_name)) =
            (&self.web_settings, &self.web_settings_part_name)
        {
            let changed = self
                .package
                .get_part(part_name)
                .and_then(|xml| CT_WebSettings::from_xml(xml).ok())
                .as_ref()
                != Some(web_settings);
            if changed {
                self.package.set_part(part_name, web_settings.to_xml()?);
            }
        }

        if self.theme_dirty {
            let theme = self
                .theme
                .as_ref()
                .ok_or_else(|| Error::Other("dirty theme model is missing".to_owned()))?;
            let part_name = self
                .theme_part_name
                .as_ref()
                .ok_or_else(|| Error::Other("dirty theme part name is missing".to_owned()))?;
            self.package.set_part(
                part_name,
                theme.to_xml().map_err(|error| {
                    Error::Other(format!("theme serialization failed: {error}"))
                })?,
            );
        }

        if self.font_table_dirty {
            let table = self
                .font_table
                .as_ref()
                .ok_or_else(|| Error::Other("dirty font-table model is missing".to_owned()))?;
            let part_name = self
                .font_table_part_name
                .as_ref()
                .ok_or_else(|| Error::Other("dirty font-table part name is missing".to_owned()))?;
            self.package.set_part(part_name, table.to_xml()?);
        }

        self.flush_dirty_related_story_models_in(template)?;

        if let (Some(comments), Some(part_name)) = (
            &self.comments_extended,
            self.comments_extended_part_name.clone(),
        ) {
            let changed = self
                .package
                .get_part(&part_name)
                .and_then(|xml| {
                    oxoffice_docx_oxml::comments_extended::CT_CommentsEx::from_xml(xml).ok()
                })
                .as_ref()
                != Some(comments);
            if changed {
                self.package.set_part(&part_name, comments.to_xml()?);
            }
            self.ensure_part_relationship_checked(
                &part_name,
                crate::comments::COMMENTS_EXTENDED_REL_TYPE,
                crate::comments::COMMENTS_EXTENDED_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!(
                    "comments-extended relationship allocation failed: {error}"
                ))
            })?;
        }

        if self.glossary_dirty {
            let glossary = self
                .glossary
                .as_ref()
                .ok_or_else(|| Error::Other("dirty glossary model is missing".to_owned()))?;
            let part_name = self
                .glossary_part_name
                .as_ref()
                .ok_or_else(|| Error::Other("dirty glossary part name is missing".to_owned()))?;
            self.package.set_part(part_name, glossary.to_xml()?);
        }

        // Serialize core properties to the package relationship's target.
        if let Some(core_xml) = self
            .core_properties
            .as_deref()
            .map(CoreProperties::to_xml)
            .transpose()?
        {
            let core_part = self.reserve_core_properties_bundle()?;
            let changed = self
                .package
                .get_part(&core_part)
                .and_then(|xml| CoreProperties::from_xml(xml).ok())
                .as_ref()
                != self.core_properties.as_deref();
            if changed {
                self.package.set_part(&core_part, core_xml);
            }
        }
        if let Some(application_xml) = self
            .application_properties
            .as_ref()
            .map(AppProperties::to_xml)
            .transpose()?
        {
            let application_part = self.reserve_application_properties_bundle()?;
            let changed = self
                .package
                .get_part(&application_part)
                .and_then(|xml| AppProperties::from_xml(xml).ok())
                .as_ref()
                != self.application_properties.as_ref();
            if changed {
                self.package.set_part(&application_part, application_xml);
            }
        }
        if let Some(custom_xml) = self
            .custom_properties
            .as_ref()
            .map(CustomProperties::to_xml)
            .transpose()?
        {
            let custom_part = self.reserve_custom_properties_bundle()?;
            let changed = self
                .package
                .get_part(&custom_part)
                .and_then(|xml| CustomProperties::from_xml(xml).ok())
                .as_ref()
                != self.custom_properties.as_ref();
            if changed {
                self.package.set_part(&custom_part, custom_xml);
            }
        }

        Ok(())
    }

    pub(crate) fn flush_dirty_related_story_models(&mut self) -> Result<()> {
        self.flush_dirty_related_story_models_in(None)
    }

    pub(super) fn flush_dirty_related_story_models_in(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        // Existing footnote XML remains the authority. Typed additions are
        // inserted into that root once so producer namespace declarations and
        // unmodelled root children never pass through the lossy typed model.
        if self.footnotes_dirty && !self.footnotes.footnotes.is_empty() {
            let footnotes_part_name = self.footnotes_part_name.clone();
            let footnotes_part = self
                .reserve_document_part_bundle(
                    footnotes_part_name.as_deref(),
                    "/word/footnotes.xml",
                    rel_types::FOOTNOTES,
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
                )
                .map_err(|error| {
                    Error::Other(format!("footnotes relationship allocation failed: {error}"))
                })?;
            self.footnotes_part_name = Some(footnotes_part.clone());
            let mut emitted_part = None;
            let xml = if let Some(existing_xml) = self.package.get_part_shared(&footnotes_part) {
                let mut parent = if let Some(template) = template.as_deref_mut() {
                    template.take_related_source(&footnotes_part, existing_xml)?
                } else {
                    None
                };
                let existing = if let (Some(template), Some(source)) =
                    (template.as_deref_mut(), parent.as_ref())
                {
                    template.operation.source_notes(source)?
                } else {
                    oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(existing_xml)?
                };
                let mut additions = Vec::new();
                let mut addition_sources = Vec::new();
                let mut all_sources = parent.is_some();
                for (index, note) in self.footnotes.footnotes.iter().enumerate() {
                    if existing.footnotes.iter().any(|candidate| {
                        candidate.id == note.id && candidate.note_type == note.note_type
                    }) {
                        continue;
                    }
                    let source = if let Some(template) = template.as_deref_mut() {
                        let read = (index < template.sources.read_note_count)
                            .then_some(parent.as_deref())
                            .flatten();
                        serialized_note_fragment_in(
                            note,
                            StoryKind::Footnote,
                            index,
                            read,
                            template,
                        )?
                    } else {
                        None
                    };
                    if let (Some(template), Some(source)) = (template.as_deref_mut(), source) {
                        let copied = template.operation.repeat_source(&source)?;
                        additions.push(template.operation.finish_source(copied)?);
                        addition_sources.push(Arc::new(source));
                    } else {
                        all_sources = false;
                        additions.push(serialized_footnote_fragment(note)?);
                    }
                }
                if existing.footnotes.len() + additions.len() != self.footnotes.footnotes.len() {
                    return Err(Error::Other(
                        "typed footnote mutation is not an additive package update".to_owned(),
                    ));
                }
                if all_sources {
                    let template = template.as_deref_mut().expect("scoped additive notes");
                    let source = append_story_sources_to_root(
                        template.operation,
                        parent.take().expect("actual existing note Source"),
                        &addition_sources,
                        b"footnotes",
                        b"footnote",
                    )?;
                    let copied = template.operation.repeat_source(&source)?;
                    let bytes = template.operation.finish_source(copied)?;
                    emitted_part = Some(source);
                    bytes
                } else {
                    append_story_fragments_to_root(
                        existing_xml,
                        b"footnotes",
                        b"footnote",
                        &additions,
                    )?
                }
            } else if let Some(template) = template.as_deref_mut() {
                let mut texts = Vec::new();
                let mut fields = Vec::new();
                let mut raw = Vec::new();
                if !template.sources.notes_model_read {
                    for (index, note) in self.footnotes.footnotes.iter().enumerate() {
                        if let Some(inputs) = template.lineage.note_writer_inputs(index, note) {
                            texts.extend(inputs.texts);
                            fields.extend(inputs.fields);
                            raw.extend(inputs.raw);
                        }
                    }
                }
                let source = template.operation.source_model_notes(
                    &self.footnotes,
                    false,
                    &texts,
                    &fields,
                    &raw,
                    true,
                )?;
                let copied = template.operation.repeat_source(&source)?;
                let bytes = template.operation.finish_source(copied)?;
                emitted_part = Some(Arc::from(source));
                bytes
            } else {
                let mut footnotes = self.footnotes.clone();
                for footnote in &mut footnotes.footnotes {
                    for paragraph in &mut footnote.paragraphs {
                        close_typed_story_drawing_namespaces(paragraph)?;
                    }
                }
                footnotes.to_xml_footnotes()?
            };
            self.package.set_part(&footnotes_part, xml);
            if let (Some(template), Some(source)) = (template.as_deref_mut(), emitted_part) {
                template.sources.put_related_source(
                    &footnotes_part,
                    self.package
                        .get_part_shared(&footnotes_part)
                        .expect("installed note part"),
                    source,
                );
            }
            self.footnotes_dirty = false;
        }

        // An existing comments part is modelled and flushed to its resolved
        // relationship target. An absent part remains absent until the later
        // comment API creates one deliberately.
        if self.comments_dirty
            && let (Some(comments), Some(part_name)) =
                (&self.comments, self.comments_part_name.clone())
        {
            let emitted = if let Some(writer) = template.as_deref_mut() {
                let source = if let Some(source) = writer.sources.comments_reader_source.as_ref() {
                    writer
                        .operation
                        .source_model_comments_from_read(comments, source)?
                } else {
                    let inputs = writer
                        .lineage
                        .comments_writer_inputs(comments)
                        .unwrap_or_default();
                    writer.operation.source_model_comments(
                        comments,
                        &inputs.texts,
                        &inputs.fields,
                        &inputs.raw,
                    )?
                };
                let retained = writer.operation.repeat_source(&source)?;
                Some((writer.operation.finish_source(retained)?, source))
            } else {
                None
            };
            let (xml, source) = if let Some((bytes, source)) = emitted {
                (bytes, Some(source))
            } else {
                let mut comments = comments.clone();
                for comment in &mut comments.comments {
                    for paragraph in &mut comment.paragraphs {
                        close_typed_story_drawing_namespaces(paragraph)?;
                    }
                }
                (comments.to_xml()?, None)
            };
            self.package.set_part(&part_name, xml);
            if let (Some(writer), Some(source)) = (template, source) {
                writer.sources.comments_reader_source = None;
                writer.sources.put_related_source(
                    &part_name,
                    self.package
                        .get_part_shared(&part_name)
                        .expect("actual comments publication"),
                    source,
                );
            }
            self.comments_dirty = false;
            self.ensure_part_relationship_checked(
                &part_name,
                rel_types::COMMENTS,
                crate::comments::COMMENTS_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("comments relationship allocation failed: {error}"))
            })?;
        }
        Ok(())
    }

    pub(super) fn flush_document_to_package(&mut self) -> Result<()> {
        self.flush_document_to_package_in(None)
    }

    fn flush_document_to_package_in(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        // A read-only save with unsafe retained namespace shadows keeps the
        // producer's complete scopes and every retained raw subtree byte for
        // byte. A modified document fails closed when canonical serialization
        // could change the meaning or bytes of retained content.
        // A main part that still holds its opened bytes is reused only when
        // the namespace context and the current serialization match that open.
        let kept = self.kept_main_part();
        let existing_document_xml = self.package.get_part(&self.doc_part_name);
        let typed_document_is_unchanged = if kept.is_some() && self.opened_main_still_serializes() {
            #[cfg(debug_assertions)]
            {
                assert_kept_main_document_without_initializing(
                    kept.as_deref(),
                    existing_document_xml,
                    &self.document,
                );
            }
            true
        } else if let Some(parsed) = kept.as_deref().and_then(Parsed::main_document) {
            #[cfg(debug_assertions)]
            {
                let fresh = existing_document_xml.and_then(|xml| CT_Document::from_xml(xml).ok());
                assert_eq!(
                    fresh.as_ref(),
                    Some(parsed),
                    "the retained source parse disagrees with a fresh parse"
                );
            }
            parsed == &*self.document
        } else if let Some(proved) = existing_document_xml.and_then(|xml| {
            self.main_part_blocks
                .as_ref()
                .and_then(|blocks| blocks.proved_document_matches(xml, &self.document))
        }) {
            #[cfg(debug_assertions)]
            {
                let fresh = existing_document_xml.and_then(|xml| CT_Document::from_xml(xml).ok());
                assert_eq!(
                    fresh.as_ref() == Some(&*self.document),
                    proved,
                    "the proved existing part disagrees with a fresh parse"
                );
            }
            proved
        } else {
            let parsed = existing_document_xml.and_then(|xml| CT_Document::from_xml(xml).ok());
            let unchanged = parsed.as_ref() == Some(&*self.document);
            drop(parsed);
            unchanged
        };
        if typed_document_is_unchanged
            && let Some(template) = &mut template
            && template.sources.source.is_none()
            && let Some((part, length)) = template.sources.original_part.take()
        {
            // This branch actually keeps the first staging clone's ingress
            // part. Later generated or refreshed parts cannot enter it.
            template.sources.source = Some(Arc::new(template.operation.source_part(
                &part,
                0..length,
                &BTreeMap::new(),
            )?));
        }
        let scanned;
        let nested_namespace_owners =
            match kept.as_deref().and_then(Parsed::nested_namespace_owners) {
                Some(owners) => {
                    #[cfg(debug_assertions)]
                    {
                        assert_eq!(
                            format!("{owners:?}"),
                            format!(
                                "{:?}",
                                existing_document_xml
                                    .map(nested_modeled_namespace_owners)
                                    .transpose()
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default()
                            ),
                            "the kept nested namespace owners differ from a scan"
                        );
                    }
                    owners
                }
                None => {
                    let proved_owners = existing_document_xml.and_then(|xml| {
                        self.main_part_blocks
                            .as_ref()
                            .and_then(|blocks| blocks.retained_nested_owners(xml))
                    });
                    if let Some(owners) = proved_owners {
                        #[cfg(debug_assertions)]
                        {
                            let fresh = existing_document_xml
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
                        // `nested_modeled_namespace_owners` notes this scan itself.
                        scanned = existing_document_xml
                            .map(nested_modeled_namespace_owners)
                            .transpose()?
                            .unwrap_or_default();
                        &scanned
                    }
                }
            };
        let unsafe_prefix = unsafe_serializer_namespace_prefix(
            &self.root_namespace_declarations,
            &self.body_namespace_declarations,
            existing_document_xml,
        )
        .or_else(|| unsafe_nested_namespace_prefix(nested_namespace_owners));
        let kept_xml = existing_document_xml.map(<[u8]>::to_vec);
        let mut replayed_output_has_retained_namespace_owner = false;
        let doc_xml = if typed_document_is_unchanged {
            None
        } else {
            if let Some(prefix) = unsafe_prefix {
                return Err(Error::Other(format!(
                    "cannot serialize a modified document with a shadowed `{prefix}` namespace"
                )));
            }
            let serialized = match template.as_deref_mut() {
                Some(template) => self.main_part_xml_in(Some(template))?,
                None => self.main_part_xml()?,
            };
            let spans = self
                .main_part_blocks
                .as_ref()
                .and_then(|blocks| blocks.proved_namespace_spans(&serialized));
            let (replayed, has_retained_owner) = replay_nested_namespace_declarations_in(
                &serialized,
                nested_namespace_owners,
                spans.as_deref(),
                template.as_deref_mut(),
            )?;
            replayed_output_has_retained_namespace_owner = has_retained_owner;
            Some(replayed)
        };
        let namespace_scopes = document_namespace_scopes(
            doc_xml
                .as_deref()
                .or(kept_xml.as_deref())
                .expect("compared existing document XML"),
        )?;
        // The replay classification above belongs to the old source. A newly
        // serialized owner can introduce its own namespace scope. Certify the
        // exact output, reusing only facts qualified for those output bytes.
        let local_cell_owner_free = if replayed_output_has_retained_namespace_owner {
            // A successful actual selected output owner is enough to block
            // direct edits. Unknown output still needs its existing scan.
            false
        } else if let Some(xml) = &doc_xml {
            let owners = OnceLock::new();
            if let Some(classified) = self
                .main_part_blocks
                .as_ref()
                .and_then(|blocks| blocks.retained_nested_owners(xml))
            {
                let _ = owners.set(Some(classified));
            }
            let (blocked, _) = plain_local_edit_facts(
                &namespace_scopes.root_declarations,
                &namespace_scopes.body_declarations,
                xml,
                &owners,
            );
            !blocked
        } else {
            unsafe_prefix.is_none() && nested_namespace_owners.is_empty()
        };
        if let Some(doc_xml) = doc_xml {
            self.package.set_part(&self.doc_part_name, doc_xml);
            if let Some(template) = template {
                let source =
                    template.sources.source.as_ref().ok_or_else(|| {
                        Error::Other("published main writer lost its source".into())
                    })?;
                let bytes = self
                    .package
                    .get_part_shared(&self.doc_part_name)
                    .expect("actual main publication");
                template.sources.published_main = Some((Arc::clone(bytes), Arc::clone(source)));
            }
        }
        self.root_namespace_declarations = namespace_scopes.root_declarations;
        self.body_namespace_declarations = namespace_scopes.body_declarations;
        self.body_namespace_bindings = namespace_scopes.body_bindings;
        self.plain_local_edit_blocked = typed_document_is_unchanged && unsafe_prefix.is_some();
        self.local_cell_source = LocalCellSource::classified(
            &self.package,
            &self.doc_part_name,
            &self.root_namespace_declarations,
            &self.body_namespace_declarations,
            local_cell_owner_free,
        );
        if !typed_document_is_unchanged {
            self.unsavable_body_edit_cache = OnceLock::new();
        }
        Ok(())
    }
}

impl Document {
    pub(super) fn refresh_related_story_caches(&mut self) -> Result<()> {
        self.refresh_related_story_caches_in(None)
    }

    pub(super) fn refresh_related_story_caches_in(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        if let Some(part_name) = self.footnotes_part_name.as_deref()
            && let Some(xml) = self.package.get_part_shared(part_name)
        {
            if let Some(template) = template.as_deref_mut() {
                template.sources.notes_model_read = true;
                if let Some(source) = template.take_related_source(part_name, xml)? {
                    self.footnotes = template.operation.source_notes(&source)?;
                    template.sources.read_note_count = self.footnotes.footnotes.len();
                    template.sources.put_related_source(part_name, xml, source);
                } else {
                    self.footnotes = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml)?;
                }
            } else {
                self.footnotes = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml)?;
            }
            self.footnotes_dirty = false;
        }
        if let Some(part_name) = self.comments_part_name.as_deref()
            && let Some(xml) = self.package.get_part_shared(part_name)
        {
            self.comments = Some(if let Some(writer) = template {
                let source = writer.take_related_source(part_name, xml)?.ok_or_else(|| {
                    Error::Other("comments refresh lost its actual source".into())
                })?;
                let comments = writer.operation.source_comments(&source)?;
                writer.sources.comments_reader_source = Some(Arc::clone(&source));
                writer.sources.put_related_source(part_name, xml, source);
                comments
            } else {
                oxoffice_docx_oxml::comments::CT_Comments::from_xml(xml)?
            });
            self.comments_dirty = false;
        }
        Ok(())
    }
}
