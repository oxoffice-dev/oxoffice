//! Hyperlinks and click actions: reading and setting the hyperlink or target slide of a shape or a run.

use super::*;

impl Presentation {
    /// Returns the target of one relationship of a slide, the address that a
    /// hyperlink naming this relationship id opens.
    ///
    /// Pass the `r:id` of an `a:hlinkClick`. An external hyperlink returns
    /// its URL, and an internal relationship, such as a jump to another
    /// slide, returns its stored relative target, as python-pptx `address`
    /// does. An unknown slide or id returns `None`.
    pub fn hyperlink_address(&self, slide_index: usize, relationship_id: &str) -> Option<&str> {
        let record = self.slides.get(slide_index)?;
        self.package
            .get_part_rels(&record.part_name)?
            .get_by_id(relationship_id)
            .map(|relationship| relationship.target.as_str())
    }

    /// Resolves the stored click address of one slide shape by its non-visual id.
    pub fn shape_hyperlink_address(
        &self,
        slide_index: usize,
        shape_id: u32,
    ) -> Result<Option<&str>> {
        self.shape_hyperlink_address_at(ShapeOwner::Slide(slide_index), shape_id)
    }

    /// Resolves the click address in the actual physical owner's relationship scope.
    pub fn shape_hyperlink_address_at(
        &self,
        owner: ShapeOwner,
        shape_id: u32,
    ) -> Result<Option<&str>> {
        let (_, location) = self.locate_shape_click("shape hyperlink", owner, shape_id)?;
        let part = self.owner_part_name(owner)?;
        Ok(location
            .relationship_id
            .as_deref()
            .and_then(|id| self.package.get_part_rels(part)?.get_by_id(id))
            .map(|relationship| relationship.target.as_str()))
    }

    /// Returns the zero-based index of the slide that the click action of one
    /// slide shape jumps to, as python-pptx `click_action.target_slide` does.
    ///
    /// The shape is found by its non-visual id as for
    /// [`Self::shape_hyperlink_address`]. A named slide jump,
    /// `ppaction://hlinksldjump`, returns the slide its relationship targets.
    /// A `ppaction://hlinkshowjump` to the first, last, next, or previous
    /// slide returns that slide, counted from `slide_index`. Any other click
    /// action, or none, returns `None`, and so does a jump that names no slide
    /// of the presentation, such as a next-slide jump on the last slide, where
    /// python-pptx raises `ValueError`.
    pub fn shape_target_slide(&self, slide_index: usize, shape_id: u32) -> Result<Option<usize>> {
        let (_, location) = self.locate_shape_click(
            "shape target slide",
            ShapeOwner::Slide(slide_index),
            shape_id,
        )?;
        Ok(self.click_target_slide(
            slide_index,
            location.action.as_deref(),
            location.relationship_id.as_deref(),
        ))
    }

