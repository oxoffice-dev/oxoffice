//! Opening a presentation: new, files, bytes and encrypted packages, and resolving its masters, layouts, notes, handouts and comments.

use super::*;

const DEFAULT_CORE_PROPERTIES_PART: &str = "/docProps/core.xml";

impl Presentation {
    /// Creates an empty 16:9 presentation from the bundled standard template.
    #[cfg(feature = "default-template")]
    pub fn new() -> Result<Self> {
        Self::from_bytes(DEFAULT_TEMPLATE)
    }

    /// Opens a presentation from a `.pptx` path.
    ///
    /// The presentation remembers the file, and [`Presentation::save`]
    /// refuses to replace it unless asked, see
    /// [`Presentation::save_with_options`].
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut presentation = Self::from_read_package(OpcPackage::open(path.as_ref())?)?;
        presentation.source_path = std::fs::canonicalize(path).ok();
        Ok(presentation)
    }

    /// Opens a password-protected Agile-encrypted presentation from a path.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn open_encrypted<P: AsRef<Path>>(path: P, password: &str) -> Result<Self> {
        let file = std::fs::File::open(path.as_ref()).map_err(OpcError::from)?;
        let mut presentation =
            Self::from_read_package(OpcPackage::from_encrypted_reader(file, password)?)?;
        presentation.source_path = std::fs::canonicalize(path).ok();
        Ok(presentation)
    }

    /// Opens a presentation from in-memory `.pptx` bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_read_package(OpcPackage::from_reader(Cursor::new(bytes))?)
    }

    /// Opens a password-protected Agile-encrypted presentation from bytes.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn from_encrypted_bytes(bytes: &[u8], password: &str) -> Result<Self> {
        Self::from_encrypted_bytes_with_limits(bytes, password, PackageReadLimits::UNBOUNDED)
    }

    /// Opens an Agile-encrypted presentation with bounded archive expansion.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn from_encrypted_bytes_with_limits(
        bytes: &[u8],
        password: &str,
        limits: PackageReadLimits,
    ) -> Result<Self> {
        Self::from_read_package(OpcPackage::from_encrypted_reader_with_limits(
            Cursor::new(bytes),
            password,
            limits,
        )?)
    }

    /// Opens a package read from bytes, keeping what it parsed for commits.
    pub(super) fn from_read_package(package: OpcPackage) -> Result<Self> {
        let mut presentation = Self::from_package(package)?;
        presentation.parsed = Some(Arc::new(Parsed::of(&presentation, None)));
        Ok(presentation)
    }

    pub(super) fn from_package(package: OpcPackage) -> Result<Self> {
        Self::from_package_reusing(package, None)
    }

    /// Opens `package`, taking from `earlier` each record still parsed from
    /// the bytes the package holds for its part, and the hash of each medium
    /// whose bytes its media store holds, instead of parsing and hashing
    /// them again.
    pub(super) fn from_package_reusing(
        package: OpcPackage,
        earlier: Option<(&Parsed, &MediaStore)>,
    ) -> Result<Self> {
        let parsed = earlier.map(|(parsed, _)| parsed);
        let package_signatures_invalidated =
            embedded::known_invalid_package_signature_on_open(&package);
        let media_store = match earlier {
            Some((_, media)) => media.rescan(&package),
            None => MediaStore::scan(&package),
        };
        let main_relationship = package
            .package_rels
            .get_by_type(rel_types::DOCUMENT)
            .ok_or(Error::MissingMainDocument)?;
        reject_external("/", main_relationship)?;
        let presentation_part = OpcPackage::resolve_rel_target("/", &main_relationship.target);
        let main_content_type = package
            .content_types
            .content_type_for(&presentation_part)
            .map(str::to_owned);
        PresentationPackageClass::from_content_type(main_content_type.as_deref().unwrap_or(""))
            .ok_or(Error::UnsupportedPackageClass {
                content_type: main_content_type,
            })?;
        let presentation =
            match parsed.and_then(|parsed| parsed.presentation(&package, &presentation_part)) {
                Some(presentation) => presentation.clone(),
                None => CT_Presentation::from_xml(required_part(&package, &presentation_part)?)
                    .map_err(|error| Error::MalformedPart {
                        part_name: presentation_part.clone(),
                        message: error.to_string(),
                    })?,
            };
        let core_properties_relationship =
            package.package_rels.get_by_type(rel_types::CORE_PROPERTIES);
        if let Some(relationship) = core_properties_relationship {
            reject_external("/", relationship)?;
        }
        let core_properties_part_name = core_properties_relationship
            .map(|relationship| OpcPackage::resolve_rel_target("/", &relationship.target));
        let core_properties = if let Some(part_name) = &core_properties_part_name {
            let xml = required_part(&package, part_name)?;
            Some(
                CoreProperties::from_xml(xml).map_err(|error| Error::MalformedPart {
                    part_name: part_name.clone(),
                    message: error.to_string(),
                })?,
            )
        } else {
            None
        };
        let application_properties = property_part(
            &package,
            rel_types::EXTENDED_PROPERTIES,
            content_types::EXTENDED_PROPERTIES,
        )?
        .map(|name| {
            AppProperties::from_xml(required_part(&package, &name)?).map_err(|error| {
                invalid_shape_mutation("application properties", error.to_string())
            })
        })
        .transpose()?;
        let custom_properties = property_part(
            &package,
            rel_types::CUSTOM_PROPERTIES,
            content_types::CUSTOM_PROPERTIES,
        )?
        .map(|name| {
            CustomProperties::from_xml(required_part(&package, &name)?)
                .map_err(|error| invalid_shape_mutation("custom properties", error.to_string()))
        })
        .transpose()?;
        let layouts = resolve_layouts(&package, &presentation_part, &presentation, parsed)?;
        let masters = resolve_master_records(&package, &presentation_part, &presentation, parsed)?;
        let themes = Vec::new();
        let (comment_authors_part, comment_authors) =
            resolve_comment_authors(&package, &presentation_part)?;
        let (notes_master_part, notes_master) =
            resolve_notes_master(&package, &presentation_part, parsed)?;
        let (handout_master_part, handout_master) =
            resolve_handout_master(&package, &presentation_part, parsed)?;

        let presentation_relationships = package.get_part_rels(&presentation_part);
        let mut slides = Vec::with_capacity(presentation.slide_ids.len());
        for slide_id in &presentation.slide_ids {
            let relationship = presentation_relationships
                .and_then(|relationships| relationships.get_by_id(&slide_id.relationship_id))
                .ok_or_else(|| Error::MissingRelationship {
                    source_part: presentation_part.clone(),
                    relationship_id: slide_id.relationship_id.clone(),
                })?;
            require_relationship_type(&presentation_part, relationship, rel_types::SLIDE)?;
            reject_external(&presentation_part, relationship)?;
            let slide_part =
                OpcPackage::resolve_rel_target(&presentation_part, &relationship.target);
            if let Some(record) =
                parsed.and_then(|parsed| parsed.slide(&package, &slide_part, slide_id.id))
            {
                slides.push(record);
                continue;
            }
            let slide_xml = required_part(&package, &slide_part)?;
            let slide = CT_Slide::from_xml(slide_xml).map_err(|error| Error::MalformedPart {
                part_name: slide_part.clone(),
                message: error.to_string(),
            })?;
            let notes = resolve_notes(&package, &slide_part)?;
            let comments = resolve_comments(&package, &slide_part, &slide)?;
            slides.push(Arc::new(SlideRecord {
                id: slide_id.id,
                part_name: slide_part,
                slide,
                notes,
                comments,
            }));
        }
        validate_collaboration_model(&comment_authors, &slides).map_err(|message| {
            Error::MalformedPart {
                part_name: presentation_part.clone(),
                message,
            }
        })?;

        let signature_baseline = embedded::signature_baseline(&package);
        Ok(Self {
            package,
            media_store,
            presentation_part,
            presentation,
            core_properties,
            core_properties_part_name: core_properties_part_name
                .unwrap_or_else(|| DEFAULT_CORE_PROPERTIES_PART.to_owned()),
            core_properties_dirty: false,
            application_properties,
            custom_properties,
            comment_authors_part,
            comment_authors,
            comment_authors_dirty: false,
            notes_master_part,
            notes_master,
            notes_master_dirty: false,
            handout_master_part,
            handout_master,
            handout_master_dirty: false,
            embedded_invalidated_signatures: HashSet::new(),
            package_signatures_invalidated,
            signature_baseline,
            source_path: None,
            layouts,
            masters,
            themes,
            slides,
            parsed: None,
        })
    }
}

