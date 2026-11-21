//! Themes: reading a master's theme, applying another presentation's theme with its master, and editing theme names, colors and fonts.

use super::*;

/// One slide master's theme, as [`Presentation::theme`] reads it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Theme {
    /// The `a:theme/@name`.
    pub name: Option<String>,
    /// The colour scheme in `dk1`, `lt1`, `dk2`, `lt2`, `accent1` to
    /// `accent6`, `hlink`, `folHlink` order. A system colour reads as its
    /// last computed value, and a colour oxoffice cannot resolve as `None`.
    pub colors: Vec<(&'static str, Option<RgbColor>)>,
    /// The heading fonts, `+mj-lt` and its East Asian and complex-script peers.
    pub major_font: ThemeFonts,
    /// The body fonts, `+mn-lt` and its East Asian and complex-script peers.
    pub minor_font: ThemeFonts,
}

impl Theme {
    /// Returns one scheme colour by its slot name, such as `accent1`.
    pub fn color(&self, slot: &str) -> Option<RgbColor> {
        self.colors
            .iter()
            .find(|(name, _)| *name == slot)
            .and_then(|(_, color)| *color)
    }
}

/// The typefaces of one theme font collection, empty when the theme names none.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeFonts {
    pub latin: String,
    pub east_asian: String,
    pub complex_script: String,
}

impl Presentation {
    fn theme_part_for_edit(&self, master_index: usize) -> Result<String> {
        let master_part = self
            .master_part(master_index)?
            .ok_or_else(|| invalid_shape_mutation("edit theme", "master index is out of range"))?;
        related_internal_part(&self.package, &master_part, rel_types::THEME)?
            .ok_or_else(|| footer_malformed(&master_part, "master has no theme relationship"))
    }

