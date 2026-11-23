//! The guard projection: what both inputs hold, read independently of the engine.

use super::*;

impl GuardField {
    fn of(field: &oxoffice_docx_oxml::text::Field) -> Self {
        Self {
            graphs: Vec::new(),
            instruction: field.effective_instruction(),
            complex: field.is_complex(),
            cached: field.cached_result.clone(),
            dirty: field.dirty,
            locked: field.locked(),
            legacy: field.legacy_form.clone(),
            segments: field
                .effective_display_field_segments()
                .into_iter()
                .map(|(text, properties, _)| (text.into_owned(), properties.cloned()))
                .collect(),
            instruction_fields: field
                .nested_fields_in_source_order()
                .into_iter()
                .map(Self::of)
                .collect(),
            result_fields: field
                .cached_fields_in_source_order()
                .into_iter()
                .map(Self::of)
                .collect(),
        }
    }
}

impl PartialEq for GuardItem {
    fn eq(&self, other: &Self) -> bool {
        self.content == other.content
            && self.hosts == other.hosts
            && self.fields == other.fields
            && self.content_metadata == other.content_metadata
    }
}

struct GuardProjection<'a> {
    document: &'a Document,
    owner: &'a str,
    options: &'a ComparisonOptions,
    sources: &'a SourceSnapshots,
    hosts: Vec<GuardHost>,
    fields: Vec<GuardField>,
    field_keys: Vec<(usize, Vec<usize>, usize)>,
}

