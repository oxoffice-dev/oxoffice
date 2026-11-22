//! Simple fields (fldSimple): parsing, ordered results and their ownership.

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct SimpleFieldHeader {
    syntax: Option<InstructionSyntax>,
    dirty: Option<bool>,
    instruction_attribute: Option<usize>,
    dirty_attribute: Option<usize>,
}
fn simple_header_attribute_index(
    start: &BytesStart<'_>,
    prefixes: &[String],
    local: &[u8],
) -> Option<usize> {
    start
        .attributes()
        .enumerate()
        .find_map(|(index, attribute)| {
            let attribute = attribute.ok()?;
            crate::properties::is_word_attribute(attribute.key.as_ref(), local, prefixes)
                .then_some(index)
        })
}

impl SimpleFieldHeader {
    pub(crate) fn read(start: &BytesStart<'_>, prefixes: &[String], empty: bool) -> Self {
        let instruction_attribute = simple_header_attribute_index(start, prefixes, b"instr");
        let Some(instruction) = optional_word_attribute(start, b"instr", prefixes) else {
            return Self {
                syntax: None,
                dirty: None,
                instruction_attribute,
                dirty_attribute: None,
            };
        };
        let syntax =
            classify_instruction_tokens(instruction.to_owned(), lex_field_text(&instruction));
        if empty && syntax.name.is_empty() {
            return Self {
                syntax: None,
                dirty: None,
                instruction_attribute,
                dirty_attribute: None,
            };
        }
        let dirty = optional_word_attribute(start, b"dirty", prefixes)
            .and_then(|value| crate::shared::ST_OnOff::parse(&value));
        Self {
            syntax: Some(syntax),
            dirty,
            instruction_attribute,
            dirty_attribute: simple_header_attribute_index(start, prefixes, b"dirty"),
        }
    }
    pub(crate) fn matches_attributes(&self, start: &BytesStart<'_>, prefixes: &[String]) -> bool {
        self.instruction_attribute == simple_header_attribute_index(start, prefixes, b"instr")
            && (self.syntax.is_none()
                || self.dirty_attribute == simple_header_attribute_index(start, prefixes, b"dirty"))
    }
    pub(crate) fn is_raw(&self) -> bool {
        self.syntax.is_none()
    }
}

pub(super) fn parse_empty_simple_field(
    start: &BytesStart<'_>,
    raw: &[u8],
    prefixes: Vec<String>,
    mapped: Option<&crate::document::WordFieldInput>,
) -> Result<Option<(Field, ParsedFieldSummary)>> {
    let header = SimpleFieldHeader::read(start, &prefixes, true);
    let Some(syntax) = header.syntax else {
        return Ok(None);
    };
    let dirty = header.dirty;
    let metadata = ParsedFieldMetadata::read(
        crate::document::WordXmlInput::Contiguous(raw),
        FieldForm::Simple,
        &prefixes,
        &syntax.name,
        "",
    );
    let summary = admit_initial_field_capture(
        &syntax,
        &[],
        "",
        raw.len(),
        &prefixes,
        &metadata,
        ParsedFieldCache::Runs {
            runs: &[],
            summaries: Some(&[]),
        },
        mapped.map(crate::document::WordFieldInput::registry),
    )?;
    let instruction = syntax.materialize(Vec::new()).0;
    Ok(Some((
        Field::parsed(
            instruction,
            String::new(),
            Vec::new(),
            dirty,
            FieldForm::Simple,
            (raw.to_vec(), prefixes),
            Some(metadata),
        ),
        summary,
    )))
}

pub(crate) fn parse_simple_field(
    raw: &[u8],
    word_prefixes: &[String],
    mapped: Option<&crate::document::WordFieldInput>,
) -> Result<Option<Field>> {
    Ok(
        parse_simple_field_with_summary(raw, word_prefixes, None, None, mapped)?
            .map(|(field, _)| field),
    )
}

pub(super) fn parse_simple_field_with_summary(
    raw: &[u8],
    word_prefixes: &[String],
    prepared_header: Option<SimpleFieldHeader>,
    prepared_selector: Option<(bool, std::ops::Range<usize>)>,
    mapped: Option<&crate::document::WordFieldInput>,
) -> Result<Option<(Field, ParsedFieldSummary)>> {
    let parsed = parse_simple_field_body(
        raw,
        word_prefixes,
        prepared_header,
        prepared_selector,
        mapped,
    )?;
    if let (Some(mapped), Some((field, _))) = (mapped, &parsed) {
        mapped.register(field)?;
    }
    Ok(parsed)
}

