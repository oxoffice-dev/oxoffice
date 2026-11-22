//! Staging an edit on a clone of the document, and committing it.

use super::*;

impl Document {
    /// Admit each loaded recursive owner before staging clones it.
    pub(crate) fn validate_model_resource_limits(&self) -> Result<()> {
        self.document.validate_resource_limits()?;
        for note in &self.footnotes.footnotes {
            for paragraph in &note.paragraphs {
                paragraph.validate_resource_limits()?;
            }
        }
        if let Some(comments) = &self.comments {
            for comment in &comments.comments {
                for paragraph in &comment.paragraphs {
                    paragraph.validate_resource_limits()?;
                }
            }
        }
        if let Some(glossary) = &self.glossary {
            glossary.validate_resource_limits()?;
        }
        let mut field_error = None;
        let mut preflight = |paragraph: &CT_P| {
            if field_error.is_some() {
                return;
            }
            for run in paragraph.source_runs() {
                for content in &run.content {
                    if let oxoffice_docx_oxml::text::RunContent::Field(field) = content
                        && let Err(error) = crate::field::preflight_current_field(field)
                    {
                        field_error = Some(error);
                        return;
                    }
                }
            }
        };
        visit_body_paragraphs(&self.document.body.content, false, &mut preflight);
        for note in &self.footnotes.footnotes {
            for paragraph in &note.paragraphs {
                preflight(paragraph);
            }
        }
        if let Some(comments) = &self.comments {
            for comment in &comments.comments {
                for paragraph in &comment.paragraphs {
                    preflight(paragraph);
                }
            }
        }
        if let Some(glossary) = &self.glossary {
            for part in &glossary.doc_parts {
                visit_body_paragraphs(&part.body.content, false, &mut preflight);
            }
        }
        if let Some(error) = field_error {
            return Err(error);
        }
        // Header/footer and endnote parts are source-owned OPC bytes here,
        // not recursive cached models. Their independent parsers and writers
        // admit owners when those parts are projected or edited.
        Ok(())
    }

    /// Clone all package and typed state while discarding derived layout caches.
    pub fn clone_for_staging(&self) -> Self {
        Self {
            package: self.package.clone(),
            document: self.document.clone(),
            root_namespace_declarations: self.root_namespace_declarations.clone(),
            body_namespace_declarations: self.body_namespace_declarations.clone(),
            body_namespace_bindings: self.body_namespace_bindings.clone(),
            plain_local_edit_blocked: self.plain_local_edit_blocked,
            local_cell_source: self.local_cell_source.clone(),
            parsed: self.parsed.clone(),
            main_body_scans: Arc::clone(&self.main_body_scans),
            main_part_blocks: self.main_part_blocks.clone(),
            unsavable_body_edit_cache: self
                .unsavable_body_edit_cache
                .get()
                .cloned()
                .map(OnceLock::from)
                .unwrap_or_default(),
            styles: Arc::clone(&self.styles),
            styles_part_cache: self.styles_part_cache.clone(),
            numbering: self.numbering.clone(),
            core_properties: self.core_properties.clone(),
            application_properties: self.application_properties.clone(),
            custom_properties: self.custom_properties.clone(),
            core_properties_part_name: self.core_properties_part_name.clone(),
            application_properties_part_name: self.application_properties_part_name.clone(),
            custom_properties_part_name: self.custom_properties_part_name.clone(),
            custom_properties_owned: self.custom_properties_owned,
            doc_part_name: self.doc_part_name.clone(),
            styles_part_name: self.styles_part_name.clone(),
            numbering_part_name: self.numbering_part_name.clone(),
            settings: self.settings.clone(),
            settings_part_name: self.settings_part_name.clone(),
            settings_owned: self.settings_owned,
            web_settings: self.web_settings.clone(),
            web_settings_part_name: self.web_settings_part_name.clone(),
            web_settings_owned: self.web_settings_owned,
            theme: self.theme.clone(),
            theme_part_name: self.theme_part_name.clone(),
            theme_dirty: self.theme_dirty,
            font_table: self.font_table.clone(),
            font_table_part_name: self.font_table_part_name.clone(),
            font_table_dirty: self.font_table_dirty,
            identifiers: self.identifiers.clone(),
            footnotes: self.footnotes.clone(),
            footnotes_part_name: self.footnotes_part_name.clone(),
            footnotes_root_namespace_declarations: self
                .footnotes_root_namespace_declarations
                .clone(),
            footnotes_dirty: self.footnotes_dirty,
            comments: self.comments.clone(),
            comments_part_name: self.comments_part_name.clone(),
            comments_dirty: self.comments_dirty,
            comments_extended: self.comments_extended.clone(),
            comments_extended_part_name: self.comments_extended_part_name.clone(),
            comments_owned: self.comments_owned,
            comments_extended_owned: self.comments_extended_owned,
            embedded_invalidated_signatures: self.embedded_invalidated_signatures.clone(),
            freed_image_relationships: self.freed_image_relationships.clone(),
            package_signatures_invalidated: self.package_signatures_invalidated,
            source_path: self.source_path.clone(),
            glossary: self.glossary.clone(),
            glossary_part_name: self.glossary_part_name.clone(),
            glossary_dirty: self.glossary_dirty,
            layout_cache: Mutex::new(None),
            normal_layout_engine: Mutex::new(None),
            deterministic_layout_cache: Mutex::new(None),
            bundled_fallback_layout_engine: Mutex::new(None),
        }
    }

