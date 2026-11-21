//! Writing a paragraph: CT_P to XML with its fields, boundaries and hyperlinks.

use super::*;

#[allow(non_snake_case)]
impl CT_P {
    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.to_xml_with_para_id(writer, None)
    }

    pub(crate) fn to_xml_with_para_id<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        para_id: Option<&str>,
    ) -> Result<()> {
        self.write_xml_core(writer, para_id, XmlWriteRole::Publication, None)
    }

    pub(crate) fn writer_field_roots(&self) -> Vec<&Field> {
        let mut fields: Vec<_> = self
            .runs
            .iter()
            .flat_map(|run| &run.content)
            .filter_map(|content| {
                if let RunContent::Field(field) = content {
                    Some(field)
                } else {
                    None
                }
            })
            .collect();
        for (_, _, _, control) in &self.content_controls {
            fields.extend(control.writer_field_roots());
        }
        fields
    }

    pub(crate) fn write_xml_core<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        para_id: Option<&str>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        let mut work = WordOperationWork::model_fields(self.writer_field_roots())?;
        self.write_xml_core_in(writer, para_id, role, source_context, &mut work, None)
    }

    pub(crate) fn write_xml_core_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        para_id: Option<&str>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        work: &mut WordOperationWork,
        emission: Option<&FieldXmlEmission>,
    ) -> Result<()> {
        let _source_scope = (role == XmlWriteRole::Source).then(|| root_binding_scope(0));
        crate::document::admit_word_model(crate::document::WordOwner::Paragraph(self))?;
        if para_id.is_none()
            && self.properties.is_none()
            && self.runs.is_empty()
            && self.hyperlinks.is_empty()
            && self.comment_ranges.is_empty()
            && self.bookmark_markers.is_empty()
            && self.extra_xml.is_empty()
            && self.content_controls.is_empty()
            && self.revisions.is_empty()
            && self.equations.is_empty()
        {
            let mut start = FieldXmlStart::new("w:p", emission)?;
            if role == XmlWriteRole::Source {
                start.qualify(&[], role, source_context)?;
            } else {
                start.restore_word_binding()?;
            }
            start.write(writer, true)?;
            return Ok(());
        }

        let mut start = FieldXmlStart::new("w:p", emission)?;
        for (position, raw) in &self.extra_xml {
            if Self::raw_is_root_attributes(*position, raw) {
                start.root_record(raw, para_id.map(|_| (W14_NS, "paraId")), role)?;
            }
        }
        if let Some(para_id) = para_id {
            start.push("w14:paraId", para_id)?;
        }
        // Retained root declarations also govern the generated Publication
        // QName. A local foreign w binding must keep its opaque names intact.
        let word_prefix = start.qualify(&[], role, source_context)?;
        let own_bindings = raw_namespace_declarations(start.element())?;
        let mut child_context = source_context.unwrap_or(&[]).to_vec();
        for (prefix, uri) in &own_bindings {
            child_context.retain(|(known, _)| *known != prefix);
            child_context.push((prefix, uri));
        }
        let _paragraph_scope = emitted_binding_scope(start.element(), false, false)?;
        start.write(writer, false)?;

        self.write_xml_content_in(
            writer,
            role,
            (role == XmlWriteRole::Source).then_some(child_context.as_slice()),
            work,
            emission,
        )?;
        if let Some(emission) = emission {
            emission.write_word_end(writer, &word_prefix, "p")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:p"))))?;
        }
        Ok(())
    }

    /// The single substantive paragraph content emitter. A source fragment has
    /// no discarded paragraph root from which it could inherit declarations.
    pub(crate) fn write_xml_content<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        let mut work = WordOperationWork::model_fields(self.writer_field_roots())?;
        self.write_xml_content_in(writer, role, source_context, &mut work, None)
    }

    pub(crate) fn write_xml_content_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        work: &mut WordOperationWork,
        emission: Option<&FieldXmlEmission>,
    ) -> Result<()> {
        self.write_xml_content_mapped_in(writer, role, source_context, work, emission, None)
    }

    pub(crate) fn write_xml_content_mapped_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        work: &mut WordOperationWork,
        emission: Option<&FieldXmlEmission>,
        extra_maps: Option<&WordExtraSourceMaps>,
    ) -> Result<()> {
        if extra_maps.is_some_and(|maps| maps.len() != self.extra_xml.len()) {
            return Err(OxmlError::InvalidValue(
                "paragraph extra source map alignment changed".into(),
            ));
        }
        let publication_snapshot = emitted_bindings();
        let publication_root_bindings = publication_snapshot.bindings();
        if let Some(ref props) = self.properties {
            props.write_xml_role_in(writer, role, source_context, emission)?;
        }

        // Build a set of run indices that are inside hyperlinks
        let mut hyperlink_runs: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for (hl_idx, hl) in self.hyperlinks.iter().enumerate() {
            for run_idx in hl.run_start..hl.run_end {
                hyperlink_runs.insert(run_idx, hl_idx);
            }
        }

        let mut current_hyperlink: Option<usize> = None;
        let mut hyperlink_scope = None;
        let mut current_ruby: Option<usize> = None;
        let mut ruby_scope = None;
        let mut written_field_owner = None;
        let mut written_span_end = 0;
        for (run_idx, run) in self.runs.iter().enumerate() {
            if run_idx < written_span_end {
                continue;
            }
            let in_hl = hyperlink_runs.get(&run_idx).copied();

            // A ruby annotation wraps its base runs, so it closes before any
            // paragraph boundary content that sits between two runs.
            if let Some(ruby_index) = current_ruby
                && self.rubies[ruby_index].base_end == run_idx
            {
                self.rubies[ruby_index].write_end(writer, role, source_context, emission)?;
                current_ruby = None;
                ruby_scope = None;
            }

            // Paragraph boundary content is a sibling of the hyperlink.
            if current_hyperlink.is_some() && current_hyperlink != in_hl {
                let hyperlink_index = current_hyperlink.expect("open hyperlink exists");
                write_hyperlink_boundary(
                    writer,
                    &self.revisions,
                    hyperlink_index,
                    &self.hyperlinks[hyperlink_index],
                    run_idx,
                    role,
                    source_context,
                    Some(work),
                    emission,
                )?;
                if let Some(emission) = emission {
                    emission.write_word_end_planned(
                        writer,
                        safe_hyperlink_name(
                            &self.hyperlinks[hyperlink_index],
                            "w",
                            "rdocxW",
                            crate::namespace::W_NS,
                            source_context,
                            role,
                        ),
                        "hyperlink",
                    )?;
                } else {
                    writer.write_event(Event::End(BytesEnd::new(hyperlink_qname(
                        &self.hyperlinks[hyperlink_index],
                        role,
                        source_context,
                    ))))?;
                }
                current_hyperlink = None;
                hyperlink_scope = None;
            }

            write_paragraph_boundary(
                writer,
                ParagraphBoundary {
                    role,
                    source_context,
                    emission,
                    extra_xml: &self.extra_xml,
                    extra_maps,
                    content_controls: &self.content_controls,
                    markers: &self.comment_ranges,
                    hyperlinks: &self.hyperlinks,
                    revisions: &self.revisions,
                    equations: &self.equations,
                },
                run_idx,
                work,
            )?;

            write_empty_hyperlinks(
                writer,
                &self.hyperlinks,
                &self.revisions,
                run_idx,
                role,
                source_context,
                work,
                emission,
            )?;

            // Open hyperlink if entering one
            if let Some(hl_idx) = in_hl
                && current_hyperlink != in_hl
            {
                let hl = &self.hyperlinks[hl_idx];
                hyperlink_scope = Some(write_hyperlink_start_in(
                    writer,
                    hl,
                    role,
                    source_context,
                    emission,
                )?);
                current_hyperlink = in_hl;
            }

            if let Some(hyperlink_index) = current_hyperlink {
                write_hyperlink_boundary(
                    writer,
                    &self.revisions,
                    hyperlink_index,
                    &self.hyperlinks[hyperlink_index],
                    run_idx,
                    role,
                    source_context,
                    Some(work),
                    emission,
                )?;
            }

            let mut run_context = source_context.unwrap_or(&[]).to_vec();
            if let Some(index) = current_hyperlink {
                for (name, uri) in &self.hyperlinks[index].extra_attributes {
                    if let Some(prefix) = name
                        .strip_prefix("xmlns:")
                        .or_else(|| (name == "xmlns").then_some(""))
                    {
                        run_context.retain(|(known, _)| *known != prefix);
                        run_context.push((prefix, uri));
                    }
                }
            }
            // Ruby's retained vectors have the approved canonical-Word
            // consumer contract, established on its surviving generated root.
            let run_source_context =
                (role == XmlWriteRole::Source).then_some(run_context.as_slice());
            let foreign_word_namespace = current_hyperlink
                .and_then(|index| shadowed_word_namespace(&self.hyperlinks[index]));
            let mut publication_scope = vec![("w", crate::namespace::W_NS)];
            for &(prefix, namespace) in &publication_root_bindings {
                publication_scope.retain(|(known, _)| *known != prefix);
                publication_scope.push((prefix, namespace));
            }
            if let Some(index) = current_hyperlink {
                for (name, namespace) in &self.hyperlinks[index].extra_attributes {
                    if let Some(prefix) = name
                        .strip_prefix("xmlns:")
                        .or_else(|| (name == "xmlns").then_some(""))
                    {
                        publication_scope.retain(|(known, _)| *known != prefix);
                        publication_scope.push((prefix, namespace.as_str()));
                    }
                }
            }
            if let Some((span_end, raw, true, owner)) = self.field_span_at(run_idx) {
                write_field_raw_output(
                    writer,
                    raw,
                    owner,
                    foreign_word_namespace,
                    (role == XmlWriteRole::Publication).then_some(publication_scope.as_slice()),
                    work,
                    None,
                )?;
                written_field_owner = None;
                written_span_end = span_end;
                continue;
            }

            // Fields are paragraph children even though the facade stores them
            // in synthetic runs for the existing inline traversal contract.
            if run.content.len() == 1
                && let RunContent::Field(field) = &run.content[0]
            {
                if field.span.is_some() {
                    if role == XmlWriteRole::Publication {
                        write_published_run_field(
                            writer,
                            field,
                            foreign_word_namespace,
                            run.properties.as_ref(),
                            &publication_scope,
                            work,
                        )?;
                    } else {
                        write_run_field(
                            writer,
                            field,
                            foreign_word_namespace,
                            run.properties.as_ref(),
                            run_source_context,
                            work,
                        )?;
                    }
                    written_field_owner = None;
                    continue;
                }
                let owner_id = field.source_owner_id();
                if owner_id.is_some() && owner_id == written_field_owner {
                    continue;
                }
                let field = if let Some(owner_id) = owner_id {
                    let owner_fields = self.runs[run_idx..]
                        .iter()
                        .map_while(|run| match run.content.as_slice() {
                            [RunContent::Field(field)]
                                if field.source_owner_id() == Some(owner_id) =>
                            {
                                Some(field)
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    let owner_changed = owner_fields.iter().any(|field| !field.is_unchanged());
                    if owner_fields.len() > 1 && owner_changed {
                        return Err(OxmlError::InvalidValue(
                            "a field sharing one physical run was changed".to_owned(),
                        ));
                    }
                    written_field_owner = Some(owner_id);
                    field
                } else {
                    written_field_owner = None;
                    field
                };
                write_field_with_result_properties(
                    writer,
                    field,
                    current_hyperlink
                        .and_then(|index| shadowed_word_namespace(&self.hyperlinks[index])),
                    run.properties.as_ref(),
                    (role == XmlWriteRole::Publication).then_some(publication_scope.as_slice()),
                    run_source_context,
                    work,
                )?;
                continue;
            }

            if run
                .content
                .iter()
                .any(|content| matches!(content, RunContent::Field(_)))
            {
                for field in run.content.iter().filter_map(|content| match content {
                    RunContent::Field(field) => Some(field),
                    _ => None,
                }) {
                    let Some(owner_id) = field.source_owner_id() else {
                        continue;
                    };
                    let owner_fields = self
                        .runs
                        .iter()
                        .flat_map(|candidate| &candidate.content)
                        .filter(|content| {
                            matches!(content, RunContent::Field(candidate) if candidate.source_owner_id() == Some(owner_id))
                        })
                        .count();
                    if owner_fields > 1 {
                        return Err(OxmlError::InvalidValue(
                            "a field sharing one physical run was changed".to_owned(),
                        ));
                    }
                }
                written_field_owner = None;
                write_mixed_field_run(
                    writer,
                    run,
                    current_hyperlink
                        .and_then(|index| shadowed_word_namespace(&self.hyperlinks[index])),
                    (role == XmlWriteRole::Publication).then_some(publication_scope.as_slice()),
                    run_source_context,
                    work,
                    emission,
                )?;
                continue;
            }

            written_field_owner = None;

            if current_ruby.is_none()
                && let Some(ruby_index) = self
                    .rubies
                    .iter()
                    .position(|ruby| ruby.base_start == run_idx && ruby.base_end > run_idx)
            {
                ruby_scope = Some(self.rubies[ruby_index].write_start(
                    writer,
                    role,
                    source_context,
                    emission,
                )?);
                current_ruby = Some(ruby_index);
            }

            if let Some(hyperlink_index) = current_hyperlink {
                run.write_xml_role(
                    writer,
                    shadowed_word_namespace(&self.hyperlinks[hyperlink_index]),
                    role,
                    source_context,
                    emission,
                    work.template_word_capture(),
                )?;
            } else {
                run.write_xml_role(
                    writer,
                    None,
                    role,
                    run_source_context,
                    emission,
                    work.template_word_capture(),
                )?;
            }
        }

        if let Some(ruby_index) = current_ruby {
            self.rubies[ruby_index].write_end(writer, role, source_context, emission)?;
        }

        drop(ruby_scope);
        // Close any remaining open hyperlink
        if let Some(hyperlink_index) = current_hyperlink {
            write_hyperlink_boundary(
                writer,
                &self.revisions,
                hyperlink_index,
                &self.hyperlinks[hyperlink_index],
                self.runs.len(),
                role,
                source_context,
                Some(work),
                emission,
            )?;
            if let Some(emission) = emission {
                emission.write_word_end_planned(
                    writer,
                    safe_hyperlink_name(
                        &self.hyperlinks[hyperlink_index],
                        "w",
                        "rdocxW",
                        crate::namespace::W_NS,
                        source_context,
                        role,
                    ),
                    "hyperlink",
                )?;
            } else {
                writer.write_event(Event::End(BytesEnd::new(hyperlink_qname(
                    &self.hyperlinks[hyperlink_index],
                    role,
                    source_context,
                ))))?;
            }
        }

        drop(hyperlink_scope);
        write_paragraph_boundary(
            writer,
            ParagraphBoundary {
                role,
                source_context,
                emission,
                extra_xml: &self.extra_xml,
                extra_maps,
                content_controls: &self.content_controls,
                markers: &self.comment_ranges,
                hyperlinks: &self.hyperlinks,
                revisions: &self.revisions,
                equations: &self.equations,
            },
            self.runs.len(),
            work,
        )?;
        write_empty_hyperlinks(
            writer,
            &self.hyperlinks,
            &self.revisions,
            self.runs.len(),
            role,
            source_context,
            work,
            emission,
        )?;

        Ok(())
    }
}

fn write_paragraph_boundary<W: std::io::Write>(
    writer: &mut Writer<W>,
    boundary: ParagraphBoundary<'_>,
    run_index: usize,
    work: &mut WordOperationWork,
) -> Result<()> {
    let extras = boundary
        .extra_xml
        .iter()
        .enumerate()
        .filter(|(_, (position, _))| *position == run_index)
        .map(|(index, (_, raw))| (index, raw))
        .collect::<Vec<_>>();
    for raw_index in 0..=extras.len() {
        let boundary_markers = boundary
            .markers
            .iter()
            .filter(|marker| {
                marker.run_index() == run_index
                    && marker.raw_before().min(extras.len()) == raw_index
            })
            .collect::<Vec<_>>();
        for marker_index in 0..=boundary_markers.len() {
            for (_, _, _, sdt) in
                boundary
                    .content_controls
                    .iter()
                    .filter(|(at, raw_before, markers_before, _)| {
                        *at == run_index
                            && (*raw_before).min(extras.len()) == raw_index
                            && (*markers_before).min(boundary_markers.len()) == marker_index
                    })
            {
                sdt.write_xml_role_in(
                    writer,
                    boundary.role,
                    boundary.source_context,
                    work,
                    boundary.emission,
                )?;
            }
            if let Some(marker) = boundary_markers.get(marker_index) {
                let (tag, id) = match marker {
                    CommentRangeMarker::Start { id, .. } => ("w:commentRangeStart", *id),
                    CommentRangeMarker::End { id, .. } => ("w:commentRangeEnd", *id),
                };
                let mut value = itoa::Buffer::new();
                let mut element = FieldXmlStart::new(tag, boundary.emission)?;
                element.push("w:id", value.format(id))?;
                element.write(writer, true)?;
            }
        }
        if let Some((source_index, raw)) = extras.get(raw_index) {
            if let Some((_, _, equation)) = boundary
                .equations
                .iter()
                .find(|(at, slot, _)| *at == run_index && *slot == raw_index)
            {
                equation.write_xml_in(writer, boundary.emission)?;
            } else if let Some((_, _, revision)) =
                boundary.revisions.iter().find(|(at, slot, _)| {
                    *at == run_index
                        && hyperlink_revision_index(*slot).is_none()
                        && *slot == raw_index
                })
            {
                revision.write_xml_role_in(
                    writer,
                    boundary.role,
                    boundary.source_context,
                    work,
                    boundary.emission,
                )?;
            } else if let Some((hyperlink_index, _hyperlink)) = boundary
                .hyperlinks
                .iter()
                .enumerate()
                .find(|(_, hyperlink)| {
                    hyperlink.run_start == run_index
                        && hyperlink.run_end == run_index
                        && hyperlink.preserved_raw_before == Some(raw_index)
                })
            {
                let replacement = if let Some(emission) = boundary.emission {
                    let (output, _outer) = emission.new_source_record_output();
                    let mut replacement_writer = Writer::new(output);
                    write_hyperlink_record(
                        &mut replacement_writer,
                        &boundary,
                        hyperlink_index,
                        run_index,
                        Some(work),
                    )
                    .map_err(|error| work.writer_error(error))?;
                    replacement_writer.into_inner().into_mapped_parts().0
                } else {
                    let mut replacement = Vec::new();
                    write_hyperlink_record(
                        &mut Writer::new(&mut replacement),
                        &boundary,
                        hyperlink_index,
                        run_index,
                        Some(work),
                    )?;
                    replacement
                };
                // Only this exact already-debited record can reuse its receipt.
                let _prepaid = boundary
                    .emission
                    .map(|emission| emission.prepaid_xml_transport(&replacement))
                    .transpose()?;
                writer.get_mut().write_all(&replacement)?;
            } else {
                let _transport = boundary
                    .extra_maps
                    .and_then(|maps| maps[*source_index].as_deref())
                    .map(|map| work.mapped_source_record(raw, map.clone()))
                    .transpose()?;
                writer.get_mut().write_all(raw)?;
            }
        }
    }
    Ok(())
}

pub(super) fn write_hyperlink_record<W: std::io::Write>(
    writer: &mut Writer<W>,
    boundary: &ParagraphBoundary<'_>,
    index: usize,
    run_index: usize,
    work: Option<&mut WordOperationWork>,
) -> Result<()> {
    let hyperlink = &boundary.hyperlinks[index];
    let _scope = write_hyperlink_start_in(
        writer,
        hyperlink,
        boundary.role,
        boundary.source_context,
        boundary.emission,
    )?;
    write_hyperlink_boundary(
        writer,
        boundary.revisions,
        index,
        hyperlink,
        run_index,
        boundary.role,
        boundary.source_context,
        work,
        boundary.emission,
    )?;
    if let Some(emission) = boundary.emission {
        let plan = safe_hyperlink_name(
            hyperlink,
            "w",
            "rdocxW",
            crate::namespace::W_NS,
            boundary.source_context,
            boundary.role,
        );
        emission.write_word_end_planned(writer, plan, "hyperlink")?;
    } else {
        writer.write_event(Event::End(BytesEnd::new(hyperlink_qname(
            hyperlink,
            boundary.role,
            boundary.source_context,
        ))))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_hyperlink_boundary<W: std::io::Write>(
    writer: &mut Writer<W>,
    revisions: &[(usize, usize, CT_Revision)],
    hyperlink_index: usize,
    hyperlink: &HyperlinkSpan,
    run_index: usize,
    role: XmlWriteRole,
    source_context: Option<&[(&str, &str)]>,
    mut work: Option<&mut WordOperationWork>,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    let boundary_revisions = revisions
        .iter()
        .filter(|(at, slot, _)| {
            *at == run_index && hyperlink_revision_index(*slot) == Some(hyperlink_index)
        })
        .map(|(_, _, revision)| revision)
        .collect::<Vec<_>>();
    let relative_boundary = run_index.saturating_sub(hyperlink.run_start);
    for revision_index in 0..=boundary_revisions.len() {
        for (_, _, raw) in hyperlink.extra_xml.iter().filter(|(boundary, before, _)| {
            *boundary == relative_boundary
                && (*before).min(boundary_revisions.len()) == revision_index
        }) {
            writer.get_mut().write_all(raw)?;
        }
        if let Some(revision) = boundary_revisions.get(revision_index) {
            if let Some(work) = work.as_deref_mut() {
                revision.write_xml_role_in(writer, role, source_context, work, emission)?;
            } else {
                revision.write_xml_with_role_override(
                    writer,
                    None,
                    role,
                    source_context,
                    emission,
                )?;
            }
        }
    }
    Ok(())
}

fn write_hyperlink_start_in<W: std::io::Write>(
    writer: &mut Writer<W>,
    hyperlink: &HyperlinkSpan,
    role: XmlWriteRole,
    source_context: Option<&[(&str, &str)]>,
    emission: Option<&FieldXmlEmission>,
) -> Result<RootBindingScope> {
    let word = safe_hyperlink_name(
        hyperlink,
        "w",
        "rdocxW",
        crate::namespace::W_NS,
        source_context,
        role,
    );
    let relationship = safe_hyperlink_name(hyperlink, "r", "rdocxR", R_NS, source_context, role);
    let (mut element, word_prefix) = FieldXmlStart::planned_word(word, "hyperlink", emission)?;
    if word.declaration {
        element.push_word("xmlns", &word_prefix, crate::namespace::W_NS)?;
    }
    if hyperlink.rel_id.is_some() && relationship.declaration {
        element.push_planned_namespace(relationship, R_NS)?;
    }
    if let Some(value) = &hyperlink.rel_id {
        element.push_planned_word(relationship, "id", value)?;
    }
    if let Some(value) = &hyperlink.anchor {
        element.push_word(&word_prefix, "anchor", value)?;
    }
    if let Some(value) = &hyperlink.tooltip {
        element.push_word(&word_prefix, "tooltip", value)?;
    }
    if let Some(value) = &hyperlink.doc_location {
        element.push_word(&word_prefix, "docLocation", value)?;
    }
    for (name, value) in &hyperlink.extra_attributes {
        element.push(name, value)?;
    }
    let scope = emitted_binding_scope(element.element(), false, false)?;
    element.write(writer, false)?;
    Ok(scope)
}

fn hyperlink_qname(
    hyperlink: &HyperlinkSpan,
    role: XmlWriteRole,
    source_context: Option<&[(&str, &str)]>,
) -> String {
    let (prefix, _) = safe_hyperlink_prefix(
        hyperlink,
        "w",
        "rdocxW",
        crate::namespace::W_NS,
        source_context,
        role,
    );
    format!("{prefix}:hyperlink")
}

fn safe_hyperlink_prefix(
    hyperlink: &HyperlinkSpan,
    preferred: &str,
    fallback: &str,
    namespace: &str,
    source_context: Option<&[(&str, &str)]>,
    role: XmlWriteRole,
) -> (String, bool) {
    let plan = safe_hyperlink_name(
        hyperlink,
        preferred,
        fallback,
        namespace,
        source_context,
        role,
    );
    (plan.render(), plan.declaration)
}

pub(super) fn safe_hyperlink_name<'a>(
    hyperlink: &'a HyperlinkSpan,
    preferred: &'a str,
    fallback: &'a str,
    namespace: &str,
    source_context: Option<&[(&'a str, &str)]>,
    role: XmlWriteRole,
) -> GeneratedWordName<'a> {
    if role == XmlWriteRole::Source {
        let mut bindings = source_context.unwrap_or(&[]).to_vec();
        for (name, uri) in &hyperlink.extra_attributes {
            if let Some(prefix) = name
                .strip_prefix("xmlns:")
                .or_else(|| (name == "xmlns").then_some(""))
            {
                bindings.retain(|(known, _)| *known != prefix);
                bindings.push((prefix, uri));
            }
        }
        if bindings.contains(&(preferred, namespace)) {
            return GeneratedWordName {
                prefix: GeneratedWordPrefix::Existing(preferred),
                declaration: false,
            };
        }
        if let Some((prefix, _)) = bindings
            .iter()
            .find(|(prefix, uri)| !prefix.is_empty() && *uri == namespace)
        {
            return GeneratedWordName {
                prefix: GeneratedWordPrefix::Existing(prefix),
                declaration: false,
            };
        }
        if !bindings.iter().any(|(prefix, _)| *prefix == preferred) {
            return GeneratedWordName {
                prefix: GeneratedWordPrefix::Existing(preferred),
                declaration: true,
            };
        }
        for suffix in 0usize.. {
            let mut digits = itoa::Buffer::new();
            let tail = if suffix == 0 {
                ""
            } else {
                digits.format(suffix)
            };
            if !bindings
                .iter()
                .any(|(prefix, _)| prefix.strip_prefix(fallback) == Some(tail))
            {
                return GeneratedWordName {
                    prefix: GeneratedWordPrefix::Fresh(fallback, suffix),
                    declaration: true,
                };
            }
        }
        unreachable!();
    }
    match hyperlink
        .extra_attributes
        .iter()
        .find(|(name, _)| name.strip_prefix("xmlns:") == Some(preferred))
    {
        None => {
            return GeneratedWordName {
                prefix: GeneratedWordPrefix::Existing(preferred),
                declaration: false,
            };
        }
        Some((_, value)) if value == namespace => {
            return GeneratedWordName {
                prefix: GeneratedWordPrefix::Existing(preferred),
                declaration: false,
            };
        }
        Some(_) => {}
    }
    for suffix in 0usize.. {
        let mut digits = itoa::Buffer::new();
        let tail = if suffix == 0 {
            ""
        } else {
            digits.format(suffix)
        };
        if !hyperlink.extra_attributes.iter().any(|(name, _)| {
            name.strip_prefix("xmlns:")
                .and_then(|prefix| prefix.strip_prefix(fallback))
                == Some(tail)
        }) {
            return GeneratedWordName {
                prefix: GeneratedWordPrefix::Fresh(fallback, suffix),
                declaration: true,
            };
        }
    }
    unreachable!("the finite attribute set cannot occupy every prefix")
}

fn shadowed_word_namespace(hyperlink: &HyperlinkSpan) -> Option<&str> {
    hyperlink
        .extra_attributes
        .iter()
        .find(|(name, value)| name == "xmlns:w" && value != crate::namespace::W_NS)
        .map(|(_, value)| value.as_str())
}

#[allow(clippy::too_many_arguments)] // The inherited Field role is independent of physical hyperlink boundaries.
fn write_empty_hyperlinks<W: std::io::Write>(
    writer: &mut Writer<W>,
    hyperlinks: &[HyperlinkSpan],
    revisions: &[(usize, usize, CT_Revision)],
    run_index: usize,
    role: XmlWriteRole,
    source_context: Option<&[(&str, &str)]>,
    work: &mut WordOperationWork,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    for (hyperlink_index, hyperlink) in hyperlinks.iter().enumerate().filter(|(_, hyperlink)| {
        hyperlink.run_start == run_index
            && hyperlink.run_end == run_index
            && hyperlink.preserved_raw_before.is_none()
    }) {
        let _hyperlink_scope =
            write_hyperlink_start_in(writer, hyperlink, role, source_context, emission)?;
        write_hyperlink_boundary(
            writer,
            revisions,
            hyperlink_index,
            hyperlink,
            run_index,
            role,
            source_context,
            Some(work),
            emission,
        )?;
        if let Some(emission) = emission {
            emission.write_word_end_planned(
                writer,
                safe_hyperlink_name(
                    hyperlink,
                    "w",
                    "rdocxW",
                    crate::namespace::W_NS,
                    source_context,
                    role,
                ),
                "hyperlink",
            )?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(hyperlink_qname(
                hyperlink,
                role,
                source_context,
            ))))?;
        }
    }
    Ok(())
}