    fn click_target_slide(
        &self,
        slide_index: usize,
        action: Option<&str>,
        relationship_id: Option<&str>,
    ) -> Option<usize> {
        let action = action?;
        let (verb, query) = action.split_once('?').unwrap_or((action, ""));
        match verb {
            SLIDE_JUMP_ACTION => {
                let part_name = &self.slides[slide_index].part_name;
                relationship_id
                    .and_then(|id| self.package.get_part_rels(part_name)?.get_by_id(id))
                    .filter(|relationship| {
                        relationship.rel_type == rel_types::SLIDE
                            && !relationship_is_external(relationship)
                    })
                    .and_then(|relationship| {
                        let target =
                            OpcPackage::resolve_rel_target(part_name, &relationship.target);
                        self.slides
                            .iter()
                            .position(|slide| slide.part_name == target)
                    })
            }
            SHOW_JUMP_ACTION => {
                match query
                    .split('&')
                    .find_map(|field| field.strip_prefix("jump="))
                {
                    Some("firstslide") => Some(0),
                    Some("lastslide") => self.slides.len().checked_sub(1),
                    Some("nextslide") => {
                        Some(slide_index + 1).filter(|next| *next < self.slides.len())
                    }
                    Some("previousslide") => slide_index.checked_sub(1),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Sets or clears the external click hyperlink of one slide shape atomically.
    ///
    /// A new address replaces any other click action of the shape, such as a
    /// slide jump, and keeps the other attributes and children of an existing
    /// `a:hlinkClick`.
    pub fn set_shape_hyperlink(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        address: Option<&str>,
    ) -> Result<()> {
        self.set_shape_hyperlink_at(ShapeOwner::Slide(slide_index), shape_id, address)
    }

    /// Changes one physical owner click address without modifying other source parts.
    pub fn set_shape_hyperlink_at(
        &mut self,
        owner: ShapeOwner,
        shape_id: u32,
        address: Option<&str>,
    ) -> Result<()> {
        const OPERATION: &str = "set shape hyperlink";
        self.owner_data(owner)?;
        if address.is_some_and(|address| {
            address.is_empty()
                || address.chars().any(|character| {
                    character.is_control() || matches!(character, '\u{FFFE}' | '\u{FFFF}')
                })
        }) {
            return Err(invalid_shape_mutation(
                OPERATION,
                "a hyperlink address must be non-empty text without control characters",
            ));
        }
        self.set_shape_click_action(
            OPERATION,
            owner,
            shape_id,
            address.map(ShapeClickTarget::Address),
        )
    }

    /// Makes the click action of one slide shape jump to the slide at
    /// `target_slide_index`, or clears it with `None`, as python-pptx
    /// `click_action.target_slide` does.
    ///
    /// The shape is found as for [`Self::set_shape_hyperlink`]. Its
    /// `a:hlinkClick` gets the action `ppaction://hlinksldjump` and names the
    /// slide's relationship to the target slide, which is reused when the
    /// slide has one. Jumping to the current target changes nothing, and a
    /// relationship that only the old click action named is removed.
    pub fn set_shape_target_slide(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        target_slide_index: Option<usize>,
    ) -> Result<()> {
        if let Some(target) = target_slide_index {
            self.require_slide_index(target)?;
        }
        self.set_shape_click_action(
            "set shape target slide",
            ShapeOwner::Slide(slide_index),
            shape_id,
            target_slide_index.map(ShapeClickTarget::Slide),
        )
    }

    /// Serializes one slide and finds the `p:cNvPr` of its single child or
    /// group member whose id is `shape_id`, with its click action.
    fn locate_shape_click(
        &self,
        operation: &'static str,
        owner: ShapeOwner,
        shape_id: u32,
    ) -> Result<(Vec<u8>, ShapeHyperlinkLocation)> {
        self.owner_data(owner)?;
        if shape_id_count(&self.owner_data(owner)?.shape_tree.children, shape_id) != 1 {
            return Err(invalid_shape_mutation(
                operation,
                "shape id is missing or ambiguous",
            ));
        }
        let xml = match owner {
            ShapeOwner::Slide(index) => self.slides[index].slide.to_xml(),
            ShapeOwner::Layout(index) => self.layouts[index].layout.to_xml(),
            ShapeOwner::Master(index) => self.masters[index]
                .master
                .as_ref()
                .expect("checked owner")
                .to_xml(),
        }
        .map_err(|error| invalid_shape_mutation(operation, error.to_string()))?;
        let location = locate_shape_hyperlink(&xml, shape_id)?
            .ok_or_else(|| invalid_shape_mutation(operation, "shape has no p:cNvPr"))?;
        Ok((xml, location))
    }

    fn set_shape_click_action(
        &mut self,
        operation: &'static str,
        owner: ShapeOwner,
        shape_id: u32,
        target: Option<ShapeClickTarget<'_>>,
    ) -> Result<()> {
        let (xml, location) = self.locate_shape_click(operation, owner, shape_id)?;
        let part_name = self.owner_part_name(owner)?.to_owned();
        let mut relationships = self
            .package
            .get_part_rels(&part_name)
            .cloned()
            .unwrap_or_default();
        let current = location
            .relationship_id
            .as_deref()
            .and_then(|id| relationships.get_by_id(id));
        let action = location.action.as_deref();
        let link = match target {
            None if location.click.is_none() => return Ok(()),
            None => None,
            Some(ShapeClickTarget::Address(address)) => {
                if action.is_none()
                    && current.is_some_and(|relationship| {
                        relationship.rel_type == rel_types::HYPERLINK
                            && relationship_is_external(relationship)
                            && relationship.target == address
                    })
                {
                    return Ok(());
                }
                let id = relationships
                    .items
                    .iter()
                    .find(|relationship| {
                        relationship.rel_type == rel_types::HYPERLINK
                            && relationship_is_external(relationship)
                            && relationship.target == address
                    })
                    .map(|relationship| relationship.id.clone())
                    .unwrap_or_else(|| relationships.add_external(rel_types::HYPERLINK, address));
                Some((id, None))
            }
            Some(ShapeClickTarget::Slide(index)) => {
                let target_part = &self.slides[index].part_name;
                let is_target = |relationship: &Relationship| {
                    relationship.rel_type == rel_types::SLIDE
                        && !relationship_is_external(relationship)
                        && OpcPackage::resolve_rel_target(&part_name, &relationship.target)
                            == *target_part
                };
                if action == Some(SLIDE_JUMP_ACTION) && current.is_some_and(is_target) {
                    return Ok(());
                }
                let id = relationships
                    .items
                    .iter()
                    .find(|relationship| is_target(relationship))
                    .map(|relationship| relationship.id.clone())
                    .unwrap_or_else(|| {
                        relationships.add(
                            rel_types::SLIDE,
                            &relative_part_target(&part_name, target_part),
                        )
                    });
                Some((id, Some(SLIDE_JUMP_ACTION)))
            }
        };
        let rewritten = rewrite_shape_hyperlink(
            &xml,
            &location,
            link.as_ref().map(|(id, action)| (id.as_str(), *action)),
        )?;
        let before = relationship_ids(&xml)
            .map_err(|error| invalid_shape_mutation(operation, error.to_string()))?;
        let after = relationship_ids(&rewritten)
            .map_err(|error| invalid_shape_mutation(operation, error.to_string()))?;
        let removed_parts = relationships
            .items
            .iter()
            .filter(|relationship| {
                before.contains(&relationship.id)
                    && !after.contains(&relationship.id)
                    && !relationship_is_external(relationship)
            })
            .map(|relationship| OpcPackage::resolve_rel_target(&part_name, &relationship.target))
            .collect::<HashSet<_>>();
        relationships.items.retain(|relationship| {
            !before.contains(&relationship.id) || after.contains(&relationship.id)
        });
        let mut candidate = self.clone();
        candidate.package = self.staged_package(true)?;
        match owner {
            ShapeOwner::Slide(index) => {
                Arc::make_mut(&mut candidate.slides[index]).slide =
                    CT_Slide::from_xml(&rewritten)
                        .map_err(|error| invalid_shape_mutation(operation, error.to_string()))?
            }
            ShapeOwner::Layout(index) => {
                Arc::make_mut(&mut candidate.layouts[index]).layout =
                    CT_SlideLayout::from_xml(&rewritten)
                        .map_err(|error| invalid_shape_mutation(operation, error.to_string()))?;
                Arc::make_mut(&mut candidate.layouts[index]).dirty = false;
            }
            ShapeOwner::Master(index) => {
                Arc::make_mut(&mut candidate.masters[index]).master =
                    Ok(CT_SlideMaster::from_xml(&rewritten)
                        .map_err(|error| invalid_shape_mutation(operation, error.to_string()))?);
                Arc::make_mut(&mut candidate.masters[index]).dirty = false;
            }
        }
        candidate.package.set_part(&part_name, rewritten);
        candidate.package.set_part_rels(&part_name, relationships);
        prune_unreachable_parts(&mut candidate.package, &removed_parts);
        candidate.media_store = candidate.media_store.rescan(&candidate.package);
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// Points one text run's click hyperlink at an external `address`, or
    /// removes it with `None`, as python-pptx `hyperlink.address` does.
    ///
    /// The run is the zero-based regular run of a paragraph in the ordinary
    /// shape whose `p:cNvPr/@id` is `shape_id`, inside groups too. It gets an
    /// `a:hlinkClick` naming the slide's external hyperlink relationship to
    /// `address`, which is reused when the slide has one. Assigning the
    /// current address with no action changes nothing. Retargeting preserves
    /// the other click attributes and opaque children. Clearing removes the
    /// click, and relationships it alone referenced are pruned. An empty address, one with a
    /// control character, or a shape id that is missing, shared, or not an
    /// ordinary shape with that run is rejected without change.
    pub fn set_run_hyperlink(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        paragraph_index: usize,
        run_index: usize,
        address: Option<&str>,
    ) -> Result<()> {
        const OPERATION: &str = "set run hyperlink";
        self.require_slide_index(slide_index)?;
        if address.is_some_and(|address| {
            address.is_empty()
                || address.chars().any(|character| {
                    character.is_control() || matches!(character, '\u{FFFE}' | '\u{FFFF}')
                })
        }) {
            return Err(invalid_shape_mutation(
                OPERATION,
                "a hyperlink address must be non-empty text without control characters",
            ));
        }
        self.set_run_click(
            OPERATION,
            slide_index,
            (shape_id, paragraph_index, run_index),
            RunClickHost::Shape,
            address.map(ShapeClickTarget::Address),
        )
    }

    /// Points one table-cell run's click hyperlink at an external `address`.
    ///
    /// The relationship is the same slide-scoped external hyperlink a text-box
    /// run uses. An empty address, a control character, or a missing cell run
    /// is rejected before the slide or its relationships are published.
    pub fn set_cell_run_hyperlink(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        cell: (usize, usize),
        paragraph_index: usize,
        run_index: usize,
        address: Option<&str>,
    ) -> Result<()> {
        const OPERATION: &str = "set cell run hyperlink";
        self.require_slide_index(slide_index)?;
        if address.is_some_and(|address| {
            address.is_empty()
                || address.chars().any(|character| {
                    character.is_control() || matches!(character, '\u{FFFE}' | '\u{FFFF}')
                })
        }) {
            return Err(invalid_shape_mutation(
                OPERATION,
                "a hyperlink address must be non-empty text without control characters",
            ));
        }
        self.set_run_click(
            OPERATION,
            slide_index,
            (shape_id, paragraph_index, run_index),
            RunClickHost::Cell {
                row: cell.0,
                column: cell.1,
            },
            address.map(ShapeClickTarget::Address),
        )
    }

    /// Sets or clears one regular run named slide jump after validating both owners.
    pub fn set_run_target_slide(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        paragraph_index: usize,
        run_index: usize,
        target_slide_index: Option<usize>,
    ) -> Result<()> {
        self.require_slide_index(slide_index)?;
        if let Some(target) = target_slide_index {
            self.require_slide_index(target)?;
        }
        self.set_run_click(
            "set run target slide",
            slide_index,
            (shape_id, paragraph_index, run_index),
            RunClickHost::Shape,
            target_slide_index.map(ShapeClickTarget::Slide),
        )
    }

    /// Returns a regular run slide target after validating the unique physical shape.
    pub fn run_target_slide(
        &self,
        slide_index: usize,
        shape_id: u32,
        paragraph_index: usize,
        run_index: usize,
    ) -> Result<Option<usize>> {
        self.require_slide_index(slide_index)?;
        if shape_id_count(
            &self.slides[slide_index]
                .slide
                .common_slide_data
                .shape_tree
                .children,
            shape_id,
        ) != 1
        {
            return Err(invalid_shape_mutation(
                "read run target slide",
                "shape id is missing or ambiguous",
            ));
        }
        let missing = || {
            invalid_shape_mutation(
                "read run target slide",
                format!(
                    "shape id {shape_id} has no ordinary shape run {run_index} in paragraph {paragraph_index}"
                ),
            )
        };
        let body = find_shape(
            &self.slides[slide_index]
                .slide
                .common_slide_data
                .shape_tree
                .children,
            shape_id,
        )
        .and_then(|shape| shape.text_body.as_ref())
        .ok_or_else(missing)?;
        let hyperlink = TextFrameRef { body }
            .paragraph(paragraph_index)
            .and_then(|paragraph| paragraph.run(run_index))
            .ok_or_else(missing)?
            .properties()
            .and_then(|properties| properties.hyperlink_click.clone());
        Ok(hyperlink.and_then(|hyperlink| {
            self.click_target_slide(
                slide_index,
                hyperlink.action.as_deref(),
                hyperlink.relationship_id.as_deref(),
            )
        }))
    }

    fn set_run_click(
        &mut self,
        operation: &'static str,
        slide_index: usize,
        (shape_id, paragraph_index, run_index): (u32, usize, usize),
        host: RunClickHost,
        target: Option<ShapeClickTarget<'_>>,
    ) -> Result<()> {
        let record = &self.slides[slide_index];
        let part_name = record.part_name.clone();
        if shape_id_count(
            &record.slide.common_slide_data.shape_tree.children,
            shape_id,
        ) > 1
        {
            return Err(invalid_shape_mutation(
                operation,
                format!("shape id {shape_id} is not unique on the slide"),
            ));
        }
        let target_part = match target {
            Some(ShapeClickTarget::Slide(index)) => Some(self.slides[index].part_name.clone()),
            _ => None,
        };
        let mut slide = record.slide.clone();
        let mut relationships = self
            .package
            .get_part_rels(&part_name)
            .cloned()
            .unwrap_or_default();
        let had_click = {
            let missing = || {
                let detail = match host {
                    RunClickHost::Shape => format!(
                        "shape id {shape_id} has no ordinary shape run {run_index} in paragraph {paragraph_index}"
                    ),
                    RunClickHost::Cell { row, column } => format!(
                        "shape id {shape_id} has no table cell run {run_index} in paragraph {paragraph_index} at row {row} column {column}"
                    ),
                };
                invalid_shape_mutation(operation, detail)
            };
            let body = click_text_body_mut(
                &mut slide.common_slide_data.shape_tree.children,
                shape_id,
                host,
            )
            .ok_or_else(missing)?;
            let mut paragraph = TextFrame { body }
                .into_paragraph_mut(paragraph_index)
                .ok_or_else(missing)?;
            let mut run = paragraph.run_mut(run_index).ok_or_else(missing)?;
            let mut properties = run.properties().cloned().unwrap_or_default();
            let had_click = properties.hyperlink_click.is_some();
            let old = properties
                .hyperlink_click
                .as_ref()
                .and_then(|hyperlink| hyperlink.relationship_id.clone())
                .filter(|id| !id.is_empty());
            let action = properties
                .hyperlink_click
                .as_ref()
                .and_then(|hyperlink| hyperlink.action.as_deref());
            let current = old.as_deref().and_then(|id| relationships.get_by_id(id));
            let is_address = |relationship: &Relationship, address: &str| {
                relationship.rel_type == rel_types::HYPERLINK
                    && relationship_is_external(relationship)
                    && relationship.target == address
            };
            let is_slide = |relationship: &Relationship, target_part: &str| {
                relationship.rel_type == rel_types::SLIDE
                    && !relationship_is_external(relationship)
                    && OpcPackage::resolve_rel_target(&part_name, &relationship.target)
                        == target_part
            };
            let unchanged = match (target, target_part.as_deref()) {
                (None, _) => properties.hyperlink_click.is_none(),
                (Some(ShapeClickTarget::Address(address)), _) => {
                    action.is_none()
                        && current.is_some_and(|relationship| is_address(relationship, address))
                }
                (Some(ShapeClickTarget::Slide(_)), Some(target_part)) => {
                    action == Some(SLIDE_JUMP_ACTION)
                        && current.is_some_and(|relationship| is_slide(relationship, target_part))
                }
                (Some(ShapeClickTarget::Slide(_)), None) => unreachable!("the part was read"),
            };
            if unchanged {
                return Ok(());
            }
            let new = match (target, target_part.as_deref()) {
                (None, _) => None,
                (Some(ShapeClickTarget::Address(address)), _) => Some((
                    relationships
                        .items
                        .iter()
                        .find(|relationship| is_address(relationship, address))
                        .map(|relationship| relationship.id.clone())
                        .unwrap_or_else(|| {
                            relationships.add_external(rel_types::HYPERLINK, address)
                        }),
                    None,
                )),
                (Some(ShapeClickTarget::Slide(_)), Some(target_part)) => Some((
                    relationships
                        .items
                        .iter()
                        .find(|relationship| is_slide(relationship, target_part))
                        .map(|relationship| relationship.id.clone())
                        .unwrap_or_else(|| {
                            relationships.add(
                                rel_types::SLIDE,
                                &relative_part_target(&part_name, target_part),
                            )
                        }),
                    Some(SLIDE_JUMP_ACTION),
                )),
                (Some(ShapeClickTarget::Slide(_)), None) => unreachable!("the part was read"),
            };
            let retained = properties.hyperlink_click.clone();
            properties.hyperlink_click = new.map(|(id, action)| {
                let mut hyperlink = retained.clone().unwrap_or_default();
                hyperlink.relationship_id = Some(id);
                hyperlink.action = action.map(str::to_owned);
                hyperlink
            });
            run.set_properties(properties);
            had_click
        };
        // Validate the staged serialization even when this creates the first
        // click. An author-supplied invalid modeled value must fail before
        // either the slide or its relationship graph is published.
        let after = slide_relationship_ids(&slide)?;
        let mut removed_parts = HashSet::new();
        if had_click {
            // The old a:hlinkClick can carry more than its r:id, such as an
            // a:snd click sound naming an audio relationship. Remove every
            // relationship this edit left without a reference.
            let before = slide_relationship_ids(&self.slides[slide_index].slide)?;
            removed_parts.extend(
                relationships
                    .items
                    .iter()
                    .filter(|relationship| {
                        before.contains(&relationship.id)
                            && !after.contains(&relationship.id)
                            && !relationship_is_external(relationship)
                    })
                    .map(|relationship| {
                        OpcPackage::resolve_rel_target(&part_name, &relationship.target)
                    }),
            );
            relationships.items.retain(|relationship| {
                !before.contains(&relationship.id) || after.contains(&relationship.id)
            });
        }
        let mut package = self.package.clone();
        package.set_part_rels(&part_name, relationships);
        let media = if prune_unreachable_parts(&mut package, &removed_parts) {
            self.media_store.rescan(&package)
        } else {
            self.media_store.clone()
        };
        Arc::make_mut(&mut self.slides[slide_index]).slide = slide;
        self.package = package;
        self.media_store = media;
        Ok(())
    }
}

/// Counts the slide children and group members whose `p:cNvPr/@id` is
/// `shape_id`.
struct ShapeHyperlinkLocation {
    root_start: usize,
    root_head_end: usize,
    root_name: String,
    empty_root: bool,
    click: Option<(usize, usize, usize, bool)>,
    relationship_id: Option<String>,
    relationship_attribute: Option<Vec<u8>>,
    action: Option<String>,
}

/// The action verb of a click action that jumps to one named slide.
const SLIDE_JUMP_ACTION: &str = "ppaction://hlinksldjump";

/// The action verb of a click action that jumps within the slide show, such
/// as to the next slide.
const SHOW_JUMP_ACTION: &str = "ppaction://hlinkshowjump";

/// What a click action set on a slide shape opens.
#[derive(Clone, Copy)]
enum ShapeClickTarget<'a> {
    /// An external hyperlink address.
    Address(&'a str),
    /// The zero-based index of a slide of the presentation.
    Slide(usize),
}

fn shape_xml_attribute(
    reader: &NsReader<&[u8]>,
    start: &XmlBytesStart<'_>,
    expected_namespace: Option<&str>,
    name: &[u8],
) -> Result<Option<(String, Vec<u8>)>> {
    for attribute in start.attributes().with_checks(false) {
        let attribute = attribute
            .map_err(|error| invalid_shape_mutation("shape hyperlink", error.to_string()))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        let matches_namespace = match expected_namespace {
            Some(expected) => xml_is(&namespace, expected),
            None => matches!(namespace, XmlResolveResult::Unbound),
        };
        if matches_namespace && local.as_ref() == name {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                .map_err(|error| invalid_shape_mutation("shape hyperlink", error.to_string()))?;
            return Ok(Some((value.into_owned(), attribute.key.as_ref().to_vec())));
        }
    }
    Ok(None)
}

fn locate_shape_hyperlink(xml: &[u8], shape_id: u32) -> Result<Option<ShapeHyperlinkLocation>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut target_depth = None;
    let mut click_depth = None;
    let mut location: Option<ShapeHyperlinkLocation> = None;
    loop {
        let start_offset = reader.buffer_position() as usize;
        let (is_p, is_a, event) = {
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| invalid_shape_mutation("shape hyperlink", error.to_string()))?;
            (xml_is(&namespace, P_NS), xml_is(&namespace, A_NS), event)
        };
        let end_offset = reader.buffer_position() as usize;
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                let empty = xml[start_offset..end_offset].ends_with(b"/>");
                if target_depth.is_none()
                    && is_p
                    && xml_local_name(start.name().as_ref()) == b"cNvPr"
                    && shape_xml_attribute(&reader, &start, None, b"id")?
                        .and_then(|(id, _)| id.parse::<u32>().ok())
                        == Some(shape_id)
                {
                    let root_name = std::str::from_utf8(start.name().as_ref())
                        .map_err(|error| {
                            invalid_shape_mutation("shape hyperlink", error.to_string())
                        })?
                        .to_owned();
                    location = Some(ShapeHyperlinkLocation {
                        root_start: start_offset,
                        root_head_end: end_offset,
                        root_name,
                        empty_root: empty,
                        click: None,
                        relationship_id: None,
                        relationship_attribute: None,
                        action: None,
                    });
                    if empty {
                        return Ok(location);
                    }
                    target_depth = Some(depth + 1);
                } else if target_depth == Some(depth)
                    && is_a
                    && xml_local_name(start.name().as_ref()) == b"hlinkClick"
                {
                    let found = location.as_mut().expect("target depth has location");
                    if found.click.is_some() {
                        return Err(invalid_shape_mutation(
                            "shape hyperlink",
                            "duplicate a:hlinkClick",
                        ));
                    }
                    if let Some((id, attribute)) =
                        shape_xml_attribute(&reader, &start, Some(R_NS), b"id")?
                    {
                        found.relationship_id = Some(id);
                        found.relationship_attribute = Some(attribute);
                    }
                    found.action = shape_xml_attribute(&reader, &start, None, b"action")?
                        .map(|(action, _)| action);
                    found.click = Some((start_offset, end_offset, end_offset, empty));
                    if !empty {
                        click_depth = Some(depth + 1);
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            XmlEvent::End(_) => {
                if click_depth == Some(depth) {
                    let found = location.as_mut().expect("click depth has location");
                    if let Some(click) = found.click.as_mut() {
                        click.2 = end_offset;
                    }
                    click_depth = None;
                }
                if target_depth == Some(depth) {
                    return Ok(location);
                }
                depth = depth.saturating_sub(1);
            }
            XmlEvent::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

/// Rewrites the located `a:hlinkClick` to name `link`, a relationship id
/// and an optional action, or removes it for `None`.
///
/// An existing element keeps its other attributes and children, and loses
/// its action when `link` has none.
fn rewrite_shape_hyperlink(
    xml: &[u8],
    location: &ShapeHyperlinkLocation,
    link: Option<(&str, Option<&str>)>,
) -> Result<Vec<u8>> {
    let Some((id, action)) = link else {
        let Some((start, _, end, _)) = location.click else {
            return Ok(xml.to_vec());
        };
        let mut rewritten = Vec::with_capacity(xml.len() - (end - start));
        rewritten.extend_from_slice(&xml[..start]);
        rewritten.extend_from_slice(&xml[end..]);
        return Ok(rewritten);
    };
    if let Some((start, head_end, _, empty)) = location.click {
        let mut reader = quick_xml::Reader::from_reader(&xml[start..head_end]);
        let mut buffer = Vec::new();
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| invalid_shape_mutation("shape hyperlink", error.to_string()))?;
        let original = match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => start,
            _ => {
                return Err(invalid_shape_mutation(
                    "shape hyperlink",
                    "malformed a:hlinkClick",
                ));
            }
        };
        let name = std::str::from_utf8(original.name().as_ref())
            .map_err(|error| invalid_shape_mutation("shape hyperlink", error.to_string()))?
            .to_owned();
        let mut replacement = XmlBytesStart::new(name);
        let mut replaced = false;
        let mut action_written = false;
        for attribute in original.attributes().with_checks(false) {
            let attribute = attribute
                .map_err(|error| invalid_shape_mutation("shape hyperlink", error.to_string()))?;
            if location.relationship_attribute.as_deref() == Some(attribute.key.as_ref()) {
                let key = std::str::from_utf8(attribute.key.as_ref()).map_err(|error| {
                    invalid_shape_mutation("shape hyperlink", error.to_string())
                })?;
                replacement.push_attribute((key, id));
                replaced = true;
            } else if attribute.key.as_ref() == b"action" {
                if let Some(action) = action {
                    replacement.push_attribute(("action", action));
                    action_written = true;
                }
            } else {
                replacement.push_attribute(attribute);
            }
        }
        if !replaced {
            replacement.push_attribute(("xmlns:r", R_NS));
            replacement.push_attribute(("r:id", id));
        }
        if let Some(action) = action.filter(|_| !action_written) {
            replacement.push_attribute(("action", action));
        }
        let mut writer = XmlWriter::new(Vec::new());
        writer
            .write_event(if empty {
                XmlEvent::Empty(replacement)
            } else {
                XmlEvent::Start(replacement)
            })
            .map_err(|error| invalid_shape_mutation("shape hyperlink", error.to_string()))?;
        let head = writer.into_inner();
        let mut rewritten = Vec::with_capacity(xml.len() + head.len());
        rewritten.extend_from_slice(&xml[..start]);
        rewritten.extend_from_slice(&head);
        rewritten.extend_from_slice(&xml[head_end..]);
        return Ok(rewritten);
    }
    let action = action
        .map(|action| format!(r#" action="{action}""#))
        .unwrap_or_default();
    let click = format!(r#"<a:hlinkClick xmlns:a="{A_NS}" xmlns:r="{R_NS}" r:id="{id}"{action}/>"#);
    let mut rewritten = Vec::with_capacity(xml.len() + click.len() + location.root_name.len() + 3);
    if location.empty_root {
        let head = &xml[location.root_start..location.root_head_end];
        let open = head
            .strip_suffix(b"/>")
            .ok_or_else(|| invalid_shape_mutation("shape hyperlink", "malformed empty p:cNvPr"))?;
        rewritten.extend_from_slice(&xml[..location.root_start]);
        rewritten.extend_from_slice(open);
        rewritten.extend_from_slice(b">");
        rewritten.extend_from_slice(click.as_bytes());
        rewritten.extend_from_slice(format!("</{}>", location.root_name).as_bytes());
        rewritten.extend_from_slice(&xml[location.root_head_end..]);
    } else {
        rewritten.extend_from_slice(&xml[..location.root_head_end]);
        rewritten.extend_from_slice(click.as_bytes());
        rewritten.extend_from_slice(&xml[location.root_head_end..]);
    }
    Ok(rewritten)
}

fn find_shape(children: &[ShapeTreeChild], shape_id: u32) -> Option<&CT_Shape> {
    children.iter().find_map(|child| match child {
        ShapeTreeChild::Shape(shape) if child.non_visual_id() == Some(shape_id) => Some(shape),
        ShapeTreeChild::GroupShape(group) => find_shape(&group.children, shape_id),
        _ => None,
    })
}

#[derive(Clone, Copy)]
enum RunClickHost {
    Shape,
    Cell { row: usize, column: usize },
}

fn click_text_body_mut(
    children: &mut [ShapeTreeChild],
    shape_id: u32,
    host: RunClickHost,
) -> Option<&mut CT_TextBody> {
    for child in children {
        let id = child.non_visual_id();
        if id == Some(shape_id) {
            match host {
                RunClickHost::Shape => {
                    if let ShapeTreeChild::Shape(shape) = child {
                        return shape.text_body.as_mut();
                    }
                }
                RunClickHost::Cell { row, column } => {
                    if let ShapeTreeChild::GraphicFrame(frame) = child {
                        return frame
                            .graphic_data
                            .table_mut()
                            .and_then(|table| table.rows.get_mut(row))
                            .and_then(|row| row.cells.get_mut(column))
                            .and_then(|cell| cell.text_body.as_mut());
                    }
                }
            }
        }
        if let ShapeTreeChild::GroupShape(group) = child
            && let Some(body) = click_text_body_mut(&mut group.children, shape_id, host)
        {
            return Some(body);
        }
    }
    None
}