fn resolve_master_records(
    package: &OpcPackage,
    presentation_part: &str,
    presentation: &CT_Presentation,
    parsed: Option<&Parsed>,
) -> Result<Vec<Arc<MasterRecord>>> {
    let mut masters = Vec::with_capacity(presentation.slide_master_ids.len());
    for entry in &presentation.slide_master_ids {
        let relationship = package
            .get_part_rels(presentation_part)
            .and_then(|rels| rels.get_by_id(&entry.relationship_id))
            .ok_or_else(|| Error::MissingRelationship {
                source_part: presentation_part.to_owned(),
                relationship_id: entry.relationship_id.clone(),
            })?;
        require_relationship_type(presentation_part, relationship, rel_types::SLIDE_MASTER)?;
        reject_external(presentation_part, relationship)?;
        let part_name = OpcPackage::resolve_rel_target(presentation_part, &relationship.target);
        if let Some(record) = parsed.and_then(|parsed| parsed.master(package, &part_name, entry.id))
        {
            masters.push(record);
            continue;
        }
        let master = CT_SlideMaster::from_xml(required_part(package, &part_name)?)
            .map_err(|error| error.to_string());
        masters.push(Arc::new(MasterRecord {
            id: entry.id,
            part_name,
            master,
            dirty: false,
        }));
    }
    Ok(masters)
}

