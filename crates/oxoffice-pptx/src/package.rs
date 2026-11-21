//! Saving and committing: package classes, bytes, files, encryption and signatures, the staged package, and the checked commit of an edit.

use super::*;

/// The startup and macro class carried by a modern PresentationML package.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationPackageClass {
    Presentation,
    MacroEnabledPresentation,
    Template,
    MacroEnabledTemplate,
    Slideshow,
    MacroEnabledSlideshow,
}

impl PresentationPackageClass {
    pub(super) fn from_content_type(content_type: &str) -> Option<Self> {
        match content_type {
            content_types::PRESENTATION => Some(Self::Presentation),
            content_types::PRESENTATION_MACRO_ENABLED => Some(Self::MacroEnabledPresentation),
            content_types::PRESENTATION_TEMPLATE => Some(Self::Template),
            content_types::PRESENTATION_TEMPLATE_MACRO_ENABLED => Some(Self::MacroEnabledTemplate),
            content_types::SLIDESHOW => Some(Self::Slideshow),
            content_types::SLIDESHOW_MACRO_ENABLED => Some(Self::MacroEnabledSlideshow),
            _ => None,
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Presentation => content_types::PRESENTATION,
            Self::MacroEnabledPresentation => content_types::PRESENTATION_MACRO_ENABLED,
            Self::Template => content_types::PRESENTATION_TEMPLATE,
            Self::MacroEnabledTemplate => content_types::PRESENTATION_TEMPLATE_MACRO_ENABLED,
            Self::Slideshow => content_types::SLIDESHOW,
            Self::MacroEnabledSlideshow => content_types::SLIDESHOW_MACRO_ENABLED,
        }
    }

    /// The class that a `.pptx`, `.pptm`, `.potx`, `.potm`, `.ppsx`, or `.ppsm` path names.
    fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "pptx" => Some(Self::Presentation),
            "pptm" => Some(Self::MacroEnabledPresentation),
            "potx" => Some(Self::Template),
            "potm" => Some(Self::MacroEnabledTemplate),
            "ppsx" => Some(Self::Slideshow),
            "ppsm" => Some(Self::MacroEnabledSlideshow),
            _ => None,
        }
    }

    fn is_macro_enabled(self) -> bool {
        matches!(
            self,
            Self::MacroEnabledPresentation
                | Self::MacroEnabledTemplate
                | Self::MacroEnabledSlideshow
        )
    }
}

impl Presentation {
    // Imported ambiguous shape identities remain readable and source-preserved.
    // They cannot be newly created or edited past a checked structural boundary.
    // Every other validation issue keeps the existing save assertion.
    pub(super) fn save_invariants_preserved(&self) -> bool {
        self.validate().iter().all(|issue| {
            // Finite XML diagnostics remain report-only at ordinary save.
            // Checked replacement owns raw/staged source admission separately.
            if issue.is_report_only() {
                return true;
            }
            let ValidationIssue::DuplicateShapeId { slide, .. } = issue else {
                return false;
            };
            self.slides.get(*slide).is_some_and(|record| {
                self.package
                    .get_part(&record.part_name)
                    .and_then(|xml| CT_Slide::from_xml(xml).ok())
                    .as_ref()
                    == Some(&record.slide)
            })
        })
    }