fn parse_simple_field_body(
    raw: &[u8],
    word_prefixes: &[String],
    mut prepared_header: Option<SimpleFieldHeader>,
    prepared_selector: Option<(bool, std::ops::Range<usize>)>,
    mapped: Option<&crate::document::WordFieldInput>,
) -> Result<Option<(Field, ParsedFieldSummary)>> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let (syntax, dirty, prefixes) = loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(start) if matches_local_name(start.name().as_ref(), b"fldSimple") => {
                let prefixes = word_prefixes_at(&start, word_prefixes)?;
                if !is_word_element(start.name().as_ref(), b"fldSimple", &prefixes) {
                    return Ok(None);
                }
                let header = prepared_header
                    .take()
                    .unwrap_or_else(|| SimpleFieldHeader::read(&start, &prefixes, false));
                let Some(syntax) = header.syntax else {
                    return Ok(None);
                };
                break (syntax, header.dirty, prefixes);
            }
            Event::Empty(start) if matches_local_name(start.name().as_ref(), b"fldSimple") => {
                let prefixes = word_prefixes_at(&start, word_prefixes)?;
                if !is_word_element(start.name().as_ref(), b"fldSimple", &prefixes) {
                    return Ok(None);
                }
                return parse_empty_simple_field(&start, raw, prefixes.into_owned(), mapped);
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    };

    let initial_result_start = reader.buffer_position() as usize;
    let (ordered, result_bounds) = if let Some(prepared) = prepared_selector {
        prepared
    } else {
        simple_result_ownership(raw, &prefixes, initial_result_start)?
    };
    if ordered {
        crate::document::admit_paragraph_result_field_input(
            raw,
            &prefixes,
            initial_result_start,
            result_bounds.clone(),
            mapped.map(crate::document::WordFieldInput::registry),
        )?;
        let result_input = mapped
            .map(|input| input.slice(result_bounds.clone()))
            .transpose()?;
        let paragraph = parse_ordered_field_result(
            &raw[result_bounds.clone()],
            &prefixes,
            result_input.as_ref(),
        )?;
        let cached_result = paragraph.accepted_text();
        if syntax.name.is_empty() {
            return Ok(None);
        }
        let metadata = ParsedFieldMetadata::read(
            crate::document::WordXmlInput::Contiguous(raw),
            FieldForm::Simple,
            &prefixes,
            &syntax.name,
            &cached_result,
        );
        let summary = admit_initial_field_capture(
            &syntax,
            &[],
            &cached_result,
            raw.len(),
            &prefixes,
            &metadata,
            ParsedFieldCache::Ordered {
                paragraph: &paragraph,
                source_bytes: result_bounds.len(),
            },
            mapped.map(crate::document::WordFieldInput::registry),
        )?;
        let instruction = syntax.materialize(Vec::new()).0;
        let mut field = Field::parsed(
            instruction,
            cached_result.clone(),
            Vec::new(),
            dirty,
            FieldForm::Simple,
            (raw.to_vec(), prefixes.into_owned()),
            Some(metadata),
        );
        field.ordered_result = Some(OrderedFieldResult {
            paragraph: Arc::new(paragraph),
            normalized_xml: Arc::new(raw[result_bounds.clone()].to_vec()),
            source_range: result_bounds,
            normalized_cache: cached_result,
            pending: false,
            pending_payload: false,
            pending_payload_source_bytes: 0,
        });
        field
            .ordered_result
            .as_ref()
            .expect("retained result")
            .paragraph
            .append_ordered_comment_ranges(&mut 0, &mut field.parsed_cached_comment_ranges);
        return Ok(Some((field, summary)));
    }
    let result_xml = &raw[result_bounds.clone()];
    let possible_nested = result_xml.windows(7).any(|bytes| bytes == b"fldChar")
        || result_xml.windows(9).any(|bytes| bytes == b"fldSimple")
        || source_has_note_reference_name(result_xml);
    if possible_nested {
        crate::document::admit_paragraph_result_field_input(
            raw,
            &prefixes,
            initial_result_start,
            result_bounds.clone(),
            mapped.map(crate::document::WordFieldInput::registry),
        )?;
        // This is the canonical construction for this result, before any flat
        // run pass. Its nested fields and note references have one identity.
        let result_input = mapped
            .map(|input| input.slice(result_bounds.clone()))
            .transpose()?;
        let paragraph = parse_ordered_field_result(result_xml, &prefixes, result_input.as_ref())?;
        if syntax.name.is_empty() {
            return Ok(None);
        }
        let cached_result: String = paragraph.runs.iter().map(cached_run_display).collect();
        let metadata = ParsedFieldMetadata::read(
            crate::document::WordXmlInput::Contiguous(raw),
            FieldForm::Simple,
            &prefixes,
            &syntax.name,
            &cached_result,
        );
        let summary = admit_initial_field_capture(
            &syntax,
            &[],
            &cached_result,
            raw.len(),
            &prefixes,
            &metadata,
            ParsedFieldCache::Runs {
                runs: &paragraph.runs,
                summaries: None,
            },
            mapped.map(crate::document::WordFieldInput::registry),
        )?;
        let instruction = syntax.materialize(Vec::new()).0;
        let mut field = Field::parsed(
            instruction,
            cached_result,
            Vec::new(),
            dirty,
            FieldForm::Simple,
            (raw.to_vec(), prefixes.into_owned()),
            Some(metadata),
        );
        field.parsed_cached_comment_ranges = paragraph.comment_ranges;
        field.simple_cached_runs = Some(paragraph.runs);
        return Ok(Some((field, summary)));
    }
    let cursor = mapped.map(|input| input.cursor(raw)).transpose()?;
    let mut cached_result = String::new();
    let mut cached_segments = Vec::new();
    let mut result_run_index = 0usize;
    let mut result_runs = Vec::new();
    loop {
        buffer.clear();
        let event_start = reader.buffer_position();
        match reader.read_event_into(&mut buffer)? {
            Event::Start(start) => {
                let child_prefixes = word_prefixes_at(&start, &prefixes)?;
                if is_word_element(start.name().as_ref(), b"r", &child_prefixes) {
                    let run_raw = capture_element(&mut reader, &start)?;
                    let child_input = cursor.as_ref().and_then(|cursor| {
                        cursor.field_input(&reader, event_start..reader.buffer_position(), &run_raw)
                    });
                    let run = parse_run_raw_with_input(
                        &run_raw,
                        &child_prefixes,
                        None,
                        child_input.as_ref(),
                    )?;
                    if let Some(text) = simple_result_run_display(&run_raw, &child_prefixes)? {
                        cached_result.push_str(&text);
                        cached_segments.push(CachedDisplaySegment {
                            run_index: result_run_index,
                            text,
                            properties: run.properties.clone(),
                        });
                    }
                    result_runs.push(run);
                    result_run_index += 1;
                } else {
                    reader.read_to_end_into(start.name(), &mut Vec::new())?;
                }
            }
            Event::End(end) if matches_local_name(end.name().as_ref(), b"fldSimple") => {
                break;
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if syntax.name.is_empty() {
        return Ok(None);
    }
    let metadata = ParsedFieldMetadata::read(
        crate::document::WordXmlInput::Contiguous(raw),
        FieldForm::Simple,
        &prefixes,
        &syntax.name,
        &cached_result,
    );
    let summary = admit_initial_field_capture(
        &syntax,
        &[],
        &cached_result,
        raw.len(),
        &prefixes,
        &metadata,
        ParsedFieldCache::Runs {
            runs: &result_runs,
            summaries: None,
        },
        mapped.map(crate::document::WordFieldInput::registry),
    )?;
    let instruction = syntax.materialize(Vec::new()).0;
    let mut field = Field::parsed(
        instruction,
        cached_result,
        cached_segments,
        dirty,
        FieldForm::Simple,
        (raw.to_vec(), prefixes.to_vec()),
        Some(metadata),
    );
    // Keep physical facts even when opaque source children have a legacy
    // scalar display. Semantic cache readers still require display agreement.
    field.simple_cached_runs = Some(result_runs);
    Ok(Some((field, summary)))
}

/// Carry source identity only along the actual edited paragraph's same logical
/// path/content slot, after the emitted candidate has been parsed independently.
pub(crate) fn retain_ordered_field_source_identities(
    previous: &CT_P,
    current: &mut CT_P,
) -> Result<()> {
    retain_ordered_field_source_identities_in(previous, current, None)
}

pub(crate) fn retain_ordered_field_source_identities_in(
    previous: &CT_P,
    current: &mut CT_P,
    origins: Option<&Arc<std::sync::Mutex<FieldOriginRegistry>>>,
) -> Result<()> {
    let old_paths = previous.accepted_run_paths();
    let parsed_paths = current.accepted_run_paths();
    if old_paths.len() != parsed_paths.len()
        || old_paths
            .iter()
            .zip(&parsed_paths)
            .any(|(old, new)| old.segments() != new.segments())
    {
        // The actual writer's new reader may still hold deferred raw revisions.
        // Materialize its checked semantic view before the unchanged identity
        // proof. This does not relax or replace any path/field fact comparison.
        crate::revision::prepare_semantic_paragraph_projection(current)?;
    }
    // These Arc aliases were made by RevisionSourceFacts from these exact
    // freshly parsed physical revision occurrences, before any rebinding COW.
    let revision_sources = current
        .revisions
        .iter()
        .map(|(_, _, revision)| revision.shared_content_paragraph())
        .collect::<Vec<_>>();
    let new_paths = current.accepted_run_paths();
    if old_paths.len() != new_paths.len()
        || old_paths
            .iter()
            .zip(&new_paths)
            .any(|(old, new)| old.segments() != new.segments())
    {
        return Err(OxmlError::InvalidValue(
            "normalized field owner changed its accepted source paths".into(),
        ));
    }
    for path in old_paths {
        let old = previous.accepted_run(&path).ok_or_else(|| {
            OxmlError::InvalidValue("previous ordered field path is stale".into())
        })?;
        let new = current
            .accepted_run_segments_mut(path.segments())
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized ordered field path is stale".into())
            })?;
        if old.content.len() != new.content.len() {
            if old
                .content
                .iter()
                .chain(&new.content)
                .any(|content| matches!(content, RunContent::Field(_)))
            {
                return Err(OxmlError::InvalidValue(
                    "normalized field source content slots changed".into(),
                ));
            }
            continue;
        }
        for (old, new) in old.content.iter().zip(&mut new.content) {
            if let (RunContent::Field(old), RunContent::Field(new)) = (old, new) {
                retain_field_source_identity(old, new, origins)?;
            }
        }
    }
    for ((_, _, revision), source) in current.revisions.iter_mut().zip(revision_sources) {
        if let Some(source) = source {
            revision.synchronize_source_fact_aliases(&source)?;
        }
    }
    Ok(())
}