    /// Independent cold reference state for debug agreement oracles only.
    #[cfg(debug_assertions)]
    pub(super) fn clone_for_debug_reference(&self) -> Self {
        let mut copy = self.clone_for_staging();
        copy.parsed = None;
        copy.main_part_blocks = None;
        copy.main_body_scans = Arc::new(Mutex::new(MainBodyScans::default()));
        copy
    }
}

impl Document {
    /// Commit staged package state without discarding reusable layout work.
    pub(super) fn take_staged_commit_origin(&mut self) -> StagedCommitOrigin {
        let package = self.package.clone();
        StagedCommitOrigin {
            package,
            signature_invalidated: self.package_signatures_invalidated,
            normal_engine: std::mem::replace(&mut self.normal_layout_engine, Mutex::new(None)),
            bundled_engine: std::mem::replace(
                &mut self.bundled_fallback_layout_engine,
                Mutex::new(None),
            ),
        }
    }

    pub(super) fn finish_staged_candidate(
        source_package: &OpcPackage,
        source_invalidated: bool,
        normal_engine: &mut Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
        bundled_engine: &mut Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
        mut candidate: Self,
    ) -> Self {
        candidate.package_signatures_invalidated |= candidate
            .package
            .package_rels
            .items
            .iter()
            .any(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN)
            && (source_invalidated
                || crate::embedded::synchronized_package_mutation_invalidates_signature(
                    source_package,
                    &candidate.package,
                ));
        std::mem::swap(normal_engine, &mut candidate.normal_layout_engine);
        std::mem::swap(
            bundled_engine,
            &mut candidate.bundled_fallback_layout_engine,
        );
        candidate
    }

    /// Apply `edit`, a properties edit, without staging the whole document.
    ///
    /// `edit` first reserves the properties part in the package graph and the
    /// identifier registry, which can fail, then writes the properties model
    /// and its part name, which cannot. The package graph, whose part bytes
    /// are shared, and the registry are saved first and put back on refusal,
    /// so a refused edit leaves no reserved name or relationship id. A
    /// success leaves what [`Self::commit_staged_mutation`] leaves for a
    /// staged clone: the derived layout is dropped and a signed package
    /// records a package mutation. Debug builds run the staged edit on a
    /// clone and compare. `edit` is `Fn` for that second run.
    pub(super) fn edit_properties(&mut self, edit: impl Fn(&mut Self) -> Result<()>) -> Result<()> {
        #[cfg(debug_assertions)]
        let staged = self.clone_for_staging();
        let package = self.package.clone();
        let identifiers = self.identifiers.clone();
        let result = edit(self);
        match &result {
            Ok(()) => {
                self.package_signatures_invalidated |=
                    self.package.package_rels.items.iter().any(|relationship| {
                        relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN
                    }) && (self.package_signatures_invalidated
                        || crate::embedded::synchronized_package_mutation_invalidates_signature(
                            &package,
                            &self.package,
                        ));
                self.layout_cache
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                self.deterministic_layout_cache
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
            }
            Err(_) => {
                self.package = package;
                self.identifiers = identifiers;
            }
        }
        #[cfg(debug_assertions)]
        self.assert_properties_edit_matches_staged(staged, &edit, &result);
        result
    }

    /// Debug proof that a direct properties edit left the package graph, the
    /// properties models and their part names of the staged edit, or
    /// refused with its error.
    #[cfg(debug_assertions)]
    fn assert_properties_edit_matches_staged(
        &self,
        mut staged: Self,
        edit: &impl Fn(&mut Self) -> Result<()>,
        result: &Result<()>,
    ) {
        match (result, edit(&mut staged)) {
            (Ok(()), Ok(())) => {
                assert!(
                    staged.package.same_package_as(&self.package),
                    "direct properties edit changed another package graph"
                );
                assert_eq!(
                    staged.identifiers, self.identifiers,
                    "direct properties edit left another identifier registry"
                );
                assert_eq!(staged.core_properties, self.core_properties);
                assert_eq!(
                    format!("{:?}", staged.application_properties),
                    format!("{:?}", self.application_properties)
                );
                // Compared as written: a NaN number is not == itself.
                assert!(
                    match (&staged.custom_properties, &self.custom_properties) {
                        (Some(staged), Some(committed)) => staged.is_same_as(committed),
                        (staged, committed) => staged.is_none() && committed.is_none(),
                    },
                    "direct properties edit left other custom properties"
                );
                assert_eq!(
                    (
                        &staged.core_properties_part_name,
                        &staged.application_properties_part_name,
                        &staged.custom_properties_part_name,
                        staged.custom_properties_owned,
                    ),
                    (
                        &self.core_properties_part_name,
                        &self.application_properties_part_name,
                        &self.custom_properties_part_name,
                        self.custom_properties_owned,
                    ),
                    "direct properties edit named another part"
                );
            }
            (Err(direct), Err(staged)) => assert_eq!(direct.to_string(), staged.to_string()),
            (Ok(()), Err(staged)) => panic!("direct properties edit hid the staged error {staged}"),
            (Err(direct), Ok(())) => panic!("direct properties edit refused with {direct}"),
        }
    }

    pub(crate) fn commit_staged_mutation(&mut self, candidate: Self) {
        let source_path = self.source_path.take();
        *self = Self::finish_staged_candidate(
            &self.package,
            self.package_signatures_invalidated,
            &mut self.normal_layout_engine,
            &mut self.bundled_fallback_layout_engine,
            candidate,
        );
        self.source_path = source_path;
    }
}