    /// Serialises the package through the deterministic OPC writer.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        debug_assert!(
            self.save_invariants_preserved(),
            "invalid presentation at byte save boundary: {:?}",
            self.validate()
        );
        let mut output = Cursor::new(Vec::new());
        self.saved_package()?.write_to(&mut output)?;
        Ok(output.into_inner())
    }

    /// The package [`Presentation::to_bytes`] writes.
    pub(super) fn saved_package(&self) -> Result<OpcPackage> {
        let package_signatures_invalidated = self.package_signatures_invalidated
            || self.retained_package_signature_would_be_invalidated()?;
        let mut package = self.staged_package(true)?;
        embedded::persist_invalidated_package_signature(
            &mut package,
            package_signatures_invalidated,
        )?;
        Ok(package)
    }

    /// The package that reading back what [`Presentation::to_bytes`] writes
    /// opens, or the error it raises. It is reread in memory when what the
    /// last opening parsed is known.
    pub(super) fn saved_package_read_back(&self) -> Result<OpcPackage> {
        debug_assert!(
            self.save_invariants_preserved(),
            "invalid presentation at byte save boundary: {:?}",
            self.validate()
        );
        let package = self.saved_package()?;
        if let Some(parsed) = self.parsed.as_deref() {
            return Ok(package.reread(&parsed.package)?);
        }
        let mut output = Cursor::new(Vec::new());
        package.write_to(&mut output)?;
        output.set_position(0);
        Ok(OpcPackage::from_reader(output)?)
    }

    /// Whether this presentation and `other` save to the same bytes: what
    /// comparing their [`Presentation::to_bytes`], this one's first, gives,
    /// without writing either ZIP. Packages read back are equal exactly when
    /// the bytes written are.
    #[doc(hidden)]
    pub fn saves_as(&self, other: &Presentation) -> Result<bool> {
        let same: Result<bool> = (|| {
            Ok(self
                .saved_package_read_back()?
                .same_package_as(&other.saved_package_read_back()?))
        })();
        // Debug builds, which run the binding tests, compare the bytes too.
        #[cfg(debug_assertions)]
        match (
            &same,
            (|| Ok::<_, Error>(self.to_bytes()? == other.to_bytes()?))(),
        ) {
            (Ok(same), Ok(bytes)) => assert_eq!(*same, bytes, "saves_as differs from to_bytes"),
            (Err(same), Err(bytes)) => assert_eq!(same.to_string(), bytes.to_string()),
            (same, bytes) => panic!("saves_as gives {same:?} where to_bytes gives {bytes:?}"),
        }
        same
    }

    /// Returns the exact modern package class carried by the main part.
    pub fn package_class(&self) -> Result<PresentationPackageClass> {
        let content_type = self
            .package
            .content_types
            .content_type_for(&self.presentation_part)
            .map(str::to_owned);
        PresentationPackageClass::from_content_type(content_type.as_deref().unwrap_or(""))
            .ok_or(Error::UnsupportedPackageClass { content_type })
    }

    /// Serialises an output copy with the selected package class.
    ///
    /// Executable payloads and every unrelated part and relationship remain
    /// byte-preserved. The live presentation keeps its original class.
    pub fn to_bytes_as(&self, class: PresentationPackageClass) -> Result<Vec<u8>> {
        debug_assert!(
            self.save_invariants_preserved(),
            "invalid presentation at package-class save boundary: {:?}",
            self.validate()
        );
        let class_changed = self.package_class()? != class;
        let preserve_signed_parts = self
            .package
            .package_rels
            .get_by_type(rel_types::DIGITAL_SIGNATURE_ORIGIN)
            .is_some();
        let package_signatures_invalidated = self.package_signatures_invalidated
            || self.retained_package_signature_would_be_invalidated()?
            || (class_changed && preserve_signed_parts);
        let mut package = self.staged_package(true)?;
        package
            .content_types
            .add_override(&self.presentation_part, class.content_type());
        embedded::persist_invalidated_package_signature(
            &mut package,
            package_signatures_invalidated,
        )?;
        let mut output = Cursor::new(Vec::new());
        package.write_to(&mut output)?;
        Ok(output.into_inner())
    }

    /// Serialises the bytes that [`Presentation::save`] writes to `path`.
    pub fn to_bytes_for_path<P: AsRef<Path>>(&self, path: P) -> Result<Vec<u8>> {
        let Some(target) = PresentationPackageClass::from_path(path.as_ref()) else {
            return self.to_bytes();
        };
        let current = self.package_class()?;
        if target == current {
            return self.to_bytes();
        }
        let carries_vba = self
            .package
            .get_part_rels(&self.presentation_part)
            .is_some_and(|relationships| {
                relationships.get_by_type(rel_types::VBA_PROJECT).is_some()
            });
        if carries_vba && !target.is_macro_enabled() {
            return Err(invalid_presentation_mutation(
                "save",
                format!(
                    "{} cannot be written: the presentation carries a VBA project and the extension names a macro-free class, save it as .pptm, .potm, or .ppsm",
                    path.as_ref().display()
                ),
            ));
        }
        self.to_bytes_as(target)
    }

    /// Serialises the current presentation through the fixed Agile write profile.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn to_encrypted_bytes(&self, password: &str) -> Result<Vec<u8>> {
        let package_signatures_invalidated = self.package_signatures_invalidated
            || self.retained_package_signature_would_be_invalidated()?;
        let mut package = self.staged_package(true)?;
        embedded::persist_invalidated_package_signature(
            &mut package,
            package_signatures_invalidated,
        )?;
        let mut output = Vec::new();
        package.write_encrypted_to(&mut output, password)?;
        Ok(output)
    }

    /// Verifies signatures against the current staged presentation package.
    ///
    /// Cryptographic validity authenticates the embedded certificate's key. It
    /// does not establish certificate-chain trust, which remains caller policy.
    #[cfg(feature = "digital-signatures")]
    pub fn verify_signatures(&self) -> Result<Vec<SignatureReport>> {
        Ok(self.staged_package(true)?.verify_signatures()?)
    }

    /// Signs the current staged presentation with the strict RSA-SHA256 profile.
    ///
    /// The private key must be PKCS#8 DER or one unencrypted PRIVATE KEY PEM
    /// block. The certificate must be X.509 DER or one CERTIFICATE PEM block.
    /// The live package changes only after the shared signer verifies the cloned
    /// candidate with complete coverage. Certificate-chain trust remains caller
    /// policy.
    #[cfg(all(feature = "digital-signatures", not(target_arch = "wasm32")))]
    pub fn sign(
        &mut self,
        private_key_pkcs8_der: &[u8],
        certificate_der: &[u8],
    ) -> Result<SignatureReport> {
        let mut candidate = self.staged_package(true)?;
        let report = candidate.sign(private_key_pkcs8_der, certificate_der)?;
        if !report.cryptographically_valid || !report.coverage_complete {
            return Err(OpcError::SignatureCreationFailed(
                "staged presentation signature did not verify with complete coverage".to_owned(),
            )
            .into());
        }
        self.package = candidate;
        Ok(report)
    }
}

