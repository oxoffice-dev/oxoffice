//! Charts: reading chart data, types and legends, replacing data and legends, and adding charts with their parts.

use super::*;

impl Presentation {
    /// Reads the current cached data of the uniquely identified chart frame.
    pub fn chart_data(&self, slide_index: usize, shape_id: u32) -> Result<(ChartKind, ChartData)> {
        let (_, _, chart_part, _, _) =
            self.chart_owner(slide_index, shape_id, "read chart data")?;
        oxoffice_chart::read_chart_data(required_part(&self.package, &chart_part)?)
            .map_err(|error| invalid_chart_mutation("read chart data", error.to_string()))
    }

    fn chart_source_owner(
        &self,
        slide_index: usize,
        shape_id: u32,
        operation: &'static str,
    ) -> Result<(Vec<usize>, String, String)> {
        let owner = ShapeOwner::Slide(slide_index);
        let data = self.owner_data(owner)?;
        if chart_shape_id_count(&data.shape_tree.children, shape_id) != 1 {
            return Err(invalid_chart_mutation(
                operation,
                format!("shape id {shape_id} is missing or ambiguous"),
            ));
        }
        let path = path_of_id(&data.shape_tree.children, shape_id)
            .ok_or_else(|| invalid_chart_mutation(operation, "chart has no physical owner"))?;
        let Some(ShapeTreeChild::GraphicFrame(frame)) = child_at(&data.shape_tree.children, &path)
        else {
            return Err(invalid_chart_mutation(
                operation,
                "the selected shape is not a chart frame",
            ));
        };
        let relationship_id = frame
            .chart_relationship_id()
            .filter(|_| matches!(frame.graphic_data.payload(), GraphicDataPayload::Chart(_)))
            .ok_or_else(|| {
                invalid_chart_mutation(operation, "the selected graphic frame is not a chart")
            })?;
        let slide_part = self.owner_part_name(owner)?;
        let chart_part = chart_internal_part(
            &self.package,
            slide_part,
            relationship_id,
            rel_types::CHART,
            operation,
        )?;
        if self.package.content_types.content_type_for(&chart_part) != Some(content_types::CHART) {
            return Err(invalid_chart_mutation(
                operation,
                "chart target has an invalid content type",
            ));
        }
        Ok((path, relationship_id.to_owned(), chart_part))
    }

    /// Reads precise supported chart metadata without requiring cached data or a workbook.
    /// Valid unsupported chart types return None. Invalid physical owners are errors.
    pub fn chart_type(&self, slide_index: usize, shape_id: u32) -> Result<Option<i64>> {
        let (_, _, chart_part) =
            self.chart_source_owner(slide_index, shape_id, "read chart type")?;
        oxoffice_chart::read_chart_type(required_part(&self.package, &chart_part)?)
            .map_err(|error| invalid_chart_mutation("read chart type", error.to_string()))
    }

    fn chart_owner(
        &self,
        slide_index: usize,
        shape_id: u32,
        operation: &'static str,
    ) -> Result<(Vec<usize>, String, String, String, String)> {
        let (path, relationship_id, chart_part) =
            self.chart_source_owner(slide_index, shape_id, operation)?;
        let chart_xml = required_part(&self.package, &chart_part)?;
        let workbook_id = oxoffice_chart::chart_workbook_relationship_id(chart_xml)
            .map_err(|error| invalid_chart_mutation(operation, error.to_string()))?
            .ok_or_else(|| {
                invalid_chart_mutation(operation, "chart has no embedded workbook reference")
            })?;
        let workbook_part = chart_internal_part(
            &self.package,
            &chart_part,
            &workbook_id,
            rel_types::PACKAGE,
            operation,
        )?;
        if self.package.content_types.content_type_for(&workbook_part)
            != Some(content_types::EMBEDDED_WORKBOOK)
        {
            return Err(invalid_chart_mutation(
                operation,
                "workbook target has an invalid content type",
            ));
        }
        // Retained style, colour and opaque relationships must keep their actual targets.
        if let Some(relationships) = self.package.get_part_rels(&chart_part) {
            for relationship in &relationships.items {
                if !relationship_is_external(relationship) {
                    required_part(
                        &self.package,
                        &OpcPackage::resolve_rel_target(&chart_part, &relationship.target),
                    )?;
                }
            }
        }
        Ok((
            path,
            relationship_id,
            chart_part,
            workbook_id,
            workbook_part,
        ))
    }

