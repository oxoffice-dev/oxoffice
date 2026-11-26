//! Slide masters and layouts: master text styles, backgrounds, layout lists, duplicating and removing layouts, a slide's layout, and the placeholder geometry slides inherit.

use super::*;

impl Presentation {
    /// Returns the current master model, retaining an unprojectable source as an error.
    pub fn master(&self, index: usize) -> Option<&CT_SlideMaster> {
        self.masters.get(index)?.master.as_ref().ok()
    }

    pub fn master_count(&self) -> usize {
        self.masters.len()
    }

    /// Returns the actual optional producer master ID.
    pub fn master_id(&self, index: usize) -> Option<u32> {
        self.masters.get(index)?.id
    }

    pub fn layout(&self, index: usize) -> Option<&CT_SlideLayout> {
        self.layouts.get(index).map(|record| &record.layout)
    }
}

impl Presentation {
    /// Reads a direct master title/body/other paragraph level, numbered one through nine.
    pub fn master_text_style(
        &self,
        master_index: usize,
        style: &str,
        level: usize,
    ) -> Result<Option<&CT_TextParagraphProperties>> {
        if !(1..=9).contains(&level) {
            return Err(invalid_shape_mutation(
                "master text style",
                "level must be from 1 through 9",
            ));
        }
        let master = self.master(master_index).ok_or_else(|| {
            invalid_shape_mutation("master text style", "master owner is absent or malformed")
        })?;
        let styles = master.text_styles.as_ref();
        let list = match style {
            "title" => styles.map(|styles| &styles.title_style),
            "body" => styles.map(|styles| &styles.body_style),
            "other" => styles.map(|styles| &styles.other_style),
            _ => {
                return Err(invalid_shape_mutation(
                    "master text style",
                    "style must be title, body or other",
                ));
            }
        };
        Ok(list.and_then(|list| list.level(level)))
    }