fn retain_field_source_identity(
    previous: &Field,
    current: &mut Field,
    origins: Option<&Arc<std::sync::Mutex<FieldOriginRegistry>>>,
) -> Result<()> {
    // Capture the actual freshly parsed current source before the identity-only
    // COW below. The old slot's source ID is never proof of physical origin.
    let current_map = origins.and_then(|origins| {
        let registry = origins.lock().expect("field origin registry poisoned");
        registry
            .source_map(current)
            .map(|map| (map, registry.original_for(current)))
    });
    if previous.form() != current.form()
        || previous.effective_instruction() != current.effective_instruction()
        || previous.effective_display() != current.effective_display()
    {
        return Err(OxmlError::InvalidValue(
            "normalized field source slot changed its published facts".into(),
        ));
    }
    if let (
        FieldSource::Parsed {
            source_id,
            owner_id,
            ..
        },
        FieldSource::Parsed {
            source_id: next_id,
            owner_id: next_owner,
            ..
        },
    ) = (previous.source.as_ref(), Arc::make_mut(&mut current.source))
    {
        *next_id = *source_id;
        *next_owner = *owner_id;
    }
    let structure_current = match previous.source.as_ref() {
        FieldSource::Parsed {
            original_instruction,
            ..
        }
        | FieldSource::New {
            original_instruction,
            ..
        } => {
            previous.instruction.raw == original_instruction.raw
                || !instruction_structure_eq(&previous.instruction, original_instruction)
        }
    };
    if structure_current {
        for (old, new) in previous
            .instruction
            .arguments
            .iter()
            .zip(&mut current.instruction.arguments)
        {
            if let (FieldArgument::Nested(old), FieldArgument::Nested(new)) = (old, new) {
                retain_field_source_identity(old, new, origins)?;
            }
        }
        for (old, new) in previous
            .instruction
            .switches
            .iter()
            .zip(&mut current.instruction.switches)
        {
            if let (Some(FieldArgument::Nested(old)), Some(FieldArgument::Nested(new))) =
                (&old.argument, &mut new.argument)
            {
                retain_field_source_identity(old, new, origins)?;
            }
        }
    }
    let children = previous.cached_fields_in_source_order();
    if children.len() != current.cached_fields_in_source_order().len() {
        return Err(OxmlError::InvalidValue(
            "normalized cached field source slot cardinality changed".into(),
        ));
    }
    for (index, old) in children.into_iter().enumerate() {
        let new = current.cached_field_mut(index).ok_or_else(|| {
            OxmlError::InvalidValue("normalized cached field source slot disappeared".into())
        })?;
        retain_field_source_identity(old, new, origins)?;
    }
    if let Some(owner) = current.ordered_result.as_mut() {
        owner.pending = false;
    }
    if let (Some(origins), Some((map, facts))) = (origins, current_map) {
        current.template_word_receipt = previous.template_word_receipt.clone();
        origins
            .lock()
            .expect("field origin registry poisoned")
            .register_forwarded(current, map, facts)?;
    }
    Ok(())
}