    /// Read effective settings of one uniquely owned chart.
    pub fn chart_legend(&self, slide_index: usize, shape_id: u32) -> Result<LegendSettings> {
        let (_, _, chart, _, _) = self.chart_owner(slide_index, shape_id, "read chart legend")?;
        oxoffice_chart::chart_legend_settings(required_part(&self.package, &chart)?)
            .map_err(|e| invalid_chart_mutation("read chart legend", e.to_string()))
    }

    /// Edit one legend without changing its workbook or other chart owners.
    pub fn set_chart_legend(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        visible: Option<bool>,
        position: Option<LegendPosition>,
        overlay: Option<bool>,
    ) -> Result<()> {
        let (_, _, chart, _, _) = self.chart_owner(slide_index, shape_id, "set chart legend")?;
        let bytes = required_part(&self.package, &chart)?;
        let candidate = oxoffice_chart::set_chart_legend(bytes, visible, position, overlay)
            .map_err(|e| invalid_chart_mutation("set chart legend", e.to_string()))?;
        if candidate == bytes {
            return Ok(());
        }
        self.publish_chart_parts(slide_index, shape_id, candidate, None)
    }

    /// Replaces one live chart's data and workbook together, isolating shared owners.
    pub fn replace_chart_data(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        data: &ChartData,
    ) -> Result<()> {
        const OPERATION: &str = "replace chart data";
        let (_, _, old_chart, _, old_workbook) =
            self.chart_owner(slide_index, shape_id, OPERATION)?;
        let chart_bytes = required_part(&self.package, &old_chart)?;
        let workbook_bytes = required_part(&self.package, &old_workbook)?;
        let (chart_candidate, workbook_candidate) =
            oxoffice_chart::replace_chart_data(chart_bytes, workbook_bytes, data)
                .map_err(|error| invalid_chart_mutation(OPERATION, error.to_string()))?;
        if chart_candidate == chart_bytes && workbook_candidate == workbook_bytes {
            return Ok(());
        }
        self.publish_chart_parts(
            slide_index,
            shape_id,
            chart_candidate,
            Some(workbook_candidate),
        )
    }