impl GuardProjection<'_> {
    fn paragraph(&mut self, original: &CT_P, projected: &mut CT_P) -> Result<()> {
        for boundary in 0..=original.runs.len() {
            for (index, (_, _, _, control)) in original
                .content_controls
                .iter()
                .enumerate()
                .filter(|(_, (at, _, _, _))| *at == boundary)
            {
                self.control(control, &mut projected.content_controls[index].3)?;
            }
            if let Some(run) = original.runs.get(boundary) {
                self.run(run, &mut projected.runs[boundary])?;
            }
        }
        for link in &mut projected.hyperlinks {
            if let Some(id) = &mut link.rel_id {
                normalize_relationship_id(id, self.document, self.owner)?;
            }
        }
        Ok(())
    }

    fn body(&mut self, original: &BodyContent, projected: &mut BodyContent) -> Result<()> {
        match (original, projected) {
            (BodyContent::Paragraph(left), BodyContent::Paragraph(right)) => {
                self.paragraph(left, right)
            }
            (BodyContent::Table(left), BodyContent::Table(right)) => self.table(left, right),
            (BodyContent::ContentControl(left), BodyContent::ContentControl(right)) => {
                self.control(left, right)
            }
            (BodyContent::RawXml(_), BodyContent::RawXml(_)) => Ok(()),
            _ => Err(Error::Other("guard body clone changed kind".into())),
        }
    }

    fn table(&mut self, original: &CT_Tbl, projected: &mut CT_Tbl) -> Result<()> {
        for boundary in 0..=original.rows.len() {
            for (index, (_, _, control)) in original
                .content_controls
                .iter()
                .enumerate()
                .filter(|(_, (at, _, _))| *at == boundary)
            {
                self.control(control, &mut projected.content_controls[index].2)?;
            }
            if let Some(row) = original.rows.get(boundary) {
                self.row(row, &mut projected.rows[boundary])?;
            }
        }
        Ok(())
    }

    fn row(&mut self, original: &CT_Row, projected: &mut CT_Row) -> Result<()> {
        for boundary in 0..=original.cells.len() {
            for (index, (_, _, control)) in original
                .content_controls
                .iter()
                .enumerate()
                .filter(|(_, (at, _, _))| *at == boundary)
            {
                self.control(control, &mut projected.content_controls[index].2)?;
            }
            if let Some(cell) = original.cells.get(boundary) {
                self.cell(cell, &mut projected.cells[boundary])?;
            }
        }
        Ok(())
    }

    fn cell(&mut self, original: &CT_Tc, projected: &mut CT_Tc) -> Result<()> {
        for (left, right) in original.content.iter().zip(&mut projected.content) {
            match (left, right) {
                (CellContent::Paragraph(left), CellContent::Paragraph(right)) => {
                    self.paragraph(left, right)?
                }
                (CellContent::Table(left), CellContent::Table(right)) => self.table(left, right)?,
                (CellContent::ContentControl(left), CellContent::ContentControl(right)) => {
                    self.control(left, right)?
                }
                _ => return Err(Error::Other("guard cell clone changed kind".into())),
            }
        }
        Ok(())
    }

    fn control(&mut self, original: &CT_Sdt, projected: &mut CT_Sdt) -> Result<()> {
        for (left, right) in original.content.iter().zip(&mut projected.content) {
            match (left, right) {
                (SdtContent::Paragraph(left), SdtContent::Paragraph(right)) => {
                    self.paragraph(left, right)?
                }
                (SdtContent::Table(left), SdtContent::Table(right)) => self.table(left, right)?,
                (SdtContent::Row(left), SdtContent::Row(right)) => self.row(left, right)?,
                (SdtContent::Cell(left), SdtContent::Cell(right)) => self.cell(left, right)?,
                (SdtContent::Run(left), SdtContent::Run(right)) => self.run(left, right)?,
                (SdtContent::ContentControl(left), SdtContent::ContentControl(right)) => {
                    self.control(left, right)?
                }
                (SdtContent::RawXml(_), SdtContent::RawXml(_)) => {}
                _ => return Err(Error::Other("guard control clone changed kind".into())),
            }
        }
        Ok(())
    }

    fn host(
        &mut self,
        source: &RunSource,
        span: Range<usize>,
        typed: bool,
        drawing: Option<&oxoffice_docx_oxml::drawing::CT_Drawing>,
    ) -> Result<Option<GuardHost>> {
        let actual = &source.scoped[span.clone()];
        let mut reader = Reader::from_str(actual);
        if !matches!(reader.read_event(), Ok(Event::Start(_) | Event::Empty(_))) {
            return Ok(None);
        }
        let closed = source_child(&source.scoped, span)?;
        let boxes = policy_textbox_intervals(&closed)?;
        if boxes.is_empty() {
            return Ok(None);
        }
        let added = element_opening(&closed)?
            .0
            .checked_sub(element_opening(actual)?.0)
            .ok_or_else(|| Error::Other("guard source closure changed its opening".into()))?;
        if closed[element_opening(&closed)?.0..] != actual[element_opening(actual)?.0..] {
            return Err(Error::Other(
                "guard source closure changed descendant bytes".into(),
            ));
        }
        let mut outside = Vec::new();
        let mut bodies = Vec::new();
        let mut cursor = 0;
        let drawing_span = if drawing.is_some() {
            if boxes.len() != 1 {
                return Err(Error::Other(
                    "selected modeled drawing has ambiguous body source".into(),
                ));
            }
            boxes[0].3.clone()
        } else {
            None
        };
        for (box_span, bindings, _, _) in boxes {
            let box_xml = &closed[box_span.clone()];
            let inner = element_inner_range_any_prefix(box_xml, "txbxContent")?;
            let start = box_span.start + inner.start - added;
            let end = box_span.start + inner.end - added;
            if !typed && drawing.is_none() {
                outside.push(actual[cursor..start].to_owned());
            }
            if typed {
                let box_start = box_span.start - added;
                let box_end = box_span.end - added;
                outside.push(actual[box_start..start].to_owned());
                outside.push(actual[end..box_end].to_owned());
            }
            let inner_xml = &actual[start..end];
            let model = story_document(inner_xml, &bindings)?;
            bodies.push(guard_source_body(
                &model,
                inner_xml,
                &bindings,
                self.document,
                self.owner,
                self.options,
            )?);
            cursor = end;
        }
        if !typed && drawing.is_none() {
            outside.push(actual[cursor..].to_owned());
        }
        if let Some(span) = drawing_span {
            let start = span.start - added;
            let end = span.end - added;
            let inner = element_inner_range_any_prefix(&actual[start..end], "drawing")?;
            outside.push(actual[..start + inner.start].to_owned());
            outside.push(actual[start + inner.end..].to_owned());
        }
        let drawing = drawing
            .map(|drawing| -> Result<_> {
                let mut drawing = drawing_signature(drawing);
                normalize_drawing_identity(&mut drawing, self.document, self.owner)?;
                if let Some(shape) = drawing
                    .anchor
                    .as_mut()
                    .and_then(|anchor| anchor.shape.as_mut())
                {
                    shape.text_body = None;
                    shape.text.clear();
                    shape.source_text_box_owner = None;
                }
                Ok(drawing)
            })
            .transpose()?;
        Ok(Some(GuardHost {
            outside,
            bodies,
            drawing,
        }))
    }

    fn run(&mut self, original: &CT_R, projected: &mut CT_R) -> Result<()> {
        if !self.options.ignore_fields {
            for content in &original.content {
                if let RunContent::Field(field) = content {
                    fn keys(
                        field: &oxoffice_docx_oxml::text::Field,
                        root: usize,
                        path: Vec<usize>,
                        output: &mut Vec<(usize, Vec<usize>, usize)>,
                    ) {
                        output.push((root, path.clone(), std::ptr::from_ref(field) as usize));
                        for (slot, child) in field
                            .cached_fields_in_source_order()
                            .into_iter()
                            .enumerate()
                        {
                            let mut child_path = path.clone();
                            child_path.push(slot);
                            keys(child, root, child_path, output);
                        }
                    }
                    fn graph_identity(
                        identity: &mut GuardField,
                        field: &oxoffice_docx_oxml::text::Field,
                        sources: &SourceSnapshots,
                        document: &Document,
                        owner: &str,
                    ) -> Result<()> {
                        let key = std::ptr::from_ref(field) as usize;
                        if let Some(graphs) = sources.field_graphs.get(&key) {
                            identity.graphs = graphs.clone();
                            for (_, id) in &mut identity.graphs {
                                normalize_relationship_id(id, document, owner)?;
                            }
                        }
                        for (identity, child) in identity
                            .instruction_fields
                            .iter_mut()
                            .zip(field.nested_fields_in_source_order())
                        {
                            graph_identity(identity, child, sources, document, owner)?;
                        }
                        for (identity, child) in identity
                            .result_fields
                            .iter_mut()
                            .zip(field.cached_fields_in_source_order())
                        {
                            graph_identity(identity, child, sources, document, owner)?;
                        }
                        Ok(())
                    }
                    let root = self.fields.len();
                    let mut identity = GuardField::of(field);
                    graph_identity(
                        &mut identity,
                        field,
                        self.sources,
                        self.document,
                        self.owner,
                    )?;
                    self.fields.push(identity);
                    keys(field, root, Vec::new(), &mut self.field_keys);
                }
            }
        }
        let Some(source) = self.sources.runs.get(&(original as *const CT_R as usize)) else {
            return Ok(());
        };
        let mut slots = Vec::new();
        for (ordinal, slot) in source.raw_slots.iter().enumerate() {
            if let Some((span, _, _)) = slot {
                slots.push((span.start, RunHostRole::Raw(ordinal, ordinal), span.clone()));
            }
        }
        for (ordinal, slot) in source.typed_slots.iter().enumerate() {
            if let Some(span) = slot {
                slots.push((
                    span.start,
                    RunHostRole::Typed(ordinal, ordinal),
                    span.clone(),
                ));
            }
        }
        slots.sort_by_key(|(start, _, _)| *start);
        for (_, role, span) in slots {
            match role {
                RunHostRole::Raw(local, _) => {
                    // A selected modeled compatibility drawing is represented
                    // once by its physical raw slot, never again by alt_drawings.
                    let alternate = CT_R::raw_child_has_alternate_content_drawing(
                        original.extra_xml_positions[local],
                    );
                    let selected = if alternate {
                        let index = original.extra_xml_positions[..local]
                            .iter()
                            .filter(|encoded| {
                                CT_R::raw_child_has_alternate_content_drawing(**encoded)
                            })
                            .count();
                        let selected = original.alt_drawings.get(index);
                        if !selected
                            .and_then(|drawing| drawing.anchor.as_ref())
                            .and_then(|anchor| anchor.shape.as_ref())
                            .is_some_and(|shape| shape.text_body.is_some())
                        {
                            continue;
                        }
                        selected
                    } else {
                        None
                    };
                    if let Some(identity) = self.host(source, span, false, selected)? {
                        projected.extra_xml[local].clear();
                        self.hosts.push(identity);
                    }
                }
                RunHostRole::Typed(local, _) => {
                    let Some(RunContent::Drawing(drawing)) = original.content.get(local) else {
                        continue;
                    };
                    if !drawing
                        .anchor
                        .as_ref()
                        .and_then(|anchor| anchor.shape.as_ref())
                        .is_some_and(|shape| shape.text_body.is_some() || !shape.text.is_empty())
                    {
                        continue;
                    }
                    let Some(identity) = self.host(source, span, true, None)? else {
                        return Err(Error::Other(
                            "typed textbox has no admitted source body".into(),
                        ));
                    };
                    let RunContent::Drawing(drawing) = &mut projected.content[local] else {
                        unreachable!()
                    };
                    if let Some(shape) = drawing
                        .anchor
                        .as_mut()
                        .and_then(|anchor| anchor.shape.as_mut())
                    {
                        shape.text_body = None;
                        shape.text.clear();
                        shape.source_text_box_owner = None;
                    }
                    self.hosts.push(identity);
                }
            }
        }
        Ok(())
    }
}