pub(super) fn contiguous_ordered_result_runs(paragraph: &CT_P) -> Option<&[CT_R]> {
    let paths = paragraph.accepted_run_paths();
    if paths.len() == paragraph.runs.len()
        && paths
            .iter()
            .enumerate()
            .all(|(index, path)| path.segments() == [AcceptedRunPathSegment::Run(index)])
    {
        return Some(&paragraph.runs);
    }
    if paragraph.runs.is_empty()
        && paragraph.content_controls.is_empty()
        && paragraph.revisions.len() == 1
    {
        return paragraph.revisions[0]
            .2
            .editable_accepted_paragraph()
            .and_then(contiguous_ordered_result_runs);
    }
    None
}

pub(crate) fn ordered_paragraph_xml_in(
    paragraph: &CT_P,
    source_context: Option<&[(&str, &str)]>,
    work: &mut WordOperationWork,
    local_limit: Option<usize>,
) -> Result<Vec<u8>> {
    Ok(ordered_paragraph_mapped_xml_in(paragraph, source_context, work, local_limit)?.0)
}

pub(super) fn ordered_paragraph_mapped_xml_in(
    paragraph: &CT_P,
    source_context: Option<&[(&str, &str)]>,
    work: &mut WordOperationWork,
    local_limit: Option<usize>,
) -> Result<(Vec<u8>, crate::document::WordSourceMap)> {
    let emission = work.field_output_metric.then(|| work.field_xml_emission());
    paragraph_content_mapped_in(
        paragraph,
        source_context,
        work,
        local_limit,
        emission.as_ref(),
        None,
    )
}

pub(crate) fn paragraph_content_mapped_in(
    paragraph: &CT_P,
    source_context: Option<&[(&str, &str)]>,
    work: &mut WordOperationWork,
    local_limit: Option<usize>,
    emission: Option<&FieldXmlEmission>,
    extra_maps: Option<&WordExtraSourceMaps>,
) -> Result<(Vec<u8>, crate::document::WordSourceMap)> {
    let _source_scope = root_binding_scope(0);
    let output = if emission.is_some() {
        work.normalized_output(local_limit)
    } else {
        work.model_source_output()
    };
    let mut writer = Writer::new(output);
    paragraph
        .write_xml_content_mapped_in(
            &mut writer,
            XmlWriteRole::Source,
            source_context,
            work,
            emission,
            extra_maps,
        )
        .map_err(|error| work.writer_error(error))?;
    Ok(writer.into_inner().into_mapped_parts())
}