    /// Applies the selected source schemes, preserving destination root and extension source.
    pub fn apply_theme(
        &mut self,
        source: &OpcPackage,
        source_master: Option<usize>,
        import_masters: bool,
        layout_map: &[(usize, usize)],
    ) -> Result<()> {
        let source_deck = source
            .main_document_part()
            .map(|_| Self::from_package(source.clone()))
            .transpose()?;
        let source_theme = if let Some(source_deck) = &source_deck {
            let index = match source_master {
                Some(index) if index < source_deck.master_count() => index,
                None if source_deck.master_count() == 1 => 0,
                _ => {
                    return Err(invalid_shape_mutation(
                        "apply theme",
                        "select exactly one source master",
                    ));
                }
            };
            source_deck.theme_part_for_edit(index)?
        } else {
            if source_master.is_some() || import_masters || !layout_map.is_empty() {
                return Err(invalid_shape_mutation(
                    "apply theme",
                    "a theme package has no master or layout inventory",
                ));
            }
            let relationships = source
                .package_rels
                .items
                .iter()
                .filter(|relationship| relationship.rel_type == rel_types::THEME)
                .collect::<Vec<_>>();
            if relationships.len() != 1 || relationship_is_external(relationships[0]) {
                return Err(invalid_shape_mutation(
                    "apply theme",
                    "one internal package theme relationship is required",
                ));
            }
            OpcPackage::resolve_rel_target("/", &relationships[0].target)
        };
        if source.content_types.content_type_for(&source_theme) != Some(content_types::THEME) {
            return Err(footer_malformed(
                &source_theme,
                "source theme content type is not supported",
            ));
        }
        let xml = required_part(source, &source_theme)?;
        oxoffice_drawing::theme::CT_OfficeStyleSheet::from_xml(xml)
            .map_err(|error| footer_malformed(&source_theme, error.to_string()))?;
        let schemes = ["clrScheme", "fontScheme", "fmtScheme"]
            .into_iter()
            .map(|name| portable_theme_scheme(xml, name))
            .collect::<Result<Vec<_>>>()?;
        if !import_masters && !layout_map.is_empty() {
            return Err(invalid_shape_mutation(
                "apply theme",
                "layout mapping requires imported masters",
            ));
        }
        let staged = self.staged_package(true)?;
        let mut candidate = self.clone();
        candidate.package = staged.clone();
        let mut themes = HashSet::new();
        for index in 0..self.master_count() {
            themes.insert(self.theme_part_for_edit(index)?);
        }
        for part in themes {
            let destination = required_part(&staged, &part)?;
            let mut patched = destination.to_vec();
            let mut edits = Vec::new();
            for (name, scheme) in ["clrScheme", "fontScheme", "fmtScheme"]
                .into_iter()
                .zip(&schemes)
            {
                let owner = theme_source_owner(destination, &["theme", "themeElements", name])?
                    .ok_or_else(|| {
                        footer_malformed(&part, format!("qualified {name} is absent"))
                    })?;
                edits.push((owner.start, owner.end, scheme));
            }
            edits.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
            let source_relationships = source
                .get_part_rels(&source_theme)
                .cloned()
                .unwrap_or_default();
            let mut destination_relationships = candidate
                .package
                .get_part_rels(&part)
                .cloned()
                .unwrap_or_default();
            let mut relationships = HashMap::new();
            for (_, _, scheme) in &edits {
                for id in relationship_ids(scheme)
                    .map_err(|error| footer_malformed(&source_theme, error.to_string()))?
                {
                    if relationships.contains_key(&id) {
                        continue;
                    }
                    let relationship = source_relationships.get_by_id(&id).ok_or_else(|| {
                        footer_malformed(
                            &source_theme,
                            format!("missing selected scheme relationship {id}"),
                        )
                    })?;
                    if relationship_is_external(relationship)
                        || relationship.rel_type != rel_types::IMAGE
                    {
                        return Err(footer_malformed(
                            &source_theme,
                            format!(
                                "unsupported selected scheme relationship {id}: {}",
                                relationship.rel_type
                            ),
                        ));
                    }
                    let image_part =
                        OpcPackage::resolve_rel_target(&source_theme, &relationship.target);
                    preflight_internal_transfer_image(source, &image_part, "apply theme")?;
                    let filename = image_part.rsplit('/').next().unwrap_or("image");
                    let image = required_part(source, &image_part)?;
                    let copied =
                        candidate
                            .media_store
                            .insert(&mut candidate.package, image, filename);
                    if candidate.package.content_types.content_type_for(&copied)
                        != source.content_types.content_type_for(&image_part)
                    {
                        return Err(footer_malformed(
                            &image_part,
                            "image content type does not match its copied program",
                        ));
                    }
                    let new_id = destination_relationships
                        .add(rel_types::IMAGE, &relative_part_target(&part, &copied));
                    relationships.insert(id, new_id);
                }
            }
            for (start, end, scheme) in edits {
                let rewritten = rewrite_exact_rel_ids(scheme, &relationships)
                    .map_err(|error| footer_malformed(&source_theme, error.to_string()))?;
                patched.splice(start..end, rewritten);
            }
            oxoffice_drawing::theme::CT_OfficeStyleSheet::from_xml(&patched)
                .map_err(|error| footer_malformed(&part, error.to_string()))?;
            candidate.package.set_part(&part, patched);
            candidate
                .package
                .set_part_rels(&part, destination_relationships);
        }
        if import_masters {
            let source_deck = source_deck.as_ref().ok_or_else(|| {
                invalid_shape_mutation("apply theme", "source has no master inventory")
            })?;
            let selected = source_master.unwrap_or(0);
            let original_layouts = self.layout_count();
            let imported = candidate.import_selected_master(source_deck, selected)?;
            let mut destinations = HashSet::new();
            for &(destination, source_index) in layout_map {
                if destination >= original_layouts
                    || source_index >= imported.len()
                    || !destinations.insert(destination)
                {
                    return Err(invalid_shape_mutation(
                        "apply theme",
                        "layout map has a duplicate or out-of-range owner",
                    ));
                }
            }
            for &(destination, source_index) in layout_map {
                let original_part = self.layouts[destination].part_name.clone();
                let imported_part = candidate.layouts[imported[source_index]].part_name.clone();
                for slide in &candidate.slides {
                    let relationships = candidate
                        .package
                        .get_part_rels(&slide.part_name)
                        .ok_or_else(|| {
                            footer_malformed(&slide.part_name, "slide relationships are absent")
                        })?;
                    let layouts = relationships
                        .items
                        .iter()
                        .filter(|relationship| relationship.rel_type == rel_types::SLIDE_LAYOUT)
                        .collect::<Vec<_>>();
                    if layouts.len() != 1 || relationship_is_external(layouts[0]) {
                        return Err(footer_malformed(
                            &slide.part_name,
                            "one internal layout relationship is required",
                        ));
                    }
                    if OpcPackage::resolve_rel_target(&slide.part_name, &layouts[0].target)
                        == original_part
                    {
                        let changes = HashMap::from([(
                            layouts[0].id.clone(),
                            relative_part_target(&slide.part_name, &imported_part),
                        )]);
                        let changed =
                            retarget_import_relationships(&relationships.to_xml()?, &changes)?;
                        candidate
                            .package
                            .part_rels
                            .insert(slide.part_name.clone(), changed);
                    }
                }
            }
        }
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// Copies only the selected branding graph into fresh part scopes.
    fn import_selected_master(&mut self, source: &Self, selected: usize) -> Result<Vec<usize>> {
        let master_part = source
            .master_part(selected)?
            .ok_or_else(|| invalid_shape_mutation("import master", "source master is absent"))?;
        let theme_part = source.theme_part_for_edit(selected)?;
        let layouts = source.master_layout_indices(selected)?;
        let source_package = source.staged_package(true)?;
        let mut parts = vec![master_part.clone(), theme_part.clone()];
        parts.extend(
            layouts
                .iter()
                .map(|index| source.layouts[*index].part_name.clone()),
        );
        let allowed: HashSet<_> = parts.iter().cloned().collect();
        let mut images = HashSet::new();
        for part in &parts {
            let xml = required_part(&source_package, part)?;
            let (namespace, root, content_type) = if part == &master_part {
                (
                    oxoffice_pptx_oxml::namespace::P_NS,
                    "sldMaster",
                    content_types::SLIDE_MASTER,
                )
            } else if part == &theme_part {
                (A_NS, "theme", content_types::THEME)
            } else {
                (
                    oxoffice_pptx_oxml::namespace::P_NS,
                    "sldLayout",
                    content_types::SLIDE_LAYOUT,
                )
            };
            property_child_ranges(xml, namespace, root)?;
            if source_package.content_types.content_type_for(part) != Some(content_type) {
                return Err(footer_malformed(
                    part,
                    "selected source part has an invalid content type",
                ));
            }
            for id in
                relationship_ids(xml).map_err(|error| footer_malformed(part, error.to_string()))?
            {
                if source_package
                    .get_part_rels(part)
                    .and_then(|relationships| relationships.get_by_id(&id))
                    .is_none()
                {
                    return Err(footer_malformed(
                        part,
                        format!("selected qualified relationship reference {id} is absent"),
                    ));
                }
            }
            for relationship in source_package
                .get_part_rels(part)
                .into_iter()
                .flat_map(|relationships| &relationships.items)
            {
                if relationship_is_external(relationship) {
                    if relationship.rel_type == rel_types::HYPERLINK {
                        continue;
                    }
                    return Err(footer_malformed(
                        part,
                        format!(
                            "unsupported selected external relationship {}",
                            relationship.id
                        ),
                    ));
                }
                let target = OpcPackage::resolve_rel_target(part, &relationship.target);
                match relationship.rel_type.as_str() {
                    rel_types::SLIDE_MASTER if part != &master_part && target == master_part => {}
                    rel_types::SLIDE_LAYOUT
                        if part == &master_part
                            && allowed.contains(&target)
                            && target != theme_part
                            && target != master_part => {}
                    rel_types::THEME if part == &master_part && target == theme_part => {}
                    rel_types::IMAGE => {
                        preflight_internal_transfer_image(
                            &source_package,
                            &target,
                            "import master",
                        )?;
                        images.insert(target);
                    }
                    _ => {
                        return Err(footer_malformed(
                            part,
                            format!(
                                "unsupported selected relationship {}: {}",
                                relationship.id, relationship.rel_type
                            ),
                        ));
                    }
                }
            }
        }
        let mut mapping = HashMap::new();
        for part in &parts {
            let fresh = fresh_related_part_name(&self.package, part);
            self.package.set_part(&fresh, Vec::new());
            mapping.insert(part.clone(), fresh);
        }
        for image in images {
            let filename = image.rsplit('/').next().unwrap_or("image");
            let copied = self.media_store.insert(
                &mut self.package,
                required_part(&source_package, &image)?,
                filename,
            );
            if self.package.content_types.content_type_for(&copied)
                != source_package.content_types.content_type_for(&image)
            {
                return Err(footer_malformed(
                    &image,
                    "image content type does not match its copied program",
                ));
            }
            mapping.insert(image, copied);
        }
        for part in &parts {
            let destination = &mapping[part];
            let content_type = source_package
                .content_types
                .content_type_for(part)
                .ok_or_else(|| footer_malformed(part, "selected source content type is absent"))?;
            let mut xml = required_part(&source_package, part)?.to_vec();
            if part == &master_part {
                let mut next = self
                    .masters
                    .iter()
                    .filter_map(|record| record.master.as_ref().ok())
                    .map(|master| master.slide_layout_ids())
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|error| footer_malformed(part, error.to_string()))?
                    .into_iter()
                    .flatten()
                    .map(|(id, _)| id)
                    .max()
                    .unwrap_or(2_147_483_647);
                let mut ids = HashMap::new();
                for (_, relationship) in source
                    .master(selected)
                    .ok_or_else(|| footer_malformed(part, "master model is absent"))?
                    .slide_layout_ids()
                    .map_err(|error| footer_malformed(part, error.to_string()))?
                {
                    next = next.checked_add(1).ok_or_else(|| {
                        invalid_shape_mutation("import master", "layout ID space exhausted")
                    })?;
                    ids.insert(relationship, next);
                }
                xml = reassign_imported_layout_ids(&xml, &ids)?;
            }
            self.package.set_part(destination, xml);
            self.package
                .content_types
                .add_override(destination, content_type);
            if let Some(relationships) = source_package.get_part_rels(part) {
                let changes = relationships
                    .items
                    .iter()
                    .filter(|relationship| !relationship_is_external(relationship))
                    .map(|relationship| {
                        let target = OpcPackage::resolve_rel_target(part, &relationship.target);
                        mapping
                            .get(&target)
                            .map(|fresh| {
                                (
                                    relationship.id.clone(),
                                    relative_part_target(destination, fresh),
                                )
                            })
                            .ok_or_else(|| footer_malformed(part, "selected closure is incomplete"))
                    })
                    .collect::<Result<HashMap<_, _>>>()?;
                self.package.set_part_rels(
                    destination,
                    retarget_import_relationships(&relationships.to_xml()?, &changes)?,
                );
            }
        }
        let new_master = mapping[&master_part].clone();
        let id = self
            .presentation
            .slide_master_ids
            .iter()
            .filter_map(|item| item.id)
            .max()
            .unwrap_or(2_147_483_647)
            .max(2_147_483_647)
            .checked_add(1)
            .ok_or_else(|| invalid_shape_mutation("import master", "master ID space exhausted"))?;
        let relationships = self
            .package
            .get_part_rels(&self.presentation_part)
            .ok_or_else(|| {
                footer_malformed(
                    &self.presentation_part,
                    "presentation relationships are absent",
                )
            })?;
        let source_relationships = relationships.to_xml()?;
        let mut allocated = relationships.clone();
        let relationship_id = allocated.add(
            rel_types::SLIDE_MASTER,
            &relative_part_target(&self.presentation_part, &new_master),
        );
        let added = allocated
            .get_by_id(&relationship_id)
            .expect("allocated master relationship");
        let relationships = edit_master_layout_relationship(&source_relationships, added, false)?;
        self.package
            .set_part_rels(&self.presentation_part, relationships);
        let presentation_source = required_part(&self.package, &self.presentation_part)?;
        let changed = append_imported_master_id(presentation_source, id, &relationship_id)?;
        self.presentation = CT_Presentation::from_xml(&changed)
            .map_err(|error| footer_malformed(&self.presentation_part, error.to_string()))?;
        self.package.set_part(&self.presentation_part, changed);
        let master = CT_SlideMaster::from_xml(required_part(&self.package, &new_master)?)
            .map_err(|error| footer_malformed(&new_master, error.to_string()))?;
        self.masters.push(Arc::new(MasterRecord {
            id: Some(id),
            part_name: new_master,
            master: Ok(master),
            dirty: false,
        }));
        let mut imported = Vec::new();
        for index in layouts {
            let part_name = mapping[&source.layouts[index].part_name].clone();
            imported.push(self.layouts.len());
            self.layouts.push(Arc::new(LayoutRecord {
                part_name,
                layout: source.layouts[index].layout.clone(),
                dirty: false,
            }));
        }
        Ok(imported)
    }