    fn publish_chart_parts(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        chart_candidate: Vec<u8>,
        workbook_candidate: Option<Vec<u8>>,
    ) -> Result<()> {
        const OPERATION: &str = "publish chart parts";
        let (path, relationship_id, old_chart, workbook_id, old_workbook) =
            self.chart_owner(slide_index, shape_id, OPERATION)?;
        let owner = ShapeOwner::Slide(slide_index);
        let slide_part = self.owner_part_name(owner)?.to_owned();
        let source_xml = self.slides[slide_index]
            .slide
            .to_xml()
            .map_err(|error| invalid_chart_mutation(OPERATION, error.to_string()))?;
        let relationship_uses = relationship_ids(&source_xml)
            .map_err(|error| invalid_chart_mutation(OPERATION, error.to_string()))?
            .into_iter()
            .filter(|id| *id == relationship_id)
            .count();
        let shared_chart = relationship_uses > 1 || chart_part_users(&self.package, &old_chart) > 1;
        let shared_workbook = workbook_candidate.is_some()
            && (shared_chart || chart_part_users(&self.package, &old_workbook) > 1);
        let mut staged = self.clone();
        let chart_part = if shared_chart {
            chart_unused_part(&staged.package, "/ppt/charts/chart", ".xml")
        } else {
            old_chart.clone()
        };
        let workbook_part = if shared_workbook {
            chart_unused_part(&staged.package, "/ppt/embeddings/Workbook", ".xlsx")
        } else {
            old_workbook.clone()
        };
        let mut chart_relationships = staged
            .package
            .get_part_rels(&old_chart)
            .cloned()
            .ok_or_else(|| invalid_chart_mutation(OPERATION, "chart has no relationship graph"))?;
        for relationship in &mut chart_relationships.items {
            if !relationship_is_external(relationship) {
                let target = if relationship.id == workbook_id {
                    workbook_part.clone()
                } else {
                    OpcPackage::resolve_rel_target(&old_chart, &relationship.target)
                };
                relationship.target = relative_part_target(&chart_part, &target);
            }
        }
        let original_slide_relationships = staged
            .package
            .get_part_rels(&slide_part)
            .cloned()
            .ok_or_else(|| invalid_chart_mutation(OPERATION, "slide has no relationship graph"))?;
        let mut relationships = original_slide_relationships.clone();
        if relationship_uses > 1 {
            let isolated_id = relationships.add(
                rel_types::CHART,
                &relative_part_target(&slide_part, &chart_part),
            );
            let Some(ShapeTreeChild::GraphicFrame(frame)) = child_at_mut(
                &mut Arc::make_mut(&mut staged.slides[slide_index])
                    .slide
                    .common_slide_data
                    .shape_tree
                    .children,
                &path,
            ) else {
                return Err(invalid_chart_mutation(
                    OPERATION,
                    "selected chart owner disappeared",
                ));
            };
            let source = frame
                .to_xml()
                .map_err(|error| invalid_chart_mutation(OPERATION, error.to_string()))?;
            let source = chart_frame_relationship(&source, &isolated_id)?;
            **frame = CT_GraphicFrame::from_xml(&source)
                .map_err(|error| invalid_chart_mutation(OPERATION, error.to_string()))?;
        } else {
            let relationship = relationships
                .items
                .iter_mut()
                .find(|item| item.id == relationship_id)
                .ok_or_else(|| {
                    invalid_chart_mutation(OPERATION, "selected chart relationship disappeared")
                })?;
            relationship.target = relative_part_target(&slide_part, &chart_part);
        }
        let original_chart_relationships = self
            .package
            .get_part_rels(&old_chart)
            .expect("validated chart graph");
        let chart_relationships =
            chart_relationships_candidate(original_chart_relationships, &chart_relationships)?;
        let relationships =
            chart_relationships_candidate(&original_slide_relationships, &relationships)?;
        staged.package.set_part_rels(&slide_part, relationships);
        staged.package.set_part(&chart_part, chart_candidate);
        if let Some(bytes) = workbook_candidate {
            staged.package.set_part(&workbook_part, bytes);
        }
        staged
            .package
            .set_part_rels(&chart_part, chart_relationships);
        staged
            .package
            .content_types
            .add_override(&chart_part, content_types::CHART);
        staged
            .package
            .content_types
            .add_override(&workbook_part, content_types::EMBEDDED_WORKBOOK);
        prune_unreachable_parts(
            &mut staged.package,
            &HashSet::from([old_chart, old_workbook]),
        );
        staged.commit_owned_parts(None)?;
        *self = staged;
        Ok(())
    }

    /// Appends an editable relationship-backed chart to one slide.
    #[allow(clippy::too_many_arguments)]
    pub fn add_chart(
        &mut self,
        slide_index: usize,
        kind: ChartKind,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
        data: &ChartData,
    ) -> Result<ShapeRef<'_>> {
        let slide = self
            .slides
            .get(slide_index)
            .ok_or(Error::UnknownSlideIndex {
                index: slide_index,
                slide_count: self.slides.len(),
            })?;
        validate_chart_extents(width, height)?;
        let slide_part = slide.part_name.clone();
        let id = ShapeIdAllocator::scan(&slide.slide.common_slide_data.shape_tree).allocate();
        let chart_part = chart_unused_part(&self.package, "/ppt/charts/chart", ".xml");
        let workbook_part = chart_unused_part(&self.package, "/ppt/embeddings/Workbook", ".xlsx");