pub(super) fn replace_ordered_result_text(
    paragraph: &mut CT_P,
    value: &str,
    source_context: Option<&[(&str, &str)]>,
    local_work: usize,
    work: &mut WordOperationWork,
    mut produced: Option<&mut Vec<TemplateProducedText>>,
) -> Result<()> {
    oxoffice_core::xml::reject_non_xml_characters("field result", value)?;
    // These are separate units under the already selected local guard. The
    // operation's aggregate reservation must also surround this producer.
    let overflow =
        || OxmlError::InvalidValue("normalized result exceeds its preflight source work".into());
    let mut actions = 0usize;
    let mut text_bytes = 0usize;
    for part in FieldResultParts::new(value) {
        actions = actions.checked_add(1).ok_or_else(overflow)?;
        if let FieldResultPart::Text(text) = part {
            text_bytes = text_bytes.checked_add(text.len()).ok_or_else(overflow)?;
        }
    }
    if actions > local_work || text_bytes > local_work {
        return Err(overflow());
    }
    let reserved = work.prepare_leaf_actions(actions)?;
    let mut replacement = Vec::with_capacity(actions);
    let mut replacement_inputs = Vec::new();
    for part in FieldResultParts::new(value) {
        if let FieldResultPart::Text(text) = part
            && produced.is_some()
            && let Some(input) = template_word_scalar_input(work, text)
        {
            replacement_inputs.push((replacement.len(), input.origin));
        }
        replacement.push(part.into_run_content());
    }
    work.complete_leaf_actions(reserved);
    let mut written = false;
    for path in paragraph.accepted_run_paths() {
        let Some(mut run) = paragraph.accepted_run(&path).cloned() else {
            return Err(OxmlError::InvalidValue(
                "ordered result text path is stale".into(),
            ));
        };
        let mut next = Vec::new();
        let mut boundaries = Vec::with_capacity(run.content.len() + 1);
        for content in std::mem::take(&mut run.content) {
            boundaries.push(next.len());
            match content {
                RunContent::Text(_) | RunContent::Tab | RunContent::Break(..) => {
                    if !written {
                        let start = next.len();
                        next.extend(replacement.iter().cloned());
                        if let [AcceptedRunPathSegment::Run(index)] = path.segments()
                            && let Some(produced) = produced.as_deref_mut()
                        {
                            // Direct slots retain this actual assignment across
                            // Field-only normalization. Nested owners can refresh
                            // through a parser and need its real Text transcript.
                            for (slot, origin) in &replacement_inputs {
                                produced.push(TemplateProducedText {
                                    run: *index,
                                    content: start + slot,
                                    origin: origin.clone(),
                                });
                            }
                        }
                        written = true;
                    }
                }
                RunContent::Field(mut child) => {
                    if child.locked() == Some(true) {
                        return Err(OxmlError::InvalidValue(
                            "ordered text override reaches a locked nested field owner".into(),
                        ));
                    }
                    child.cached_result = if written {
                        String::new()
                    } else {
                        value.to_owned()
                    };
                    child.normalize_cached_result_owner_in(work)?;
                    written = true;
                    next.push(RunContent::Field(child));
                }
                other => next.push(other),
            }
        }
        boundaries.push(next.len());
        let property = usize::from(run.properties.is_some());
        for at in &mut run.extra_xml_positions {
            let old = CT_R::raw_child_position(*at).saturating_sub(property);
            let mapped = property + boundaries[old.min(boundaries.len() - 1)];
            CT_R::set_raw_child_position(at, mapped);
        }
        run.content = next;
        if !paragraph.replace_ordered_result_slot(path.segments(), vec![run], source_context)? {
            return Err(OxmlError::InvalidValue(
                "ordered text replacement owner disappeared".into(),
            ));
        }
    }
    if !written && !value.is_empty() {
        let mut run = CT_R::new("");
        run.content = replacement;
        if let Some(first) = paragraph
            .accepted_run_paths()
            .first()
            .and_then(|path| paragraph.accepted_run(path))
        {
            run.properties = first.properties.clone();
        }
        let index = paragraph.runs.len();
        if !paragraph.insert_run_after_boundary_items(index, usize::MAX, run) {
            return Err(OxmlError::InvalidValue(
                "ordered result end boundary disappeared".into(),
            ));
        }
        if let Some(produced) = produced {
            for (slot, origin) in replacement_inputs {
                produced.push(TemplateProducedText {
                    run: index,
                    content: slot,
                    origin,
                });
            }
        }
    }
    Ok(())
}

