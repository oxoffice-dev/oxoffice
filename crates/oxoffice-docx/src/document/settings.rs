//! Document settings, web settings and the document theme: reading, setting and removing each value.

use super::*;

impl Document {
    /// Return valid document-protection metadata recorded in the settings part.
    ///
    /// This reports author intent and password-verification metadata. It does
    /// not enforce an access-control boundary or verify a password.
    pub fn document_protection(&self) -> Option<&DocumentProtection> {
        self.settings.as_ref()?.document_protection()
    }
}

impl Document {
    /// Enable or disable automatic document hyphenation.
    pub fn set_auto_hyphenation(&mut self, enabled: bool) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        let created = candidate.settings_part_name.is_none();
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        let part_name = match &candidate.settings_part_name {
            Some(part_name) => part_name.clone(),
            None => candidate
                .identifiers
                .reserve_preferred_part_name(DEFAULT_SETTINGS_PART)?,
        };
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_automatic_hyphenation(enabled)?;
        candidate.invalidate_layout();
        candidate.settings_part_name = Some(part_name.clone());
        candidate.settings_owned |= created;
        candidate
            .ensure_part_relationship_checked(
                &part_name,
                rel_types::SETTINGS,
                SETTINGS_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("settings relationship allocation failed: {error}"))
            })?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Return whether distinct even-page header and footer stories are enabled.
    pub fn even_and_odd_headers(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(CT_Settings::even_and_odd_headers)
    }

    /// Enable or disable distinct even-page header and footer story selection.
    pub fn set_even_and_odd_headers(&mut self, enabled: bool) -> Result<()> {
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_even_and_odd_headers(enabled)?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Return whether the settings part asks Word to update fields when it
    /// opens the document, or `None` when the part does not say.
    pub fn update_fields_on_open(&self) -> Option<bool> {
        self.settings.as_ref()?.update_fields()
    }

    /// Document-wide note policy in the relationship-resolved settings part.
    pub fn note_properties(&self, family: NoteFamily) -> Option<&CT_NoteProperties> {
        let settings = self.settings.as_ref()?;
        match family {
            NoteFamily::Footnote => settings.footnote_properties(),
            NoteFamily::Endnote => settings.endnote_properties(),
        }
    }

    /// Set a checked document-wide note policy and publish the staged package.
    pub fn set_note_policy(&mut self, family: NoteFamily, policy: NotePolicy) -> Result<()> {
        let mut properties = self.note_properties(family).cloned().unwrap_or_default();
        let checked = policy.properties(family)?;
        properties.pos = checked.pos;
        properties.num_fmt = checked.num_fmt;
        properties.num_start = checked.num_start;
        properties.num_restart = checked.num_restart;
        self.stage_settings_mutation(|settings| match family {
            NoteFamily::Footnote => settings.set_footnote_properties(properties),
            NoteFamily::Endnote => settings.set_endnote_properties(properties),
        })
    }

    /// Remove an explicit document-wide note policy.
    pub fn remove_note_policy(&mut self, family: NoteFamily) -> Result<Option<CT_NoteProperties>> {
        let Some(mut properties) = self.note_properties(family).cloned() else {
            return Ok(None);
        };
        if properties.pos.is_none()
            && properties.num_fmt.is_none()
            && properties.num_start.is_none()
            && properties.num_restart.is_none()
        {
            return Ok(None);
        }
        let removed = properties.clone();
        properties.pos = None;
        properties.num_fmt = None;
        properties.num_start = None;
        properties.num_restart = None;
        if properties.special_references.is_empty() && properties.extra_xml.is_empty() {
            match family {
                NoteFamily::Footnote => {
                    self.stage_settings_removal(CT_Settings::remove_footnote_properties)?;
                }
                NoteFamily::Endnote => {
                    self.stage_settings_removal(CT_Settings::remove_endnote_properties)?;
                }
            }
        } else {
            self.stage_settings_mutation(|settings| match family {
                NoteFamily::Footnote => settings.set_footnote_properties(properties),
                NoteFamily::Endnote => settings.set_endnote_properties(properties),
            })?;
        }
        Ok(Some(removed))
    }

    /// Ask Word to update fields when it opens the document, stop asking with
    /// `Some(false)`, or remove the setting with `None`.
    pub fn set_update_fields_on_open(&mut self, value: Option<bool>) -> Result<()> {
        if value.is_none() && self.settings.is_none() {
            return Ok(());
        }
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_update_fields(value)?;
        candidate.prune_empty_owned_settings();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Return document-wide OfficeMath defaults from the settings part.
    pub fn math_properties(&self) -> Option<&MathProperties> {
        self.settings.as_ref()?.math_properties()
    }

    /// Set document-wide OfficeMath defaults in the relationship-resolved settings part.
    pub fn set_math_properties(&mut self, properties: MathProperties) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        let created = candidate.settings_part_name.is_none();
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        let part_name = match &candidate.settings_part_name {
            Some(part_name) => part_name.clone(),
            None => candidate
                .identifiers
                .reserve_preferred_part_name(DEFAULT_SETTINGS_PART)?,
        };
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_math_properties(properties)?;
        candidate.invalidate_layout();
        candidate.settings_part_name = Some(part_name.clone());
        candidate.settings_owned |= created;
        candidate
            .ensure_part_relationship_checked(
                &part_name,
                rel_types::SETTINGS,
                SETTINGS_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("settings relationship allocation failed: {error}"))
            })?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Return a document variable by name.
    pub fn document_variable(&self, name: &str) -> Option<&str> {
        self.settings.as_ref()?.document_variable(name)
    }

    /// Set a document variable in the relationship-resolved settings part.
    pub fn set_document_variable(&mut self, name: &str, value: &str) -> Result<()> {
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_document_variable(name.to_owned(), value.to_owned())?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Remove one document variable by name.
    pub fn remove_document_variable(&mut self, name: &str) -> Result<Option<String>> {
        let Some(settings) = &self.settings else {
            return Ok(None);
        };
        if settings.document_variable(name).is_none() {
            return Ok(None);
        }
        let mut candidate = self.settings_mutation_candidate()?;
        let removed = candidate
            .settings
            .as_mut()
            .expect("settings candidate retains its model")
            .remove_document_variable(name)?
            .map(|variable| variable.value);
        candidate.prune_empty_owned_settings();
        self.commit_staged_mutation(candidate);
        Ok(removed)
    }

    /// Return typed compatibility settings in package order.
    pub fn compatibility_settings(&self) -> &[CompatibilitySetting] {
        self.settings
            .as_ref()
            .map(CT_Settings::compatibility_settings)
            .unwrap_or_default()
    }

    /// Add or replace one compatibility setting selected by name and URI.
    pub fn set_compatibility_setting(&mut self, name: &str, uri: &str, value: &str) -> Result<()> {
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_compatibility_setting(CompatibilitySetting {
                name: name.to_owned(),
                uri: uri.to_owned(),
                value: value.to_owned(),
            })?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Remove one compatibility setting selected by name and URI.
    pub fn remove_compatibility_setting(
        &mut self,
        name: &str,
        uri: &str,
    ) -> Result<Option<CompatibilitySetting>> {
        let Some(settings) = &self.settings else {
            return Ok(None);
        };
        if !settings
            .compatibility_settings()
            .iter()
            .any(|setting| setting.name == name && setting.uri == uri)
        {
            return Ok(None);
        }
        let mut candidate = self.settings_mutation_candidate()?;
        let removed = candidate
            .settings
            .as_mut()
            .expect("settings candidate retains its model")
            .remove_compatibility_setting(name, uri)?;
        candidate.prune_empty_owned_settings();
        self.commit_staged_mutation(candidate);
        Ok(removed)
    }

    pub fn default_tab_stop(&self) -> Option<oxoffice_core::Twips> {
        self.settings.as_ref()?.default_tab_stop()
    }

    pub fn set_default_tab_stop(&mut self, value: oxoffice_core::Twips) -> Result<()> {
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_default_tab_stop(value)?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    pub fn remove_default_tab_stop(&mut self) -> Result<Option<oxoffice_core::Twips>> {
        if self.default_tab_stop().is_none() {
            return Ok(None);
        }
        let mut candidate = self.settings_mutation_candidate()?;
        let removed = candidate
            .settings
            .as_mut()
            .expect("settings candidate retains its model")
            .remove_default_tab_stop()?;
        candidate.prune_empty_owned_settings();
        self.commit_staged_mutation(candidate);
        Ok(removed)
    }

    pub fn character_spacing_control(&self) -> Option<CharacterSpacingControl> {
        self.settings.as_ref()?.character_spacing_control()
    }

    pub fn set_character_spacing_control(&mut self, value: CharacterSpacingControl) -> Result<()> {
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_character_spacing_control(value)?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    pub fn remove_character_spacing_control(&mut self) -> Result<Option<CharacterSpacingControl>> {
        if self.character_spacing_control().is_none() {
            return Ok(None);
        }
        let mut candidate = self.settings_mutation_candidate()?;
        let removed = candidate
            .settings
            .as_mut()
            .expect("settings candidate retains its model")
            .remove_character_spacing_control()?;
        candidate.prune_empty_owned_settings();
        self.commit_staged_mutation(candidate);
        Ok(removed)
    }

    pub fn theme_font_language(&self) -> Option<&ThemeFontLanguage> {
        self.settings.as_ref()?.theme_font_language()
    }

    /// Set the default document theme languages.
    ///
    /// The getter and remover already existed. The setter completes the pair,
    /// so a caller can author what it is allowed to take away.
    pub fn set_theme_font_language(&mut self, value: ThemeFontLanguage) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_theme_font_language(value))
    }

    pub fn remove_theme_font_language(&mut self) -> Result<Option<ThemeFontLanguage>> {
        if self.theme_font_language().is_none() {
            return Ok(None);
        }
        let mut candidate = self.settings_mutation_candidate()?;
        let removed = candidate
            .settings
            .as_mut()
            .expect("settings candidate retains its model")
            .remove_theme_font_language()?;
        candidate.prune_empty_owned_settings();
        self.commit_staged_mutation(candidate);
        Ok(removed)
    }

    /// Stage one settings mutation, apply it, and commit it atomically.
    ///
    /// Every typed settings member routes through here, which keeps part
    /// allocation, relationship allocation and layout invalidation in one
    /// place instead of once per accessor.
    pub(super) fn stage_settings_mutation(
        &mut self,
        apply: impl FnOnce(&mut CT_Settings) -> std::result::Result<(), oxoffice_docx_oxml::OxmlError>,
    ) -> Result<()> {
        let mut candidate = self.settings_mutation_candidate()?;
        apply(candidate.settings.get_or_insert_with(CT_Settings::new))?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Remove one typed settings member and prune an empty owned part.
    fn stage_settings_removal<T>(
        &mut self,
        apply: impl FnOnce(
            &mut CT_Settings,
        ) -> std::result::Result<Option<T>, oxoffice_docx_oxml::OxmlError>,
    ) -> Result<Option<T>> {
        if self.settings.is_none() {
            return Ok(None);
        }
        let mut candidate = self.settings_mutation_candidate()?;
        let removed = apply(
            candidate
                .settings
                .as_mut()
                .expect("settings candidate retains its model"),
        )?;
        candidate.prune_empty_owned_settings();
        self.commit_staged_mutation(candidate);
        Ok(removed)
    }

    /// Stage one web settings mutation, applying it to a part created on
    /// demand.
    fn stage_web_settings_mutation(
        &mut self,
        apply: impl FnOnce(
            &mut CT_WebSettings,
        ) -> std::result::Result<(), oxoffice_docx_oxml::OxmlError>,
    ) -> Result<()> {
        let mut candidate = self.web_settings_mutation_candidate()?;
        apply(
            candidate
                .web_settings
                .get_or_insert_with(CT_WebSettings::new),
        )?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Remove one typed web settings member and prune an empty owned part.
    fn stage_web_settings_removal<T>(
        &mut self,
        apply: impl FnOnce(
            &mut CT_WebSettings,
        ) -> std::result::Result<Option<T>, oxoffice_docx_oxml::OxmlError>,
    ) -> Result<Option<T>> {
        if self.web_settings.is_none() {
            return Ok(None);
        }
        let mut candidate = self.web_settings_mutation_candidate()?;
        let removed = apply(
            candidate
                .web_settings
                .as_mut()
                .expect("web settings candidate retains its model"),
        )?;
        candidate.prune_empty_owned_web_settings();
        self.commit_staged_mutation(candidate);
        Ok(removed)
    }

    /// Report every supported settings child the typed model could not own.
    ///
    /// A package authored entirely through this facade reports nothing.
    pub fn settings_diagnostics(&self) -> &[SettingsDiagnostic] {
        self.settings
            .as_ref()
            .map_or(&[][..], CT_Settings::diagnostics)
    }

    /// Report every supported web settings child the typed model could not own.
    pub fn web_settings_diagnostics(&self) -> &[SettingsDiagnostic] {
        self.web_settings
            .as_ref()
            .map_or(&[][..], CT_WebSettings::diagnostics)
    }

    /// Report every HTML division the web settings part declares.
    ///
    /// A `w:divId` on a paragraph resolves only when its value appears here.
    pub fn web_division_ids(&self) -> Vec<u32> {
        self.web_settings
            .as_ref()
            .map(CT_WebSettings::div_ids)
            .unwrap_or_default()
    }

    /// Record caller-supplied document-protection metadata verbatim.
    ///
    /// Password derivation is an explicit non-goal. Nothing here computes a
    /// hash, chooses a salt or picks a spin count.
    pub fn set_document_protection(&mut self, value: DocumentProtection) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_document_protection(value))
    }

    /// Remove document-protection metadata.
    pub fn remove_document_protection(&mut self) -> Result<Option<DocumentProtection>> {
        self.stage_settings_removal(CT_Settings::remove_document_protection)
    }

    /// Return the bounded mail-merge configuration from the settings part.
    ///
    /// The name carries the `_settings` suffix because `mail_merge` already
    /// names the field-merge operation on this type.
    pub fn mail_merge_settings(&self) -> Option<&MailMerge> {
        self.settings.as_ref()?.mail_merge()
    }

    /// Replace the modeled mail-merge members at their schema positions.
    pub fn set_mail_merge_settings(&mut self, value: MailMerge) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_mail_merge(value))
    }

    /// Remove every modeled mail-merge member.
    pub fn remove_mail_merge_settings(&mut self) -> Result<Option<MailMerge>> {
        self.stage_settings_removal(CT_Settings::remove_mail_merge)
    }

    /// Return every modeled `w:compat` toggle option in schema order.
    pub fn compatibility_options(&self) -> &[(CompatibilityOption, bool)] {
        self.settings
            .as_ref()
            .map_or(&[][..], CT_Settings::compatibility_options)
    }

    /// Return one `w:compat` toggle option.
    pub fn compatibility_option(&self, option: CompatibilityOption) -> Option<bool> {
        self.settings.as_ref()?.compatibility_option(option)
    }

    /// Set one `w:compat` toggle option at its schema position.
    pub fn set_compatibility_option(
        &mut self,
        option: CompatibilityOption,
        enabled: bool,
    ) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_compatibility_option(option, enabled))
    }

    /// Remove one `w:compat` toggle option.
    pub fn remove_compatibility_option(
        &mut self,
        option: CompatibilityOption,
    ) -> Result<Option<bool>> {
        self.stage_settings_removal(|settings| settings.remove_compatibility_option(option))
    }

    /// Remove the document automatic-hyphenation toggle.
    pub fn remove_auto_hyphenation(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_automatic_hyphenation)
    }

    /// Remove distinct even-page header and footer story selection.
    pub fn remove_even_and_odd_headers(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_even_and_odd_headers)
    }

    /// Remove document-wide OfficeMath defaults.
    pub fn remove_math_properties(&mut self) -> Result<Option<MathProperties>> {
        self.stage_settings_removal(CT_Settings::remove_math_properties)
    }

    /// Return the document view Word opens with from the settings part.
    pub fn view(&self) -> Option<DocumentView> {
        self.settings.as_ref()?.view()
    }

    /// Set the document view Word opens with in the relationship-resolved settings part.
    pub fn set_view(&mut self, value: DocumentView) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_view(value))
    }

    /// Remove the document view Word opens with, pruning an empty part this facade created.
    pub fn remove_view(&mut self) -> Result<Option<DocumentView>> {
        self.stage_settings_removal(CT_Settings::remove_view)
    }

    /// Return the magnification preset and percentage from the settings part.
    pub fn zoom(&self) -> Option<DocumentZoom> {
        self.settings.as_ref()?.zoom()
    }

    /// Set the magnification preset and percentage in the relationship-resolved settings part.
    pub fn set_zoom(&mut self, value: DocumentZoom) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_zoom(value))
    }

    /// Remove the magnification preset and percentage, pruning an empty part this facade created.
    pub fn remove_zoom(&mut self) -> Result<Option<DocumentZoom>> {
        self.stage_settings_removal(CT_Settings::remove_zoom)
    }

    /// Return the `w:removePersonalInformation` toggle from the settings part.
    pub fn remove_personal_information(&self) -> Option<bool> {
        self.settings.as_ref()?.remove_personal_information()
    }

    /// Set the `w:removePersonalInformation` toggle in the relationship-resolved settings part.
    pub fn set_remove_personal_information(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_remove_personal_information(value))
    }

    /// Remove the `w:removePersonalInformation` toggle, pruning an empty part this facade created.
    pub fn remove_remove_personal_information(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_remove_personal_information)
    }

    /// Return the `w:removeDateAndTime` toggle from the settings part.
    pub fn remove_date_and_time(&self) -> Option<bool> {
        self.settings.as_ref()?.remove_date_and_time()
    }

    /// Set the `w:removeDateAndTime` toggle in the relationship-resolved settings part.
    pub fn set_remove_date_and_time(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_remove_date_and_time(value))
    }

    /// Remove the `w:removeDateAndTime` toggle, pruning an empty part this facade created.
    pub fn remove_remove_date_and_time(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_remove_date_and_time)
    }

    /// Return the `w:mirrorMargins` toggle from the settings part.
    pub fn mirror_margins(&self) -> Option<bool> {
        self.settings.as_ref()?.mirror_margins()
    }

    /// Set the `w:mirrorMargins` toggle in the relationship-resolved settings part.
    pub fn set_mirror_margins(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_mirror_margins(value))
    }

    /// Remove the `w:mirrorMargins` toggle, pruning an empty part this facade created.
    pub fn remove_mirror_margins(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_mirror_margins)
    }

    /// Return the `w:gutterAtTop` toggle from the settings part.
    pub fn gutter_at_top(&self) -> Option<bool> {
        self.settings.as_ref()?.gutter_at_top()
    }

    /// Set the `w:gutterAtTop` toggle in the relationship-resolved settings part.
    pub fn set_gutter_at_top(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_gutter_at_top(value))
    }

    /// Remove the `w:gutterAtTop` toggle, pruning an empty part this facade created.
    pub fn remove_gutter_at_top(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_gutter_at_top)
    }

    /// Return the spelling and grammar proofing state from the settings part.
    pub fn proof_state(&self) -> Option<DocumentProofState> {
        self.settings.as_ref()?.proof_state()
    }

    /// Set the spelling and grammar proofing state in the relationship-resolved settings part.
    pub fn set_proof_state(&mut self, value: DocumentProofState) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_proof_state(value))
    }

    /// Remove the spelling and grammar proofing state, pruning an empty part this facade created.
    pub fn remove_proof_state(&mut self) -> Result<Option<DocumentProofState>> {
        self.stage_settings_removal(CT_Settings::remove_proof_state)
    }

    /// Return the `w:linkStyles` toggle from the settings part.
    pub fn link_styles(&self) -> Option<bool> {
        self.settings.as_ref()?.link_styles()
    }

    /// Set the `w:linkStyles` toggle in the relationship-resolved settings part.
    pub fn set_link_styles(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_link_styles(value))
    }

    /// Remove the `w:linkStyles` toggle, pruning an empty part this facade created.
    pub fn remove_link_styles(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_link_styles)
    }

    /// Return the `w:trackRevisions` toggle from the settings part.
    pub fn track_revisions(&self) -> Option<bool> {
        self.settings.as_ref()?.track_revisions()
    }

    /// Set the `w:trackRevisions` toggle in the relationship-resolved settings part.
    pub fn set_track_revisions(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_track_revisions(value))
    }

    /// Remove the `w:trackRevisions` toggle, pruning an empty part this facade created.
    pub fn remove_track_revisions(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_track_revisions)
    }

    /// Return the `w:doNotTrackMoves` toggle from the settings part.
    pub fn do_not_track_moves(&self) -> Option<bool> {
        self.settings.as_ref()?.do_not_track_moves()
    }

    /// Set the `w:doNotTrackMoves` toggle in the relationship-resolved settings part.
    pub fn set_do_not_track_moves(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_do_not_track_moves(value))
    }

    /// Remove the `w:doNotTrackMoves` toggle, pruning an empty part this facade created.
    pub fn remove_do_not_track_moves(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_do_not_track_moves)
    }

    /// Return the `w:doNotTrackFormatting` toggle from the settings part.
    pub fn do_not_track_formatting(&self) -> Option<bool> {
        self.settings.as_ref()?.do_not_track_formatting()
    }

    /// Set the `w:doNotTrackFormatting` toggle in the relationship-resolved settings part.
    pub fn set_do_not_track_formatting(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_do_not_track_formatting(value))
    }

    /// Remove the `w:doNotTrackFormatting` toggle, pruning an empty part this facade created.
    pub fn remove_do_not_track_formatting(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_do_not_track_formatting)
    }

    /// Return the maximum count of consecutive hyphenated lines from the settings part.
    pub fn consecutive_hyphen_limit(&self) -> Option<i32> {
        self.settings.as_ref()?.consecutive_hyphen_limit()
    }

    /// Set the maximum count of consecutive hyphenated lines in the relationship-resolved settings part.
    pub fn set_consecutive_hyphen_limit(&mut self, value: i32) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_consecutive_hyphen_limit(value))
    }

    /// Remove the maximum count of consecutive hyphenated lines, pruning an empty part this facade created.
    pub fn remove_consecutive_hyphen_limit(&mut self) -> Result<Option<i32>> {
        self.stage_settings_removal(CT_Settings::remove_consecutive_hyphen_limit)
    }

    /// Return the hyphenation zone width from the settings part.
    pub fn hyphenation_zone(&self) -> Option<oxoffice_core::Twips> {
        self.settings.as_ref()?.hyphenation_zone()
    }

    /// Set the hyphenation zone width in the relationship-resolved settings part.
    pub fn set_hyphenation_zone(&mut self, value: oxoffice_core::Twips) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_hyphenation_zone(value))
    }

    /// Remove the hyphenation zone width, pruning an empty part this facade created.
    pub fn remove_hyphenation_zone(&mut self) -> Result<Option<oxoffice_core::Twips>> {
        self.stage_settings_removal(CT_Settings::remove_hyphenation_zone)
    }

    /// Return the `w:doNotHyphenateCaps` toggle from the settings part.
    pub fn do_not_hyphenate_caps(&self) -> Option<bool> {
        self.settings.as_ref()?.do_not_hyphenate_caps()
    }

    /// Set the `w:doNotHyphenateCaps` toggle in the relationship-resolved settings part.
    pub fn set_do_not_hyphenate_caps(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_do_not_hyphenate_caps(value))
    }

    /// Remove the `w:doNotHyphenateCaps` toggle, pruning an empty part this facade created.
    pub fn remove_do_not_hyphenate_caps(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_do_not_hyphenate_caps)
    }

    /// Return the default table style identifier from the settings part.
    pub fn default_table_style(&self) -> Option<&str> {
        self.settings.as_ref()?.default_table_style()
    }

    /// Set the default table style identifier in the relationship-resolved settings part.
    pub fn set_default_table_style(&mut self, value: &str) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_default_table_style(value.to_owned()))
    }

    /// Remove the default table style identifier, pruning an empty part this facade created.
    pub fn remove_default_table_style(&mut self) -> Result<Option<String>> {
        self.stage_settings_removal(CT_Settings::remove_default_table_style)
    }

    /// Return the `w:bookFoldRevPrinting` toggle from the settings part.
    pub fn book_fold_rev_printing(&self) -> Option<bool> {
        self.settings.as_ref()?.book_fold_rev_printing()
    }

    /// Set the `w:bookFoldRevPrinting` toggle in the relationship-resolved settings part.
    pub fn set_book_fold_rev_printing(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_book_fold_rev_printing(value))
    }

    /// Remove the `w:bookFoldRevPrinting` toggle, pruning an empty part this facade created.
    pub fn remove_book_fold_rev_printing(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_book_fold_rev_printing)
    }

    /// Return the `w:bookFoldPrinting` toggle from the settings part.
    pub fn book_fold_printing(&self) -> Option<bool> {
        self.settings.as_ref()?.book_fold_printing()
    }

    /// Set the `w:bookFoldPrinting` toggle in the relationship-resolved settings part.
    pub fn set_book_fold_printing(&mut self, value: bool) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_book_fold_printing(value))
    }

    /// Remove the `w:bookFoldPrinting` toggle, pruning an empty part this facade created.
    pub fn remove_book_fold_printing(&mut self) -> Result<Option<bool>> {
        self.stage_settings_removal(CT_Settings::remove_book_fold_printing)
    }

    /// Return the signed sheet count per book-fold booklet from the settings part.
    pub fn book_fold_printing_sheets(&self) -> Option<i32> {
        self.settings.as_ref()?.book_fold_printing_sheets()
    }

    /// Set the signed sheet count per book-fold booklet in the relationship-resolved settings part.
    pub fn set_book_fold_printing_sheets(&mut self, value: i32) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_book_fold_printing_sheets(value))
    }

    /// Remove the signed sheet count per book-fold booklet, pruning an empty part this facade created.
    pub fn remove_book_fold_printing_sheets(&mut self) -> Result<Option<i32>> {
        self.stage_settings_removal(CT_Settings::remove_book_fold_printing_sheets)
    }

    /// Return the decimal separator Word uses in fields from the settings part.
    pub fn decimal_symbol(&self) -> Option<&str> {
        self.settings.as_ref()?.decimal_symbol()
    }

    /// Set the decimal separator Word uses in fields in the relationship-resolved settings part.
    pub fn set_decimal_symbol(&mut self, value: &str) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_decimal_symbol(value.to_owned()))
    }

    /// Remove the decimal separator Word uses in fields, pruning an empty part this facade created.
    pub fn remove_decimal_symbol(&mut self) -> Result<Option<String>> {
        self.stage_settings_removal(CT_Settings::remove_decimal_symbol)
    }

    /// Return the list separator Word uses in fields from the settings part.
    pub fn list_separator(&self) -> Option<&str> {
        self.settings.as_ref()?.list_separator()
    }

    /// Set the list separator Word uses in fields in the relationship-resolved settings part.
    pub fn set_list_separator(&mut self, value: &str) -> Result<()> {
        self.stage_settings_mutation(|settings| settings.set_list_separator(value.to_owned()))
    }

    /// Remove the list separator Word uses in fields, pruning an empty part this facade created.
    pub fn remove_list_separator(&mut self) -> Result<Option<String>> {
        self.stage_settings_removal(CT_Settings::remove_list_separator)
    }

    /// Return the character encoding Word writes web pages with from the web settings part.
    pub fn web_encoding(&self) -> Option<&str> {
        self.web_settings.as_ref()?.encoding()
    }

    /// Set the character encoding Word writes web pages with in the relationship-resolved web settings part.
    pub fn set_web_encoding(&mut self, value: &str) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_encoding(value.to_owned()))
    }

    /// Remove the character encoding Word writes web pages with, pruning an empty part this facade created.
    pub fn remove_web_encoding(&mut self) -> Result<Option<String>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_encoding)
    }

    /// Return the `w:optimizeForBrowser` toggle from the web settings part.
    pub fn web_optimize_for_browser(&self) -> Option<bool> {
        self.web_settings.as_ref()?.optimize_for_browser()
    }

    /// Set the `w:optimizeForBrowser` toggle in the relationship-resolved web settings part.
    pub fn set_web_optimize_for_browser(&mut self, value: bool) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_optimize_for_browser(value))
    }

    /// Remove the `w:optimizeForBrowser` toggle, pruning an empty part this facade created.
    pub fn remove_web_optimize_for_browser(&mut self) -> Result<Option<bool>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_optimize_for_browser)
    }

    /// Return the `w:relyOnVML` toggle from the web settings part.
    pub fn web_rely_on_vml(&self) -> Option<bool> {
        self.web_settings.as_ref()?.rely_on_vml()
    }

    /// Set the `w:relyOnVML` toggle in the relationship-resolved web settings part.
    pub fn set_web_rely_on_vml(&mut self, value: bool) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_rely_on_vml(value))
    }

    /// Remove the `w:relyOnVML` toggle, pruning an empty part this facade created.
    pub fn remove_web_rely_on_vml(&mut self) -> Result<Option<bool>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_rely_on_vml)
    }

    /// Return the `w:allowPNG` toggle from the web settings part.
    pub fn web_allow_png(&self) -> Option<bool> {
        self.web_settings.as_ref()?.allow_png()
    }

    /// Set the `w:allowPNG` toggle in the relationship-resolved web settings part.
    pub fn set_web_allow_png(&mut self, value: bool) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_allow_png(value))
    }

    /// Remove the `w:allowPNG` toggle, pruning an empty part this facade created.
    pub fn remove_web_allow_png(&mut self) -> Result<Option<bool>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_allow_png)
    }

    /// Return the `w:doNotRelyOnCSS` toggle from the web settings part.
    pub fn web_do_not_rely_on_css(&self) -> Option<bool> {
        self.web_settings.as_ref()?.do_not_rely_on_css()
    }

    /// Set the `w:doNotRelyOnCSS` toggle in the relationship-resolved web settings part.
    pub fn set_web_do_not_rely_on_css(&mut self, value: bool) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_do_not_rely_on_css(value))
    }

    /// Remove the `w:doNotRelyOnCSS` toggle, pruning an empty part this facade created.
    pub fn remove_web_do_not_rely_on_css(&mut self) -> Result<Option<bool>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_do_not_rely_on_css)
    }

    /// Return the `w:doNotSaveAsSingleFile` toggle from the web settings part.
    pub fn web_do_not_save_as_single_file(&self) -> Option<bool> {
        self.web_settings.as_ref()?.do_not_save_as_single_file()
    }

    /// Set the `w:doNotSaveAsSingleFile` toggle in the relationship-resolved web settings part.
    pub fn set_web_do_not_save_as_single_file(&mut self, value: bool) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_do_not_save_as_single_file(value))
    }

    /// Remove the `w:doNotSaveAsSingleFile` toggle, pruning an empty part this facade created.
    pub fn remove_web_do_not_save_as_single_file(&mut self) -> Result<Option<bool>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_do_not_save_as_single_file)
    }

    /// Return the `w:doNotOrganizeInFolder` toggle from the web settings part.
    pub fn web_do_not_organize_in_folder(&self) -> Option<bool> {
        self.web_settings.as_ref()?.do_not_organize_in_folder()
    }

    /// Set the `w:doNotOrganizeInFolder` toggle in the relationship-resolved web settings part.
    pub fn set_web_do_not_organize_in_folder(&mut self, value: bool) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_do_not_organize_in_folder(value))
    }

    /// Remove the `w:doNotOrganizeInFolder` toggle, pruning an empty part this facade created.
    pub fn remove_web_do_not_organize_in_folder(&mut self) -> Result<Option<bool>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_do_not_organize_in_folder)
    }

    /// Return the `w:doNotUseLongFileNames` toggle from the web settings part.
    pub fn web_do_not_use_long_file_names(&self) -> Option<bool> {
        self.web_settings.as_ref()?.do_not_use_long_file_names()
    }

    /// Set the `w:doNotUseLongFileNames` toggle in the relationship-resolved web settings part.
    pub fn set_web_do_not_use_long_file_names(&mut self, value: bool) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_do_not_use_long_file_names(value))
    }

    /// Remove the `w:doNotUseLongFileNames` toggle, pruning an empty part this facade created.
    pub fn remove_web_do_not_use_long_file_names(&mut self) -> Result<Option<bool>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_do_not_use_long_file_names)
    }

    /// Return the target pixel density for exported web pages from the web settings part.
    pub fn web_pixels_per_inch(&self) -> Option<i32> {
        self.web_settings.as_ref()?.pixels_per_inch()
    }

    /// Set the target pixel density for exported web pages in the relationship-resolved web settings part.
    pub fn set_web_pixels_per_inch(&mut self, value: i32) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_pixels_per_inch(value))
    }

    /// Remove the target pixel density for exported web pages, pruning an empty part this facade created.
    pub fn remove_web_pixels_per_inch(&mut self) -> Result<Option<i32>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_pixels_per_inch)
    }

    /// Return the target screen size, such as `800x600` from the web settings part.
    pub fn web_target_screen_size(&self) -> Option<&str> {
        self.web_settings.as_ref()?.target_screen_size()
    }

    /// Set the target screen size, such as `800x600` in the relationship-resolved web settings part.
    pub fn set_web_target_screen_size(&mut self, value: &str) -> Result<()> {
        self.stage_web_settings_mutation(|settings| {
            settings.set_target_screen_size(value.to_owned())
        })
    }

    /// Remove the target screen size, such as `800x600`, pruning an empty part this facade created.
    pub fn remove_web_target_screen_size(&mut self) -> Result<Option<String>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_target_screen_size)
    }

    /// Return the `w:saveSmartTagsAsXml` toggle from the web settings part.
    pub fn web_save_smart_tags_as_xml(&self) -> Option<bool> {
        self.web_settings.as_ref()?.save_smart_tags_as_xml()
    }

    /// Set the `w:saveSmartTagsAsXml` toggle in the relationship-resolved web settings part.
    pub fn set_web_save_smart_tags_as_xml(&mut self, value: bool) -> Result<()> {
        self.stage_web_settings_mutation(|settings| settings.set_save_smart_tags_as_xml(value))
    }

    /// Remove the `w:saveSmartTagsAsXml` toggle, pruning an empty part this facade created.
    pub fn remove_web_save_smart_tags_as_xml(&mut self) -> Result<Option<bool>> {
        self.stage_web_settings_removal(CT_WebSettings::remove_save_smart_tags_as_xml)
    }

    /// Return the relationship-resolved DrawingML theme.
    pub fn theme(&self) -> Option<&oxoffice_drawing::theme::CT_OfficeStyleSheet> {
        self.theme.as_ref()
    }

    /// Replace or create the relationship-resolved DrawingML theme atomically.
    pub fn set_theme(&mut self, theme: oxoffice_drawing::theme::CT_OfficeStyleSheet) -> Result<()> {
        theme
            .to_xml()
            .map_err(|error| Error::Other(format!("theme serialization failed: {error}")))?;
        let mut candidate = self.clone_for_staging();
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        let part_name = match &candidate.theme_part_name {
            Some(part_name) => part_name.clone(),
            None => candidate
                .identifiers
                .reserve_preferred_part_name(DEFAULT_THEME_PART)?,
        };
        candidate
            .ensure_part_relationship_checked(&part_name, rel_types::THEME, content_types::THEME)
            .map_err(|error| {
                Error::Other(format!("theme relationship allocation failed: {error}"))
            })?;
        candidate.theme = Some(theme);
        candidate.theme_part_name = Some(part_name);
        candidate.theme_dirty = true;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Set the document-wide Latin, East Asian, and bidirectional languages.
    pub fn set_language_defaults(&mut self, value: ThemeFontLanguage) -> Result<()> {
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_theme_font_language(value)?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }
}