    /// Changes the owning theme's name without rewriting unrelated source XML.
    pub fn set_theme_name(&mut self, master_index: usize, name: &str) -> Result<()> {
        let part = self.theme_part_for_edit(master_index)?;
        let source = required_part(&self.package, &part)?;
        let owner = theme_source_owner(source, &["theme"])?
            .ok_or_else(|| invalid_shape_mutation("edit theme name", "theme root is absent"))?;
        let bytes = if owner.attributes.iter().any(|(key, _, _)| key == "name") {
            patch_theme_attribute(source, &owner, "name", name)?
        } else {
            let mut reader = NsReader::from_reader(&source[owner.start..]);
            let mut buffer = Vec::new();
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| footer_malformed(&part, error))?;
            let insertion =
                owner.start + reader.buffer_position() as usize - if owner.empty { 2 } else { 1 };
            let mut bytes = source[..insertion].to_vec();
            bytes.extend_from_slice(
                format!(" name=\"{}\"", escape_attribute_value(name)).as_bytes(),
            );
            bytes.extend_from_slice(&source[insertion..]);
            bytes
        };
        if bytes == source {
            return Ok(());
        }
        CT_OfficeStyleSheet::from_xml(&bytes).map_err(|error| footer_malformed(&part, error))?;
        let mut candidate = self.clone();
        candidate.package.set_part(&part, bytes);
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// Replaces one qualified scheme color without rewriting unrelated source XML.
    pub fn set_theme_color(
        &mut self,
        master_index: usize,
        slot: &str,
        rgb: RgbColor,
    ) -> Result<()> {
        const SLOTS: &[&str] = &[
            "dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5",
            "accent6", "hlink", "folHlink",
        ];
        if !SLOTS.contains(&slot) {
            return Err(invalid_shape_mutation(
                "edit theme color",
                "unknown theme color slot",
            ));
        }
        let part = self.theme_part_for_edit(master_index)?;
        let source = required_part(&self.package, &part)?;
        let mut selected = None;
        for choice in [
            "srgbClr",
            "scrgbClr",
            "hslClr",
            "sysClr",
            "schemeClr",
            "prstClr",
        ] {
            let path = ["theme", "themeElements", "clrScheme", slot, choice];
            if let Some(owner) = theme_source_owner(source, &path)?
                && selected.replace((choice, owner)).is_some()
            {
                return Err(invalid_shape_mutation(
                    "edit theme color",
                    "ambiguous theme color choice",
                ));
            }
        }
        let (choice, owner) = selected.ok_or_else(|| {
            invalid_shape_mutation("edit theme color", "qualified color owner is absent")
        })?;
        let [r, g, b] = rgb.components();
        let value = format!("{r:02X}{g:02X}{b:02X}");
        let bytes = if choice == "srgbClr" {
            let patched = patch_theme_attribute(source, &owner, "val", &value)?;
            clear_theme_color_transforms(&patched, slot)?
        } else {
            // Changing the concrete color choice owns its schema value, but cannot discard opaque payload.
            if !owner.empty
                || owner.attributes.iter().any(|(name, _, _)| {
                    !matches!(
                        (choice, name.as_str()),
                        ("sysClr", "val" | "lastClr")
                            | ("schemeClr" | "prstClr", "val")
                            | ("scrgbClr", "r" | "g" | "b")
                            | ("hslClr", "hue" | "sat" | "lum")
                    ) && name != "xmlns"
                        && !name.starts_with("xmlns:")
                })
            {
                return Err(invalid_shape_mutation(
                    "edit theme color",
                    "changing the color type would discard opaque source",
                ));
            }
            let replacement = format!("<a:srgbClr xmlns:a=\"{A_NS}\" val=\"{value}\"/>");
            let mut bytes = source[..owner.start].to_vec();
            bytes.extend_from_slice(replacement.as_bytes());
            bytes.extend_from_slice(&source[owner.end..]);
            bytes
        };
        if bytes == source {
            return Ok(());
        }
        CT_OfficeStyleSheet::from_xml(&bytes).map_err(|error| footer_malformed(&part, error))?;
        let mut candidate = self.clone();
        candidate.package.set_part(&part, bytes);
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// Changes one qualified typeface while retaining all other lexical source facts.
    pub fn set_theme_font(
        &mut self,
        master_index: usize,
        major: bool,
        script: &str,
        typeface: &str,
    ) -> Result<()> {
        oxoffice_core::xml::reject_non_xml_characters("theme typeface", typeface)
            .map_err(|error| invalid_shape_mutation("edit theme font", error.to_string()))?;
        if typeface.chars().any(|ch| ch.is_control()) {
            return Err(invalid_shape_mutation(
                "edit theme font",
                "typeface contains a control character",
            ));
        }
        let script = match script {
            "latin" => "latin",
            "east_asian" => "ea",
            "complex_script" => "cs",
            _ => {
                return Err(invalid_shape_mutation(
                    "edit theme font",
                    "unknown font script slot",
                ));
            }
        };
        let part = self.theme_part_for_edit(master_index)?;
        let source = required_part(&self.package, &part)?;
        let path = [
            "theme",
            "themeElements",
            "fontScheme",
            if major { "majorFont" } else { "minorFont" },
            script,
        ];
        let owner = theme_source_owner(source, &path)?.ok_or_else(|| {
            invalid_shape_mutation("edit theme font", "qualified typeface owner is absent")
        })?;
        let bytes = patch_theme_attribute(source, &owner, "typeface", typeface)?;
        if bytes == source {
            return Ok(());
        }
        CT_OfficeStyleSheet::from_xml(&bytes).map_err(|error| footer_malformed(&part, error))?;
        let mut candidate = self.clone();
        candidate.package.set_part(&part, bytes);
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// Reads the current theme reached by one presentation master.
    pub fn theme(&self, master_index: usize) -> Result<Option<Theme>> {
        let Some(master_part) = self.master_part(master_index)? else {
            return Ok(None);
        };
        let theme_part = related_internal_part(&self.package, &master_part, rel_types::THEME)?
            .ok_or_else(|| Error::MalformedPart {
                part_name: master_part.clone(),
                message: "slide master has no theme relationship".to_owned(),
            })?;
        let theme = oxoffice_drawing::theme::CT_OfficeStyleSheet::from_xml(required_part(
            &self.package,
            &theme_part,
        )?)
        .map_err(|error| Error::MalformedPart {
            part_name: theme_part,
            message: error.to_string(),
        })?;
        let elements = &theme.theme_elements;
        let fonts = |collection: &oxoffice_drawing::theme::CT_FontCollection| ThemeFonts {
            latin: collection.latin.typeface.clone(),
            east_asian: collection.east_asian.typeface.clone(),
            complex_script: collection.complex_script.typeface.clone(),
        };
        Ok(Some(Theme {
            name: theme.name.clone(),
            colors: elements
                .color_scheme
                .iter()
                .map(|(slot, choice)| {
                    let color = oxoffice_drawing::color::resolve_color(
                        choice,
                        &oxoffice_drawing::color::ColorMap::default(),
                        &[],
                    )
                    .ok()
                    .map(|color| RgbColor::new(color.red, color.green, color.blue));
                    (slot.as_str(), color)
                })
                .collect(),
            major_font: fonts(&elements.font_scheme.major_font),
            minor_font: fonts(&elements.font_scheme.minor_font),
        }))
    }
}

struct ThemeSourceOwner {
    start: usize,
    end: usize,
    empty: bool,
    attributes: Vec<(String, String, (usize, usize))>,
}

fn theme_source_owner(source: &[u8], expected: &[&str]) -> Result<Option<ThemeSourceOwner>> {
    let mut reader = NsReader::from_reader(source);
    let mut buffer = Vec::new();
    let mut path = Vec::new();
    let mut found = None;
    let mut pending = None;
    loop {
        let begin = reader.buffer_position() as usize;
        let (qualified, event) = {
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| invalid_shape_mutation("edit theme", error.to_string()))?;
            (xml_is(&namespace, A_NS), event)
        };
        let finish = reader.buffer_position() as usize;
        let empty = matches!(&event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                let local =
                    String::from_utf8_lossy(xml_local_name(start.name().as_ref())).into_owned();
                if path.is_empty() && (!qualified || local != "theme") {
                    return Err(invalid_shape_mutation(
                        "edit theme",
                        "qualified DrawingML theme root is required",
                    ));
                }
                path.push(if qualified { local } else { String::new() });
                if path.iter().map(String::as_str).eq(expected.iter().copied()) {
                    if found.is_some() || pending.is_some() {
                        return Err(invalid_shape_mutation(
                            "edit theme",
                            "duplicate selected theme owner",
                        ));
                    }
                    let mut attributes = Vec::new();
                    for attribute in start.attributes() {
                        let attribute = attribute.map_err(|error| {
                            invalid_shape_mutation("edit theme", error.to_string())
                        })?;
                        let raw_start: &[u8] = start.as_ref();
                        let offset =
                            attribute.value.as_ptr() as usize - raw_start.as_ptr() as usize;
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                            .map_err(|error| {
                                invalid_shape_mutation("edit theme", error.to_string())
                            })?
                            .into_owned();
                        attributes.push((
                            String::from_utf8_lossy(attribute.key.as_ref()).into_owned(),
                            value,
                            (
                                begin + 1 + offset,
                                begin + 1 + offset + attribute.value.len(),
                            ),
                        ));
                    }
                    let owner = ThemeSourceOwner {
                        start: begin,
                        end: finish,
                        empty,
                        attributes,
                    };
                    if empty {
                        found = Some(owner);
                    } else {
                        pending = Some((path.len(), owner));
                    }
                }
                if empty {
                    path.pop();
                }
            }
            XmlEvent::End(_) => {
                if pending
                    .as_ref()
                    .is_some_and(|(depth, _)| *depth == path.len())
                {
                    let (_, mut owner) = pending.take().expect("selected owner has depth");
                    owner.end = finish;
                    found = Some(owner);
                }
                path.pop();
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(found)
}

/// Closes a copied theme scheme with its actual inherited namespace bindings.
fn portable_theme_scheme(source: &[u8], name: &str) -> Result<Vec<u8>> {
    let owner =
        theme_source_owner(source, &["theme", "themeElements", name])?.ok_or_else(|| {
            invalid_shape_mutation("apply theme", format!("qualified {name} is absent"))
        })?;
    portable_owned_fragment(source, owner.start, owner.end)
}

/// Carries actual namespace and compatibility scope for copied theme/background owners.
pub(super) fn portable_owned_fragment(
    source: &[u8],
    owner_start: usize,
    owner_end: usize,
) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(source);
    let mut buffer = Vec::new();
    let mut scopes: Vec<Vec<(String, String)>> = Vec::new();
    type CompatibilityPolicy = (String, String, Vec<(String, String)>);
    let mut policies: Vec<Vec<CompatibilityPolicy>> = Vec::new();
    loop {
        let begin = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| invalid_shape_mutation("apply theme", error.to_string()))?;
        let finish = reader.buffer_position() as usize;
        let empty = matches!(&event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                let mut inherited = scopes.last().cloned().unwrap_or_default();
                let mut local = HashSet::new();
                let inherited_policies = policies.last().cloned().unwrap_or_default();
                let mut local_policies = Vec::new();
                for attribute in start.attributes() {
                    let attribute = attribute.map_err(|error| {
                        invalid_shape_mutation("apply theme", error.to_string())
                    })?;
                    let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
                    let (namespace, policy_name) =
                        reader.resolver().resolve_attribute(attribute.key);
                    if xml_is(
                        &namespace,
                        "http://schemas.openxmlformats.org/markup-compatibility/2006",
                    ) {
                        if !matches!(
                            policy_name.as_ref(),
                            b"Ignorable"
                                | b"ProcessContent"
                                | b"PreserveElements"
                                | b"PreserveAttributes"
                                | b"MustUnderstand"
                        ) {
                            return Err(invalid_shape_mutation(
                                "apply theme",
                                "unsupported inherited markup compatibility policy",
                            ));
                        }
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                            .map_err(|error| {
                                invalid_shape_mutation("apply theme", error.to_string())
                            })?
                            .into_owned();
                        local_policies.push((key.clone(), value));
                    }
                    if key == "xmlns" || key.starts_with("xmlns:") {
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                            .map_err(|error| {
                                invalid_shape_mutation("apply theme", error.to_string())
                            })?
                            .into_owned();
                        inherited.retain(|(old, _)| old != &key);
                        inherited.push((key.clone(), value));
                        local.insert(key);
                    }
                }
                let mut effective_policies = inherited_policies.clone();
                for (key, value) in &local_policies {
                    let name = key.rsplit(':').next().unwrap_or(key);
                    if let Some((old_key, old_value, old_scope)) = effective_policies
                        .iter()
                        .find(|(old, _, _)| old.rsplit(':').next() == Some(name))
                    {
                        if old_key != key || old_value != value || old_scope != &inherited {
                            return Err(invalid_shape_mutation(
                                "apply theme",
                                "nested markup compatibility policy needs an unsupported merge",
                            ));
                        }
                    } else {
                        effective_policies.push((key.clone(), value.clone(), inherited.clone()));
                    }
                }
                if begin == owner_start {
                    let insertion = finish - if empty { 2 } else { 1 };
                    let mut result = source[owner_start..insertion].to_vec();
                    for (key, value, declaration_scope) in &inherited_policies {
                        let property = key.rsplit(':').next().unwrap_or(key);
                        let mut prefixes = vec![key.split(':').next().unwrap_or(key)];
                        for token in value.split_whitespace() {
                            prefixes.push(if matches!(property, "Ignorable" | "MustUnderstand") {
                                token
                            } else {
                                token.split_once(':').map(|(prefix, _)| prefix).ok_or_else(|| invalid_shape_mutation("apply theme", "unqualified inherited compatibility QName is unsupported"))?
                            });
                        }
                        for prefix in prefixes {
                            let declaration = format!("xmlns:{prefix}");
                            let before = declaration_scope
                                .iter()
                                .find(|(name, _)| name == &declaration)
                                .map(|(_, uri)| uri);
                            let after = inherited
                                .iter()
                                .find(|(name, _)| name == &declaration)
                                .map(|(_, uri)| uri);
                            if before.is_none() || before != after {
                                return Err(invalid_shape_mutation(
                                    "apply theme",
                                    "inherited compatibility prefix was rebound or undeclared",
                                ));
                            }
                        }
                        if !local_policies
                            .iter()
                            .any(|(local, _)| local.rsplit(':').next() == Some(property))
                        {
                            result.extend_from_slice(
                                format!(" {key}=\"{}\"", quick_xml::escape::escape(value))
                                    .as_bytes(),
                            );
                        }
                    }
                    for (key, value) in inherited {
                        if !local.contains(&key) {
                            result.extend_from_slice(
                                format!(" {key}=\"{}\"", quick_xml::escape::escape(&value))
                                    .as_bytes(),
                            );
                        }
                    }
                    result.extend_from_slice(&source[insertion..owner_end]);
                    return Ok(result);
                }
                if !empty {
                    scopes.push(inherited);
                    policies.push(effective_policies);
                }
            }
            XmlEvent::End(_) => {
                scopes.pop();
                policies.pop();
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Err(invalid_shape_mutation(
        "apply theme",
        "selected scheme source is incomplete",
    ))
}

/// Patches only owned Target values in a copied relationship scope.
fn retarget_import_relationships(
    source: &[u8],
    changes: &HashMap<String, String>,
) -> Result<Relationships> {
    let original = Relationships::from_xml(source)?;
    let mut reader = NsReader::from_reader(source);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut edits = Vec::new();
    let mut seen = HashSet::new();
    loop {
        let begin = reader.buffer_position() as usize;
        let (qualified, event) = {
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(OpcError::from)?;
            (
                xml_is(
                    &namespace,
                    "http://schemas.openxmlformats.org/package/2006/relationships",
                ),
                event,
            )
        };
        let empty = matches!(&event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                if depth == 1 && qualified && start.local_name().as_ref() == b"Relationship" {
                    let mut id = None;
                    let mut target = None;
                    for attribute in start.attributes() {
                        let attribute = attribute
                            .map_err(quick_xml::Error::from)
                            .map_err(OpcError::from)?;
                        if attribute.key.as_ref() == b"Id" {
                            id = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        start.decoder(),
                                    )
                                    .map_err(OpcError::from)?
                                    .into_owned(),
                            );
                        } else if attribute.key.as_ref() == b"Target" {
                            let raw: &[u8] = start.as_ref();
                            let offset = attribute.value.as_ptr() as usize - raw.as_ptr() as usize;
                            target = Some((
                                begin + 1 + offset,
                                begin + 1 + offset + attribute.value.len(),
                            ));
                        }
                    }
                    if let Some(id) = id
                        && let Some(changed) = changes.get(&id)
                    {
                        let range = target.ok_or_else(|| {
                            invalid_shape_mutation("import relationships", "owned Target is absent")
                        })?;
                        if !seen.insert(id) {
                            return Err(invalid_shape_mutation(
                                "import relationships",
                                "duplicate selected relationship",
                            ));
                        }
                        edits.push((range, changed));
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            XmlEvent::End(_) => {
                depth = depth.saturating_sub(1);
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if seen.len() != changes.len() {
        return Err(invalid_shape_mutation(
            "import relationships",
            "selected qualified relationship is absent",
        ));
    }
    edits.sort_by_key(|((begin, _), _)| std::cmp::Reverse(*begin));
    let mut result = source.to_vec();
    for ((begin, end), value) in edits {
        result.splice(begin..end, quick_xml::escape::escape(value).bytes());
    }
    let parsed = Relationships::from_xml(&result)?;
    let mut expected = original.items;
    for relationship in &mut expected {
        if let Some(target) = changes.get(&relationship.id) {
            relationship.target = target.clone();
        }
    }
    if parsed.items != expected {
        return Err(invalid_shape_mutation(
            "import relationships",
            "retarget changed unrelated relationship facts",
        ));
    }
    Ok(parsed)
}

/// Allocates fresh IDs in the selected imported master's existing list owners.
fn reassign_imported_layout_ids(source: &[u8], ids: &HashMap<String, u32>) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(source);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut list = false;
    let mut seen = HashSet::new();
    let mut edits = Vec::new();
    loop {
        let begin = reader.buffer_position() as usize;
        let (qualified, event) = {
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(OpcError::from)?;
            (
                xml_is(&namespace, oxoffice_pptx_oxml::namespace::P_NS),
                event,
            )
        };
        let empty = matches!(&event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                if depth == 0 && (!qualified || start.local_name().as_ref() != b"sldMaster") {
                    return Err(invalid_shape_mutation(
                        "import master",
                        "qualified master root is required",
                    ));
                }
                if depth == 1 && qualified && start.local_name().as_ref() == b"sldLayoutIdLst" {
                    list = true;
                }
                if depth == 2 && list && qualified && start.local_name().as_ref() == b"sldLayoutId"
                {
                    let mut relationship = None;
                    let mut range = None;
                    for attribute in start.attributes() {
                        let attribute = attribute
                            .map_err(quick_xml::Error::from)
                            .map_err(OpcError::from)?;
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        if local.as_ref() == b"id"
                            && xml_is(&namespace, oxoffice_pptx_oxml::namespace::R_NS)
                        {
                            relationship = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        start.decoder(),
                                    )
                                    .map_err(OpcError::from)?
                                    .into_owned(),
                            );
                        } else if attribute.key.as_ref() == b"id" {
                            let raw: &[u8] = start.as_ref();
                            let offset = attribute.value.as_ptr() as usize - raw.as_ptr() as usize;
                            range = Some((
                                begin + 1 + offset,
                                begin + 1 + offset + attribute.value.len(),
                            ));
                        }
                    }
                    let relationship = relationship.ok_or_else(|| {
                        invalid_shape_mutation("import master", "layout relationship ID is absent")
                    })?;
                    let id = ids.get(&relationship).ok_or_else(|| {
                        invalid_shape_mutation(
                            "import master",
                            "layout list has an unsupported route",
                        )
                    })?;
                    if !seen.insert(relationship) {
                        return Err(invalid_shape_mutation(
                            "import master",
                            "duplicate layout relationship",
                        ));
                    }
                    edits.push((
                        range.ok_or_else(|| {
                            invalid_shape_mutation("import master", "layout ID is absent")
                        })?,
                        id,
                    ));
                }
                if !empty {
                    depth += 1;
                }
            }
            XmlEvent::End(_) => {
                if depth == 2 {
                    list = false;
                }
                depth = depth.saturating_sub(1);
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if seen.len() != ids.len() {
        return Err(invalid_shape_mutation(
            "import master",
            "qualified layout inventory is incomplete",
        ));
    }
    edits.sort_by_key(|((begin, _), _)| std::cmp::Reverse(*begin));
    let mut result = source.to_vec();
    for ((begin, end), id) in edits {
        result.splice(begin..end, id.to_string().bytes());
    }
    CT_SlideMaster::from_xml(&result)
        .map_err(|error| invalid_shape_mutation("import master", error.to_string()))?;
    Ok(result)
}

/// Adds a presentation master ID without rewriting unrelated presentation children.
fn append_imported_master_id(source: &[u8], id: u32, relationship: &str) -> Result<Vec<u8>> {
    let (children, root_end) =
        property_child_ranges(source, oxoffice_pptx_oxml::namespace::P_NS, "presentation")?;
    let lists = children
        .iter()
        .filter(|(uri, name, _, _)| {
            uri.as_deref() == Some(oxoffice_pptx_oxml::namespace::P_NS) && name == "sldMasterIdLst"
        })
        .collect::<Vec<_>>();
    if lists.len() > 1 {
        return Err(invalid_shape_mutation(
            "import master",
            "duplicate master ID lists",
        ));
    }
    let item = format!(
        "<p:sldMasterId xmlns:p=\"{}\" xmlns:r=\"{}\" id=\"{id}\" r:id=\"{}\"/>",
        oxoffice_pptx_oxml::namespace::P_NS,
        oxoffice_pptx_oxml::namespace::R_NS,
        quick_xml::escape::escape(relationship)
    );
    let mut result = source.to_vec();
    if let Some((_, _, begin, end)) = lists.first().copied() {
        let mut reader = NsReader::from_reader(&source[*begin..*end]);
        let mut buffer = Vec::new();
        // Read the physical token only. Its qualified ownership was proved in the parent scope above.
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(OpcError::from)?;
        let head_end = *begin + reader.buffer_position() as usize;
        match event {
            XmlEvent::Empty(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                let expanded = format!(">{item}</{name}>");
                result.splice(head_end - 2..head_end, expanded.bytes());
            }
            XmlEvent::Start(_) => {
                let close = source[*begin..*end]
                    .windows(2)
                    .rposition(|token| token == b"</")
                    .ok_or_else(|| {
                        invalid_shape_mutation("import master", "master list is incomplete")
                    })?
                    + *begin;
                result.splice(close..close, item.bytes());
            }
            _ => {
                return Err(invalid_shape_mutation(
                    "import master",
                    "master list root is unsupported",
                ));
            }
        }
    } else {
        let insertion = children
            .iter()
            .find(|(uri, _, _, _)| uri.as_deref() == Some(oxoffice_pptx_oxml::namespace::P_NS))
            .map(|(_, _, begin, _)| *begin)
            .unwrap_or(root_end);
        let list = format!(
            "<p:sldMasterIdLst xmlns:p=\"{}\">{item}</p:sldMasterIdLst>",
            oxoffice_pptx_oxml::namespace::P_NS
        );
        result.splice(insertion..insertion, list.bytes());
    }
    CT_Presentation::from_xml(&result)
        .map_err(|error| invalid_shape_mutation("import master", error.to_string()))?;
    Ok(result)
}

/// Changes one owned master layout relationship without rewriting retained source.
pub(super) fn edit_master_layout_relationship(
    source: &[u8],
    selected: &Relationship,
    remove: bool,
) -> Result<Relationships> {
    const OPC_NS: &[u8] = b"http://schemas.openxmlformats.org/package/2006/relationships";
    let original = Relationships::from_xml(source)?;
    if !matches!(
        selected.rel_type.as_str(),
        rel_types::SLIDE_LAYOUT | rel_types::SLIDE_MASTER
    ) || relationship_is_external(selected)
        || original.get_by_id(&selected.id).is_some() != remove
    {
        return Err(invalid_shape_mutation(
            "layout relationship",
            "selected ownership does not match source",
        ));
    }
    let mut reader = quick_xml::NsReader::from_reader(source);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut root = None;
    let mut close = None;
    let mut selected_range = None;
    let mut pending = None;
    loop {
        let begin = reader.buffer_position() as usize;
        let (qualified, event) = {
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(OpcError::from)?;
            (
                matches!(namespace, quick_xml::name::ResolveResult::Bound(value) if value.as_ref() == OPC_NS),
                event,
            )
        };
        let finish = reader.buffer_position() as usize;
        let empty = matches!(&event, quick_xml::events::Event::Empty(_));
        match event {
            quick_xml::events::Event::Start(start) | quick_xml::events::Event::Empty(start) => {
                if depth == 0 {
                    if !qualified
                        || start.local_name().as_ref() != b"Relationships"
                        || empty
                        || root.replace(()).is_some()
                    {
                        return Err(invalid_shape_mutation(
                            "layout relationship",
                            "unsupported relationship root",
                        ));
                    }
                } else if depth == 1 && qualified && start.local_name().as_ref() == b"Relationship"
                {
                    let mut id = None;
                    for attribute in start.attributes() {
                        let attribute = attribute
                            .map_err(quick_xml::Error::from)
                            .map_err(OpcError::from)?;
                        if attribute.key.as_ref() == b"Id" {
                            id = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        quick_xml::XmlVersion::Implicit1_0,
                                        start.decoder(),
                                    )
                                    .map_err(OpcError::from)?
                                    .into_owned(),
                            );
                        }
                    }
                    if id.as_deref() == Some(selected.id.as_str()) {
                        if empty {
                            selected_range = Some((begin, finish));
                        } else {
                            pending = Some(begin);
                        }
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            quick_xml::events::Event::End(_) => {
                if depth == 2
                    && let Some(begin) = pending.take()
                {
                    selected_range = Some((begin, finish));
                }
                if depth == 1 {
                    close = Some(begin);
                }
                depth = depth.saturating_sub(1);
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut result = source.to_vec();
    if remove {
        let (begin, finish) = selected_range.ok_or_else(|| {
            invalid_shape_mutation(
                "layout relationship",
                "selected qualified relationship is absent",
            )
        })?;
        result.drain(begin..finish);
    } else {
        let insertion = close.ok_or_else(|| {
            invalid_shape_mutation("layout relationship", "relationship root is incomplete")
        })?;
        let added = format!(
            "<Relationship xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\" Id=\"{}\" Type=\"{}\" Target=\"{}\"/>",
            quick_xml::escape::escape(&selected.id),
            quick_xml::escape::escape(&selected.rel_type),
            quick_xml::escape::escape(&selected.target)
        );
        result.splice(insertion..insertion, added.bytes());
    }
    let parsed = Relationships::from_xml(&result)?;
    let mut expected = original.items;
    if remove {
        expected.retain(|relationship| relationship.id != selected.id);
    } else {
        expected.push(selected.clone());
    }
    if parsed.items != expected {
        return Err(invalid_shape_mutation(
            "layout relationship",
            "source splice changed unrelated relationships",
        ));
    }
    Ok(parsed)
}

fn clear_theme_color_transforms(source: &[u8], slot: &str) -> Result<Vec<u8>> {
    const TRANSFORMS: &[&str] = &[
        "tint", "shade", "comp", "inv", "gray", "alpha", "alphaOff", "alphaMod", "hue", "hueOff",
        "hueMod", "sat", "satOff", "satMod", "lum", "lumOff", "lumMod", "red", "redOff", "redMod",
        "green", "greenOff", "greenMod", "blue", "blueOff", "blueMod", "gamma", "invGamma",
    ];
    let mut reader = NsReader::from_reader(source);
    let mut buffer = Vec::new();
    let mut path = Vec::new();
    let mut pending = None;
    let mut ranges = Vec::new();
    loop {
        let begin = reader.buffer_position() as usize;
        let (qualified, event) = {
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| invalid_shape_mutation("edit theme color", error.to_string()))?;
            (xml_is(&namespace, A_NS), event)
        };
        let finish = reader.buffer_position() as usize;
        let empty = matches!(&event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                if pending.is_some() {
                    return Err(invalid_shape_mutation(
                        "edit theme color",
                        "color transform contains opaque child payload",
                    ));
                }
                let local =
                    String::from_utf8_lossy(xml_local_name(start.name().as_ref())).into_owned();
                if path.iter().map(String::as_str).eq([
                    "theme",
                    "themeElements",
                    "clrScheme",
                    slot,
                    "srgbClr",
                ]) && TRANSFORMS.contains(&local.as_str())
                {
                    if !qualified {
                        return Err(invalid_shape_mutation(
                            "edit theme color",
                            "foreign color transform lookalike",
                        ));
                    }
                    for attribute in start.attributes() {
                        let attribute = attribute.map_err(|error| {
                            invalid_shape_mutation("edit theme color", error.to_string())
                        })?;
                        let name = attribute.key.as_ref();
                        if name != b"val" && name != b"xmlns" && !name.starts_with(b"xmlns:") {
                            return Err(invalid_shape_mutation(
                                "edit theme color",
                                "color transform contains opaque attributes",
                            ));
                        }
                    }
                    if empty {
                        ranges.push((begin, finish));
                    } else {
                        pending = Some((path.len() + 1, begin));
                    }
                }
                path.push(if qualified { local } else { String::new() });
                if empty {
                    path.pop();
                }
            }
            XmlEvent::End(_) => {
                if pending
                    .as_ref()
                    .is_some_and(|(depth, _)| *depth == path.len())
                {
                    let (_, begin) = pending.take().expect("color transform depth exists");
                    ranges.push((begin, finish));
                }
                path.pop();
            }
            XmlEvent::Text(text)
                if pending.is_some() && text.iter().all(u8::is_ascii_whitespace) => {}
            XmlEvent::Eof => break,
            _ if pending.is_some() => {
                return Err(invalid_shape_mutation(
                    "edit theme color",
                    "color transform contains opaque body payload",
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
    let mut result = Vec::new();
    let mut cursor = 0;
    for (begin, end) in ranges {
        result.extend_from_slice(&source[cursor..begin]);
        cursor = end;
    }
    result.extend_from_slice(&source[cursor..]);
    Ok(result)
}

fn patch_theme_attribute(
    source: &[u8],
    owner: &ThemeSourceOwner,
    attribute: &str,
    value: &str,
) -> Result<Vec<u8>> {
    let (_, original, (begin, end)) = owner
        .attributes
        .iter()
        .find(|(name, _, _)| name == attribute)
        .ok_or_else(|| {
            invalid_shape_mutation("edit theme", "selected schema attribute is absent")
        })?;
    if original == value {
        return Ok(source.to_vec());
    }
    let escaped = escape_attribute_value(value);
    let mut result = source[..*begin].to_vec();
    result.extend_from_slice(escaped.as_bytes());
    result.extend_from_slice(&source[*end..]);
    Ok(result)
}