        let mut package = self.package.clone();
        let slide_relationship_id = package.get_or_create_part_rels(&slide_part).add(
            rel_types::CHART,
            &relative_part_target(&slide_part, &chart_part),
        );
        let workbook_relationship_id = package.get_or_create_part_rels(&chart_part).add(
            rel_types::PACKAGE,
            &relative_part_target(&chart_part, &workbook_part),
        );
        let (chart_xml, workbook_bytes) =
            oxoffice_chart::authored_chart_parts(kind, data, &workbook_relationship_id)
                .map_err(|error| invalid_chart_mutation("add chart", error.to_string()))?;
        let frame = CT_GraphicFrame::new_chart(
            id,
            &format!("Chart {id}"),
            positioned_transform(left, top, width, height),
            &slide_relationship_id,
        )
        .map_err(|error| invalid_chart_mutation("add chart", error.to_string()))?;
        frame
            .to_xml()
            .map_err(|error| invalid_chart_mutation("add chart", error.to_string()))?;

        package.set_part(&chart_part, chart_xml);
        package.set_part(&workbook_part, workbook_bytes);
        package
            .content_types
            .add_override(&chart_part, content_types::CHART);
        package
            .content_types
            .add_override(&workbook_part, content_types::EMBEDDED_WORKBOOK);

        self.package = package;
        let tree = &mut Arc::make_mut(&mut self.slides[slide_index])
            .slide
            .common_slide_data
            .shape_tree;
        Ok(shape_ref(tree.append_child(ShapeTreeChild::GraphicFrame(
            Box::new(frame),
        ))))
    }
}

fn chart_shape_id_count(children: &[ShapeTreeChild], id: u32) -> usize {
    children
        .iter()
        .map(|child| {
            usize::from(child.non_visual_id() == Some(id))
                + match child {
                    ShapeTreeChild::GroupShape(group) => chart_shape_id_count(&group.children, id),
                    ShapeTreeChild::AlternateContent(alternate) => alternate
                        .selected_fallback()
                        .map_or(0, |children| chart_shape_id_count(children, id)),
                    _ => 0,
                }
        })
        .sum()
}

fn chart_unused_part(package: &OpcPackage, prefix: &str, suffix: &str) -> String {
    let mut number = next_numbered_part_number(package, prefix, suffix);
    loop {
        let candidate = format!("{prefix}{number}{suffix}");
        let occupied = package
            .parts
            .keys()
            .chain(package.part_rels.keys())
            .chain(package.content_types.overrides.keys())
            .any(|part| part.eq_ignore_ascii_case(&candidate));
        if !occupied {
            return candidate;
        }
        number += 1;
    }
}

