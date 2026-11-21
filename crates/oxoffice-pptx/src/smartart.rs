//! SmartArt: reading diagrams and their node text, editing node text, and resolving and checking their part graphs.

use super::*;

/// One relationship-resolved diagram part.
#[derive(Clone, Debug, PartialEq)]
pub enum DiagramPart<T> {
    Parsed(T),
    External(String),
    MissingTarget(String),
    Invalid(String),
}

/// One SmartArt frame and the diagram resources owned by its relationship scope.
#[derive(Clone, Debug, PartialEq)]
pub struct SmartArtInfo {
    pub slide_index: usize,
    pub shape_id: u32,
    pub relationships: DiagramRelationshipIds,
    pub data: DiagramPart<CT_DiagramData>,
    pub layout: DiagramPart<CT_DiagramLayoutDefinition>,
    pub style: DiagramPart<CT_DiagramStyleDefinition>,
    pub colors: DiagramPart<CT_DiagramColorsDefinition>,
    pub drawing: Option<DiagramPart<CT_DiagramDrawing>>,
}

impl Presentation {
    /// Inspects slide, layout, and master SmartArt in producing-scope order.
    pub fn smart_art(&self, slide_index: usize) -> Result<Vec<SmartArtInfo>> {
        let record = self
            .slides
            .get(slide_index)
            .ok_or(Error::UnknownSlideIndex {
                index: slide_index,
                slide_count: self.slides.len(),
            })?;
        let mut infos = Vec::new();
        collect_smartart_infos(
            &self.package,
            slide_index,
            &record.part_name,
            &record.slide.common_slide_data.shape_tree.children,
            &mut infos,
        );
        let Some(layout_part) =
            related_internal_part(&self.package, &record.part_name, rel_types::SLIDE_LAYOUT)?
        else {
            return Ok(infos);
        };
        let layout = CT_SlideLayout::from_xml(required_part(&self.package, &layout_part)?)
            .map_err(|error| Error::MalformedPart {
                part_name: layout_part.clone(),
                message: error.to_string(),
            })?;
        collect_smartart_infos(
            &self.package,
            slide_index,
            &layout_part,
            &layout.common_slide_data.shape_tree.children,
            &mut infos,
        );
        let Some(master_part) =
            related_internal_part(&self.package, &layout_part, rel_types::SLIDE_MASTER)?
        else {
            return Ok(infos);
        };
        let master = CT_SlideMaster::from_xml(required_part(&self.package, &master_part)?)
            .map_err(|error| Error::MalformedPart {
                part_name: master_part.clone(),
                message: error.to_string(),
            })?;
        collect_smartart_infos(
            &self.package,
            slide_index,
            &master_part,
            &master.common_slide_data.shape_tree.children,
            &mut infos,
        );
        Ok(infos)
    }

    /// Replaces one supported SmartArt node's text atomically.
    pub fn set_smart_art_node_text(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        model_id: &str,
        text: &str,
    ) -> Result<()> {
        let record = self
            .slides
            .get(slide_index)
            .ok_or(Error::UnknownSlideIndex {
                index: slide_index,
                slide_count: self.slides.len(),
            })?;
        let relationships = find_smartart_relationships(
            &record.slide.common_slide_data.shape_tree.children,
            shape_id,
        )
        .ok_or_else(|| Error::InvalidPresentationMutation {
            operation: "set SmartArt node text",
            message: format!("shape {shape_id} is not a SmartArt frame"),
        })?
        .clone();
        let slide_part = record.part_name.clone();
        let (data_part, mut data) =
            validate_smartart_relationship_roles(&self.package, &slide_part, &relationships)?;
        data.set_node_text(model_id, text)
            .map_err(|error| Error::InvalidPresentationMutation {
                operation: "set SmartArt node text",
                message: error.to_string(),
            })?;
        let xml = data.to_xml().map_err(|error| Error::MalformedPart {
            part_name: data_part.clone(),
            message: error.to_string(),
        })?;
        CT_DiagramData::from_xml(&xml).map_err(|error| Error::MalformedPart {
            part_name: data_part.clone(),
            message: error.to_string(),
        })?;

        let mut staged = self.clone();
        staged.package.set_part(&data_part, xml);
        // Editing diagram data does not introduce a new shape XML admission gate.
        // Keep all pre-existing validation categories blocking as before.
        let issues = staged
            .validate()
            .into_iter()
            .filter(|issue| !matches!(issue, ValidationIssue::InvalidXmlChild { .. }))
            .collect::<Vec<_>>();
        if !issues.is_empty() {
            return Err(Error::InvalidPresentationMutation {
                operation: "set SmartArt node text",
                message: format!("staged diagram graph is invalid: {issues:?}"),
            });
        }
        *self = staged.checked_commit(|staged| {
            debug_assert!(
                staged.save_invariants_preserved(),
                "invalid presentation at byte save boundary: {:?}",
                staged.validate()
            );
            staged.reopen(staged.saved_package()?)
        })?;
        Ok(())
    }
}