impl Presentation {
    pub(super) fn staged_package(
        &self,
        preserve_unchanged_modelled_parts: bool,
    ) -> Result<OpcPackage> {
        if self.core_properties_dirty
            && self
                .package
                .package_rels
                .get_by_type(rel_types::CORE_PROPERTIES)
                .is_none()
            && self.package.contains_part(&self.core_properties_part_name)
        {
            return Err(Error::CorePropertiesPartCollision {
                part_name: self.core_properties_part_name.clone(),
            });
        }
        let mut package = self.package.clone();
        self.media_store.write_new_parts(&mut package);
        if let Some(properties) = self
            .core_properties
            .as_ref()
            .filter(|_| self.core_properties_dirty)
        {
            let xml = properties.to_xml().map_err(|error| Error::MalformedPart {
                part_name: self.core_properties_part_name.clone(),
                message: error.to_string(),
            })?;
            package.set_part(&self.core_properties_part_name, xml);
            package.content_types.add_override(
                &self.core_properties_part_name,
                content_types::CORE_PROPERTIES,
            );
            if package
                .package_rels
                .get_by_type(rel_types::CORE_PROPERTIES)
                .is_none()
            {
                let target = self
                    .core_properties_part_name
                    .strip_prefix('/')
                    .unwrap_or(&self.core_properties_part_name);
                package.package_rels.add(rel_types::CORE_PROPERTIES, target);
            }
        }
        let presentation_changed = match self
            .parsed
            .as_deref()
            .and_then(|parsed| parsed.presentation(&self.package, &self.presentation_part))
        {
            Some(parsed) => *parsed != self.presentation,
            None => {
                self.package
                    .get_part(&self.presentation_part)
                    .and_then(|xml| CT_Presentation::from_xml(xml).ok())
                    .as_ref()
                    != Some(&self.presentation)
            }
        };
        if !preserve_unchanged_modelled_parts || presentation_changed {
            package.set_part(
                &self.presentation_part,
                self.presentation
                    .to_xml()
                    .map_err(|error| Error::MalformedPart {
                        part_name: self.presentation_part.clone(),
                        message: error.to_string(),
                    })?,
            );
        }
        if let Some(part_name) = self
            .comment_authors_part
            .as_ref()
            .filter(|_| self.comment_authors_dirty)
        {
            package.set_part(
                part_name,
                self.comment_authors
                    .to_xml()
                    .map_err(|error| Error::MalformedPart {
                        part_name: part_name.clone(),
                        message: error.to_string(),
                    })?,
            );
            package
                .content_types
                .add_override(part_name, content_types::POWERPOINT_AUTHORS);
        }
        if let (Some(part_name), Some(master)) = (
            self.notes_master_part
                .as_ref()
                .filter(|_| self.notes_master_dirty),
            &self.notes_master,
        ) {
            package.set_part(
                part_name,
                master.to_xml().map_err(|error| Error::MalformedPart {
                    part_name: part_name.clone(),
                    message: error.to_string(),
                })?,
            );
        }
        if let (Some(part_name), Some(master)) = (
            self.handout_master_part
                .as_ref()
                .filter(|_| self.handout_master_dirty),
            &self.handout_master,
        ) {
            package.set_part(
                part_name,
                master.to_xml().map_err(|error| Error::MalformedPart {
                    part_name: part_name.clone(),
                    message: error.to_string(),
                })?,
            );
        }
        for record in self.masters.iter().filter(|record| record.dirty) {
            let master = record
                .master
                .as_ref()
                .map_err(|message| Error::MalformedPart {
                    part_name: record.part_name.clone(),
                    message: message.clone(),
                })?;
            let original = self
                .package
                .get_part(&record.part_name)
                .and_then(|xml| CT_SlideMaster::from_xml(xml).ok());
            if original.as_ref() != Some(master) {
                package.set_part(
                    &record.part_name,
                    master.to_xml().map_err(|error| Error::MalformedPart {
                        part_name: record.part_name.clone(),
                        message: error.to_string(),
                    })?,
                );
            }
        }
        for record in self.layouts.iter().filter(|record| record.dirty) {
            let original = self
                .package
                .get_part(&record.part_name)
                .and_then(|xml| CT_SlideLayout::from_xml(xml).ok());
            if original.as_ref() != Some(&record.layout) {
                package.set_part(
                    &record.part_name,
                    record
                        .layout
                        .to_xml()
                        .map_err(|error| Error::MalformedPart {
                            part_name: record.part_name.clone(),
                            message: error.to_string(),
                        })?,
                );
            }
        }
        for record in self.themes.iter().filter(|record| record.dirty) {
            let original = self
                .package
                .get_part(&record.part_name)
                .and_then(|xml| CT_OfficeStyleSheet::from_xml(xml).ok());
            if original.as_ref() != Some(&record.theme) {
                package.set_part(
                    &record.part_name,
                    record
                        .theme
                        .to_xml()
                        .map_err(|error| Error::MalformedPart {
                            part_name: record.part_name.clone(),
                            message: error.to_string(),
                        })?,
                );
            }
        }
        for record in &self.slides {
            // A record parsed from the bytes its parts still hold is unchanged
            // without parsing them again.
            let parsed = self
                .parsed
                .as_deref()
                .filter(|parsed| parsed.holds_slide(record));
            let parsed_from =
                |part: &str| parsed.is_some_and(|parsed| parsed.same_bytes(&self.package, part));
            let slide_changed = !parsed_from(&record.part_name) && {
                self.package
                    .get_part(&record.part_name)
                    .and_then(|xml| CT_Slide::from_xml(xml).ok())
                    .as_ref()
                    != Some(&record.slide)
            };
            if !preserve_unchanged_modelled_parts || slide_changed {
                package.set_part(
                    &record.part_name,
                    record
                        .slide
                        .to_xml()
                        .map_err(|error| Error::MalformedPart {
                            part_name: record.part_name.clone(),
                            message: error.to_string(),
                        })?,
                );
            }
            if let Some(notes) = &record.notes {
                let notes_changed = !parsed_from(&notes.part_name) && {
                    self.package
                        .get_part(&notes.part_name)
                        .and_then(|xml| CT_NotesSlide::from_xml(xml).ok())
                        .as_ref()
                        != Some(&notes.notes)
                };
                if !preserve_unchanged_modelled_parts || notes_changed {
                    package.set_part(
                        &notes.part_name,
                        notes.notes.to_xml().map_err(|error| Error::MalformedPart {
                            part_name: notes.part_name.clone(),
                            message: error.to_string(),
                        })?,
                    );
                }
            }
            if let Some(comments) = record.comments.as_ref().filter(|comments| comments.dirty) {
                package.set_part(
                    &comments.part_name,
                    comments
                        .comments
                        .to_xml()
                        .map_err(|error| Error::MalformedPart {
                            part_name: comments.part_name.clone(),
                            message: error.to_string(),
                        })?,
                );
                package
                    .content_types
                    .add_override(&comments.part_name, content_types::POWERPOINT_COMMENTS);
            }
        }
        Ok(package)
    }