    /// Changes only the selected qualified level source and validates before publication.
    pub fn set_master_text_style(
        &mut self,
        master_index: usize,
        style: &str,
        level: usize,
        properties: CT_TextParagraphProperties,
    ) -> Result<()> {
        let current = self
            .master_text_style(master_index, style, level)?
            .cloned()
            .unwrap_or_default();
        if current == properties {
            return Ok(());
        }
        let mut candidate = self.clone();
        candidate.package = self.staged_package(true)?;
        let part = self
            .master_part(master_index)?
            .ok_or_else(|| invalid_shape_mutation("master text style", "master owner is absent"))?;
        let source = required_part(&candidate.package, &part)?;
        let (children, root_end) =
            property_child_ranges(source, oxoffice_pptx_oxml::namespace::P_NS, "sldMaster")?;
        let styles = children
            .iter()
            .filter(|(uri, name, _, _)| {
                uri.as_deref() == Some(oxoffice_pptx_oxml::namespace::P_NS) && name == "txStyles"
            })
            .collect::<Vec<_>>();
        if styles.len() > 1 {
            return Err(footer_malformed(&part, "master has ambiguous text styles"));
        }
        let tag = format!("a:lvl{level}pPr");
        let mut writer = quick_xml::Writer::new(Vec::new());
        properties
            .write_xml(&mut writer, &tag)
            .map_err(|error| footer_malformed(&part, error.to_string()))?;
        let mut replacement = writer.into_inner();
        let insertion = oxoffice_core::xml_element::start_tag(&replacement)
            .map_err(|_| footer_malformed(&part, "level source has no start token"))?
            .attributes_end;
        let mut fragment_reader =
            oxoffice_core::xml_reader::Reader::from_reader(replacement.as_slice());
        let start = match fragment_reader.read_event().map_err(OpcError::from)? {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => start,
            _ => return Err(footer_malformed(&part, "level source is not an element")),
        };
        let mut has_a = false;
        for attribute in start.attributes() {
            let attribute = attribute
                .map_err(quick_xml::Error::from)
                .map_err(OpcError::from)?;
            if attribute.key.as_ref() == b"xmlns:a" {
                if attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                    .map_err(OpcError::from)?
                    .as_ref()
                    != A_NS
                {
                    return Err(footer_malformed(&part, "level canonical prefix is rebound"));
                }
                has_a = true;
            }
        }
        if !has_a {
            replacement.splice(insertion..insertion, format!(" xmlns:a=\"{A_NS}\"").bytes());
        }
        let style_tag = format!("{style}Style");
        let (begin, end, changed) = if let Some((_, _, begin, end)) = styles.first() {
            let mut styles_source = portable_owned_fragment(source, *begin, *end)?;
            let (style_children, _) = property_child_ranges(
                &styles_source,
                oxoffice_pptx_oxml::namespace::P_NS,
                "txStyles",
            )?;
            let selected = style_children
                .iter()
                .filter(|(uri, name, _, _)| {
                    uri.as_deref() == Some(oxoffice_pptx_oxml::namespace::P_NS)
                        && name == &style_tag
                })
                .collect::<Vec<_>>();
            if selected.len() != 1 {
                return Err(footer_malformed(
                    &part,
                    "selected master style is absent or ambiguous",
                ));
            }
            let (_, _, style_begin, style_end) = selected[0];
            let mut list_source =
                portable_owned_fragment(&styles_source, *style_begin, *style_end)?;
            let (levels, list_end) = property_child_ranges(
                &list_source,
                oxoffice_pptx_oxml::namespace::P_NS,
                &style_tag,
            )?;
            let level_tag = format!("lvl{level}pPr");
            let selected_levels = levels
                .iter()
                .filter(|(uri, name, _, _)| uri.as_deref() == Some(A_NS) && name == &level_tag)
                .collect::<Vec<_>>();
            if selected_levels.len() > 1 {
                return Err(footer_malformed(
                    &part,
                    "selected master level is ambiguous",
                ));
            }
            let range = if let Some((_, _, begin, end)) = selected_levels.first() {
                *begin..*end
            } else {
                let before = levels
                    .iter()
                    .find(|(uri, name, _, _)| {
                        uri.as_deref() == Some(A_NS)
                            && name
                                .strip_prefix("lvl")
                                .and_then(|name| name.strip_suffix("pPr"))
                                .and_then(|number| number.parse::<usize>().ok())
                                .is_some_and(|number| number > level)
                    })
                    .map(|(_, _, begin, _)| *begin)
                    .unwrap_or(list_end);
                before..before
            };
            list_source.splice(range, replacement);
            styles_source.splice(*style_begin..*style_end, list_source);
            (*begin, *end, styles_source)
        } else {
            let mut fragment = format!(
                "<p:txStyles xmlns:p=\"{}\" xmlns:a=\"{A_NS}\">",
                oxoffice_pptx_oxml::namespace::P_NS
            )
            .into_bytes();
            for slot in ["title", "body", "other"] {
                fragment.extend_from_slice(format!("<p:{slot}Style>").as_bytes());
                if slot == style {
                    fragment.extend_from_slice(&replacement);
                }
                fragment.extend_from_slice(format!("</p:{slot}Style>").as_bytes());
            }
            fragment.extend_from_slice(b"</p:txStyles>");
            let before = children
                .iter()
                .find(|(uri, name, _, _)| {
                    uri.as_deref() == Some(oxoffice_pptx_oxml::namespace::P_NS) && name == "extLst"
                })
                .map(|(_, _, begin, _)| *begin)
                .unwrap_or(root_end);
            (before, before, fragment)
        };
        let mut changed_source = source.to_vec();
        changed_source.splice(begin..end, changed);
        let master = CT_SlideMaster::from_xml(&changed_source)
            .map_err(|error| footer_malformed(&part, error.to_string()))?;
        candidate.package.set_part(&part, changed_source);
        Arc::make_mut(&mut candidate.masters[master_index]).master = Ok(master);
        Arc::make_mut(&mut candidate.masters[master_index]).dirty = false;
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// Reads only the directly owned background fill without materializing inheritance.
    pub fn background_fill_at(&self, owner: ShapeOwner) -> Result<Option<&Fill>> {
        Ok(self
            .owner_data(owner)?
            .background
            .as_ref()
            .and_then(|background| match background.rendering() {
                oxoffice_pptx_oxml::slide_parts::BackgroundRendering::Properties(fill) => {
                    fill.as_deref()
                }
                _ => None,
            }))
    }

    /// Reads whether the part carries its own background choice.
    pub fn has_explicit_background_at(&self, owner: ShapeOwner) -> Result<bool> {
        Ok(self.owner_data(owner)?.background.is_some())
    }

    /// Detaches the effective background into its actual owner, or restores inheritance.
    pub fn set_follow_master_background_at(
        &mut self,
        owner: ShapeOwner,
        follow: bool,
    ) -> Result<()> {
        self.owner_data(owner)?;
        if matches!(owner, ShapeOwner::Master(_)) {
            return Err(invalid_shape_mutation(
                "follow master background",
                "masters have no parent background",
            ));
        }
        let explicit = self.owner_data(owner)?.background.is_some();
        if follow == !explicit {
            return Ok(());
        }
        let mut candidate = self.clone();
        candidate.package = self.staged_package(true)?;
        if follow {
            candidate.owner_data_mut(owner)?.background = None;
        } else {
            let mut ancestors = Vec::new();
            let layout = match owner {
                ShapeOwner::Slide(index) => self.slide_layout_index(index),
                ShapeOwner::Layout(index) => Some(index),
                ShapeOwner::Master(_) => unreachable!(),
            };
            if let Some(index) = layout {
                if matches!(owner, ShapeOwner::Slide(_)) {
                    ancestors.push(ShapeOwner::Layout(index));
                }
                ancestors.push(ShapeOwner::Master(self.layout_master_index(index)?));
            }
            let inherited = ancestors.into_iter().find_map(|source_owner| {
                self.owner_data(source_owner)
                    .ok()?
                    .background
                    .as_ref()
                    .map(|background| (source_owner, background.clone()))
            });
            let background = if let Some((source_owner, background)) = inherited {
                let source_part = self.owner_part_name(source_owner)?;
                let source = required_part(&candidate.package, source_part)?;
                let raw = background.raw_xml();
                let positions = source
                    .windows(raw.len())
                    .enumerate()
                    .filter_map(|(index, bytes)| (bytes == raw).then_some(index))
                    .collect::<Vec<_>>();
                if positions.len() != 1 {
                    return Err(invalid_shape_mutation(
                        "follow master background",
                        "background source is absent or ambiguous",
                    ));
                }
                let mut portable =
                    portable_owned_fragment(source, positions[0], positions[0] + raw.len())?;
                let destination_part = self.owner_part_name(owner)?.to_owned();
                let mut relationships = candidate
                    .package
                    .get_part_rels(&destination_part)
                    .cloned()
                    .unwrap_or_default();
                let mut remapped = HashMap::new();
                for id in relationship_ids(&portable).map_err(|error| {
                    invalid_shape_mutation("follow master background", error.to_string())
                })? {
                    let relationship = candidate
                        .package
                        .get_part_rels(source_part)
                        .and_then(|relationships| relationships.get_by_id(&id))
                        .ok_or_else(|| {
                            footer_malformed(
                                source_part,
                                format!("background relationship {id} is absent"),
                            )
                        })?;
                    if relationship.rel_type != rel_types::IMAGE
                        || relationship_is_external(relationship)
                    {
                        return Err(footer_malformed(
                            source_part,
                            format!("unsupported background relationship {id}"),
                        ));
                    }
                    let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
                    required_part(&candidate.package, &target)?;
                    let existing = relationships
                        .items
                        .iter()
                        .find(|item| {
                            item.rel_type == rel_types::IMAGE
                                && !relationship_is_external(item)
                                && OpcPackage::resolve_rel_target(&destination_part, &item.target)
                                    == target
                        })
                        .map(|item| item.id.clone());
                    let fresh = existing.unwrap_or_else(|| {
                        relationships.add(
                            rel_types::IMAGE,
                            &relative_part_target(&destination_part, &target),
                        )
                    });
                    remapped.insert(id, fresh);
                }
                portable = rewrite_exact_rel_ids(&portable, &remapped).map_err(|error| {
                    invalid_shape_mutation("follow master background", error.to_string())
                })?;
                candidate
                    .package
                    .set_part_rels(&destination_part, relationships);
                oxoffice_pptx_oxml::slide_parts::CT_Background::from_xml(&portable).map_err(
                    |error| invalid_shape_mutation("follow master background", error.to_string()),
                )?
            } else {
                oxoffice_pptx_oxml::slide_parts::CT_Background::from_fill(Fill::NoFill(
                    NoFill::default(),
                ))
                .map_err(|error| {
                    invalid_shape_mutation("follow master background", error.to_string())
                })?
            };
            candidate.owner_data_mut(owner)?.background = Some(background);
        }
        let previous = self.owner_relationship_ids(owner)?;
        candidate.release_unused_owner_images(owner, &previous)?;
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// Changes an optional slide or layout visibility policy without changing master shapes.
    pub fn set_show_master_shapes_at(
        &mut self,
        owner: ShapeOwner,
        show: Option<bool>,
    ) -> Result<()> {
        self.owner_data(owner)?;
        let current = match owner {
            ShapeOwner::Slide(index) => self.slides[index].slide.show_master_shapes,
            ShapeOwner::Layout(index) => self.layouts[index].layout.show_master_shapes,
            ShapeOwner::Master(_) => {
                return Err(invalid_shape_mutation(
                    "show master shapes",
                    "masters have no showMasterSp policy",
                ));
            }
        };
        if current == show {
            return Ok(());
        }
        let mut candidate = self.clone();
        match owner {
            ShapeOwner::Slide(index) => {
                Arc::make_mut(&mut candidate.slides[index])
                    .slide
                    .show_master_shapes = show
            }
            ShapeOwner::Layout(index) => {
                Arc::make_mut(&mut candidate.layouts[index])
                    .layout
                    .show_master_shapes = show;
                Arc::make_mut(&mut candidate.layouts[index]).dirty = true;
            }
            ShapeOwner::Master(_) => unreachable!(),
        }
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// Reads optional local visibility, preserving absence versus an explicit flag.
    pub fn show_master_shapes_at(&self, owner: ShapeOwner) -> Result<Option<bool>> {
        self.owner_data(owner)?;
        match owner {
            ShapeOwner::Slide(index) => Ok(self.slides[index].slide.show_master_shapes),
            ShapeOwner::Layout(index) => Ok(self.layouts[index].layout.show_master_shapes),
            ShapeOwner::Master(_) => Err(invalid_shape_mutation(
                "show master shapes",
                "masters have no showMasterSp policy",
            )),
        }
    }
}

impl Presentation {
    pub(super) fn part_inherited_target_transform(
        &self,
        owner: ShapeOwner,
        target: &ShapeTreeChild,
        operation: &'static str,
    ) -> Result<CT_Transform2D> {
        if let ShapeOwner::Layout(index) = owner
            && let Some(placeholder) = shape_placeholder(target)
        {
            let (layout, master) = self.layout_and_master(index)?;
            if let Some(transform) = inherited_xfrm(placeholder, layout, &master)
                .filter(|transform| transform.offset.is_some() && transform.extent.is_some())
            {
                return Ok(transform.clone());
            }
        }
        Err(invalid_shape_mutation(
            operation,
            "the part shape has no position and size",
        ))
    }

    /// Materializes a placeholder through the actual part inheritance chain.
    pub fn materialize_owner_geometry(&mut self, owner: ShapeOwner, path: &[usize]) -> Result<()> {
        if let ShapeOwner::Slide(index) = owner {
            return self.materialize_geometry(index, path);
        }
        let target = self.part_shape_at(owner, path)?.child;
        let own = shape_transform(target);
        if own.is_some_and(|transform| transform.offset.is_some() && transform.extent.is_some()) {
            return Ok(());
        }
        if shape_placeholder(target).is_none() {
            return Ok(());
        }
        let inherited =
            self.part_inherited_target_transform(owner, target, "materialize geometry")?;
        let mut transform = own.cloned().unwrap_or_else(|| inherited.clone());
        transform.offset = transform.offset.or(inherited.offset);
        transform.extent = transform.extent.or(inherited.extent);
        let child = child_at_mut(&mut self.owner_data_mut(owner)?.shape_tree.children, path)
            .expect("the physical path was checked");
        *shape_mut(child).transform_mut("materialize geometry")? = transform;
        Ok(())
    }
}

impl Presentation {
    /// Changes only the layout's optional cSld name.
    pub fn set_layout_name(&mut self, index: usize, name: Option<&str>) -> Result<()> {
        self.owner_data(ShapeOwner::Layout(index))?;
        if self.layouts[index].layout.common_slide_data.name.as_deref() == name {
            return Ok(());
        }
        let mut candidate = self.clone();
        Arc::make_mut(&mut candidate.layouts[index])
            .layout
            .common_slide_data
            .name = name.map(str::to_owned);
        Arc::make_mut(&mut candidate.layouts[index]).dirty = true;
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }
}

impl Presentation {
    /// Resolves one master's part name, `Ok(None)` for an unknown index.
    pub(super) fn master_part(&self, master_index: usize) -> Result<Option<String>> {
        let Some(master) = self.presentation.slide_master_ids.get(master_index) else {
            return Ok(None);
        };
        let relationship = self
            .package
            .get_part_rels(&self.presentation_part)
            .and_then(|relationships| relationships.get_by_id(&master.relationship_id))
            .ok_or_else(|| Error::MissingRelationship {
                source_part: self.presentation_part.clone(),
                relationship_id: master.relationship_id.clone(),
            })?;
        if relationship.rel_type != rel_types::SLIDE_MASTER {
            return Err(Error::MalformedPart {
                part_name: self.presentation_part.clone(),
                message: "master relationship has the wrong type".into(),
            });
        }
        reject_external(&self.presentation_part, relationship)?;
        Ok(Some(OpcPackage::resolve_rel_target(
            &self.presentation_part,
            &relationship.target,
        )))
    }

    /// Resolves a master's qualified layout list in its producer order.
    pub fn master_layout_indices(&self, master_index: usize) -> Result<Vec<usize>> {
        let record = self.masters.get(master_index).ok_or_else(|| {
            invalid_shape_mutation("master layouts", "master index is out of range")
        })?;
        let master = record
            .master
            .as_ref()
            .map_err(|error| footer_malformed(&record.part_name, error))?;
        let relationships = self
            .package
            .get_part_rels(&record.part_name)
            .ok_or_else(|| {
                footer_malformed(&record.part_name, "master relationships are absent")
            })?;
        master
            .slide_layout_ids()
            .map_err(|error| footer_malformed(&record.part_name, error))?
            .into_iter()
            .map(|(_, id)| {
                let relationship = relationships.get_by_id(&id).ok_or_else(|| {
                    footer_malformed(&record.part_name, "master layout relationship is absent")
                })?;
                if relationship.rel_type != rel_types::SLIDE_LAYOUT
                    || relationship_is_external(relationship)
                {
                    return Err(footer_malformed(
                        &record.part_name,
                        "master layout relationship has unsafe type or mode",
                    ));
                }
                let part = OpcPackage::resolve_rel_target(&record.part_name, &relationship.target);
                let index = self
                    .layouts
                    .iter()
                    .position(|layout| layout.part_name == part)
                    .ok_or_else(|| {
                        footer_malformed(
                            &part,
                            "master layout is absent from the presentation inventory",
                        )
                    })?;
                if self.layout_master_part(index)?.0 != record.part_name {
                    return Err(footer_malformed(
                        &part,
                        "layout points to a different master",
                    ));
                }
                Ok(index)
            })
            .collect()
    }

    /// Returns layout indices in the selected master's source ID-list order.
    pub fn master_layouts(&self, master_index: usize) -> Option<Vec<usize>> {
        self.master_layout_indices(master_index).ok()
    }

    /// Duplicates one layout in its existing master, preserving its part-scoped IDs and safe closure.
    pub fn duplicate_layout(&mut self, index: usize) -> Result<usize> {
        let record = self.layouts.get(index).ok_or_else(|| {
            invalid_shape_mutation("duplicate layout", "layout index is out of range")
        })?;
        let master_index = self.layout_master_index(index)?;
        if !self.master_layout_indices(master_index)?.contains(&index) {
            return Err(invalid_shape_mutation(
                "duplicate layout",
                "layout is absent from its master's qualified ID list",
            ));
        }
        let source_package = self.staged_package(true)?;
        let mut relationships = source_package
            .get_part_rels(&record.part_name)
            .cloned()
            .unwrap_or_default();
        for relationship in &relationships.items {
            if relationship_is_external(relationship) {
                if relationship.rel_type != rel_types::HYPERLINK {
                    return Err(invalid_shape_mutation(
                        "duplicate layout",
                        "unsupported external relationship in selected layout",
                    ));
                }
            } else {
                let target =
                    OpcPackage::resolve_rel_target(&record.part_name, &relationship.target);
                match relationship.rel_type.as_str() {
                    rel_types::SLIDE_MASTER => {
                        required_part(&source_package, &target)?;
                    }
                    rel_types::IMAGE => preflight_internal_transfer_image(
                        &source_package,
                        &target,
                        "duplicate layout",
                    )?,
                    _ => {
                        return Err(invalid_shape_mutation(
                            "duplicate layout",
                            "unsupported selected layout dependency",
                        ));
                    }
                }
            }
        }
        let mut largest = 2_147_483_647u32;
        for master in &self.masters {
            for (id, _) in master
                .master
                .as_ref()
                .map_err(|error| footer_malformed(&master.part_name, error))?
                .slide_layout_ids()
                .map_err(|error| footer_malformed(&master.part_name, error))?
            {
                largest = largest.max(id);
            }
        }
        let id = largest.checked_add(1).ok_or_else(|| {
            invalid_shape_mutation("duplicate layout", "master layout IDs are exhausted")
        })?;
        let mut candidate = self.clone();
        let part = fresh_related_part_name(&candidate.package, &record.part_name);
        for relationship in &mut relationships.items {
            if !relationship_is_external(relationship) {
                relationship.target = relative_part_target(
                    &part,
                    &OpcPackage::resolve_rel_target(&record.part_name, &relationship.target),
                );
            }
        }
        candidate.package.set_part(
            &part,
            required_part(&source_package, &record.part_name)?.to_vec(),
        );
        candidate.package.set_part_rels(&part, relationships);
        let content_type = source_package
            .content_types
            .content_type_for(&record.part_name)
            .ok_or_else(|| footer_malformed(&record.part_name, "layout content type is absent"))?;
        candidate
            .package
            .content_types
            .add_override(&part, content_type);
        let master_part = candidate.masters[master_index].part_name.clone();
        let relationship_source = candidate
            .package
            .get_part_rels(&master_part)
            .ok_or_else(|| footer_malformed(&master_part, "master relationships are absent"))?
            .to_xml()?;
        let relationship_id = candidate
            .package
            .get_part_rels_mut(&master_part)
            .ok_or_else(|| footer_malformed(&master_part, "master relationships are absent"))?
            .add(
                rel_types::SLIDE_LAYOUT,
                &relative_part_target(&master_part, &part),
            );
        let added_relationship = candidate
            .package
            .get_part_rels(&master_part)
            .and_then(|relationships| relationships.get_by_id(&relationship_id))
            .expect("newly allocated master relationship")
            .clone();
        let edited_relationships =
            edit_master_layout_relationship(&relationship_source, &added_relationship, false)?;
        candidate
            .package
            .part_rels
            .insert(master_part.clone(), edited_relationships);
        let master_source = required_part(&source_package, &master_part)?;
        let edited_master = Arc::make_mut(&mut candidate.masters[master_index])
            .master
            .as_mut()
            .map_err(|error| footer_malformed(&master_part, error))?
            .append_slide_layout_id(master_source, id, &relationship_id)
            .map_err(|error| footer_malformed(&master_part, error))?;
        candidate.package.set_part(&master_part, edited_master);
        Arc::make_mut(&mut candidate.masters[master_index]).dirty = false;
        candidate.layouts.push(Arc::new(LayoutRecord {
            part_name: part.clone(),
            layout: record.layout.clone(),
            dirty: false,
        }));
        candidate.commit_owned_parts(None)?;
        let inserted = candidate
            .layouts
            .iter()
            .position(|layout| layout.part_name == part)
            .ok_or_else(|| {
                footer_malformed(&part, "duplicated layout was not admitted on reopen")
            })?;
        *self = candidate;
        Ok(inserted)
    }

    /// Removes only an unused layout whose complete inbound package graph is owned by its master.
    pub fn remove_layout(&mut self, index: usize) -> Result<()> {
        let record = self.layouts.get(index).ok_or_else(|| {
            invalid_shape_mutation("remove layout", "layout index is out of range")
        })?;
        let master_index = self.layout_master_index(index)?;
        let master_part = self.masters[master_index].part_name.clone();
        let ids = self.masters[master_index]
            .master
            .as_ref()
            .map_err(|error| footer_malformed(&master_part, error))?
            .slide_layout_ids()
            .map_err(|error| footer_malformed(&master_part, error))?;
        let mut selected_id = None;
        for (source, relationships) in &self.package.part_rels {
            for relationship in &relationships.items {
                if relationship_is_external(relationship)
                    || OpcPackage::resolve_rel_target(source, &relationship.target)
                        != record.part_name
                {
                    continue;
                }
                if source != &master_part
                    || relationship.rel_type != rel_types::SLIDE_LAYOUT
                    || !ids.iter().any(|(_, id)| id == &relationship.id)
                    || selected_id.replace(relationship.id.clone()).is_some()
                {
                    return Err(invalid_shape_mutation(
                        "remove layout",
                        "layout is used or has an unsupported inbound relationship",
                    ));
                }
            }
        }
        if self.package.package_rels.items.iter().any(|relationship| {
            !relationship_is_external(relationship)
                && OpcPackage::resolve_rel_target("/", &relationship.target) == record.part_name
        }) {
            return Err(invalid_shape_mutation(
                "remove layout",
                "layout has an unsupported package-root relationship",
            ));
        }
        let selected_id = selected_id.ok_or_else(|| {
            invalid_shape_mutation("remove layout", "qualified master ownership is absent")
        })?;
        let mut candidate = self.clone();
        let mut candidates = HashSet::new();
        collect_internal_targets(&candidate.package, &record.part_name, &mut candidates);
        let source_package = self.staged_package(true)?;
        let master_source = required_part(&source_package, &master_part)?;
        let edited_master = Arc::make_mut(&mut candidate.masters[master_index])
            .master
            .as_mut()
            .map_err(|error| footer_malformed(&master_part, error))?
            .remove_slide_layout_id(master_source, &selected_id)
            .map_err(|error| footer_malformed(&master_part, error))?;
        candidate.package.set_part(&master_part, edited_master);
        Arc::make_mut(&mut candidate.masters[master_index]).dirty = false;
        let relationships = candidate
            .package
            .get_part_rels(&master_part)
            .expect("checked master relationships");
        let selected_relationship = relationships
            .get_by_id(&selected_id)
            .expect("checked selected master relationship");
        let edited_relationships =
            edit_master_layout_relationship(&relationships.to_xml()?, selected_relationship, true)?;
        candidate
            .package
            .part_rels
            .insert(master_part.clone(), edited_relationships);
        candidate.package.remove_part(&record.part_name);
        candidate.package.remove_part_rels(&record.part_name);
        candidate
            .package
            .content_types
            .remove_override(&record.part_name);
        candidate.layouts.remove(index);
        prune_unreachable_parts(&mut candidate.package, &candidates);
        candidate.media_store = candidate.media_store.rescan(&candidate.package);
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// Resolves the unique current master of an existing layout.
    pub fn layout_master_index(&self, layout_index: usize) -> Result<usize> {
        if layout_index >= self.layouts.len() {
            return Err(invalid_shape_mutation(
                "layout master",
                "layout index is out of range",
            ));
        }
        let (part, _) = self.layout_master_part(layout_index)?;
        self.masters
            .iter()
            .position(|master| master.part_name == part)
            .ok_or_else(|| {
                footer_malformed(&part, "master is absent from the presentation inventory")
            })
    }

    /// Counts every current slide-layout relationship, including hidden slides.
    pub fn layout_used_by_slides(&self, layout_index: usize) -> Result<Vec<usize>> {
        let layout = self.layouts.get(layout_index).ok_or_else(|| {
            invalid_shape_mutation("layout usage", "layout index is out of range")
        })?;
        self.slides
            .iter()
            .enumerate()
            .filter_map(|(index, slide)| {
                match related_internal_part(
                    &self.package,
                    &slide.part_name,
                    rel_types::SLIDE_LAYOUT,
                ) {
                    Ok(Some(part)) if part == layout.part_name => Some(Ok(index)),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }
}

impl Presentation {
    /// Counts masters in presentation relationship order.
    pub fn slide_master_count(&self) -> usize {
        self.presentation.slide_master_ids.len()
    }

    /// Resolves one master identity in presentation relationship order.
    pub fn slide_master_part_name(&self, index: usize) -> Result<Option<String>> {
        self.master_part(index)
    }

    /// Returns the number of layouts reachable through presentation masters.
    pub fn layout_count(&self) -> usize {
        self.layouts.len()
    }

    /// Returns the originating package identity of a reachable layout.
    pub fn layout_part_name(&self, index: usize) -> Option<&str> {
        self.layouts
            .get(index)
            .map(|record| record.part_name.as_str())
    }

    /// Returns a layout's optional `p:cSld` name by zero-based index.
    pub fn layout_name(&self, index: usize) -> Option<&str> {
        self.layouts
            .get(index)
            .and_then(|record| record.layout.common_slide_data.name.as_deref())
    }

    /// Returns the zero-based index of the layout one slide uses.
    ///
    /// The slide's internal layout relationship must target a layout that the
    /// presentation masters reach.
    pub fn slide_layout_index(&self, slide_index: usize) -> Option<usize> {
        let slide_part = &self.slides.get(slide_index)?.part_name;
        let layout_part =
            related_internal_part(&self.package, slide_part, rel_types::SLIDE_LAYOUT).ok()??;
        self.layouts
            .iter()
            .position(|layout| layout.part_name.eq_ignore_ascii_case(&layout_part))
    }

    /// Returns one slide shape's effective offset and extent in EMU.
    ///
    /// `shape_path` holds the shape's index in the slide's shape tree, then
    /// its index within each enclosing group. The shape's own transform wins
    /// whole. A placeholder without one inherits the transform of its layout
    /// placeholder, then of that placeholder's master counterpart, as
    /// rendering resolves it. The result is `(left, top, width, height)`, and
    /// a missing offset reads as zero, as rendering draws it. `None` means
    /// the resolved transform has no extent, so the shape is not drawn.
    /// [`ShapeRef::position`] and [`ShapeRef::size`] keep reporting only the
    /// shape's own values. An empty `shape_path` or one that names no shape
    /// is [`Error::InvalidShapeMutation`], as a missing picture is for
    /// [`Self::picture_image`].
    pub fn effective_geometry(
        &self,
        slide_index: usize,
        shape_path: &[usize],
    ) -> Result<Option<(Emu, Emu, Emu, Emu)>> {
        self.effective_geometry_at(ShapeOwner::Slide(slide_index), shape_path)
    }

    /// Reads the current geometry of a physical slide, layout or master shape.
    pub fn effective_geometry_at(
        &self,
        owner: ShapeOwner,
        shape_path: &[usize],
    ) -> Result<Option<(Emu, Emu, Emu, Emu)>> {
        let child = self.part_shape_at(owner, shape_path)?.child;
        let own = shape_transform(child);
        let inherited = match (own, shape_placeholder(child)) {
            (None, Some(placeholder)) => match owner {
                ShapeOwner::Slide(slide_index) => {
                    self.inherited_transform(slide_index, placeholder)?
                }
                ShapeOwner::Layout(_) => Some(self.part_inherited_target_transform(
                    owner,
                    child,
                    "read effective geometry",
                )?),
                ShapeOwner::Master(_) => None,
            },
            _ => None,
        };
        Ok(own.or(inherited.as_ref()).and_then(|transform| {
            let offset = transform.offset.unwrap_or_default();
            let extent = transform.extent?;
            Some((offset.x, offset.y, extent.cx, extent.cy))
        }))
    }

    /// Writes a placeholder's inherited geometry onto the shape itself.
    ///
    /// `shape_path` is read as in [`Self::effective_geometry`]. A placeholder
    /// whose own transform lacks an offset or an extent takes the missing
    /// part from the transform it inherits, and one without a transform
    /// copies the inherited transform whole, rotation and flips included.
    /// [`ShapeMut::set_position`] or [`ShapeMut::set_size`] then changes one
    /// pair while the shape keeps the other, and [`ShapeMut::set_rotation`]
    /// changes the angle while it keeps both, where an incomplete transform
    /// would make rendering skip the shape. Shapes that are not placeholders,
    /// placeholders with a complete transform and placeholders that inherit
    /// none are left unchanged.
    pub fn materialize_geometry(&mut self, slide_index: usize, shape_path: &[usize]) -> Result<()> {
        const OPERATION: &str = "materialize geometry";
        let child = self.shape_at(slide_index, shape_path, OPERATION)?.child;
        let own = shape_transform(child);
        if own.is_some_and(|own| own.offset.is_some() && own.extent.is_some()) {
            return Ok(());
        }
        let Some(placeholder) = shape_placeholder(child) else {
            return Ok(());
        };
        let Some(inherited) = self.inherited_transform(slide_index, placeholder)? else {
            return Ok(());
        };
        let mut materialized = own.cloned().unwrap_or_else(|| inherited.clone());
        materialized.offset = materialized.offset.or(inherited.offset);
        materialized.extent = materialized.extent.or(inherited.extent);
        let mut indices = shape_path.iter();
        let mut shape = indices
            .next()
            .and_then(|index| slide_mut(self, slide_index).into_shape_mut(*index));
        for index in indices {
            shape = shape.and_then(|shape| shape.into_child_mut(*index));
        }
        let mut shape = shape.ok_or_else(|| {
            invalid_shape_mutation(
                OPERATION,
                format!("slide {slide_index} has no editable shape at path {shape_path:?}"),
            )
        })?;
        *shape.transform_mut(OPERATION)? = materialized;
        Ok(())
    }

    /// Changes the layout one slide uses, by zero-based layout index.
    ///
    /// The slide's layout relationship is retargeted, so its placeholders
    /// inherit from the new layout and that layout's master. A placeholder
    /// without a transform of its own that the new layout does not place
    /// first receives the transform it inherited from the old layout, so it
    /// stays where it was drawn. Placeholders of the new layout that the
    /// slide lacks are not added. The layout may belong to another master,
    /// whose theme and text styles the slide then follows. The change is
    /// staged and publishes only after the staged package reopens.
    pub fn set_slide_layout(&mut self, slide_index: usize, layout_index: usize) -> Result<()> {
        const OPERATION: &str = "set slide layout";
        self.require_slide_index(slide_index)?;
        if layout_index >= self.layouts.len() {
            return Err(Error::UnknownLayoutIndex {
                index: layout_index,
                layout_count: self.layouts.len(),
            });
        }
        let current = self.slide_layout_index(slide_index).ok_or_else(|| {
            invalid_slide_mutation(
                OPERATION,
                format!("slide {slide_index} has no layout reachable through the slide masters"),
            )
        })?;
        if current == layout_index {
            return Ok(());
        }
        let (old_layout, old_master) = self.layout_and_master(current)?;
        let (new_layout, new_master) = self.layout_and_master(layout_index)?;
        let mut staged = self.clone();
        keep_unplaced_placeholder_transforms(
            &mut Arc::make_mut(&mut staged.slides[slide_index])
                .slide
                .common_slide_data
                .shape_tree
                .children,
            (old_layout, &old_master),
            (new_layout, &new_master),
        );
        let slide_part = &staged.slides[slide_index].part_name;
        let target = relative_part_target(slide_part, &staged.layouts[layout_index].part_name);
        staged
            .package
            .get_part_rels_mut(slide_part)
            .and_then(|relationships| {
                relationships
                    .items
                    .iter_mut()
                    .find(|relationship| relationship.rel_type == rel_types::SLIDE_LAYOUT)
            })
            .ok_or_else(|| {
                invalid_slide_mutation(
                    OPERATION,
                    format!("{slide_part} has no layout relationship"),
                )
            })?
            .target = target;
        self.commit_candidate(staged)
    }

    pub(super) fn shape_at(
        &self,
        slide_index: usize,
        shape_path: &[usize],
        operation: &'static str,
    ) -> Result<ShapeRef<'_>> {
        self.require_slide_index(slide_index)?;
        let slide = slide_ref(&self.slides[slide_index]);
        let mut indices = shape_path.iter();
        let mut shape = indices.next().and_then(|index| slide.shape(*index));
        for index in indices {
            shape = shape.and_then(|shape| shape.child(*index));
        }
        shape.ok_or_else(|| {
            invalid_shape_mutation(
                operation,
                format!("slide {slide_index} has no shape at path {shape_path:?}"),
            )
        })
    }

    /// Returns the transform a slide placeholder inherits through its layout.
    pub(super) fn inherited_transform(
        &self,
        slide_index: usize,
        placeholder: &CT_Placeholder,
    ) -> Result<Option<CT_Transform2D>> {
        let Some(layout_index) = self.slide_layout_index(slide_index) else {
            return Ok(None);
        };
        let (layout, master) = self.layout_and_master(layout_index)?;
        Ok(inherited_xfrm(placeholder, layout, &master).cloned())
    }

    fn layout_and_master(&self, layout_index: usize) -> Result<(&CT_SlideLayout, CT_SlideMaster)> {
        let record = &self.layouts[layout_index];
        let master_part =
            related_internal_part(&self.package, &record.part_name, rel_types::SLIDE_MASTER)?
                .ok_or_else(|| Error::MalformedPart {
                    part_name: record.part_name.clone(),
                    message: "layout has no slide master relationship".to_owned(),
                })?;
        let master = self
            .masters
            .iter()
            .find(|record| record.part_name == master_part)
            .ok_or_else(|| Error::MalformedPart {
                part_name: master_part.clone(),
                message: "layout master is not in presentation inventory".into(),
            })?
            .master
            .clone()
            .map_err(|message| Error::MalformedPart {
                part_name: master_part,
                message,
            })?;
        Ok((&record.layout, master))
    }
}

/// Gives placeholders the new layout does not place the transform they inherited.
///
/// Only placeholders without a transform of their own are touched, and
/// only when the old layout chain supplied one.
fn keep_unplaced_placeholder_transforms(
    children: &mut [ShapeTreeChild],
    old: (&CT_SlideLayout, &CT_SlideMaster),
    new: (&CT_SlideLayout, &CT_SlideMaster),
) {
    for child in children {
        let (placeholder, transform) = match child {
            ShapeTreeChild::Shape(shape) => (
                shape.placeholder.as_ref(),
                &mut shape.shape_properties.transform,
            ),
            ShapeTreeChild::Picture(picture) => (
                picture.placeholder.as_ref(),
                &mut picture.shape_properties.transform,
            ),
            ShapeTreeChild::GroupShape(group) => {
                keep_unplaced_placeholder_transforms(&mut group.children, old, new);
                continue;
            }
            _ => continue,
        };
        if let Some(placeholder) = placeholder
            && transform.is_none()
            && inherited_xfrm(placeholder, new.0, new.1).is_none()
        {
            *transform = inherited_xfrm(placeholder, old.0, old.1).cloned();
        }
    }
}