fn guard_source_body(
    model: &CT_Document,
    source: &str,
    bindings: &[(String, String)],
    document: &Document,
    owner: &str,
    options: &ComparisonOptions,
) -> Result<Vec<GuardItem>> {
    guard_source_body_at(
        model, source, bindings, document, owner, options, owner, None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn guard_source_body_at(
    model: &CT_Document,
    source: &str,
    bindings: &[(String, String)],
    document: &Document,
    owner: &str,
    options: &ComparisonOptions,
    location: &str,
    coordinates: Option<SourceCoordinates>,
) -> Result<Vec<GuardItem>> {
    let ordinary_part = coordinates.is_some();
    let spans = story_content_spans(source, bindings)?;
    let mut snapshots = SourceSnapshots {
        guard_graphs: true,
        story: location.to_owned(),
        ..SourceSnapshots::default()
    };
    if let Some(part) = document.package.get_part(owner) {
        snapshots.part_hash = Some(Sha256::digest(part).into());
        snapshots.part_len = Some(part.len());
        if let Some((base, projection)) = coordinates {
            snapshots.part_base = Some(base);
            snapshots.part_projection = projection;
        } else {
            let start = (source.as_ptr() as usize).checked_sub(part.as_ptr() as usize);
            snapshots.part_base = start.filter(|start| {
                start
                    .checked_add(source.len())
                    .is_some_and(|end| end <= part.len())
            });
        }
    }
    snapshots.body(model, (source, &spans, bindings), &HeldRevisions::default())?;
    let mut cleaned = model.clone();
    // The snapshots refer to the still-live, source-faithful model. Only its
    // explicitly corresponding analysis clone receives canonical graph values.
    normalize_relationship_identities(&mut cleaned, document, owner)?;
    let mut identities = Vec::new();
    let mut state = GuardProjection {
        document,
        owner,
        options,
        sources: &snapshots,
        hosts: Vec::new(),
        fields: Vec::new(),
        field_keys: Vec::new(),
    };
    let mut inspection = cleaned.clone();
    for (original, projected) in model.body.content.iter().zip(&mut inspection.body.content) {
        state.body(original, projected)?;
        let metadata = SourceMetadata::body(original, &snapshots, options)?;
        let observations = std::mem::take(&mut state.field_keys)
            .into_iter()
            .filter_map(|(field_index, result_path, key)| {
                snapshots
                    .field_occurrences
                    .get(&key)
                    .cloned()
                    .map(|occurrence| {
                        let metadata_record = metadata
                            .field_records
                            .get(&key)
                            .copied()
                            .unwrap_or((0, 1, None));
                        GuardFieldObservation {
                            field_index,
                            result_path,
                            occurrence,
                            metadata_record,
                        }
                    })
            })
            .collect::<Vec<_>>();
        identities.push((
            std::mem::take(&mut state.hosts),
            std::mem::take(&mut state.fields),
            metadata.facts,
            observations,
        ));
    }
    if options.ignore_formatting {
        cleaned = inspection;
    }
    let projection = if ordinary_part {
        coalesced_postcondition_body
    } else {
        postcondition_body
    };
    let content = project_source_body(&cleaned, source, bindings, options, projection)?;
    Ok(content
        .into_iter()
        .enumerate()
        .map(|(index, content)| {
            let (hosts, fields, content_metadata, field_observations) = identities
                .get_mut(index)
                .map(std::mem::take)
                .unwrap_or_default();
            GuardItem {
                content,
                hosts,
                fields,
                content_metadata,
                field_observations,
            }
        })
        .collect())
}

pub(super) fn guard_mask_coordinates(
    original: &str,
    masked: &str,
    prefix: &str,
    marker: &str,
) -> Result<Vec<(Range<usize>, Range<usize>)>> {
    let original = text_box_host_spans(original, &text_box_spans(original, prefix)?)?;
    let masked = private_text_box_host_spans(masked, prefix, marker)?;
    if original.len() != masked.len() {
        return Err(Error::Other(
            "ignored host coordinate projection lost an actual owner".into(),
        ));
    }
    Ok(original.into_iter().zip(masked).collect())
}

pub(super) fn guard_story_part(
    document: &Document,
    owner: &str,
    xml: &[u8],
    kind: ComparisonStoryKind,
    options: &ComparisonOptions,
    marker: Option<&str>,
) -> Result<Vec<GuardItem>> {
    Ok(
        guard_story_owners(document, owner, xml, kind, options, marker, false)?
            .into_iter()
            .flatten()
            .collect(),
    )
}

pub(super) fn guard_story_owners(
    document: &Document,
    owner: &str,
    xml: &[u8],
    kind: ComparisonStoryKind,
    options: &ComparisonOptions,
    marker: Option<&str>,
    note_id_locations: bool,
) -> Result<Vec<Vec<GuardItem>>> {
    let source = std::str::from_utf8(xml).map_err(utf8_error)?;
    let masked;
    let original_source = source;
    let mut coordinates = Vec::new();
    let source = if story_ignored(options, ComparisonStoryKind::TextBox) {
        let prefix = root_prefix(source, kind.root_local())?;
        masked = mask_text_box_subtrees(
            source,
            &prefix,
            marker.ok_or_else(|| Error::Other("missing guard textbox marker".into()))?,
        )?
        .0;
        coordinates = guard_mask_coordinates(
            original_source,
            &masked,
            &prefix,
            marker.ok_or_else(|| Error::Other("missing ignored host coordinate marker".into()))?,
        )?;
        masked.as_str()
    } else {
        source
    };
    let root = root_inner_range(source, kind.root_local())?;
    let root_bindings = root_namespace_bindings(source, kind.root_local(), &[])?;
    let mut output = Vec::new();
    if let Some(local) = kind.owner_local() {
        let spans = if note_id_locations {
            NotePart::parse(owner.to_owned(), source.to_owned(), NoteKind::Footnote)?
                .owners
                .into_iter()
                .filter(|record| {
                    record.id.is_some()
                        && matches!(
                            record.kind,
                            RawNoteType::Omitted | RawNoteType::ExplicitNormal
                        )
                })
                .map(|record| record.span)
                .collect()
        } else {
            direct_word_element_spans(source, local)?
        };
        for (index, span) in spans.into_iter().enumerate() {
            let owner_start = span.start;
            let actual = &source[span];
            if matches!(
                kind,
                ComparisonStoryKind::Footnote | ComparisonStoryKind::Endnote
            ) && !normal_note_owner(actual, &root_bindings)?
            {
                continue;
            }
            let inner = element_inner_range_any_prefix(actual, local)?;
            let bindings = root_namespace_bindings(actual, local, &root_bindings)?;
            let model = story_document(&actual[inner.clone()], &bindings)?;
            let location = if note_id_locations {
                let Some(id) = raw_note_attribute_in_scope(actual, &root_bindings, "id")?
                    .and_then(|value| value.parse::<i32>().ok())
                else {
                    return Err(Error::Other(
                        "normal note guard lost its qualified owner identity".into(),
                    ));
                };
                format!("{}:{owner}/{local}-id[{id}]", kind.label())
            } else {
                format!("{}:{owner}/{local}[{index}]", kind.label())
            };
            output.push(guard_source_body_at(
                &model,
                &actual[inner.clone()],
                &bindings,
                document,
                owner,
                options,
                &location,
                Some((owner_start + inner.start, coordinates.clone())),
            )?);
        }
    } else {
        let model = story_document(&source[root.clone()], &root_bindings)?;
        output = vec![guard_source_body_at(
            &model,
            &source[root.clone()],
            &root_bindings,
            document,
            owner,
            options,
            &format!("{}:{owner}", kind.label()),
            Some((root.start, coordinates)),
        )?];
    }
    Ok(output)
}

pub(super) fn normalized_story_part(
    document: &Document,
    physical_owner: &str,
    xml: &[u8],
    kind: ComparisonStoryKind,
    options: &ComparisonOptions,
    text_box_marker: Option<&str>,
    story_projection: fn(&CT_Document, &ComparisonOptions) -> Vec<String>,
) -> Result<Vec<String>> {
    let masked;
    let xml = if story_ignored(options, ComparisonStoryKind::TextBox) {
        let source = std::str::from_utf8(xml).map_err(utf8_error)?;
        let prefix = root_prefix(source, kind.root_local())?;
        let marker = text_box_marker
            .ok_or_else(|| Error::Other("missing shared comparison text-box marker".to_owned()))?;
        masked = mask_text_box_subtrees(source, &prefix, marker)?.0;
        &masked
    } else {
        std::str::from_utf8(xml).map_err(utf8_error)?
    };
    let root = root_inner_range(xml, kind.root_local())?;
    let root_bindings = root_namespace_bindings(xml, kind.root_local(), &[])?;
    if let Some(owner_local) = kind.owner_local() {
        let mut normalized = Vec::new();
        for owner in direct_word_element_spans(xml, owner_local)? {
            let owner_xml = &xml[owner];
            if matches!(
                kind,
                ComparisonStoryKind::Footnote | ComparisonStoryKind::Endnote
            ) && !normal_note_owner(owner_xml, &root_bindings)?
            {
                continue;
            }
            let inner = element_inner_range_any_prefix(owner_xml, owner_local)?;
            let root_bindings = root_namespace_bindings(xml, kind.root_local(), &[])?;
            let owner_bindings = root_namespace_bindings(owner_xml, owner_local, &root_bindings)?;
            let mut model = story_document(&owner_xml[inner.clone()], &owner_bindings)?;
            normalize_relationship_identities(&mut model, document, physical_owner)?;
            normalized.extend(project_source_body(
                &model,
                &owner_xml[inner],
                &owner_bindings,
                options,
                story_projection,
            )?);
        }
        Ok(normalized)
    } else {
        let bindings = root_namespace_bindings(xml, kind.root_local(), &[])?;
        let mut model = story_document(&xml[root.clone()], &bindings)?;
        normalize_relationship_identities(&mut model, document, physical_owner)?;
        project_source_body(&model, &xml[root], &bindings, options, story_projection)
    }
}

/// A metadata-bearing default pPr is a source property owner in both Empty
/// and paired forms. The parser otherwise exposes only the Empty form as a
/// paragraph raw boundary. Reconcile that container representation only after
/// positively checking its source QName, opaque attributes and namespace scope.
fn empty_property_source(xml: &str) -> Result<Option<(String, String)>> {
    let spans = direct_word_element_spans(xml, "pPr")?;
    let Some(span) = spans.first() else {
        return Ok(None);
    };
    let raw = &xml[span.clone()];
    let scoped = source_child(xml, span.clone())?;
    let mut reader = NsReader::from_str(&scoped);
    let (_, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let (Event::Start(root) | Event::Empty(root)) = event else {
        return Ok(None);
    };
    let name = String::from_utf8(root.name().as_ref().to_vec()).map_err(utf8_error)?;
    let mut attributes = Vec::new();
    for attribute in root.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, _) = reader.resolver().resolve_attribute(attribute.key);
        let namespace = match namespace {
            ResolveResult::Bound(uri) => Some(uri.as_ref().to_vec()),
            ResolveResult::Unbound => None,
            ResolveResult::Unknown(prefix) => {
                return Err(Error::Other(format!(
                    "unbound property attribute prefix {prefix:?}"
                )));
            }
        };
        attributes.push((
            attribute.key.as_ref().to_vec(),
            namespace,
            attribute.value.as_ref().to_vec(),
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, root.decoder())
                .map_err(|error| Error::Other(error.to_string()))?
                .into_owned(),
        ));
    }
    if attributes.is_empty() {
        return Ok(None);
    }
    let scope = reader
        .resolver()
        .bindings()
        .map(|(prefix, uri)| (format!("{prefix:?}"), uri.as_ref().to_vec()))
        .collect::<BTreeMap<_, _>>();
    let word = word_element_starts(&scoped)?;
    let mut opaque_events = Vec::new();
    let mut cursor = element_opening(&scoped)?.0;
    for child in direct_element_spans(&scoped)? {
        collect_property_source_events(&scoped[cursor..child.start], &mut opaque_events)?;
        cursor = child.end;
        // A resolved generated mark may leave an otherwise default rPr shell.
        // Attributes other than declarations or any child/content are opaque
        // source state and cannot be reconciled as an empty representation.
        if !word.contains(&child.start) || element_local_name(&scoped[child.clone()]) != "rPr" {
            return Ok(None);
        }
        let mut child_reader = Reader::from_str(&scoped[child]);
        let child_event = child_reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let empty = matches!(child_event, Event::Empty(_));
        let (Event::Start(element) | Event::Empty(element)) = child_event else {
            return Ok(None);
        };
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            if attribute.key.as_ref() != b"xmlns" && !attribute.key.as_ref().starts_with(b"xmlns:")
            {
                return Ok(None);
            }
        }
        if !empty && !matches!(child_reader.read_event(), Ok(Event::End(_))) {
            return Ok(None);
        }
    }
    let end = if element_opening(&scoped)?.1 {
        cursor
    } else {
        paragraph_close_start(&scoped)?
    };
    collect_property_source_events(&scoped[cursor..end], &mut opaque_events)?;
    Ok(Some((
        raw.to_owned(),
        format!("{name:?}:{attributes:?}:{scope:?}:{opaque_events:?}"),
    )))
}