fn resolve_layouts(
    package: &OpcPackage,
    presentation_part: &str,
    presentation: &CT_Presentation,
    parsed: Option<&Parsed>,
) -> Result<Vec<Arc<LayoutRecord>>> {
    let presentation_relationships = package.get_part_rels(presentation_part);
    let mut layouts = Vec::new();
    let mut seen = HashSet::new();
    for master in &presentation.slide_master_ids {
        let relationship = presentation_relationships
            .and_then(|relationships| relationships.get_by_id(&master.relationship_id))
            .ok_or_else(|| Error::MissingRelationship {
                source_part: presentation_part.to_owned(),
                relationship_id: master.relationship_id.clone(),
            })?;
        require_relationship_type(presentation_part, relationship, rel_types::SLIDE_MASTER)?;
        reject_external(presentation_part, relationship)?;
        let master_part = OpcPackage::resolve_rel_target(presentation_part, &relationship.target);
        required_part(package, &master_part)?;
        let Some(master_relationships) = package.get_part_rels(&master_part) else {
            continue;
        };
        for relationship in master_relationships
            .items
            .iter()
            .filter(|relationship| relationship.rel_type == rel_types::SLIDE_LAYOUT)
        {
            reject_external(&master_part, relationship)?;
            let part_name = OpcPackage::resolve_rel_target(&master_part, &relationship.target);
            if !seen.insert(part_name.clone()) {
                continue;
            }
            if let Some(record) = parsed.and_then(|parsed| parsed.layout(package, &part_name)) {
                layouts.push(record);
                continue;
            }
            let xml = required_part(package, &part_name)?;
            let layout = CT_SlideLayout::from_xml(xml).map_err(|error| Error::MalformedPart {
                part_name: part_name.clone(),
                message: error.to_string(),
            })?;
            layouts.push(Arc::new(LayoutRecord {
                part_name,
                layout,
                dirty: false,
            }));
        }
    }
    Ok(layouts)
}

fn resolve_comment_authors(
    package: &OpcPackage,
    presentation_part: &str,
) -> Result<(Option<String>, CommentAuthorList)> {
    let Some(relationships) = package.get_part_rels(presentation_part) else {
        return Ok((None, CommentAuthorList::new()));
    };
    let matches = relationships.get_all_by_type(rel_types::POWERPOINT_AUTHORS);
    if matches.len() > 1 {
        return Err(Error::DuplicateRelationships {
            source_part: presentation_part.to_owned(),
            relationship_type: rel_types::POWERPOINT_AUTHORS,
            count: matches.len(),
        });
    }
    let Some(relationship) = matches.first() else {
        return Ok((None, CommentAuthorList::new()));
    };
    reject_external(presentation_part, relationship)?;
    let part_name = OpcPackage::resolve_rel_target(presentation_part, &relationship.target);
    let authors =
        CommentAuthorList::from_xml(required_part(package, &part_name)?).map_err(|error| {
            Error::MalformedPart {
                part_name: part_name.clone(),
                message: error.to_string(),
            }
        })?;
    Ok((Some(part_name), authors))
}

fn resolve_comments(
    package: &OpcPackage,
    slide_part: &str,
    slide: &CT_Slide,
) -> Result<Option<CommentRecord>> {
    let reference =
        slide
            .modern_comment_relationship_id()
            .map_err(|error| Error::MalformedPart {
                part_name: slide_part.to_owned(),
                message: error.to_string(),
            })?;
    let relationships = package.get_part_rels(slide_part);
    let typed = relationships
        .map(|items| items.get_all_by_type(rel_types::POWERPOINT_COMMENTS))
        .unwrap_or_default();
    if typed.len() > 1 {
        return Err(Error::DuplicateRelationships {
            source_part: slide_part.to_owned(),
            relationship_type: rel_types::POWERPOINT_COMMENTS,
            count: typed.len(),
        });
    }
    let Some(reference) = reference else {
        if let Some(relationship) = typed.first() {
            reject_external(slide_part, relationship)?;
            return Err(Error::MalformedPart {
                part_name: slide_part.to_owned(),
                message: format!(
                    "modern comment relationship {} has no p188:commentRel reference",
                    relationship.id
                ),
            });
        }
        return Ok(None);
    };
    let relationship = relationships
        .and_then(|items| items.get_by_id(&reference))
        .ok_or_else(|| Error::MissingRelationship {
            source_part: slide_part.to_owned(),
            relationship_id: reference.clone(),
        })?;
    require_relationship_type(slide_part, relationship, rel_types::POWERPOINT_COMMENTS)?;
    reject_external(slide_part, relationship)?;
    let part_name = OpcPackage::resolve_rel_target(slide_part, &relationship.target);
    let comments = CommentList::from_xml(required_part(package, &part_name)?).map_err(|error| {
        Error::MalformedPart {
            part_name: part_name.clone(),
            message: error.to_string(),
        }
    })?;
    Ok(Some(CommentRecord {
        part_name,
        comments,
        dirty: false,
    }))
}