impl Document {
    pub(super) fn settings_mutation_candidate(&self) -> Result<Self> {
        let mut candidate = self.clone_for_staging();
        let created = candidate.settings_part_name.is_none();
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        let part_name = match &candidate.settings_part_name {
            Some(part_name) => part_name.clone(),
            None => candidate
                .identifiers
                .reserve_preferred_part_name(DEFAULT_SETTINGS_PART)?,
        };
        candidate.invalidate_layout();
        candidate.settings_part_name = Some(part_name.clone());
        candidate.settings_owned |= created;
        candidate
            .ensure_part_relationship_checked(
                &part_name,
                rel_types::SETTINGS,
                SETTINGS_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("settings relationship allocation failed: {error}"))
            })?;
        Ok(candidate)
    }

    fn web_settings_mutation_candidate(&self) -> Result<Self> {
        let mut candidate = self.clone_for_staging();
        let created = candidate.web_settings_part_name.is_none();
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        let part_name = match &candidate.web_settings_part_name {
            Some(part_name) => part_name.clone(),
            None => candidate
                .identifiers
                .reserve_preferred_part_name(DEFAULT_WEB_SETTINGS_PART)?,
        };
        candidate.web_settings_part_name = Some(part_name.clone());
        candidate.web_settings_owned |= created;
        candidate
            .ensure_part_relationship_checked(
                &part_name,
                rel_types::WEB_SETTINGS,
                WEB_SETTINGS_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!(
                    "web settings relationship allocation failed: {error}"
                ))
            })?;
        Ok(candidate)
    }

    fn prune_empty_owned_web_settings(&mut self) {
        if !self.web_settings_owned
            || !self
                .web_settings
                .as_ref()
                .is_some_and(CT_WebSettings::is_empty)
        {
            return;
        }
        let Some(part_name) = self.web_settings_part_name.take() else {
            return;
        };
        let owner = self.doc_part_name.clone();
        let mut removed_ids = Vec::new();
        if let Some(relationships) = self.package.get_part_rels_mut(&owner) {
            relationships.items.retain(|relationship| {
                let remove = relationship.rel_type == rel_types::WEB_SETTINGS
                    && relationship_is_internal(relationship)
                    && OpcPackage::resolve_rel_target(&owner, &relationship.target) == part_name;
                if remove {
                    removed_ids.push(relationship.id.clone());
                }
                !remove
            });
        }
        self.identifiers
            .retire_authored_story_relationships(&owner, removed_ids);
        self.package.remove_part(&part_name);
        self.package.remove_part_rels(&part_name);
        self.package.content_types.remove_override(&part_name);
        self.identifiers.retire_authored_part(&part_name);
        self.web_settings = None;
        self.web_settings_owned = false;
    }

    fn prune_empty_owned_settings(&mut self) {
        if !self.settings_owned || !self.settings.as_ref().is_some_and(CT_Settings::is_empty) {
            return;
        }
        let Some(part_name) = self.settings_part_name.take() else {
            return;
        };
        let owner = self.doc_part_name.clone();
        let mut removed_ids = Vec::new();
        if let Some(relationships) = self.package.get_part_rels_mut(&owner) {
            relationships.items.retain(|relationship| {
                let remove = relationship.rel_type == rel_types::SETTINGS
                    && relationship_is_internal(relationship)
                    && OpcPackage::resolve_rel_target(&owner, &relationship.target) == part_name;
                if remove {
                    removed_ids.push(relationship.id.clone());
                }
                !remove
            });
        }
        self.identifiers
            .retire_authored_story_relationships(&owner, removed_ids);
        self.package.remove_part(&part_name);
        self.package.remove_part_rels(&part_name);
        self.package.content_types.remove_override(&part_name);
        self.identifiers.retire_authored_part(&part_name);
        self.settings = None;
        self.settings_owned = false;
    }
}