    /// Saves the deterministic package bytes to a path.
    ///
    /// The bytes are staged in a synced sibling file and renamed over `path`
    /// through [`oxoffice_opc::write_atomic_file`], so a failed save leaves an
    /// existing file as it was. A symbolic link at `path` is kept and the file
    /// it names is replaced, and on Unix that file keeps its permission bits.
    ///
    /// A `.pptx`, `.pptm`, `.potx`, `.potm`, `.ppsx`, or `.ppsm` extension
    /// selects the main part content type, so a template saved as `.pptx`
    /// declares a presentation. Any other extension keeps the opened class,
    /// and `save_encrypted` ignores the extension. When the class changes, a
    /// presentation part that carries a VBA project
    /// cannot be saved under a macro-free extension, because the project
    /// would remain in a file that claims to carry none.
    /// [`Presentation::save_as_package_class`] performs that conversion
    /// explicitly and keeps the VBA part.
    ///
    /// A presentation opened from a file refuses to be saved over that file,
    /// whatever the spelling of `path` (a symbolic link, a relative path, a
    /// hard link), with [`Error::OverwritesSource`]. This differs from
    /// python-pptx. [`Presentation::save_with_options`] with
    /// [`SaveOptions::overwrite_source`] replaces it.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.save_with_options(path, SaveOptions::default())
    }

    /// Saves to a path, as [`Presentation::save`] does, with `options`.
    pub fn save_with_options<P: AsRef<Path>>(&self, path: P, options: SaveOptions) -> Result<()> {
        if !options.overwrite_source
            && let Some(source) = &self.source_path
            && oxoffice_opc::is_same_file(source, path.as_ref())
        {
            return Err(Error::OverwritesSource {
                path: path.as_ref().to_path_buf(),
            });
        }
        debug_assert!(
            self.save_invariants_preserved(),
            "invalid presentation at path save boundary: {:?}",
            self.validate()
        );
        write_atomic_file(
            path.as_ref(),
            &self.to_bytes_for_path(path.as_ref())?,
            "oxoffice",
            "invalid PowerPoint package file name",
            "could not allocate PowerPoint package save staging file",
        )
        .map_err(OpcError::from)?;
        Ok(())
    }

    /// Saves a password-protected presentation through an atomic path publication.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn save_encrypted<P: AsRef<Path>>(&self, path: P, password: &str) -> Result<()> {
        let bytes = self.to_encrypted_bytes(password)?;
        write_atomic_file(
            path.as_ref(),
            &bytes,
            "oxoffice",
            "invalid file name",
            "could not allocate encrypted-save staging file",
        )
        .map_err(OpcError::from)?;
        Ok(())
    }

    /// Saves a slideshow package without changing later ordinary saves.
    pub fn save_as_show<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.save_as_package_class(path, PresentationPackageClass::Slideshow)
    }

    /// Saves an output copy with the selected modern package class.
    ///
    /// The file is replaced the way [`Presentation::save`] replaces it.
    pub fn save_as_package_class<P: AsRef<Path>>(
        &self,
        path: P,
        class: PresentationPackageClass,
    ) -> Result<()> {
        write_atomic_file(
            path.as_ref(),
            &self.to_bytes_as(class)?,
            "oxoffice",
            "invalid PowerPoint package file name",
            "could not allocate PowerPoint package save staging file",
        )
        .map_err(OpcError::from)?;
        Ok(())
    }
}