pub(super) fn replace_ordered_result_runs(
    paragraph: &mut CT_P,
    replacement: Vec<CT_R>,
    source_context: Option<&[(&str, &str)]>,
) -> Result<()> {
    paragraph.validate_resource_limits()?;
    let paths = paragraph.accepted_run_paths();
    if paths.is_empty() {
        for (index, run) in replacement.into_iter().enumerate() {
            if !paragraph.insert_unwrapped_run(index, run) {
                return Err(OxmlError::InvalidValue(
                    "ordered empty result anchor disappeared".into(),
                ));
            }
        }
        return Ok(());
    }
    let mut payload = Some(replacement);
    for (index, path) in paths.iter().enumerate().rev() {
        let run = paragraph
            .accepted_run(path)
            .ok_or_else(|| OxmlError::InvalidValue("ordered run source slot disappeared".into()))?;
        for content in &run.content {
            if let RunContent::Field(field) = content
                && field.ordered_result.as_ref().is_some_and(|owner| {
                    !owner.paragraph.revisions.is_empty()
                        || !owner.paragraph.content_controls.is_empty()
                })
            {
                return Err(OxmlError::InvalidValue(
                    "retiring the selected nested field would retire held source owners".into(),
                ));
            }
        }
        let protected = !run.extra_xml.is_empty()
            || !run.alt_drawings.is_empty()
            || run.properties.as_ref().is_some_and(|properties| {
                properties.change.is_some()
                    || !properties.revision_markers.is_empty()
                    || !properties.revision_xml.is_empty()
            });
        let mut next = if index == 0 {
            payload.take().expect("first anchor payload")
        } else {
            Vec::new()
        };
        if protected {
            let mut residual = run.clone();
            residual.replace_content(Vec::new());
            next.push(residual);
        }
        if !paragraph.replace_ordered_result_slot(path.segments(), next, source_context)? {
            return Err(OxmlError::InvalidValue(
                "ordered replacement physical owner disappeared".into(),
            ));
        }
    }
    Ok(())
}

// Only these errors are produced by the in-memory selector event, attribute and
// namespace readers. Keep other errors owned and non-reusable rather than
// changing their payload or claiming a cloneable selector receipt.
#[derive(Debug, Clone)]
enum SelectorInputError {
    Xml(quick_xml::Error),
    Attribute(quick_xml::events::attributes::AttrError),
    Utf8(std::str::Utf8Error),
}

#[derive(Debug)]
enum SelectorFailure {
    Input(SelectorInputError),
    NonReusable(OxmlError),
}

impl From<OxmlError> for SelectorFailure {
    fn from(error: OxmlError) -> Self {
        match error {
            OxmlError::Xml(error) => Self::Input(SelectorInputError::Xml(error)),
            OxmlError::XmlAttr(error) => Self::Input(SelectorInputError::Attribute(error)),
            OxmlError::Utf8(error) => Self::Input(SelectorInputError::Utf8(error)),
            other => Self::NonReusable(other),
        }
    }
}

impl From<quick_xml::Error> for SelectorFailure {
    fn from(error: quick_xml::Error) -> Self {
        Self::Input(SelectorInputError::Xml(error))
    }
}

