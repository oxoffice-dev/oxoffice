//! Opening and saving: bytes, files, packages, encryption and signatures.

use super::*;

impl Document {
    /// Open a document from a file path.
    ///
    /// The document remembers the file, and [`Document::save`] refuses to
    /// replace it unless asked, see [`Document::save_with_options`].
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let package = OpcPackage::open(path.as_ref())?;
        let mut document = Self::from_opened_package(package, true, None)?;
        document.remember_source(path.as_ref());
        Ok(document)
    }

    /// Open a password-protected agile-encrypted document from a file path.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn open_encrypted<P: AsRef<Path>>(path: P, password: &str) -> Result<Self> {
        let file = std::fs::File::open(path.as_ref())?;
        let package = OpcPackage::from_encrypted_reader(file, password)?;
        let mut document = Self::from_package(package)?;
        document.remember_source(path.as_ref());
        Ok(document)
    }

    /// Record `path` as the file this document was opened from. A path that
    /// cannot be resolved records nothing.
    pub(crate) fn remember_source(&mut self, path: &Path) {
        self.source_path = std::fs::canonicalize(path).ok();
    }

    /// Open a document from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_bytes_with_limits(bytes, PackageReadLimits::UNBOUNDED)
    }

    /// Open a password-protected agile-encrypted document from bytes.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn from_encrypted_bytes(bytes: &[u8], password: &str) -> Result<Self> {
        Self::from_encrypted_bytes_with_limits(bytes, password, PackageReadLimits::UNBOUNDED)
    }

    /// Open an agile-encrypted document while bounding OPC archive expansion.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn from_encrypted_bytes_with_limits(
        bytes: &[u8],
        password: &str,
        limits: PackageReadLimits,
    ) -> Result<Self> {
        let cursor = std::io::Cursor::new(bytes);
        let package = OpcPackage::from_encrypted_reader_with_limits(cursor, password, limits)?;
        Self::from_opened_package(package, true, None)
    }

    /// Open a document from bytes while bounding OPC archive expansion.
    pub fn from_bytes_with_limits(bytes: &[u8], limits: PackageReadLimits) -> Result<Self> {
        let cursor = std::io::Cursor::new(bytes);
        let package = OpcPackage::from_reader_with_limits(cursor, limits)?;
        Self::from_opened_package(package, true, None)
    }

    /// Return the exact Word package class declared for the main document part.
    pub fn package_class(&self) -> Result<WordPackageClass> {
        validated_word_package_class(&self.package).map(|(_, class)| class)
    }

    /// Serialize a staged copy as the requested Word package class.
    ///
    /// This changes only the main part's content-type override. Macro projects
    /// and all other package payloads remain present.
    pub fn to_bytes_as(&self, class: WordPackageClass) -> Result<Vec<u8>> {
        self.validate_model_resource_limits()?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_output()?;
        candidate
            .package
            .content_types
            .add_override(&candidate.doc_part_name, class.content_type());
        candidate.package_signatures_invalidated |=
            crate::embedded::synchronized_package_mutation_invalidates_signature(
                &self.package,
                &candidate.package,
            );
        crate::embedded::persist_invalidated_package_signature(
            &mut candidate.package,
            candidate.package_signatures_invalidated,
        )?;
        let mut output = std::io::Cursor::new(Vec::new());
        candidate.package.write_to(&mut output)?;
        let bytes = output.into_inner();
        Self::from_bytes(&bytes)?;
        Ok(bytes)
    }

    /// Save a staged copy as the requested Word package class.
    pub fn save_as_package_class<P: AsRef<Path>>(
        &self,
        path: P,
        class: WordPackageClass,
    ) -> Result<()> {
        let bytes = self.to_bytes_as(class)?;
        write_atomic_file(
            path.as_ref(),
            &bytes,
            "oxoffice",
            "invalid Word package file name",
            "could not allocate Word package save staging file",
        )?;
        Ok(())
    }

    /// Verify package signatures without asserting certificate-chain trust.
    #[cfg(feature = "digital-signatures")]
    pub fn verify_signatures(
        &self,
    ) -> std::result::Result<Vec<oxoffice_opc::SignatureReport>, oxoffice_opc::OpcError> {
        self.package.verify_signatures()
    }

    /// Sign the current typed document with the strict RSA-SHA256 profile.
    ///
    /// The private key must be PKCS#8 DER or one unencrypted PRIVATE KEY PEM
    /// block. The certificate must be X.509 DER or one CERTIFICATE PEM block. Typed state is flushed and signed on a staged clone, so any
    /// serialization, key, certificate, or signature failure leaves this
    /// document unchanged. Certificate-chain trust remains caller policy.
    #[cfg(all(feature = "digital-signatures", not(target_arch = "wasm32")))]
    pub fn sign(
        &mut self,
        private_key_pkcs8_der: &[u8],
        certificate_der: &[u8],
    ) -> Result<oxoffice_opc::SignatureReport> {
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_output()?;
        let report = candidate
            .package
            .sign(private_key_pkcs8_der, certificate_der)?;
        self.commit_staged_mutation(candidate);
        Ok(report)
    }

    pub(crate) fn from_package(package: OpcPackage) -> Result<Self> {
        Self::from_opened_package(package, false, None)
    }

    pub(crate) fn from_opened_package(
        package: OpcPackage,
        read_from_bytes: bool,
        scoped_source: Option<ScopedDocumentRead<'_, '_, '_>>,
    ) -> Result<Self> {
        Self::from_package_reusing(
            package,
            read_from_bytes,
            None,
            None,
            None,
            None,
            scoped_source,
        )
    }

    pub(super) fn from_package_reusing(
        package: OpcPackage,
        read_from_bytes: bool,
        staged: Option<(&DocumentIdentifiers, &Parsed)>,
        blocks: Option<&MainPartBlocks>,
        writer_blocks: Option<&MainPartBlocks>,
        styles_read: Option<&StylesPartCache>,
        mut scoped_source: Option<ScopedDocumentRead<'_, '_, '_>>,
    ) -> Result<Self> {
        let (doc_part_name, _) = validated_word_package_class(&package)?;

        let doc_xml = package
            .get_part(&doc_part_name)
            .ok_or(Error::NoDocumentPart)?;
        let namespace_scopes = document_namespace_scopes(doc_xml)?;
        let kept = staged.map(|(_, parsed)| parsed).filter(|parsed| {
            parsed.main_part == doc_part_name
                && parsed.main_part_xml().is_some_and(|kept| kept == doc_xml)
                && parsed.root_namespace_declarations == namespace_scopes.root_declarations
                && parsed.body_namespace_declarations == namespace_scopes.body_declarations
                && parsed.body_namespace_bindings == namespace_scopes.body_bindings
        });
        // A pre-operation child cache has no operation-bound reader facts.
        // Use the existing authoritative factory below rather than reparsing
        // solely to certify the cached model. Qualified kept-model reuse stays.
        let hinted = scoped_source
            .is_none()
            .then(|| blocks.and_then(|blocks| blocks.proved_document(doc_xml)))
            .flatten();
        let nested_namespace_owners = OnceLock::new();
        if let Some(kept) = kept
            && let Some(owners) = kept.nested_namespace_owners.get()
        {
            let _ = nested_namespace_owners.set((*owners).clone());
        }
        if nested_namespace_owners.get().is_none()
            && let Some(owners) = blocks.and_then(|blocks| blocks.retained_nested_owners(doc_xml))
        {
            let _ = nested_namespace_owners.set(Some(owners));
        }
        let (plain_local_edit_blocked, unsavable_body_edit_cache) = plain_local_edit_facts(
            &namespace_scopes.root_declarations,
            &namespace_scopes.body_declarations,
            doc_xml,
            &nested_namespace_owners,
        );
        if let Some(input) = scoped_source.as_ref()
            && input.source.as_bytes() != doc_xml
        {
            // Check the known producer binding, never discover provenance or
            // grant input roles merely because two buffers have equal bytes.
            return Err(Error::Other(
                "scoped document source does not match its produced main part".into(),
            ));
        }
        let document = if let Some(document) = hinted {
            document
        } else {
            match kept.and_then(|kept| kept.main_document().cloned()) {
                Some(document) => document,
                None => match scoped_source.as_mut() {
                    Some(input) => input.operation.source_document(input.source)?,
                    None => CT_Document::from_xml(doc_xml)?,
                },
            }
        };

        // Resolve the part a relationship of the given type points at.
        let resolve_part = |rel_type: &str| -> Option<String> {
            let rels = package.get_part_rels(&doc_part_name)?;
            let rel = rels.items.iter().find(|relationship| {
                relationship.rel_type == rel_type && relationship_is_internal(relationship)
            })?;
            Some(OpcPackage::resolve_rel_target(&doc_part_name, &rel.target))
        };

        // Try to load styles, remembering where they came from.
        let styles_part_name = resolve_part(rel_types::STYLES);
        let (styles, styles_part_cache) = match styles_part_name
            .as_deref()
            .and_then(|part| package.get_part_shared(part))
        {
            Some(bytes) => {
                if let Some(kept) = styles_read.filter(|kept| Arc::ptr_eq(bytes, &kept.bytes)) {
                    (Arc::clone(&kept.parsed), Some(kept.clone()))
                } else {
                    let parsed = Arc::new(CT_Styles::from_xml(bytes)?);
                    let proof = if parsed.to_xml().is_ok() {
                        StylesSerializationProof::Serializable
                    } else {
                        StylesSerializationProof::Unserializable
                    };
                    let cache = StylesPartCache {
                        bytes: Arc::clone(bytes),
                        parsed: Arc::clone(&parsed),
                        emitted_model: None,
                        proof,
                    };
                    (parsed, Some(cache))
                }
            }
            None => (Arc::new(CT_Styles::new_default()), None),
        };

        // Try to load numbering definitions
        let numbering_part_name = resolve_part(rel_types::NUMBERING);
        let numbering = match numbering_part_name
            .as_deref()
            .and_then(|p| package.get_part(p))
        {
            Some(num_xml) => Some(CT_Numbering::from_xml(num_xml)?),
            None => None,
        };

        let settings_part_name = resolve_part(rel_types::SETTINGS);
        let settings = match settings_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => Some(CT_Settings::from_xml(xml)?),
            None => None,
        };

        let web_settings_part_name = resolve_part(rel_types::WEB_SETTINGS);
        let web_settings = match web_settings_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => Some(CT_WebSettings::from_xml(xml)?),
            None => None,
        };

        let theme_part_name = resolve_part(rel_types::THEME);
        let theme = match theme_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => oxoffice_drawing::theme::CT_OfficeStyleSheet::from_xml(xml).ok(),
            None => None,
        };

        let font_table_part_name = resolve_part(rel_types::FONT_TABLE);
        let font_table = match font_table_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => FontTable::from_xml(xml).ok(),
            None => None,
        };

        // Core properties are a package-level relationship, not a document part.
        let core_properties_part_name = package
            .package_rels
            .items
            .iter()
            .find(|relationship| {
                relationship.rel_type == CORE_PROPERTIES_REL_TYPE
                    && relationship_is_internal(relationship)
            })
            .map(|rel| OpcPackage::resolve_rel_target("/", &rel.target));
        let core_properties = core_properties_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
            .and_then(|xml| CoreProperties::from_xml(xml).ok())
            .map(Box::new);

        let application_properties_part_name = package
            .package_rels
            .items
            .iter()
            .find(|relationship| {
                relationship.rel_type == rel_types::EXTENDED_PROPERTIES
                    && relationship_is_internal(relationship)
            })
            .map(|rel| OpcPackage::resolve_rel_target("/", &rel.target));
        let application_properties = application_properties_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
            .and_then(|xml| AppProperties::from_xml(xml).ok())
            .map(repair_application_version);

        let custom_properties_part_name = package
            .package_rels
            .items
            .iter()
            .find(|relationship| {
                relationship.rel_type == rel_types::CUSTOM_PROPERTIES
                    && relationship_is_internal(relationship)
            })
            .map(|rel| OpcPackage::resolve_rel_target("/", &rel.target));
        let custom_properties = custom_properties_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
            .and_then(|xml| CustomProperties::from_xml(xml).ok());

        let footnotes_part_name = resolve_part(rel_types::FOOTNOTES);
        let footnotes_root_namespace_declarations = footnotes_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
            .map(root_namespace_declarations)
            .transpose()?
            .unwrap_or_default();
        let footnotes = match footnotes_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => {
                let paired = footnotes_part_name.as_ref().and_then(|part| {
                    scoped_source
                        .as_ref()
                        .and_then(|input| input.related.and_then(|related| related.get(part)))
                });
                if let (Some(input), Some(source)) = (scoped_source.as_mut(), paired.cloned()) {
                    match input.operation.source_notes(&source) {
                        Ok(notes) => notes,
                        Err(_) => {
                            input.operation.reserve_template_text_copy(0)?;
                            Default::default()
                        }
                    }
                } else {
                    oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml).unwrap_or_default()
                }
            }
            None => Default::default(),
        };

        let comments_part_name = resolve_part(rel_types::COMMENTS);
        let comments = match comments_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => {
                let paired = comments_part_name
                    .as_ref()
                    .and_then(|part| {
                        scoped_source
                            .as_ref()
                            .and_then(|input| input.related.and_then(|related| related.get(part)))
                    })
                    .cloned();
                Some(
                    if let (Some(input), Some(source)) = (scoped_source.as_mut(), paired) {
                        input.operation.source_comments(&source)?
                    } else {
                        oxoffice_docx_oxml::comments::CT_Comments::from_xml(xml)?
                    },
                )
            }
            None => None,
        };
        let comments_extended_part_name = resolve_part(crate::comments::COMMENTS_EXTENDED_REL_TYPE);
        let comments_extended = match comments_extended_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => Some(oxoffice_docx_oxml::comments_extended::CT_CommentsEx::from_xml(xml)?),
            None => None,
        };
        let glossary = crate::building_block::load_glossary(&package, &doc_part_name)?;
        let (glossary_part_name, glossary) = match glossary {
            Some((part_name, glossary)) => (Some(part_name), Some(glossary)),
            None => (None, None),
        };

        let package_signatures_invalidated =
            crate::embedded::known_invalid_package_signature_on_open(&package);
        let identifier_hint =
            blocks.and_then(|blocks| blocks.identifier_hint(doc_xml, &namespace_scopes));
        let identifiers = DocumentIdentifiers::scan_reusing(
            &package,
            staged.map(|(identifiers, _)| identifiers),
            identifier_hint.as_ref(),
        )?;
        let serialized = document.validate_resource_limits().ok().and_then(|()| {
            MainPartBlocks::serialize(
                &document,
                &document.body.content,
                // A producer source record proves reads, while the retained
                // writer record independently proves normalized child bytes.
                // serialize rechecks model alignment, fields and the frame.
                offered_canonical_blocks(writer_blocks.or(blocks)),
            )
            .ok()
        });
        let (serialization_proof, serialized_blocks) = match serialized {
            Some((xml, record)) => {
                let retained = block_proofs_enabled() && !record.head.is_empty();
                (Some(xml), retained.then_some(record))
            }
            None => (None, None),
        };
        let mut parsed_record = Parsed::of(
            &package,
            &doc_part_name,
            read_from_bytes,
            namespace_scopes.root_declarations.clone(),
            namespace_scopes.body_declarations.clone(),
            namespace_scopes.body_bindings.clone(),
            serialization_proof,
        );
        parsed_record.nested_namespace_owners = nested_namespace_owners;
        if let Some(kept) = kept {
            let relationships = package
                .get_part_rels(&doc_part_name)
                .map(|relationships| &relationships.items);
            let kept_relationships = kept
                .package
                .get_part_rels(&kept.main_part)
                .map(|relationships| &relationships.items);
            if relationships == kept_relationships
                && let Some(unchanged) = kept.unchanged_main_part.get()
            {
                parsed_record.unchanged_main_part = OnceLock::from((*unchanged).clone());
            }
        }
        // Every branch above obtained an actual parse of these exact source
        // bytes, either fresh or from a qualified source proof. Keep it at
        // this opening instead of parsing a fresh source a second time later.
        parsed_record.initial_writer_blocks = serialized_blocks
            .as_ref()
            .map(|record| Arc::downgrade(&record.blocks));
        let (document, opening_document) = DocumentModel::opening_pair(document);
        parsed_record.main_document = OnceLock::from(Some(opening_document));
        let parsed = Some(Arc::new(parsed_record));
        let local_cell_source = LocalCellSource::classified(
            &package,
            &doc_part_name,
            &namespace_scopes.root_declarations,
            &namespace_scopes.body_declarations,
            !plain_local_edit_blocked,
        );
        Ok(Document {
            package,
            document,
            root_namespace_declarations: namespace_scopes.root_declarations,
            body_namespace_declarations: namespace_scopes.body_declarations,
            body_namespace_bindings: namespace_scopes.body_bindings,
            plain_local_edit_blocked,
            local_cell_source,
            unsavable_body_edit_cache,
            styles,
            styles_part_cache,
            numbering,
            published_abstract_by_live: HashMap::new(),
            published_instance_by_live: HashMap::new(),
            core_properties,
            application_properties,
            custom_properties,
            core_properties_part_name,
            application_properties_part_name,
            custom_properties_part_name,
            custom_properties_owned: false,
            doc_part_name,
            styles_part_name,
            numbering_part_name,
            settings,
            settings_part_name,
            settings_owned: false,
            web_settings,
            web_settings_part_name,
            web_settings_owned: false,
            theme,
            theme_part_name,
            theme_dirty: false,
            font_table,
            font_table_part_name,
            font_table_dirty: false,
            identifiers,
            footnotes,
            footnotes_part_name,
            footnotes_root_namespace_declarations,
            footnotes_dirty: false,
            comments,
            comments_part_name,
            comments_dirty: false,
            comments_extended,
            comments_extended_part_name,
            comments_owned: false,
            comments_extended_owned: false,
            embedded_invalidated_signatures: HashSet::new(),
            package_signatures_invalidated,
            source_path: None,
            glossary,
            glossary_part_name,
            glossary_dirty: false,
            layout_cache: Mutex::new(None),
            normal_layout_engine: Mutex::new(None),
            deterministic_layout_cache: Mutex::new(None),
            bundled_fallback_layout_engine: Mutex::new(None),
            parsed,
            main_body_scans: Arc::new(Mutex::new(MainBodyScans {
                serialized: serialized_blocks,
                ..MainBodyScans::default()
            })),
            main_part_blocks: None,
        })
    }

    /// Save the document to a file path.
    ///
    /// The package is staged in a synced sibling file and renamed over `path`
    /// through [`oxoffice_opc::write_atomic_file`], so a failed save leaves an
    /// existing file as it was. A symbolic link at `path` is kept and the file
    /// it names is replaced, and on Unix that file keeps its permission bits.
    ///
    /// A `.docx`, `.docm`, `.dotx`, or `.dotm` extension selects the main
    /// part content type, so a template saved as `.docx` declares a document.
    /// Any other extension keeps the opened class, and `save_encrypted` and
    /// the Flat OPC saves ignore the extension. When the class changes, a
    /// main part that carries a VBA project cannot be saved under a
    /// macro-free extension, because the project would remain in a file that
    /// claims to carry none. [`Document::save_as_package_class`] performs
    /// that conversion explicitly and keeps the VBA part.
    ///
    /// A document opened from a file refuses to be saved over that file,
    /// whatever the spelling of `path` (a symbolic link, a relative path, a
    /// hard link), with [`Error::OverwritesSource`]. This differs from
    /// python-docx. [`Document::save_with_options`] with
    /// [`SaveOptions::overwrite_source`] replaces it.
    pub fn save<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        self.save_with_options(path, SaveOptions::default())
    }

    /// Save the document to a file path, as [`Document::save`] does, with
    /// `options`.
    pub fn save_with_options<P: AsRef<Path>>(
        &mut self,
        path: P,
        options: SaveOptions,
    ) -> Result<()> {
        let path = path.as_ref();
        if !options.overwrite_source
            && let Some(source) = &self.source_path
            && oxoffice_opc::is_same_file(source, path)
        {
            return Err(Error::OverwritesSource {
                path: path.to_path_buf(),
            });
        }
        self.validate_model_resource_limits()?;
        if let Some(class) = self.package_class_for_path(path)? {
            return self.save_as_package_class(path, class);
        }
        let (mut candidate, normalized) = self.staged_current_output()?;
        if normalized {
            candidate = candidate.reopen_prepared_staged()?;
        }
        candidate.package.save(path)?;
        if normalized {
            *self = candidate;
        }
        Ok(())
    }

    /// Whether this document and `other` save the same bytes.
    ///
    /// This save is prepared first, then `other`. The first error is returned.
    /// Packages opened from bytes are compared by [`OpcPackage::reread`].
    /// A new, in-memory, or Flat OPC package is written and compared as a ZIP.
    #[doc(hidden)]
    pub fn saves_as(&self, other: &Document) -> Result<bool> {
        let same: Result<bool> = (|| match (self.read_package(), other.read_package()) {
            (Some(read), Some(other_read)) => Ok(self
                .saved_package()?
                .reread(read)?
                .same_package_as(&other.saved_package()?.reread(other_read)?)),
            _ => {
                let mut bytes = std::io::Cursor::new(Vec::new());
                self.saved_package()?.write_to(&mut bytes)?;
                let mut other_bytes = std::io::Cursor::new(Vec::new());
                other.saved_package()?.write_to(&mut other_bytes)?;
                Ok(bytes.into_inner() == other_bytes.into_inner())
            }
        })();
        #[cfg(debug_assertions)]
        {
            let forgetting = |document: &Document| -> Result<Document> {
                document.validate_model_resource_limits()?;
                Ok(document.clone_for_debug_reference())
            };
            let bytes = (|| -> Result<bool> {
                Ok(forgetting(self)?.to_bytes()? == forgetting(other)?.to_bytes()?)
            })();
            match (&same, bytes) {
                (Ok(same), Ok(bytes)) => assert_eq!(*same, bytes, "saves_as differs from to_bytes"),
                (Err(same), Err(bytes)) => assert_eq!(same.to_string(), bytes.to_string()),
                (same, bytes) => panic!("saves_as gives {same:?} where to_bytes gives {bytes:?}"),
            }
        }
        same
    }

    pub(super) fn read_package(&self) -> Option<&OpcPackage> {
        self.parsed
            .as_deref()
            .filter(|parsed| parsed.read_from_bytes)
            .map(|parsed| &parsed.package)
    }

    pub(super) fn prepare_current_field_models<'a>(
        &mut self,
        mut template: Option<(
            &mut oxoffice_docx_oxml::document::ScopedWordOperation<'a>,
            &mut crate::template::TemplateWordLineage<'a>,
            Option<&oxoffice_docx_oxml::document::ScopedWordSource>,
            Option<&oxoffice_docx_oxml::document::ScopedWordSource>,
        )>,
    ) -> Result<bool> {
        let mut paragraph_index = 0usize;
        let mut changed = false;
        let mut error = None;
        let mut normalize = |paragraph: &mut CT_P, main_body: bool, comment_body: bool| {
            let current_paragraph = paragraph_index;
            paragraph_index += 1;
            if error.is_some() {
                return;
            }
            let result = (|| -> Result<()> {
                for path in paragraph.accepted_run_paths() {
                    let run = paragraph.accepted_run(&path).ok_or_else(|| {
                        Error::Other("current Field publication path is stale".into())
                    })?;
                    let mut fields = Vec::new();
                    for (slot, content) in run.content.iter().enumerate() {
                        if let oxoffice_docx_oxml::text::RunContent::Field(field) = content {
                            let current =
                                if let Some((operation, lineage, reader, comments_reader)) =
                                    template.as_mut()
                                {
                                    let bypass = crate::field::preflight_current_field(field)?;
                                    if let Some(source) = if main_body {
                                        reader.as_ref()
                                    } else if comment_body {
                                        comments_reader.as_ref()
                                    } else {
                                        None
                                    } {
                                        operation.prepare_template_field_from_read(
                                            field, source, !bypass,
                                        )?
                                    } else if let Some(frozen) =
                                        lineage.initial_field(current_paragraph, &path, slot)
                                    {
                                        operation.prepare_template_field(field, frozen, !bypass)?
                                    } else {
                                        crate::field::prepared_current_field(field)?
                                    }
                                } else {
                                    crate::field::prepared_current_field(field)?
                                };
                            if field != &current {
                                fields.push((slot, current));
                            }
                        }
                    }
                    if fields.is_empty() {
                        continue;
                    }
                    let installed_slots: Vec<_> = fields.iter().map(|(slot, _)| *slot).collect();
                    let installed = if let Some((operation, lineage, reader, comments_reader)) =
                        template.as_mut()
                    {
                        let frozen = if (main_body && reader.is_some())
                            || (comment_body && comments_reader.is_some())
                        {
                            None
                        } else {
                            lineage.initial_paragraph(current_paragraph)
                        };
                        operation
                            .install_template_prepared_fields(paragraph, &path, fields, frozen)?
                    } else {
                        let mut replacement = run.clone();
                        for (slot, field) in fields {
                            replacement.content[slot] =
                                oxoffice_docx_oxml::text::RunContent::Field(field);
                        }
                        paragraph.replace_accepted_run(&path, replacement)?
                    };
                    if !installed {
                        return Err(Error::Other(
                            "current Field publication owner disappeared".into(),
                        ));
                    }
                    if let Some((_, lineage, _, _)) = template.as_mut() {
                        for slot in installed_slots {
                            lineage.installed_field(current_paragraph, &path, slot);
                        }
                    }
                    changed = true;
                }
                Ok(())
            })();
            if let Err(failure) = result {
                error = Some(failure);
            }
        };
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            normalize(paragraph, true, false)
        });
        for note in &mut self.footnotes.footnotes {
            for paragraph in &mut note.paragraphs {
                normalize(paragraph, false, false);
            }
        }
        if let Some(comments) = &mut self.comments {
            for comment in &mut comments.comments {
                for paragraph in &mut comment.paragraphs {
                    normalize(paragraph, false, true);
                }
            }
        }
        if let Some(glossary) = &mut self.glossary {
            for part in &mut glossary.doc_parts {
                visit_body_paragraphs_mut(&mut part.body.content, &mut |paragraph| {
                    normalize(paragraph, false, false)
                });
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(changed)
    }

    fn staged_current_output(&self) -> Result<(Self, bool)> {
        self.validate_model_resource_limits()?;
        let mut candidate = self.clone_for_staging();
        let normalized = candidate.prepare_staged_output()?;
        crate::embedded::persist_invalidated_package_signature(
            &mut candidate.package,
            candidate.package_signatures_invalidated,
        )?;
        Ok((candidate, normalized))
    }

    fn saved_package(&self) -> Result<OpcPackage> {
        Ok(self.staged_current_output()?.0.package)
    }

    /// Save the document to a byte vector. Successful checked publication also
    /// installs normalized current owners for subsequent borrowed readers.
    pub fn to_bytes(&mut self) -> Result<Vec<u8>> {
        let (mut candidate, normalized) = self.staged_current_output()?;
        if normalized {
            candidate = candidate.reopen_prepared_staged()?;
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        candidate.package.write_to(&mut buf)?;
        let bytes = buf.into_inner();
        if normalized {
            *self = candidate;
        }
        Ok(bytes)
    }

    /// Serialize the bytes that [`Document::save`] writes to `path`.
    pub fn to_bytes_for_path<P: AsRef<Path>>(&mut self, path: P) -> Result<Vec<u8>> {
        match self.package_class_for_path(path.as_ref())? {
            Some(class) => self.to_bytes_as(class),
            None => self.to_bytes(),
        }
    }

    /// The class a save to `path` converts to, or `None` to keep the opened class.
    fn package_class_for_path(&self, path: &Path) -> Result<Option<WordPackageClass>> {
        let Some(target) = WordPackageClass::from_path(path) else {
            return Ok(None);
        };
        // Read the override itself: a new compact document has no main part
        // bytes until staging, so `package_class` cannot validate it yet.
        let current = self
            .package
            .content_types
            .override_for(&self.doc_part_name)
            .and_then(WordPackageClass::from_content_type);
        if current == Some(target) {
            return Ok(None);
        }
        let carries_vba =
            self.package
                .get_part_rels(&self.doc_part_name)
                .is_some_and(|relationships| {
                    relationships.get_by_type(rel_types::VBA_PROJECT).is_some()
                });
        if carries_vba && !target.is_macro_enabled() {
            return Err(Error::Other(format!(
                "cannot save {}: the document carries a VBA project and the extension names a macro-free class, save it as .docm or .dotm",
                path.display()
            )));
        }
        Ok(Some(target))
    }

    /// Save a password-protected document using the fixed Agile write profile.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn save_encrypted<P: AsRef<Path>>(&self, path: P, password: &str) -> Result<()> {
        let bytes = self.to_encrypted_bytes(password)?;
        write_atomic_file(
            path.as_ref(),
            &bytes,
            "oxoffice",
            "invalid file name",
            "could not allocate encrypted-save staging file",
        )?;
        Ok(())
    }

    /// Save a password-protected document to a byte vector.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn to_encrypted_bytes(&self, password: &str) -> Result<Vec<u8>> {
        self.validate_model_resource_limits()?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_output()?;
        crate::embedded::persist_invalidated_package_signature(
            &mut candidate.package,
            candidate.package_signatures_invalidated,
        )?;
        let mut encrypted = Vec::new();
        candidate
            .package
            .write_encrypted_to(&mut encrypted, password)?;
        Ok(encrypted)
    }
}