impl Presentation {
    /// Reopen validates the changed source before any live record is replaced.
    pub(super) fn commit_owned_parts(&mut self, xml_owner: Option<ShapeOwner>) -> Result<()> {
        *self = self.checked_commit(|staged| {
            let mut package = staged.staged_package(true)?;
            let mut forced_changed = false;
            if let Some(owner) = xml_owner {
                // A checked XML edit may change retained root attributes which
                // rendering-model equality intentionally does not compare.
                // Force only this actual part, preserving all unrelated sources.
                let name = staged.owner_part_name(owner)?.to_owned();
                let xml = staged.checked_owner_xml(PptXmlOwner::Part(owner))?;
                forced_changed = staged.package.get_part(&name) != Some(xml.as_slice());
                package.set_part(&name, xml);
            }
            let invalidated = staged.package_signatures_invalidated
                || staged.retained_package_signature_would_be_invalidated()?
                || forced_changed
                    && staged
                        .package
                        .package_rels
                        .get_by_type(rel_types::DIGITAL_SIGNATURE_ORIGIN)
                        .is_some();
            embedded::persist_invalidated_package_signature(&mut package, invalidated)?;
            let mut reopened = staged.reopen(package)?;
            reopened.embedded_invalidated_signatures =
                staged.embedded_invalidated_signatures.clone();
            reopened.package_signatures_invalidated = invalidated;
            Ok(reopened)
        })?;
        Ok(())
    }