fn collect_smartart_infos(
    package: &OpcPackage,
    slide_index: usize,
    source_part: &str,
    children: &[ShapeTreeChild],
    infos: &mut Vec<SmartArtInfo>,
) {
    let mut frames = Vec::new();
    collect_smartart_frames(children, &mut frames);
    for (frame, shape_id) in frames {
        let GraphicDataPayload::SmartArt(relationship_ids) = frame.graphic_data.payload() else {
            continue;
        };
        let mut relationships = (**relationship_ids).clone();
        let data = resolve_diagram_part(
            package,
            source_part,
            &relationships.data,
            rel_types::DIAGRAM_DATA,
            CT_DiagramData::from_xml,
        );
        let layout = resolve_diagram_part(
            package,
            source_part,
            &relationships.layout,
            rel_types::DIAGRAM_LAYOUT,
            CT_DiagramLayoutDefinition::from_xml,
        );
        let style = resolve_diagram_part(
            package,
            source_part,
            &relationships.style,
            rel_types::DIAGRAM_QUICK_STYLE,
            CT_DiagramStyleDefinition::from_xml,
        );
        let colors = resolve_diagram_part(
            package,
            source_part,
            &relationships.colors,
            rel_types::DIAGRAM_COLORS,
            CT_DiagramColorsDefinition::from_xml,
        );
        let drawing_id = match &data {
            DiagramPart::Parsed(data) => data.drawing_relationship_id().map(str::to_owned),
            _ => None,
        };
        relationships.drawing = drawing_id.clone();
        let drawing = drawing_id.map(|relationship_id| {
            resolve_diagram_part(
                package,
                source_part,
                &relationship_id,
                rel_types::DIAGRAM_DRAWING,
                CT_DiagramDrawing::from_xml,
            )
        });
        infos.push(SmartArtInfo {
            slide_index,
            shape_id,
            relationships,
            data,
            layout,
            style,
            colors,
            drawing,
        });
    }
}

pub(super) fn collect_smartart_frames<'a>(
    children: &'a [ShapeTreeChild],
    frames: &mut Vec<(&'a CT_GraphicFrame, u32)>,
) {
    for child in children {
        match child {
            ShapeTreeChild::GraphicFrame(frame)
                if matches!(
                    frame.graphic_data.payload(),
                    GraphicDataPayload::SmartArt(_)
                ) =>
            {
                if let Some(shape_id) = child.non_visual_id() {
                    frames.push((frame, shape_id));
                }
            }
            ShapeTreeChild::GroupShape(group) => {
                collect_smartart_frames(&group.children, frames);
            }
            ShapeTreeChild::AlternateContent(alternate) => {
                if let Some(fallback) = alternate.selected_fallback() {
                    collect_smartart_frames(fallback, frames);
                }
            }
            _ => {}
        }
    }
}

fn find_smartart_relationships(
    children: &[ShapeTreeChild],
    shape_id: u32,
) -> Option<DiagramRelationshipIds> {
    for child in children {
        if child.non_visual_id() == Some(shape_id)
            && let ShapeTreeChild::GraphicFrame(frame) = child
            && let GraphicDataPayload::SmartArt(relationships) = frame.graphic_data.payload()
        {
            return Some((**relationships).clone());
        }
        match child {
            ShapeTreeChild::GroupShape(group) => {
                if let Some(found) = find_smartart_relationships(&group.children, shape_id) {
                    return Some(found);
                }
            }
            ShapeTreeChild::AlternateContent(alternate) => {
                if let Some(found) = alternate
                    .selected_fallback()
                    .and_then(|children| find_smartart_relationships(children, shape_id))
                {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn validate_smartart_relationship_roles(
    package: &OpcPackage,
    source_part: &str,
    relationship_ids: &DiagramRelationshipIds,
) -> Result<(String, CT_DiagramData)> {
    let data_part = require_internal_related_part(
        package,
        source_part,
        &relationship_ids.data,
        rel_types::DIAGRAM_DATA,
    )?;
    require_internal_related_part(
        package,
        source_part,
        &relationship_ids.layout,
        rel_types::DIAGRAM_LAYOUT,
    )?;
    require_internal_related_part(
        package,
        source_part,
        &relationship_ids.style,
        rel_types::DIAGRAM_QUICK_STYLE,
    )?;
    require_internal_related_part(
        package,
        source_part,
        &relationship_ids.colors,
        rel_types::DIAGRAM_COLORS,
    )?;
    let data = CT_DiagramData::from_xml(required_part(package, &data_part)?).map_err(|error| {
        Error::MalformedPart {
            part_name: data_part.clone(),
            message: error.to_string(),
        }
    })?;
    if let Some(drawing_id) = data.drawing_relationship_id() {
        require_internal_related_part(
            package,
            source_part,
            drawing_id,
            rel_types::DIAGRAM_DRAWING,
        )?;
    }
    Ok((data_part, data))
}
