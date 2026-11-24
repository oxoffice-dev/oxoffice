//! The font table and embedded fonts: declaring, embedding, extracting and obfuscating them.

use super::*;

impl Document {
    /// Return font-table records with related embedded bytes deobfuscated.
    pub fn fonts(&self) -> Vec<FontDefinition> {
        self.try_fonts().unwrap_or_default()
    }

    /// Return checked font snapshots after bounding every projected byte copy.
    pub fn try_fonts(&self) -> Result<Vec<FontDefinition>> {
        let Some(table) = self.modeled_font_table()? else {
            return Ok(Vec::new());
        };
        if table.fonts().len() > 4096 {
            return Err(Error::Other(
                "font snapshot exceeds 4096 records".to_owned(),
            ));
        }
        self.project_font_records(&table.fonts().iter().collect::<Vec<_>>())
    }

    /// The font table, `None` when the document has none. A font-table part
    /// the model cannot read is an error: every font call refuses it, rather
    /// than reading it as an empty table.
    fn modeled_font_table(&self) -> Result<Option<&FontTable>> {
        match (&self.font_table, &self.font_table_part_name) {
            (None, Some(_)) => Err(Error::Other("font table is unmodeled".to_owned())),
            (table, _) => Ok(table.as_ref()),
        }
    }