impl Document {
    /// The package a save would write now, staged on a copy.
    fn staged_output_package(&self) -> Result<OpcPackage> {
        self.validate_model_resource_limits()?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_output()?;
        crate::embedded::persist_invalidated_package_signature(
            &mut candidate.package,
            candidate.package_signatures_invalidated,
        )?;
        Ok(candidate.package)
    }

    /// Package part names [`Self::to_bytes`] would write now, each with a
    /// leading slash, in lexicographic order. They are its ZIP entries, so
    /// they include `/[Content_Types].xml`, `/_rels/.rels`, every
    /// relationship part such as `/word/_rels/document.xml.rels`, and parts
    /// with no relationship. When the package would not save, this returns
    /// the save error, such as a graph validation failure, a duplicate part
    /// identity or a character XML cannot carry.
    pub fn part_names(&self) -> Result<Vec<String>> {
        let package = self.staged_output_package()?;
        let mut names = package
            .zip_entries()?
            .into_iter()
            .map(|(entry, _)| format!("/{entry}"))
            .collect::<Vec<_>>();
        names.sort();
        Ok(names)
    }

    /// UTF-8 XML of one package part as [`Self::to_bytes`] would write it now.
    ///
    /// The part is the ZIP entry of [`Self::to_bytes`] with that name, a
    /// relationship part and `/[Content_Types].xml` included. The name may
    /// omit its leading slash and matches without regard to case. A missing,
    /// binary, or non-UTF-8 part returns an error that names the part and its
    /// content type. A UTF-8 BOM is stripped. UTF-16 is not decoded. When the
    /// package would not save, this returns the save error, as
    /// [`Self::part_names`] does.
    pub fn part_xml(&self, part_name: &str) -> Result<String> {
        let requested = part_name.trim();
        let requested = requested.strip_prefix('/').unwrap_or(requested);
        let package = self.staged_output_package()?;
        let entries = package.zip_entries()?;
        let Some((entry, data)) = entries
            .iter()
            .find(|(entry, _)| entry.eq_ignore_ascii_case(requested))
        else {
            let requested = format!("/{requested}");
            let content_type = package
                .content_types
                .content_type_for(&requested)
                .unwrap_or("none");
            return Err(Error::Other(format!(
                "part {requested} is missing (content type {content_type})"
            )));
        };
        let stored = format!("/{entry}");
        let content_type = package
            .content_types
            .content_type_for(&stored)
            .unwrap_or("none");
        // A relationship part and [Content_Types].xml are XML whatever the
        // content types declare. Every other entry is a stored part.
        let xml_type =
            !package.contains_part(&stored) || content_type.to_ascii_lowercase().contains("xml");
        if !xml_type {
            return Err(Error::Other(format!(
                "part {stored} has content type {content_type} and is not UTF-8 XML"
            )));
        }
        let body = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data.as_ref());
        // UTF-16 text without a BOM is ASCII bytes interleaved with NUL.
        // Those bytes are valid UTF-8, and XML 1.0 forbids NUL, so a NUL is
        // the non-UTF-8 refusal for an XML content type.
        let Ok(xml) = String::from_utf8(body.to_vec()) else {
            return Err(Error::Other(format!(
                "part {stored} has content type {content_type} and is not UTF-8 XML"
            )));
        };
        if xml.contains('\0') {
            return Err(Error::Other(format!(
                "part {stored} has content type {content_type} and is not UTF-8 XML"
            )));
        }
        Ok(xml)
    }
}