fn collect_property_source_events(xml: &str, events: &mut Vec<String>) -> Result<()> {
    let mut reader = Reader::from_str(xml);
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?
        {
            Event::Eof => return Ok(()),
            Event::Text(_)
            | Event::CData(_)
            | Event::Comment(_)
            | Event::PI(_)
            | Event::GeneralRef(_) => {
                events.push(xml[before..reader.buffer_position() as usize].to_owned());
            }
            _ => return Err(Error::Other("unexpected property-source event".into())),
        }
    }
}

fn project_source_body(
    model: &CT_Document,
    source: &str,
    bindings: &[(String, String)],
    options: &ComparisonOptions,
    projection: fn(&CT_Document, &ComparisonOptions) -> Vec<String>,
) -> Result<Vec<String>> {
    let spans = story_content_spans(source, bindings)?;
    let mut snapshots = SourceSnapshots {
        properties_only: true,
        ..Default::default()
    };
    snapshots.body(model, (source, &spans, bindings), &HeldRevisions::default())?;
    let mut source_properties = Vec::new();
    crate::document::visit_body_paragraphs(&model.body.content, false, &mut |paragraph| {
        source_properties.push(
            snapshots
                .paragraphs
                .get(&(paragraph as *const CT_P as usize))
                .map(|xml| empty_property_source(xml))
                .transpose()
                .map(Option::flatten),
        );
    });
    let source_properties = source_properties.into_iter().collect::<Result<Vec<_>>>()?;
    let mut cleaned = model.clone();
    let mut index = 0;
    crate::document::visit_body_paragraphs_mut(&mut cleaned.body.content, &mut |paragraph| {
        if let Some((raw, _)) = &source_properties[index] {
            super::signatures::retain_raw_children(paragraph, |payload| {
                payload.as_slice() != raw.as_bytes()
            });
        }
        index += 1;
    });
    let mut output = projection(&cleaned, options);
    if source_properties.iter().any(Option::is_some) {
        output.push(format!(
            "empty-property-source:{:?}",
            source_properties
                .iter()
                .map(|value| value.as_ref().map(|(_, identity)| identity))
                .collect::<Vec<_>>()
        ));
    }
    Ok(output)
}