    /// Opens the staged `package` again, as writing it and reading the bytes
    /// back does. With what the last opening parsed, the package is reread
    /// in memory, each record whose part bytes it still shares is reused,
    /// and only new media are hashed. Without it, the package goes through
    /// the ZIP.
    pub(super) fn reopen(&self, package: OpcPackage) -> Result<Self> {
        let Some(parsed) = self.parsed.as_deref() else {
            let mut output = Cursor::new(Vec::new());
            package.write_to(&mut output)?;
            let mut reopened = Self::from_bytes(output.get_ref())?;
            reopened.source_path = self.source_path.clone();
            return Ok(reopened);
        };
        let package = package.reread(&parsed.package)?;
        let mut reopened = Self::from_package_reusing(package, Some((parsed, &self.media_store)))?;
        reopened.parsed = Some(Arc::new(Parsed::of(&reopened, Some(parsed))));
        reopened.source_path = self.source_path.clone();
        Ok(reopened)
    }

    /// Runs `commit` on this staged presentation. Debug builds, which run the
    /// binding tests, run it again on a copy that forgets what the last
    /// opening parsed, so that it stages by parsing every slide part and
    /// reopens through the ZIP as every commit did before, and assert that
    /// both give the same presentation or the same error.
    pub(super) fn checked_commit(&self, commit: impl Fn(&Self) -> Result<Self>) -> Result<Self> {
        let committed = commit(self);
        #[cfg(debug_assertions)]
        {
            let mut through_zip = self.clone();
            through_zip.parsed = None;
            let expected = commit(&through_zip);
            assert_same_commit(&committed, &expected);
        }
        committed
    }
}

impl Presentation {
    pub(super) fn commit_candidate(&mut self, staged: Self) -> Result<()> {
        *self = staged.checked_commit(|staged| {
            let package_signatures_invalidated = staged.package_signatures_invalidated
                || staged.retained_package_signature_would_be_invalidated()?;
            let mut package = staged.staged_package(true)?;
            embedded::persist_invalidated_package_signature(
                &mut package,
                package_signatures_invalidated,
            )?;
            let mut reopened = staged.reopen(package)?;
            reopened.embedded_invalidated_signatures =
                staged.embedded_invalidated_signatures.clone();
            reopened.package_signatures_invalidated = package_signatures_invalidated;
            Ok(reopened)
        })?;
        Ok(())
    }
}