// Keep namespace aliases, foreign attributes and opaque .rels children when changing chart targets.
fn chart_relationships_candidate(
    original: &Relationships,
    candidate: &Relationships,
) -> Result<Relationships> {
    const NS: &[u8] = b"http://schemas.openxmlformats.org/package/2006/relationships";
    const OPERATION: &str = "replace chart data";
    let source = original
        .to_xml()
        .map_err(|error| invalid_chart_mutation(OPERATION, error.to_string()))?;
    let mut reader = NsReader::from_reader(source.as_slice());
    let mut buffer = Vec::new();
    let mut depth = 0;
    let mut edits = Vec::new();
    let mut root_end = None;
    loop {
        let begin = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| invalid_chart_mutation(OPERATION, error.to_string()))?;
        let qualified = matches!(namespace, XmlResolveResult::Bound(uri) if uri.as_ref() == NS);
        match event {
            XmlEvent::Start(ref start) | XmlEvent::Empty(ref start) => {
                if qualified
                    && depth == 1
                    && xml_local_name(start.name().as_ref()) == b"Relationship"
                {
                    let mut id = None;
                    let mut target = None;
                    for attribute in start.attributes() {
                        let attribute = attribute.map_err(|error| {
                            invalid_chart_mutation(OPERATION, error.to_string())
                        })?;
                        if attribute.key.as_ref() == b"Id" {
                            id = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        start.decoder(),
                                    )
                                    .map_err(|error| {
                                        invalid_chart_mutation(OPERATION, error.to_string())
                                    })?
                                    .into_owned(),
                            );
                        } else if attribute.key.as_ref() == b"Target" {
                            let offset = (attribute.value.as_ptr() as usize)
                                .checked_sub(start.as_ptr() as usize)
                                .filter(|offset| offset + attribute.value.len() <= start.len())
                                .ok_or_else(|| {
                                    invalid_chart_mutation(
                                        OPERATION,
                                        "relationship target is outside its tag",
                                    )
                                })?;
                            target = Some(
                                begin + 1 + offset..begin + 1 + offset + attribute.value.len(),
                            );
                        }
                    }
                    if let Some(id) = id {
                        let old = original.get_by_id(&id).ok_or_else(|| {
                            invalid_chart_mutation(
                                OPERATION,
                                "unmodelled relationship cannot be edited",
                            )
                        })?;
                        let new = candidate.get_by_id(&id).ok_or_else(|| {
                            invalid_chart_mutation(
                                OPERATION,
                                "chart relationship removal is unsupported",
                            )
                        })?;
                        if old.rel_type != new.rel_type || old.target_mode != new.target_mode {
                            return Err(invalid_chart_mutation(
                                OPERATION,
                                "chart relationship role changed",
                            ));
                        }
                        if old.target != new.target {
                            edits.push((
                                target.ok_or_else(|| {
                                    invalid_chart_mutation(
                                        OPERATION,
                                        "relationship target is missing",
                                    )
                                })?,
                                quick_xml::escape::escape(&new.target).as_bytes().to_vec(),
                            ));
                        }
                    }
                }
                if matches!(event, XmlEvent::Start(_)) {
                    depth += 1;
                }
            }
            XmlEvent::End(ref end) => {
                depth -= 1;
                if depth == 0
                    && qualified
                    && xml_local_name(end.name().as_ref()) == b"Relationships"
                {
                    root_end = Some(begin);
                }
            }
            XmlEvent::DocType(_) => {
                return Err(invalid_chart_mutation(
                    OPERATION,
                    "chart relationships contain a DTD",
                ));
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut additions = String::new();
    for item in &candidate.items {
        if original.get_by_id(&item.id).is_none() {
            additions.push_str(&format!("<Relationship xmlns='http://schemas.openxmlformats.org/package/2006/relationships' Id=\"{}\" Type=\"{}\" Target=\"{}\"/>", quick_xml::escape::escape(&item.id), quick_xml::escape::escape(&item.rel_type), quick_xml::escape::escape(&item.target)));
        }
    }
    if !additions.is_empty() {
        let at = root_end.ok_or_else(|| {
            invalid_chart_mutation(
                OPERATION,
                "relationship root cannot receive a chart relationship",
            )
        })?;
        edits.push((at..at, additions.into_bytes()));
    }
    let mut result = source;
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, value) in edits {
        result.splice(range, value);
    }
    Relationships::from_xml(&result)
        .map_err(|error| invalid_chart_mutation(OPERATION, error.to_string()))
}

pub(super) fn chart_internal_part(
    package: &OpcPackage,
    source: &str,
    id: &str,
    relationship_type: &'static str,
    operation: &'static str,
) -> Result<String> {
    let matches = package.get_part_rels(source).map_or(0, |items| {
        items.items.iter().filter(|item| item.id == id).count()
    });
    if matches != 1 {
        return Err(invalid_chart_mutation(
            operation,
            format!("relationship {id} is missing or ambiguous in {source}"),
        ));
    }
    require_internal_related_part(package, source, id, relationship_type)
}

fn chart_part_users(package: &OpcPackage, target: &str) -> usize {
    std::iter::once(("/", &package.package_rels))
        .chain(
            package
                .part_rels
                .iter()
                .map(|(source, items)| (source.as_str(), items)),
        )
        .flat_map(|(source, items)| items.items.iter().map(move |item| (source, item)))
        .filter(|(source, item)| {
            !relationship_is_external(item)
                && OpcPackage::resolve_rel_target(source, &item.target).eq_ignore_ascii_case(target)
        })
        .count()
}