fn resolve_notes_master(
    package: &OpcPackage,
    presentation_part: &str,
    parsed: Option<&Parsed>,
) -> Result<(Option<String>, Option<Arc<CT_NotesMaster>>)> {
    let Some(relationships) = package.get_part_rels(presentation_part) else {
        return Ok((None, None));
    };
    let matches = relationships.get_all_by_type(rel_types::NOTES_MASTER);
    if matches.len() > 1 {
        return Err(Error::DuplicateRelationships {
            source_part: presentation_part.to_owned(),
            relationship_type: rel_types::NOTES_MASTER,
            count: matches.len(),
        });
    }
    let Some(relationship) = matches.first() else {
        return Ok((None, None));
    };
    reject_external(presentation_part, relationship)?;
    let part_name = OpcPackage::resolve_rel_target(presentation_part, &relationship.target);
    if let Some(master) = parsed.and_then(|parsed| parsed.notes_master(package, &part_name)) {
        return Ok((Some(part_name), Some(master)));
    }
    let master =
        CT_NotesMaster::from_xml(required_part(package, &part_name)?).map_err(|error| {
            Error::MalformedPart {
                part_name: part_name.clone(),
                message: error.to_string(),
            }
        })?;
    Ok((Some(part_name), Some(Arc::new(master))))
}

fn resolve_handout_master(
    package: &OpcPackage,
    presentation_part: &str,
    parsed: Option<&Parsed>,
) -> Result<(Option<String>, Option<Arc<CT_HandoutMaster>>)> {
    let Some(relationships) = package.get_part_rels(presentation_part) else {
        return Ok((None, None));
    };
    let matches = relationships.get_all_by_type(rel_types::HANDOUT_MASTER);
    if matches.len() > 1 {
        return Err(Error::DuplicateRelationships {
            source_part: presentation_part.to_owned(),
            relationship_type: rel_types::HANDOUT_MASTER,
            count: matches.len(),
        });
    }
    let Some(relationship) = matches.first() else {
        return Ok((None, None));
    };
    reject_external(presentation_part, relationship)?;
    let part_name = OpcPackage::resolve_rel_target(presentation_part, &relationship.target);
    if let Some(master) = parsed.and_then(|parsed| parsed.handout_master(package, &part_name)) {
        return Ok((Some(part_name), Some(master)));
    }
    let master =
        CT_HandoutMaster::from_xml(required_part(package, &part_name)?).map_err(|error| {
            Error::MalformedPart {
                part_name: part_name.clone(),
                message: error.to_string(),
            }
        })?;
    Ok((Some(part_name), Some(Arc::new(master))))
}

fn validate_collaboration_model(
    authors: &CommentAuthorList,
    slides: &[Arc<SlideRecord>],
) -> std::result::Result<(), String> {
    let mut author_ids = HashSet::new();
    for author in &authors.authors {
        if !author_ids.insert(author.id.as_str()) {
            return Err(format!("duplicate modern comment author id {}", author.id));
        }
    }
    let mut comment_ids = HashSet::new();
    let mut comment_parts = HashSet::new();
    for slide in slides {
        let Some(comments) = &slide.comments else {
            continue;
        };
        if !comment_parts.insert(comments.part_name.as_str()) {
            return Err(format!(
                "modern comment part {} is referenced by more than one slide",
                comments.part_name
            ));
        }
        for comment in &comments.comments.comments {
            if !author_ids.contains(comment.author_id.as_str()) {
                return Err(format!(
                    "modern comment {} refers to unknown author {}",
                    comment.id, comment.author_id
                ));
            }
            if !comment_ids.insert(comment.id.as_str()) {
                return Err(format!("duplicate modern comment id {}", comment.id));
            }
            for reply in comment.replies() {
                if !author_ids.contains(reply.author_id.as_str()) {
                    return Err(format!(
                        "modern comment reply {} refers to unknown author {}",
                        reply.id, reply.author_id
                    ));
                }
                if !comment_ids.insert(reply.id.as_str()) {
                    return Err(format!("duplicate modern comment id {}", reply.id));
                }
            }
        }
    }
    Ok(())
}