impl From<SelectorFailure> for OxmlError {
    fn from(error: SelectorFailure) -> Self {
        match error {
            SelectorFailure::Input(SelectorInputError::Xml(error)) => Self::Xml(error),
            SelectorFailure::Input(SelectorInputError::Attribute(error)) => Self::XmlAttr(error),
            SelectorFailure::Input(SelectorInputError::Utf8(error)) => Self::Utf8(error),
            SelectorFailure::NonReusable(error) => error,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SimpleOwnershipAction {
    Root,
    LeadingData,
    Traverse { ordered: bool },
    Consume { ordered: bool },
    Empty { ordered: bool },
}

pub(crate) fn simple_ownership_action(
    local: &[u8],
    word: bool,
    depth: usize,
    result_seen: bool,
    empty: bool,
) -> SimpleOwnershipAction {
    if !empty && depth == 0 {
        return SimpleOwnershipAction::Root;
    }
    if depth == 1 && !result_seen && word && local == b"fldData" {
        return SimpleOwnershipAction::LeadingData;
    }
    let hyperlink = word && local == b"hyperlink";
    let ordered =
        hyperlink || (word && matches!(local, b"ins" | b"del" | b"moveFrom" | b"moveTo" | b"sdt"));
    if empty {
        SimpleOwnershipAction::Empty { ordered }
    } else if hyperlink || (word && local == b"fldSimple") {
        SimpleOwnershipAction::Traverse { ordered }
    } else {
        SimpleOwnershipAction::Consume { ordered }
    }
}

// CompleteThenProbe deliberately separates the actual matching-end read from
// the later ownership scan. A malformed candidate must fail before its inner
// namespace errors, while a completed positive ends the whole-source probe.
#[derive(Debug)]
enum OrderedOwnerAction {
    Consume,
    CompleteThenProbe,
    Enter {
        owner: Option<SdtOwner>,
        mark_content: bool,
    },
}

fn ordered_owner_action(
    local: &[u8],
    word: bool,
    parent: Option<(&[u8], Option<SdtOwner>, bool)>,
) -> OrderedOwnerAction {
    let parent_owner = parent.and_then(|(_, owner, _)| owner);
    let admitted = word
        && parent.is_none_or(|(name, _, _)| {
            crate::document::admitted_word_child(name, local)
                && (name != b"sdtContent"
                    || local == b"sdt"
                    || parent_owner.is_some_and(|owner| owner.owns(local)))
        });
    let duplicate_content = local == b"sdtContent"
        && parent.is_some_and(|(name, _, content_seen)| name == b"sdt" && content_seen);
    if !admitted || duplicate_content || matches!(local, b"r" | b"pPr" | b"rPr") {
        return OrderedOwnerAction::Consume;
    }
    if local == b"fldSimple" {
        return OrderedOwnerAction::CompleteThenProbe;
    }
    let owner = match local {
        b"p" | b"hyperlink" => Some(SdtOwner::Inline),
        b"ins" | b"del" | b"moveFrom" | b"moveTo" => parent_owner.or(Some(SdtOwner::Inline)),
        b"body" | b"hdr" | b"ftr" | b"comment" | b"footnote" | b"endnote" | b"tc" => {
            Some(SdtOwner::Body)
        }
        b"tbl" => Some(SdtOwner::Table),
        b"tr" => Some(SdtOwner::Row),
        _ => parent_owner,
    };
    OrderedOwnerAction::Enter {
        owner,
        mark_content: local == b"sdtContent" && parent.is_some_and(|(name, _, _)| name == b"sdt"),
    }
}

/// A source-only ownership probe, using the same qualified parent predicates
/// as the resource preflight. It constructs no Field or revision model.
pub(crate) fn source_has_ordered_field_owner(raw: &[u8], inherited: &[String]) -> Result<bool> {
    (|| -> std::result::Result<_, SelectorFailure> {
        let mut reader = Reader::from_reader(raw);
        let mut stack = Vec::<(Vec<String>, Vec<u8>, Option<SdtOwner>, bool)>::new();
        let mut buffer = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            match reader.read_event_into(&mut buffer)? {
                Event::Start(element) => {
                    let prefixes = word_prefixes_at(
                        &element,
                        stack.last().map_or(inherited, |frame| frame.0.as_slice()),
                    )?;
                    let local = element.local_name();
                    let word = is_word_element(element.name().as_ref(), local.as_ref(), &prefixes);
                    match ordered_owner_action(
                        local.as_ref(),
                        word,
                        stack
                            .last()
                            .map(|frame| (frame.1.as_slice(), frame.2, frame.3)),
                    ) {
                        OrderedOwnerAction::Consume => {
                            reader.read_to_end_into(element.name(), &mut Vec::new())?;
                        }
                        OrderedOwnerAction::CompleteThenProbe => {
                            let content_start = reader.buffer_position() as usize - before;
                            let mut end_reader = reader.clone();
                            end_reader.read_to_end_into(element.name(), &mut Vec::new())?;
                            let end = end_reader.buffer_position() as usize;
                            if simple_result_ownership(&raw[before..end], &prefixes, content_start)?
                                .0
                            {
                                return Ok(true);
                            }
                            reader.read_to_end_into(element.name(), &mut Vec::new())?;
                        }
                        OrderedOwnerAction::Enter {
                            owner,
                            mark_content,
                        } => {
                            let prefixes = prefixes.into_owned();
                            if mark_content && let Some(parent) = stack.last_mut() {
                                parent.3 = true;
                            }
                            stack.push((prefixes, local.as_ref().to_vec(), owner, false));
                        }
                    }
                }
                Event::End(_) => {
                    stack.pop();
                }
                Event::Eof => return Ok(false),
                _ => {}
            }
            buffer.clear();
        }
    })()
    .map_err(OxmlError::from)
}

/// Inspect qualified field/hyperlink result scopes before constructing any runs.
/// Unknown owners and run payloads remain opaque to this ownership decision.
fn simple_result_ownership(
    raw: &[u8],
    prefixes: &[String],
    initial_start: usize,
) -> Result<(bool, std::ops::Range<usize>)> {
    (|| -> std::result::Result<_, SelectorFailure> {
        let mut reader = Reader::from_reader(raw);
        reader.config_mut().trim_text(false);
        let mut contexts = Vec::<Vec<String>>::new();
        let mut buffer = Vec::new();
        let mut ordered = false;
        let mut start = initial_start;
        let mut end = initial_start;
        let mut result_seen = false;
        loop {
            let before = reader.buffer_position() as usize;
            match reader.read_event_into(&mut buffer)? {
                Event::Start(element) => {
                    let current = word_prefixes_at(
                        &element,
                        contexts.last().map_or(prefixes, Vec::as_slice),
                    )?;
                    match simple_ownership_action(
                        element.local_name().as_ref(),
                        is_word_element(
                            element.name().as_ref(),
                            element.local_name().as_ref(),
                            &current,
                        ),
                        contexts.len(),
                        result_seen,
                        false,
                    ) {
                        SimpleOwnershipAction::Root => contexts.push(current.into_owned()),
                        SimpleOwnershipAction::LeadingData => {
                            reader.read_to_end_into(element.name(), &mut Vec::new())?;
                            start = reader.buffer_position() as usize;
                        }
                        SimpleOwnershipAction::Traverse {
                            ordered: child_ordered,
                        } => {
                            result_seen = true;
                            ordered |= child_ordered;
                            contexts.push(current.into_owned());
                        }
                        SimpleOwnershipAction::Consume {
                            ordered: child_ordered,
                        } => {
                            result_seen = true;
                            ordered |= child_ordered;
                            reader.read_to_end_into(element.name(), &mut Vec::new())?;
                        }
                        SimpleOwnershipAction::Empty { .. } => {
                            unreachable!("started ownership action")
                        }
                    }
                }
                Event::Empty(element) => {
                    let current = word_prefixes_at(
                        &element,
                        contexts.last().map_or(prefixes, Vec::as_slice),
                    )?;
                    match simple_ownership_action(
                        element.local_name().as_ref(),
                        is_word_element(
                            element.name().as_ref(),
                            element.local_name().as_ref(),
                            &current,
                        ),
                        contexts.len(),
                        result_seen,
                        true,
                    ) {
                        SimpleOwnershipAction::LeadingData => {
                            start = reader.buffer_position() as usize
                        }
                        SimpleOwnershipAction::Empty {
                            ordered: child_ordered,
                        } => {
                            result_seen = true;
                            ordered |= child_ordered;
                        }
                        _ => unreachable!("empty ownership action"),
                    }
                }
                Event::End(_) => {
                    contexts.pop();
                    if contexts.is_empty() {
                        end = before;
                        break;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
        Ok((ordered, start..end))
    })()
    .map_err(OxmlError::from)
}

pub(super) fn parse_ordered_field_result(
    raw: &[u8],
    prefixes: &[String],
    mapped: Option<&crate::document::WordFieldInput>,
) -> Result<CT_P> {
    let prefix = prefixes
        .iter()
        .find(|prefix| !prefix.starts_with('\0'))
        .ok_or_else(|| OxmlError::MissingElement("field result Word scope".into()))?;
    let name = if prefix.is_empty() {
        "p".into()
    } else {
        format!("{prefix}:p")
    };
    let mut xml = format!("<{name}>").into_bytes();
    let prefix_bytes = xml.len();
    let closing = format!("</{name}>");
    xml.extend_from_slice(raw);
    xml.extend_from_slice(closing.as_bytes());
    let mut reader = Reader::from_reader(xml.as_slice());
    let root = match reader.read_event()? {
        Event::Start(root) => root,
        _ => unreachable!("constructed paragraph start"),
    };
    if let Some(mapped) = mapped {
        let wrapped = mapped.with_shell(prefix_bytes, closing.len())?;
        let mut cursor = wrapped.cursor(&xml)?;
        CT_P::from_xml_with_revision_facts(
            &mut reader,
            prefixes,
            Some(&root),
            None,
            Some(&mut cursor),
            None,
            None,
        )
    } else {
        CT_P::from_xml_with_prefixes_and_root(&mut reader, prefixes, Some(&root))
    }
}

/// Whether a preserved simple field is admitted by the complete typed parser.
#[doc(hidden)]
pub fn story_simple_field_is_typed(raw: &[u8], word_prefixes: &[String]) -> bool {
    parse_simple_field(raw, word_prefixes, None)
        .ok()
        .flatten()
        .is_some()
}

fn simple_result_run_display(raw: &[u8], word_prefixes: &[String]) -> Result<Option<String>> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let run_prefixes = loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(start) if matches_local_name(start.name().as_ref(), b"r") => {
                break word_prefixes_at(&start, word_prefixes)?;
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    };
    let mut display = None::<String>;
    loop {
        buffer.clear();
        match reader.read_event_into(&mut buffer)? {
            Event::Start(start) => {
                let prefixes = word_prefixes_at(&start, &run_prefixes)?;
                if is_word_element(start.name().as_ref(), b"t", &prefixes)
                    || is_word_element(start.name().as_ref(), b"delText", &prefixes)
                {
                    let text = reader
                        .read_text(start.name())
                        .map(|text| crate::xml_text::decode_escaped(&text))
                        .unwrap_or_default();
                    display.get_or_insert_default().push_str(&text);
                } else if is_word_element(start.name().as_ref(), b"tab", &prefixes) {
                    reader.read_to_end_into(start.name(), &mut Vec::new())?;
                    display.get_or_insert_default().push('\t');
                } else if is_word_element(start.name().as_ref(), b"br", &prefixes) {
                    let marker = match field_break_type(&start, &prefixes) {
                        BreakType::Line => '\n',
                        BreakType::Page => '\u{000c}',
                        BreakType::Column => '\u{000b}',
                    };
                    reader.read_to_end_into(start.name(), &mut Vec::new())?;
                    display.get_or_insert_default().push(marker);
                } else {
                    reader.read_to_end_into(start.name(), &mut Vec::new())?;
                }
            }
            Event::Empty(element) => {
                let prefixes = word_prefixes_at(&element, &run_prefixes)?;
                if is_word_element(element.name().as_ref(), b"t", &prefixes)
                    || is_word_element(element.name().as_ref(), b"delText", &prefixes)
                {
                    display.get_or_insert_default();
                } else if is_word_element(element.name().as_ref(), b"tab", &prefixes) {
                    display.get_or_insert_default().push('\t');
                } else if is_word_element(element.name().as_ref(), b"br", &prefixes) {
                    let marker = match field_break_type(&element, &prefixes) {
                        BreakType::Line => '\n',
                        BreakType::Page => '\u{000c}',
                        BreakType::Column => '\u{000b}',
                    };
                    display.get_or_insert_default().push(marker);
                } else if let Some(SpecialRunValue::Character(character)) =
                    parse_typed_special_run_child(&element, &prefixes)?
                {
                    // As `CT_R::content_text` reads them.
                    match character {
                        SpecialCharacter::CarriageReturn => {
                            display.get_or_insert_default().push('\n')
                        }
                        SpecialCharacter::NoBreakHyphen => {
                            display.get_or_insert_default().push('-')
                        }
                        SpecialCharacter::PositionalTab { .. } => {
                            display.get_or_insert_default().push('\t')
                        }
                        SpecialCharacter::SoftHyphen => {}
                    }
                }
            }
            Event::End(end) if matches_local_name(end.name().as_ref(), b"r") => {
                return Ok(display);
            }
            Event::Eof => return Ok(display),
            _ => {}
        }
    }
}