fn normalize_drawing_identity(
    drawing: &mut oxoffice_docx_oxml::drawing::CT_Drawing,
    document: &Document,
    owner: &str,
) -> Result<()> {
    if let Some(inline) = &mut drawing.inline {
        inline.doc_pr_id = 0;
        normalize_relationship_id(&mut inline.embed_id, document, owner)?;
        if let Some(id) = &mut inline.link_id {
            normalize_relationship_id(id, document, owner)?;
        }
        if let Some(id) = &mut inline.chart_rel_id {
            normalize_relationship_id(id, document, owner)?;
        }
    }
    if let Some(anchor) = &mut drawing.anchor {
        anchor.doc_pr_id = 0;
        normalize_relationship_id(&mut anchor.embed_id, document, owner)?;
        if let Some(id) = &mut anchor.link_id {
            normalize_relationship_id(id, document, owner)?;
        }
        if let Some(id) = &mut anchor.chart_rel_id {
            normalize_relationship_id(id, document, owner)?;
        }
    }
    Ok(())
}

/// Every picture, chart and hyperlink of `model` names its relationship by
/// what it names, so another relationship to the same payload is no change.
fn normalize_relationship_identities(
    model: &mut CT_Document,
    document: &Document,
    owner: &str,
) -> Result<()> {
    let mut result = Ok(());
    crate::document::visit_all_drawings_mut(&mut model.body.content, &mut |drawing| {
        if result.is_ok() {
            result = normalize_drawing_identity(drawing, document, owner);
        }
    });
    crate::document::visit_body_paragraphs_mut(&mut model.body.content, &mut |paragraph| {
        for id in paragraph
            .hyperlinks
            .iter_mut()
            .filter_map(|link| link.rel_id.as_mut())
        {
            if result.is_ok() {
                result = normalize_relationship_id(id, document, owner);
            }
        }
    });
    result
}

fn normalize_relationship_id(id: &mut String, document: &Document, owner: &str) -> Result<()> {
    let Some(relationship) = document
        .package
        .get_part_rels(owner)
        .and_then(|relationships| relationships.get_by_id(id))
    else {
        return Ok(());
    };
    let signature = if relationship.rel_type == rel_types::CHART {
        format!(
            "internal:{}:{}",
            rel_types::CHART,
            chart_relationship_signature(document, owner, relationship)?.0
        )
    } else if crate::document::relationship_is_internal(relationship) {
        let target = OpcPackage::resolve_rel_target(owner, &relationship.target);
        let Some(bytes) = document.package.get_part(&target) else {
            return Ok(());
        };
        let digest = Sha256::digest(bytes);
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
        }
        format!("internal:{}:{hex}", relationship.rel_type)
    } else {
        format!(
            "external:{}:{}:{}",
            relationship.rel_type,
            relationship.target_mode.as_deref().unwrap_or(""),
            relationship.target
        )
    };
    *id = signature;
    Ok(())
}