    fn project_font_records(&self, records: &[&FontRecord]) -> Result<Vec<FontDefinition>> {
        if records.len() > 4096 {
            return Err(Error::Other(
                "font snapshot exceeds 4096 records".to_owned(),
            ));
        }
        let mut bytes = 0usize;
        let mut faces = 0usize;
        let mut metadata = 0usize;
        for font in records {
            for value in [
                Some(&font.name),
                font.alternate_name.as_ref(),
                font.family.as_ref(),
                font.pitch.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                metadata = metadata
                    .checked_add(value.len())
                    .ok_or_else(|| Error::Other("font metadata size overflow".to_owned()))?;
                if value.len() > 4096 || metadata > 4 * 1024 * 1024 {
                    return Err(Error::Other(
                        "font snapshot metadata exceeds its bound".to_owned(),
                    ));
                }
            }
            for reference in &font.embedded_fonts {
                faces += 1;
                let (_, body) = self.checked_embedded_font_source(reference)?;
                bytes = bytes
                    .checked_add(body.len())
                    .ok_or_else(|| Error::Other("font snapshot size overflow".to_owned()))?;
                if body.len() > 64 * 1024 * 1024 || bytes > 256 * 1024 * 1024 || faces > 16384 {
                    return Err(Error::Other(
                        "font snapshot exceeds its byte or face bound".to_owned(),
                    ));
                }
                metadata = metadata
                    .checked_add(reference.font_key.len())
                    .and_then(|size| {
                        size.checked_add(reference.license_identity.as_ref().map_or(0, String::len))
                    })
                    .ok_or_else(|| Error::Other("font metadata size overflow".to_owned()))?;
                if reference.font_key.len() > 4096
                    || reference
                        .license_identity
                        .as_ref()
                        .is_some_and(|value| value.len() > 4096)
                    || metadata > 4 * 1024 * 1024
                {
                    return Err(Error::Other(
                        "font snapshot metadata exceeds its bound".to_owned(),
                    ));
                }
            }
        }
        records
            .iter()
            .map(|font| {
                let embedded_fonts = font
                    .embedded_fonts
                    .iter()
                    .map(|reference| {
                        let (part, bytes) = self.checked_embedded_font_source(reference)?;
                        Ok(EmbeddedFont {
                            kind: reference.kind.into(),
                            data: self
                                .embedded_font_face(&part, bytes, &reference.font_key)
                                .unwrap_or_default(),
                            font_key: reference.font_key.clone(),
                            subsetted: reference.subsetted.unwrap_or(false),
                            license: FontEmbeddingLicense {
                                authorized: reference.authorized.unwrap_or(false),
                                identity: reference.license_identity.clone().unwrap_or_default(),
                            },
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(FontDefinition {
                    name: font.name.clone(),
                    alternate_name: font.alternate_name.clone(),
                    family: font.family.clone(),
                    pitch: font.pitch.clone(),
                    embedded_fonts,
                })
            })
            .collect()
    }

    /// Resolve the actual stored part and byte length for an indexed font face.
    /// Indices follow `try_fonts()` record and face order.
    pub fn font_part_metadata(
        &self,
        record_index: usize,
        face_index: usize,
    ) -> Result<(String, usize)> {
        let reference = self
            .modeled_font_table()?
            .and_then(|table| table.fonts().get(record_index))
            .and_then(|record| record.embedded_fonts.get(face_index))
            .ok_or_else(|| Error::Other("font record or face index is out of range".to_owned()))?;
        let (part, bytes) = self.checked_embedded_font_source(reference)?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(Error::Other(
                "embedded font part exceeds its byte bound".to_owned(),
            ));
        }
        Ok((part, bytes.len()))
    }

    fn checked_embedded_font_source(
        &self,
        reference: &EmbeddedFontReference,
    ) -> Result<(String, &[u8])> {
        let (part, bytes) = self.embedded_font_source(reference)?;
        if self.embedded_font_storage(&part).is_none() {
            let content_type = self
                .package
                .content_types
                .content_type_for(&part)
                .unwrap_or("none");
            return Err(Error::Other(format!(
                "embedded font part {part} has an unsupported content type {content_type}"
            )));
        }
        Ok((part, bytes))
    }

    /// `Some(true)` is an obfuscated face, `Some(false)` a plain sfnt type,
    /// and `None` a type this reader does not accept.
    pub(super) fn embedded_font_storage(&self, part: &str) -> Option<bool> {
        let content_type = self.package.content_types.content_type_for(part)?;
        if content_type == EMBEDDED_FONT_CONTENT_TYPE {
            Some(true)
        } else {
            PLAIN_EMBEDDED_FONT_CONTENT_TYPES
                .iter()
                .any(|plain| content_type.eq_ignore_ascii_case(plain))
                .then_some(false)
        }
    }

    /// Face bytes of a checked font part. A plain part with an sfnt header is
    /// copied as stored, including under a null key. Every other accepted
    /// part is de-obfuscated.
    pub(super) fn embedded_font_face(
        &self,
        part: &str,
        body: &[u8],
        font_key: &str,
    ) -> Option<Vec<u8>> {
        if self.embedded_font_storage(part) == Some(false) && looks_like_sfnt(body) {
            let mut face = Vec::new();
            face.try_reserve_exact(body.len()).ok()?;
            face.extend_from_slice(body);
            return Some(face);
        }
        deobfuscate_odttf_with_key(body, font_key)
    }

    /// Resolve an embedded font reference to its stored part, whatever its
    /// content type.
    pub(super) fn embedded_font_source(
        &self,
        reference: &EmbeddedFontReference,
    ) -> Result<(String, &[u8])> {
        if reference.relationship_id.len() > 4096 || reference.font_key.len() > 4096 {
            return Err(Error::Other(
                "embedded font metadata exceeds its bound".to_owned(),
            ));
        }
        let owner = self
            .font_table_part_name
            .as_deref()
            .ok_or_else(|| Error::Other("font-table part name is missing".to_owned()))?;
        let relationships = self
            .package
            .get_part_rels(owner)
            .ok_or_else(|| Error::Other("font relationships are missing".to_owned()))?;
        let mut matches = relationships
            .items
            .iter()
            .filter(|rel| rel.id == reference.relationship_id);
        let relationship = matches
            .next()
            .filter(|rel| rel.rel_type == rel_types::FONT && relationship_is_internal(rel))
            .ok_or_else(|| {
                Error::Other("embedded font relationship is missing or unsupported".to_owned())
            })?;
        if matches.next().is_some() {
            return Err(Error::Other(
                "embedded font relationship is ambiguous".to_owned(),
            ));
        }
        if relationship.target.len() > 4096 {
            return Err(Error::Other(
                "embedded font target exceeds its bound".to_owned(),
            ));
        }
        let target = OpcPackage::resolve_rel_target(owner, &relationship.target);
        let part = self
            .package
            .parts
            .keys()
            .find(|part| part.eq_ignore_ascii_case(&target))
            .ok_or_else(|| Error::Other("embedded font part is missing".to_owned()))?;
        Ok((
            part.clone(),
            self.package.get_part(part).expect("checked font part"),
        ))
    }

    /// Return the presence-sensitive embedded TrueType setting.
    pub fn embed_true_type_fonts(&self) -> Result<Option<bool>> {
        if self.settings.is_none() && self.settings_part_name.is_some() {
            return Err(Error::Other("cannot inspect unmodeled settings".to_owned()));
        }
        Ok(self
            .settings
            .as_ref()
            .map(CT_Settings::embed_true_type_fonts)
            .transpose()?
            .flatten())
    }

    /// Set or remove the embedded TrueType setting atomically.
    pub fn set_embed_true_type_fonts(&mut self, value: Option<bool>) -> Result<()> {
        if self.embed_true_type_fonts()? == value {
            return Ok(());
        }
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_embed_true_type_fonts(value)?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Return the presence-sensitive embedded system font setting.
    pub fn embed_system_fonts(&self) -> Result<Option<bool>> {
        if self.settings.is_none() && self.settings_part_name.is_some() {
            return Err(Error::Other("cannot inspect unmodeled settings".to_owned()));
        }
        Ok(self
            .settings
            .as_ref()
            .map(CT_Settings::embed_system_fonts)
            .transpose()?
            .flatten())
    }

    /// Set or remove the embedded system font setting atomically.
    pub fn set_embed_system_fonts(&mut self, value: Option<bool>) -> Result<()> {
        if self.embed_system_fonts()? == value {
            return Ok(());
        }
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_embed_system_fonts(value)?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Return the presence-sensitive font subsetting setting.
    pub fn save_subset_fonts(&self) -> Result<Option<bool>> {
        if self.settings.is_none() && self.settings_part_name.is_some() {
            return Err(Error::Other("cannot inspect unmodeled settings".to_owned()));
        }
        Ok(self
            .settings
            .as_ref()
            .map(CT_Settings::save_subset_fonts)
            .transpose()?
            .flatten())
    }

    /// Set or remove the font subsetting setting atomically.
    pub fn set_save_subset_fonts(&mut self, value: Option<bool>) -> Result<()> {
        if self.save_subset_fonts()? == value {
            return Ok(());
        }
        let mut candidate = self.settings_mutation_candidate()?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_save_subset_fonts(value)?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Add or replace one descriptive font-table record, found by its name.
    /// A name several records carry is refused: `set_font_at` addresses one.
    pub fn set_font(&mut self, font: FontDefinition) -> Result<()> {
        check_font_definition(&font)?;
        let index = self
            .font_table
            .as_ref()
            .map(|table| table.font_index(&font.name))
            .transpose()?
            .flatten();
        self.set_font_record(index, font)
    }

    /// Replace the descriptive fields of the record at `index`, in
    /// `try_fonts()` order, whose name must be `font.name`: the way to change
    /// one of several records with the same name.
    pub fn set_font_at(&mut self, index: usize, font: FontDefinition) -> Result<()> {
        check_font_definition(&font)?;
        let record = self.font_record_at(index)?;
        if record.name != font.name {
            return Err(Error::Other(format!(
                "font record {index} is named {}, not {}: set_font_at does not rename",
                record.name, font.name
            )));
        }
        self.set_font_record(Some(index), font)
    }

    /// The number of font-table records, the bound of the indexes the `_at`
    /// calls take, without copying any embedded face.
    pub fn font_record_count(&self) -> Result<usize> {
        Ok(self
            .modeled_font_table()?
            .map_or(0, |table| table.fonts().len()))
    }

    /// The record at `index` in `try_fonts()` order.
    fn font_record_at(&self, index: usize) -> Result<&FontRecord> {
        match self.modeled_font_table()? {
            Some(table) => Ok(table.font_at(index)?),
            None => Err(Error::Other(format!(
                "font record index {index} is out of range: the document has no font table"
            ))),
        }
    }

    /// Set the fields of the record at `index`, or of a new record when
    /// `None`, from a checked definition.
    fn set_font_record(&mut self, index: Option<usize>, font: FontDefinition) -> Result<()> {
        if let (Some(index), Some(table)) = (index, &self.font_table) {
            let record = table.font_at(index)?;
            if record.alternate_name == font.alternate_name
                && record.family == font.family
                && record.pitch == font.pitch
            {
                return Ok(());
            }
            table.can_set_font_fields(
                index,
                font.alternate_name.is_some(),
                font.family.is_some(),
                font.pitch.is_some(),
            )?;
            table.can_clear_font_fields(
                index,
                record.alternate_name.is_some() && font.alternate_name.is_none(),
                record.family.is_some() && font.family.is_none(),
                record.pitch.is_some() && font.pitch.is_none(),
            )?;
        }
        let mut candidate = self.font_table_mutation_candidate()?;
        let table = candidate
            .font_table
            .as_mut()
            .expect("font-table mutation candidate has a model");
        let index = index.unwrap_or_else(|| table.add_font(font.name));
        table.set_font_fields(index, font.alternate_name, font.family, font.pitch);
        font_table_reads_back(table)?;
        candidate.font_table_dirty = true;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Remove one font-table record, found by its name, and every embedded
    /// face it owns. A name several records carry is refused:
    /// `remove_font_at` addresses one.
    pub fn remove_font(&mut self, name: &str) -> Result<Option<FontDefinition>> {
        let Some(index) = self
            .modeled_font_table()?
            .map(|table| table.font_index(name))
            .transpose()?
            .flatten()
        else {
            return Ok(None);
        };
        self.remove_font_record(index).map(Some)
    }

    /// Remove the record at `index`, in `try_fonts()` order, and every
    /// embedded face it owns.
    pub fn remove_font_at(&mut self, index: usize) -> Result<FontDefinition> {
        self.font_record_at(index)?;
        self.remove_font_record(index)
    }

    fn remove_font_record(&mut self, index: usize) -> Result<FontDefinition> {
        let table = self
            .font_table
            .as_ref()
            .expect("an indexed font record has a modelled table");
        table.can_remove_font(index)?;
        let record = table.font_at(index)?;
        let name = &record.name;
        // The cleanup after the record removes only its modelled faces'
        // relationships, so one its kept XML names another way must still be
        // used by the rest of the table once the record is gone.
        let named = self
            .font_table_part_name
            .as_deref()
            .and_then(|part| self.package.get_part_rels(part))
            .map(|relationships| {
                relationships
                    .items
                    .iter()
                    .filter(|relationship| {
                        !record
                            .embedded_fonts
                            .iter()
                            .any(|face| face.relationship_id == relationship.id)
                            && table.record_mentions_value(index, &relationship.id)
                    })
                    .map(|relationship| relationship.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let removed = self.project_font_records(&[record])?.remove(0);
        let mut candidate = self.font_table_mutation_candidate()?;
        let references = candidate
            .font_table
            .as_mut()
            .expect("font-table mutation candidate has a model")
            .remove_font(index)
            .embedded_fonts;
        let table = candidate
            .font_table
            .as_ref()
            .expect("font-table mutation candidate has a model");
        if let Some(orphan) = named.iter().find(|id| {
            table.embedded_relationship_reference_count(id) == 0
                && !table.retained_raw_mentions_relationship(id)
        }) {
            return Err(oxoffice_docx_oxml::OxmlError::InvalidValue(format!(
                "cannot remove a font record whose retained XML names a relationship: font {name} names {orphan}, which nothing else in the font table uses"
            ))
            .into());
        }
        for reference in references {
            candidate.remove_embedded_font_part(&reference)?;
        }
        candidate.font_table_dirty = true;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(removed)
    }

    /// Embed caller-provided font bytes after explicit license authorization,
    /// in the record named `font_name` (added when there is none). A name
    /// several records carry is refused: `embed_font_at` addresses one.
    pub fn embed_font(&mut self, font_name: &str, embedded: EmbeddedFont) -> Result<()> {
        if font_name.trim().is_empty()
            || font_name.len() > 4096
            || font_name
                .chars()
                .any(|ch| ch < ' ' || ch == '\u{fffe}' || ch == '\u{ffff}')
        {
            return Err(Error::Other("font name is empty or unsupported".to_owned()));
        }
        let index = self
            .font_table
            .as_ref()
            .map(|table| table.font_index(font_name))
            .transpose()?
            .flatten();
        self.embed_font_record(index, font_name, embedded)
    }

    /// Embed a face in the record at `index`, in `try_fonts()` order.
    pub fn embed_font_at(&mut self, index: usize, embedded: EmbeddedFont) -> Result<()> {
        let name = self.font_record_at(index)?.name.clone();
        self.embed_font_record(Some(index), &name, embedded)
    }

    fn embed_font_record(
        &mut self,
        index: Option<usize>,
        font_name: &str,
        embedded: EmbeddedFont,
    ) -> Result<()> {
        oxoffice_layout::font_embedding::check_embedding_license(
            embedded.license.authorized,
            &embedded.license.identity,
        )
        .map_err(Error::Other)?;
        if !valid_authored_sfnt(&embedded.data) {
            return Err(Error::Other(
                "embedded font bytes are not a supported sfnt font".to_owned(),
            ));
        }
        let guid = font_key_bytes(&embedded.font_key).ok_or_else(|| {
            Error::Other("embedded font key must be a 16-byte OOXML GUID".to_owned())
        })?;

        let mut candidate = self.font_table_mutation_candidate()?;
        let table = candidate
            .font_table
            .as_mut()
            .expect("font-table mutation candidate has a model");
        let index = index.unwrap_or_else(|| table.add_font(font_name.to_owned()));
        let table = candidate.font_table.as_ref().unwrap();
        let font = table.font_at(index)?;
        let kind = FontFaceKind::from(embedded.kind);
        table.can_embed_font(index, kind)?;
        let existing = font
            .embedded_fonts
            .iter()
            .find(|value| value.kind == kind)
            .cloned();
        let font_table_part = candidate
            .font_table_part_name
            .clone()
            .expect("font-table mutation candidate has a part name");

        if font
            .embedded_fonts
            .iter()
            .filter(|face| face.kind == kind)
            .count()
            > 1
        {
            return Err(Error::Other("embedded font face is ambiguous".to_owned()));
        }
        let existing_target = existing
            .as_ref()
            .map(|reference| {
                candidate
                    .checked_embedded_font_source(reference)
                    .map(|(target, _)| target)
            })
            .transpose()?;
        let obfuscated = obfuscate_odttf_with_key(&embedded.data, &guid);
        if let (Some(reference), Some(target)) = (&existing, &existing_target)
            && reference.font_key == embedded.font_key
            && reference.subsetted == Some(embedded.subsetted)
            && reference.authorized == Some(true)
            && reference.license_identity.as_ref() == Some(&embedded.license.identity)
            && candidate.package.get_part(target) == Some(obfuscated.as_slice())
            && candidate.embed_true_type_fonts()? == Some(true)
        {
            return Ok(());
        }
        let can_reuse_existing = existing.as_ref().is_some_and(|existing| {
            let relationship_reference_count =
                table.embedded_relationship_reference_count(&existing.relationship_id);
            let target = existing_target
                .as_ref()
                .expect("existing embedded font has a resolved target");
            let target_relationship_count = std::iter::once(("/", &candidate.package.package_rels))
                .chain(
                    candidate
                        .package
                        .part_rels
                        .iter()
                        .map(|(source, relationships)| (source.as_str(), relationships)),
                )
                .flat_map(|(source, relationships)| {
                    relationships
                        .items
                        .iter()
                        .map(move |relationship| (source, relationship))
                })
                .filter(|(source, relationship)| {
                    relationship_is_internal(relationship)
                        && OpcPackage::resolve_rel_target(source, &relationship.target)
                            .eq_ignore_ascii_case(target)
                })
                .count();
            relationship_reference_count == 1
                && target_relationship_count == 1
                && !table.retained_raw_mentions_relationship(&existing.relationship_id)
        });

        let (relationship_id, part_name) = if can_reuse_existing {
            (
                existing
                    .as_ref()
                    .expect("reusable embedded font exists")
                    .relationship_id
                    .clone(),
                existing_target.expect("reusable embedded font has a target"),
            )
        } else {
            let part_name = candidate
                .identifiers
                .reserve_preferred_part_name("/word/fonts/font1.odttf")?;
            let relationship_id = candidate
                .identifiers
                .reserve_relationship_id_checked(&font_table_part)?;
            let target = relative_target(&font_table_part, &part_name);
            candidate
                .package
                .get_or_create_part_rels(&font_table_part)
                .add_with_id(&relationship_id, rel_types::FONT, &target);
            (relationship_id, part_name)
        };

        candidate.package.set_part(&part_name, obfuscated);
        candidate
            .package
            .content_types
            .add_override(&part_name, EMBEDDED_FONT_CONTENT_TYPE);
        candidate
            .identifiers
            .register_content_type_override(&part_name);
        let reference = EmbeddedFontReference::new(
            kind,
            relationship_id,
            embedded.font_key,
            Some(embedded.subsetted),
            Some(true),
            Some(embedded.license.identity),
        );
        candidate
            .font_table
            .as_mut()
            .expect("font-table mutation candidate has a model")
            .set_embedded_font(index, reference);
        if !can_reuse_existing && let Some(existing) = &existing {
            candidate.remove_embedded_font_part(existing)?;
        }
        font_table_reads_back(
            candidate
                .font_table
                .as_ref()
                .expect("font-table mutation candidate has a model"),
        )?;
        if candidate.settings_part_name.is_some() && candidate.settings.is_none() {
            return Err(Error::Other("cannot mutate unmodeled settings".to_owned()));
        }
        let created = candidate.settings_part_name.is_none();
        let settings_part = match &candidate.settings_part_name {
            Some(part) => part.clone(),
            None => candidate
                .identifiers
                .reserve_preferred_part_name(DEFAULT_SETTINGS_PART)?,
        };
        candidate.ensure_part_relationship_checked(
            &settings_part,
            rel_types::SETTINGS,
            SETTINGS_CONTENT_TYPE,
        )?;
        candidate.settings_part_name = Some(settings_part);
        candidate.settings_owned |= created;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_embed_true_type_fonts(Some(true))?;
        candidate.font_table_dirty = true;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Remove one embedded face without removing its font-table record, found
    /// by its name. A name several records carry is refused:
    /// `remove_embedded_font_at` addresses one.
    pub fn remove_embedded_font(
        &mut self,
        font_name: &str,
        kind: EmbeddedFontKind,
    ) -> Result<Option<EmbeddedFont>> {
        let Some(index) = self
            .modeled_font_table()?
            .map(|table| table.font_index(font_name))
            .transpose()?
            .flatten()
        else {
            return Ok(None);
        };
        self.remove_embedded_font_record(index, kind)
    }

    /// Remove one embedded face of the record at `index`, in `try_fonts()`
    /// order, keeping the record. `None` when it has no face of that kind.
    pub fn remove_embedded_font_at(
        &mut self,
        index: usize,
        kind: EmbeddedFontKind,
    ) -> Result<Option<EmbeddedFont>> {
        self.font_record_at(index)?;
        self.remove_embedded_font_record(index, kind)
    }

    fn remove_embedded_font_record(
        &mut self,
        index: usize,
        kind: EmbeddedFontKind,
    ) -> Result<Option<EmbeddedFont>> {
        let table = self
            .font_table
            .as_ref()
            .expect("an indexed font record has a modelled table");
        table.can_remove_embedded_font(index, kind.into())?;
        let Some(reference) = table.fonts().get(index).and_then(|font| {
            font.embedded_fonts
                .iter()
                .find(|face| face.kind == kind.into())
        }) else {
            return Ok(None);
        };
        let (part, data) = self.checked_embedded_font_source(reference)?;
        if data.len() > 64 * 1024 * 1024
            || reference.font_key.len() > 4096
            || reference
                .license_identity
                .as_ref()
                .is_some_and(|value| value.len() > 4096)
        {
            return Err(Error::Other(
                "embedded font snapshot exceeds its bound".to_owned(),
            ));
        }
        let removed = EmbeddedFont {
            kind,
            data: self
                .embedded_font_face(&part, data, &reference.font_key)
                .unwrap_or_default(),
            font_key: reference.font_key.clone(),
            subsetted: reference.subsetted.unwrap_or(false),
            license: FontEmbeddingLicense {
                authorized: reference.authorized.unwrap_or(false),
                identity: reference.license_identity.clone().unwrap_or_default(),
            },
        };
        let mut candidate = self.font_table_mutation_candidate()?;
        let reference = candidate
            .font_table
            .as_mut()
            .expect("font-table mutation candidate has a model")
            .remove_embedded_font(index, kind.into())
            .expect("embedded font existed before staging");
        candidate.remove_embedded_font_part(&reference)?;
        candidate.font_table_dirty = true;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(Some(removed))
    }

    fn font_table_mutation_candidate(&self) -> Result<Self> {
        let mut candidate = self.clone_for_staging();
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        let part_name = match &candidate.font_table_part_name {
            Some(part_name) => {
                if candidate.font_table.is_none() {
                    return Err(Error::Other(format!(
                        "cannot mutate unmodeled font table at {part_name}"
                    )));
                }
                part_name.clone()
            }
            None => candidate
                .identifiers
                .reserve_preferred_part_name(DEFAULT_FONT_TABLE_PART)?,
        };
        candidate
            .ensure_part_relationship_checked(
                &part_name,
                rel_types::FONT_TABLE,
                FONT_TABLE_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!(
                    "font-table relationship allocation failed: {error}"
                ))
            })?;
        candidate.font_table.get_or_insert_with(FontTable::new);
        candidate.font_table_part_name = Some(part_name);
        Ok(candidate)
    }

    fn remove_embedded_font_part(&mut self, reference: &EmbeddedFontReference) -> Result<()> {
        let owner = self
            .font_table_part_name
            .clone()
            .ok_or_else(|| Error::Other("font-table part name is missing".to_owned()))?;
        let (target, _) = self.checked_embedded_font_source(reference)?;
        if self.font_table.as_ref().is_some_and(|table| {
            table.embedded_relationship_reference_count(&reference.relationship_id) > 0
                || table.retained_raw_mentions_relationship(&reference.relationship_id)
        }) {
            return Ok(());
        }
        if let Some(relationships) = self.package.get_part_rels_mut(&owner) {
            relationships.items.retain(|relationship| {
                relationship.id != reference.relationship_id
                    || relationship.rel_type != rel_types::FONT
                    || !relationship_is_internal(relationship)
            });
        }
        self.identifiers
            .retire_authored_story_relationships(&owner, vec![reference.relationship_id.clone()]);
        let still_referenced = std::iter::once(("/", &self.package.package_rels))
            .chain(
                self.package
                    .part_rels
                    .iter()
                    .map(|(source, relationships)| (source.as_str(), relationships)),
            )
            .any(|(source, relationships)| {
                relationships.items.iter().any(|relationship| {
                    relationship_is_internal(relationship)
                        && OpcPackage::resolve_rel_target(source, &relationship.target)
                            .eq_ignore_ascii_case(&target)
                })
            });
        if !still_referenced {
            self.package.remove_part(&target);
            self.package.remove_part_rels(&target);
            self.package.content_types.remove_override(&target);
            self.identifiers.retire_authored_part(&target);
        }
        Ok(())
    }
}

/// Refuse a font definition `set_font` cannot write: an empty or unsupported
/// name, a family or pitch outside the schema, embedded bytes.
fn check_font_definition(font: &FontDefinition) -> Result<()> {
    if font.name.trim().is_empty()
        || std::iter::once(font.name.as_str())
            .chain(font.alternate_name.as_deref())
            .any(|value| {
                value.len() > 4096
                    || value
                        .chars()
                        .any(|ch| ch < ' ' || ch == '\u{fffe}' || ch == '\u{ffff}')
            })
    {
        return Err(Error::Other(
            "font metadata is empty or unsupported".to_owned(),
        ));
    }
    if font.family.as_deref().is_some_and(|family| {
        !matches!(
            family,
            "decorative" | "modern" | "roman" | "script" | "swiss" | "auto"
        )
    }) {
        return Err(Error::Other(
            "font family must be a valid OOXML family enumeration".to_owned(),
        ));
    }
    if font
        .pitch
        .as_deref()
        .is_some_and(|pitch| !matches!(pitch, "default" | "fixed" | "variable"))
    {
        return Err(Error::Other(
            "font pitch must be a valid OOXML pitch enumeration".to_owned(),
        ));
    }
    if !font.embedded_fonts.is_empty() {
        return Err(Error::Other(
            "set_font does not accept embedded bytes, use embed_font".to_owned(),
        ));
    }
    Ok(())
}

fn font_key_bytes(value: &str) -> Option<[u8; 16]> {
    let value_bytes = value.as_bytes();
    if value_bytes.len() != 38
        || value_bytes.first() != Some(&b'{')
        || value_bytes.last() != Some(&b'}')
        || ![9, 14, 19, 24]
            .into_iter()
            .all(|index| value_bytes.get(index) == Some(&b'-'))
        || !value_bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 0 | 9 | 14 | 19 | 24 | 37) || matches!(byte, b'0'..=b'9' | b'A'..=b'F')
        })
    {
        return None;
    }
    let hex: String = value[1..37]
        .chars()
        .filter(|character| *character != '-')
        .collect();
    let mut guid = [0u8; 16];
    for (index, byte) in guid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(guid)
}

fn obfuscate_odttf_with_key(data: &[u8], guid: &[u8; 16]) -> Vec<u8> {
    let key = odttf_key_candidates(guid)[0];
    let mut result = data.to_vec();
    for (index, byte) in result.iter_mut().take(32).enumerate() {
        *byte ^= key[index % 16];
    }
    result
}

fn deobfuscate_odttf_with_key(data: &[u8], font_key: &str) -> Option<Vec<u8>> {
    if data.len() < 32 || data.len() > 64 * 1024 * 1024 {
        return None;
    }
    let guid = font_key_bytes(font_key)?;
    let candidates = odttf_key_candidates(&guid);
    let prefixes = candidates.map(|key| {
        let mut prefix = *data.first_chunk::<32>().unwrap();
        for (index, byte) in prefix.iter_mut().enumerate() {
            *byte ^= key[index % 16];
        }
        prefix
    });
    // Producer subset headers may omit the derived binary-search hints.
    let index = prefixes
        .iter()
        .position(|prefix| has_consistent_sfnt_header(prefix))
        .or_else(|| prefixes.iter().position(|prefix| looks_like_sfnt(prefix)))?;
    let mut result = Vec::new();
    result.try_reserve_exact(data.len()).ok()?;
    result.extend_from_slice(data);
    result[..32].copy_from_slice(&prefixes[index]);
    Some(result)
}

/// Bound and check every authored single-face sfnt table before obfuscation.
pub(super) fn valid_authored_sfnt(data: &[u8]) -> bool {
    oxoffice_layout::font_embedding::SfntTables::parse(data).is_ok()
}

/// The two candidate XOR keys for ODTTF deobfuscation, most likely first.
fn odttf_key_candidates(guid: &[u8; 16]) -> [[u8; 16]; 2] {
    // Read the hex string end-first, as the spec prose describes.
    let mut plain_reversed = *guid;
    plain_reversed.reverse();

    // The .NET route: `Guid.ToByteArray` byte-swaps the first three groups,
    // and the whole array is then reversed.
    let dotnet = [
        guid[3], guid[2], guid[1], guid[0], guid[5], guid[4], guid[7], guid[6], guid[8], guid[9],
        guid[10], guid[11], guid[12], guid[13], guid[14], guid[15],
    ];
    let mut dotnet_reversed = dotnet;
    dotnet_reversed.reverse();

    [plain_reversed, dotnet_reversed]
}

/// Check that `data` opens with a plausible sfnt (TrueType/OpenType) header.
///
/// This is the weak test: signature plus printable table tags. It cannot always
/// tell the two ODTTF key conventions apart, since they produce identical
/// output for the first eight bytes.
fn looks_like_sfnt(data: &[u8]) -> bool {
    let Some(signature) = data.first_chunk::<4>() else {
        return false;
    };
    match signature {
        b"\x00\x01\x00\x00" | b"OTTO" | b"true" => {}
        // A collection header has a different layout; take it on signature.
        b"ttcf" => return true,
        _ => return false,
    }

    if data.len() < 32 {
        return false;
    }

    let num_tables = u16::from_be_bytes([data[4], data[5]]);
    if num_tables == 0 || num_tables > 512 {
        return false;
    }

    // Table records begin at offset 12 and are 16 bytes each, so the first
    // record's tag is at 12..16 and the second record's tag at 28..32 — both
    // inside the 32 bytes the obfuscation touches.
    let is_tag = |tag: &[u8]| tag.iter().all(|b| (0x20..=0x7E).contains(b));
    is_tag(&data[12..16]) && (num_tables < 2 || is_tag(&data[28..32]))
}

/// The strong test: the sfnt header's binary-search hints must agree with the
/// table count.
///
/// `searchRange`, `entrySelector` and `rangeShift` are all derived from
/// `numTables`, and `entrySelector`/`rangeShift` sit in the byte range where
/// the two ODTTF key conventions differ — so this identifies the right key
/// outright whenever the font's header is spec-conformant.
fn has_consistent_sfnt_header(data: &[u8]) -> bool {
    if !looks_like_sfnt(data) || data.len() < 12 {
        return false;
    }
    if data.first_chunk::<4>() == Some(b"ttcf") {
        return false; // no table directory at this offset
    }

    let num_tables = u16::from_be_bytes([data[4], data[5]]);
    let search_range = u16::from_be_bytes([data[6], data[7]]);
    let entry_selector = u16::from_be_bytes([data[8], data[9]]);
    let range_shift = u16::from_be_bytes([data[10], data[11]]);

    let expected_selector = num_tables.ilog2() as u16;
    let expected_search_range = (1u16 << expected_selector) * 16;
    let expected_range_shift = num_tables
        .wrapping_mul(16)
        .wrapping_sub(expected_search_range);

    search_range == expected_search_range
        && entry_selector == expected_selector
        && range_shift == expected_range_shift
}

/// Refuse an edit whose font table the next open could not read back: it
/// would come back unmodelled, with every record out of reach.
fn font_table_reads_back(table: &FontTable) -> Result<()> {
    FontTable::from_xml(&table.to_xml()?)
        .map(|_| ())
        .map_err(|error| {
            Error::Other(format!(
                "the edited font table would not read back: {error}"
            ))
        })
}