/// Asserts that a commit gives what the commit through the ZIP gives: the
/// same error, or the same package, the same records serialized, the same
/// media store and the same package a save stages.
#[cfg(debug_assertions)]
fn assert_same_commit(committed: &Result<Presentation>, through_zip: &Result<Presentation>) {
    fn xml<E: std::fmt::Display>(
        xml: std::result::Result<Vec<u8>, E>,
    ) -> std::result::Result<Vec<u8>, String> {
        xml.map_err(|error| error.to_string())
    }
    fn state(presentation: &Presentation) -> impl PartialEq + std::fmt::Debug {
        let slides = presentation
            .slides
            .iter()
            .map(|record| {
                (
                    record.id,
                    record.part_name.clone(),
                    xml(record.slide.to_xml()),
                    record
                        .notes
                        .as_ref()
                        .map(|notes| (notes.part_name.clone(), xml(notes.notes.to_xml()))),
                    record.comments.as_ref().map(|comments| {
                        (
                            comments.part_name.clone(),
                            xml(comments.comments.to_xml()),
                            comments.dirty,
                        )
                    }),
                )
            })
            .collect::<Vec<_>>();
        let layouts = presentation
            .layouts
            .iter()
            .map(|record| {
                (
                    record.part_name.clone(),
                    xml(record.layout.to_xml()),
                    record.dirty,
                )
            })
            .collect::<Vec<_>>();
        let masters = presentation
            .masters
            .iter()
            .map(|record| {
                let master = match &record.master {
                    Ok(master) => xml(master.to_xml()),
                    Err(message) => Err(message.clone()),
                };
                (record.id, record.part_name.clone(), master, record.dirty)
            })
            .collect::<Vec<_>>();
        let mut media = presentation
            .media_store
            .by_id
            .iter()
            .map(|(id, bucket)| {
                let bucket = bucket
                    .iter()
                    .map(|entry| {
                        (
                            entry.part_name.clone(),
                            entry.bytes.clone(),
                            entry.extension.clone(),
                            entry.content_type.clone(),
                            entry.newly_inserted,
                        )
                    })
                    .collect::<Vec<_>>();
                (id.0, bucket)
            })
            .collect::<Vec<_>>();
        media.sort();
        let mut invalidated = presentation
            .embedded_invalidated_signatures
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        invalidated.sort();
        (
            (
                presentation.presentation_part.clone(),
                xml(presentation.presentation.to_xml()),
                presentation
                    .core_properties
                    .as_ref()
                    .map(|properties| xml(properties.to_xml())),
                presentation.core_properties_part_name.clone(),
                presentation.core_properties_dirty,
                presentation
                    .application_properties
                    .as_ref()
                    .map(|properties| xml(properties.to_xml())),
                presentation
                    .custom_properties
                    .as_ref()
                    .map(|properties| xml(properties.to_xml())),
            ),
            (
                presentation.comment_authors_part.clone(),
                xml(presentation.comment_authors.to_xml()),
                presentation.comment_authors_dirty,
                presentation.notes_master_part.clone(),
                presentation
                    .notes_master
                    .as_ref()
                    .map(|master| xml(master.to_xml())),
                presentation.notes_master_dirty,
                presentation.handout_master_part.clone(),
                presentation
                    .handout_master
                    .as_ref()
                    .map(|master| xml(master.to_xml())),
                presentation.handout_master_dirty,
            ),
            (
                invalidated,
                presentation.package_signatures_invalidated,
                presentation.themes.len(),
            ),
            (slides, layouts, masters, media),
            presentation.parsed.is_some(),
        )
    }
    fn same_package(committed: &OpcPackage, through_zip: &OpcPackage, what: &str) {
        assert!(
            committed.same_package_as(through_zip),
            "a commit gives another {what} than its ZIP round trip: {committed:?} where {through_zip:?}"
        );
    }
    match (committed, through_zip) {
        (Ok(committed), Ok(through_zip)) => {
            same_package(&committed.package, &through_zip.package, "package");
            assert_eq!(
                state(committed),
                state(through_zip),
                "a commit differs from its ZIP round trip"
            );
            match (
                committed.staged_package(true),
                through_zip.staged_package(true),
            ) {
                (Ok(committed), Ok(through_zip)) => {
                    same_package(&committed, &through_zip, "saved package")
                }
                (committed, through_zip) => assert_eq!(
                    committed.err().map(|error| error.to_string()),
                    through_zip.err().map(|error| error.to_string()),
                    "a commit saves otherwise than its ZIP round trip"
                ),
            }
        }
        (Err(committed), Err(through_zip)) => assert_eq!(
            committed.to_string(),
            through_zip.to_string(),
            "a commit fails otherwise than its ZIP round trip"
        ),
        (committed, through_zip) => panic!(
            "a commit gives {:?} where its ZIP round trip gives {:?}",
            committed.as_ref().map(|_| ()),
            through_zip.as_ref().map(|_| ())
        ),
    }
}