// Splice only the namespace-owned relationship value. The rest of the frame stays exact.
fn chart_frame_relationship(xml: &[u8], relationship_id: &str) -> Result<Vec<u8>> {
    const CHART_NS: &[u8] = b"http://schemas.openxmlformats.org/drawingml/2006/chart";
    const OPERATION: &str = "replace chart data";
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut parents = Vec::new();
    let mut selected = None;
    loop {
        let begin = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| invalid_chart_mutation(OPERATION, error.to_string()))?;
        let uri = match namespace {
            XmlResolveResult::Bound(uri) => uri.as_ref().to_vec(),
            _ => Vec::new(),
        };
        let qualified = uri == CHART_NS;
        match event {
            XmlEvent::Start(ref start) | XmlEvent::Empty(ref start) => {
                let direct = parents.len() == 3
                    && parents[0] == (P_NS.as_bytes().to_vec(), b"graphicFrame".to_vec())
                    && parents[1] == (A_NS.as_bytes().to_vec(), b"graphic".to_vec())
                    && parents[2] == (A_NS.as_bytes().to_vec(), b"graphicData".to_vec());
                if qualified && xml_local_name(start.name().as_ref()) == b"chart" && direct {
                    for attribute in start.attributes() {
                        let attribute = attribute.map_err(|error| {
                            invalid_chart_mutation(OPERATION, error.to_string())
                        })?;
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        if matches!(namespace, XmlResolveResult::Bound(uri) if uri.as_ref() == R_NS.as_bytes())
                            && local.as_ref() == b"id"
                        {
                            if selected.is_some() {
                                return Err(invalid_chart_mutation(
                                    OPERATION,
                                    "ambiguous chart relationship attribute",
                                ));
                            }
                            let offset = (attribute.value.as_ptr() as usize)
                                .checked_sub(start.as_ptr() as usize)
                                .filter(|offset| offset + attribute.value.len() <= start.len())
                                .ok_or_else(|| {
                                    invalid_chart_mutation(
                                        OPERATION,
                                        "relationship value is outside its tag",
                                    )
                                })?;
                            selected = Some(
                                begin + 1 + offset..begin + 1 + offset + attribute.value.len(),
                            );
                        }
                    }
                }
                if matches!(event, XmlEvent::Start(_)) {
                    parents.push((uri, xml_local_name(start.name().as_ref()).to_vec()));
                }
            }
            XmlEvent::End(_) => {
                parents.pop();
            }
            XmlEvent::DocType(_) => {
                return Err(invalid_chart_mutation(
                    OPERATION,
                    "chart frame contains a DTD",
                ));
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let range = selected.ok_or_else(|| {
        invalid_chart_mutation(OPERATION, "chart frame has no qualified relationship")
    })?;
    let mut result = Vec::with_capacity(xml.len() + relationship_id.len());
    result.extend_from_slice(&xml[..range.start]);
    result.extend_from_slice(relationship_id.as_bytes());
    result.extend_from_slice(&xml[range.end..]);
    Ok(result)
}

fn validate_chart_extents(width: Emu, height: Emu) -> Result<()> {
    if width.0 <= 0 || height.0 <= 0 {
        return Err(invalid_chart_mutation(
            "add chart",
            "chart width and height must be positive",
        ));
    }
    Ok(())
}

fn next_numbered_part_number(package: &OpcPackage, prefix: &str, suffix: &str) -> usize {
    let occupied = package
        .parts
        .keys()
        .filter_map(|part| numbered_part_suffix(part, prefix, suffix))
        .collect::<HashSet<_>>();
    let mut candidate = occupied
        .iter()
        .copied()
        .max()
        .and_then(|value| value.checked_add(1))
        .unwrap_or(1);
    while occupied.contains(&candidate) {
        candidate = candidate.checked_add(1).unwrap_or(1);
    }
    candidate
}

fn numbered_part_suffix(part: &str, prefix: &str, suffix: &str) -> Option<usize> {
    let value = part.strip_prefix(prefix)?.strip_suffix(suffix)?;
    let parsed = value.parse::<usize>().ok()?;
    (parsed != 0).then_some(parsed)
}

fn invalid_chart_mutation(operation: &'static str, message: impl Into<String>) -> Error {
    Error::InvalidChartMutation {
        operation,
        message: message.into(),
    }
}
