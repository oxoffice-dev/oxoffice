//! Writing fields: run fields, results with their properties, simple and complex forms.

use super::*;

/// Write one field, leaving out the run content the reader split off it.
///
/// A field read from a run it shared with text or another field keeps that
/// whole span as its source. Once the span is edited the text is written by
/// its own runs, so this field writes only its own part of the source, with
/// its own cache and dirty updates.
pub(crate) fn write_run_field<W: std::io::Write>(
    writer: &mut Writer<W>,
    field: &Field,
    foreign_word_namespace: Option<&str>,
    result_properties: Option<&CT_RPr>,
    source_context: Option<&[(&str, &str)]>,

    work: &mut WordOperationWork,
) -> Result<()> {
    let mut own = run_field_for_write(field, work)?;
    if work.template_word_capture.is_some()
        && let Cow::Owned(current) = &mut own
    {
        work.template_mint_field(field, None, current)?;
    }
    write_field_with_result_properties(
        writer,
        &own,
        foreign_word_namespace,
        result_properties,
        None,
        source_context,
        work,
    )
}

fn run_field_for_write<'a>(field: &'a Field, work: &WordOperationWork) -> Result<Cow<'a, Field>> {
    crate::document::admit_word_model(crate::document::WordOwner::Field(field))?;
    field.normalization_work_budget()?;
    let Some(span) = &field.span else {
        return Ok(Cow::Borrowed(field));
    };
    let (original_map, original_facts) = {
        let origins = work
            .field_origins
            .lock()
            .expect("field origin registry poisoned");
        (origins.source_map(field), origins.original_for(field))
    };
    let mut own = field.clone();
    own.span = None;
    let mut detached_map = None;
    if let FieldSource::Parsed {
        raw_xml,
        publication_owned,
        word_prefixes,
        ..
    } = Arc::make_mut(&mut own.source)
    {
        let ranges = field_span_parts(raw_xml, word_prefixes, None, None)?
            .and_then(|parts| {
                parts
                    .into_iter()
                    .filter_map(|part| match part {
                        FieldSpanPart::Field(ranges) => Some(ranges),
                        FieldSpanPart::Outside(_, _) => None,
                    })
                    .nth(span.field_index)
            })
            .ok_or_else(|| {
                OxmlError::InvalidValue(
                    "a field could not be separated from its physical run".to_owned(),
                )
            })?;
        if let Some(map) = original_map {
            detached_map = Some(crate::document::WordSourceMap::concat(
                &ranges
                    .iter()
                    .map(|range| map.slice(range.clone()))
                    .collect::<Result<Vec<_>>>()?,
            )?);
        }
        let projected = publication_owned.as_ref().map(|owned| {
            let mut projected = Vec::new();
            let mut offset = 0usize;
            for selected in &ranges {
                for range in owned {
                    let start = range.start.max(selected.start);
                    let end = range.end.min(selected.end);
                    if start < end {
                        projected
                            .push(offset + start - selected.start..offset + end - selected.start);
                    }
                }
                offset += selected.len();
            }
            projected
        });
        *raw_xml = FieldSpanPart::materialize_source(raw_xml, &ranges)?;
        *publication_owned = projected;
    }
    if let Some(map) = detached_map {
        work.field_origins
            .lock()
            .expect("field origin registry poisoned")
            .register_forwarded(&own, map, original_facts)?;
    }
    Ok(Cow::Owned(own))
}

pub(crate) fn write_published_run_field<W: std::io::Write>(
    writer: &mut Writer<W>,
    field: &Field,
    foreign_word_namespace: Option<&str>,
    result_properties: Option<&CT_RPr>,
    scope: &[(&str, &str)],

    work: &mut WordOperationWork,
) -> Result<()> {
    let mut own = run_field_for_write(field, work)?;
    if work.template_word_capture.is_some()
        && let Cow::Owned(current) = &mut own
    {
        work.template_mint_field(field, None, current)?;
    }
    write_field_with_result_properties(
        writer,
        &own,
        foreign_word_namespace,
        result_properties,
        Some(scope),
        None,
        work,
    )
}

pub(super) fn write_field_raw_output<W: std::io::Write>(
    writer: &mut Writer<W>,
    raw: &[u8],
    field: &Field,
    foreign_word_namespace: Option<&str>,
    publication_scope: Option<&[(&str, &str)]>,
    work: &WordOperationWork,
    word_map: Option<&crate::document::WordSourceMap>,
) -> Result<()> {
    // A parsed unchanged record can carry its constructor's Word metadata
    // through Publication, too. Require this exact live source allocation,
    // never an equal or changed Vec. No Field Original/current role is added.
    let retained_word_map = if word_map.is_none() && publication_scope.is_some() {
        match field.source.as_ref() {
            FieldSource::Parsed { raw_xml, .. }
                if raw.as_ptr() == raw_xml.as_ptr() && raw.len() == raw_xml.len() =>
            {
                work.field_origins
                    .lock()
                    .expect("field origin registry poisoned")
                    .word_source_map(field)
            }
            _ => None,
        }
    } else {
        None
    };
    let selected_map = match field.source.as_ref() {
        FieldSource::Parsed {
            raw_xml,
            publication_owned: Some(owned),
            ..
        } if raw.as_ptr() == raw_xml.as_ptr() && raw.len() == raw_xml.len() => {
            let base = word_map
                .or(retained_word_map.as_ref())
                .cloned()
                .or_else(|| {
                    publication_scope
                        .is_none()
                        .then(|| {
                            work.field_origins
                                .lock()
                                .expect("field origin registry poisoned")
                                .source_map(field)
                        })
                        .flatten()
                })
                .unwrap_or_else(|| crate::document::WordSourceMap::generated(raw.len()));
            Some(base.with_publication_owned(owned.clone())?)
        }
        _ => None,
    };
    let word_map = selected_map
        .as_ref()
        .or(word_map)
        .or(retained_word_map.as_ref());
    let _mapped = if publication_scope.is_none() && foreign_word_namespace.is_none() {
        if let Some(map) = word_map {
            let map = if selected_map.is_some() {
                map.clone()
            } else {
                map.clone().word_only()
            };
            Some(work.mapped_source_record(raw, map)?)
        } else {
            work.mapped_field_transport(field, raw)?
        }
    } else {
        None
    };
    let emission = work.field_xml_emission();
    if let Some(scope) = publication_scope {
        let final_validation = work
            .normalized_output
            .lock()
            .expect("normalization ledger poisoned")
            .final_validation_writer
            == Some(std::ptr::from_ref(&*writer) as usize);
        if final_validation {
            if let Some(plan) = published_field_root_plan(raw, field, scope)? {
                write_retained_roots_final_transport(
                    writer,
                    raw,
                    &plan,
                    &emission,
                    word_map.map(|map| (work, map)),
                )?;
            } else {
                let _mapped = word_map
                    .map(|map| work.mapped_source_record(raw, map.clone()))
                    .transpose()?;
                writer.get_mut().write_all(raw)?;
            }
            return Ok(());
        }
        let (produced, prepaid, produced_map) = if let Some(map) = word_map {
            match published_field_root_plan(raw, field, scope)? {
                Some(plan) => emit_retained_roots_mapped_in(raw, plan, Some(&emission), Some(map))?,
                None => (Cow::Borrowed(raw), false, Some(map.clone().word_only())),
            }
        } else {
            let (bytes, prepaid) = published_field_roots_in(raw, field, scope, Some(&emission))?;
            (bytes, prepaid, None)
        };
        let _mapped = produced_map
            .map(|map| work.mapped_source_record(&produced, map))
            .transpose()?;
        let _transfer = prepaid
            .then(|| emission.prepaid_xml_transport(&produced))
            .transpose()?;
        writer.get_mut().write_all(&produced)?;
        Ok(())
    } else {
        write_raw_with_word_override_in(writer, raw, foreign_word_namespace, Some(&emission))
    }
}

pub(crate) fn write_mixed_field_run<W: std::io::Write>(
    writer: &mut Writer<W>,
    run: &CT_R,
    foreign_word_namespace: Option<&str>,
    publication_scope: Option<&[(&str, &str)]>,
    source_context: Option<&[(&str, &str)]>,

    work: &mut WordOperationWork,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    let capture = work.template_word_capture.clone();
    let mut segment_start = 0usize;
    for (field_index, content) in run.content.iter().enumerate() {
        let RunContent::Field(field) = content else {
            continue;
        };
        write_run_content_segment(
            writer,
            run,
            segment_start..field_index,
            segment_start == 0,
            false,
            foreign_word_namespace,
            publication_scope.is_some(),
            source_context,
            emission,
            capture.as_ref(),
        )?;
        if let Some(scope) = publication_scope {
            write_published_run_field(
                writer,
                field,
                foreign_word_namespace,
                run.properties.as_ref(),
                scope,
                work,
            )?;
        } else {
            write_run_field(
                writer,
                field,
                foreign_word_namespace,
                run.properties.as_ref(),
                source_context,
                work,
            )?;
        }
        segment_start = field_index + 1;
    }
    write_run_content_segment(
        writer,
        run,
        segment_start..run.content.len(),
        segment_start == 0,
        true,
        foreign_word_namespace,
        publication_scope.is_some(),
        source_context,
        emission,
        capture.as_ref(),
    )
}

// Physical segment boundaries and publication scope are independent writer facts.
#[allow(clippy::too_many_arguments)]
fn write_run_content_segment<W: std::io::Write>(
    writer: &mut Writer<W>,
    run: &CT_R,
    range: std::ops::Range<usize>,
    first: bool,
    last: bool,
    foreign_word_namespace: Option<&str>,
    publication: bool,
    source_context: Option<&[(&str, &str)]>,
    emission: Option<&FieldXmlEmission>,
    capture: Option<&Arc<std::sync::Mutex<TemplateWordCapture>>>,
) -> Result<()> {
    let start = range.start;
    let end = range.end;
    let property_boundary = usize::from(run.properties.is_some());
    let lower = if first { 0 } else { property_boundary + start };
    let upper = property_boundary + end;
    let mut extra_xml = Vec::new();
    let mut extra_xml_positions = Vec::new();
    if run.extra_xml_positions.len() == run.extra_xml.len() {
        for (position, raw) in run.extra_xml_positions.iter().zip(&run.extra_xml) {
            let boundary = CT_R::raw_child_position(*position);
            if boundary < lower
                || boundary > upper
                || (*position & RAW_COMMENT_REFERENCE_FLAG != 0 && boundary == upper)
            {
                continue;
            }
            let mut mapped = *position;
            let mapped_boundary = if boundary < property_boundary {
                boundary
            } else {
                boundary.saturating_sub(start)
            };
            CT_R::set_raw_child_position(&mut mapped, mapped_boundary);
            extra_xml.push(raw.clone());
            extra_xml_positions.push(mapped);
        }
    } else if last {
        extra_xml = run.extra_xml.clone();
    }
    if start == end && extra_xml.is_empty() {
        return Ok(());
    }
    let segment = CT_R {
        properties: run.properties.clone(),
        content: run.content[start..end].to_vec(),
        extra_xml,
        extra_xml_positions,
        alt_drawings: Vec::new(),
    };
    let _texts = TemplateTextCloneLease::new(&run.content[start..end], &segment, capture);
    segment.write_xml_role(
        writer,
        foreign_word_namespace,
        if publication {
            XmlWriteRole::Publication
        } else {
            XmlWriteRole::Source
        },
        source_context,
        emission,
        capture,
    )
}

pub(super) fn collect_story_complex_field_sources(paragraph: &CT_P, sources: &mut Vec<Vec<u8>>) {
    for run in paragraph.runs() {
        for content in &run.content {
            let RunContent::Field(field) = content else {
                continue;
            };
            collect_story_complex_field_source(field, sources);
        }
    }
    for (_, _, revision) in &paragraph.revisions {
        collect_story_revision_complex_field_sources(revision, sources);
    }
}

fn collect_story_complex_field_source(field: &Field, sources: &mut Vec<Vec<u8>>) {
    if let FieldSource::Parsed {
        form: FieldForm::Complex,
        raw_xml,
        ..
    } = field.source.as_ref()
    {
        sources.push(raw_xml.clone());
    }
    for nested in field.nested_fields_in_source_order() {
        collect_story_complex_field_source(nested, sources);
    }
}

fn collect_story_revision_complex_field_sources(
    revision: &CT_Revision,
    sources: &mut Vec<Vec<u8>>,
) {
    if let Some(paragraph) = revision.content_paragraph() {
        collect_story_complex_field_sources(paragraph, sources);
    }
    for (_, nested) in revision.nested_revisions() {
        collect_story_revision_complex_field_sources(nested, sources);
    }
}

pub(super) fn is_xml_whitespace(value: &[u8]) -> bool {
    !value.is_empty()
        && value
            .iter()
            .all(|byte| matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'))
}

pub(super) fn capture_standalone_event(event: Event<'static>) -> Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    writer.write_event(event)?;
    Ok(writer.into_inner())
}

pub(super) fn write_field<W: std::io::Write>(
    writer: &mut Writer<W>,
    field: &Field,
    foreign_word_namespace: Option<&str>,

    work: &mut WordOperationWork,
) -> Result<()> {
    write_field_with_result_properties(
        writer,
        field,
        foreign_word_namespace,
        None,
        None,
        None,
        work,
    )
}

pub(super) fn source_contains_own_bookmark(
    raw: &[u8],
    prefixes: &[String],
    name: &str,
) -> Result<bool> {
    let mut reader = Reader::from_reader(raw);
    let mut contexts = Vec::<Vec<String>>::new();
    let mut buffer = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer)?;
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let inherited = contexts.last().map(Vec::as_slice).unwrap_or(prefixes);
                let current = word_prefixes_at(element, inherited)?;
                if is_word_element(element.name().as_ref(), b"bookmarkStart", &current)
                    && optional_word_attribute(element, b"name", &current).as_deref() == Some(name)
                {
                    return Ok(true);
                }
                if matches!(event, Event::Start(_)) {
                    contexts.push(current.into_owned());
                }
            }
            Event::End(_) => {
                contexts.pop();
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn ordered_instruction_source_edit<'a>(
    raw: &[u8],
    source_bytes: usize,
    word_prefixes: &[String],
    instruction: &'a str,
    before: Option<usize>,
) -> Result<Option<FieldSourceEdit<'a>>> {
    let mut reader = Reader::from_reader(raw);
    let root = match reader.read_event() {
        Ok(Event::Start(root) | Event::Empty(root)) => root,
        _ if before.is_some() => return Ok(None),
        Ok(_) => {
            return Err(OxmlError::InvalidValue(
                "ordered field has no source root".into(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    let root_end = reader.buffer_position() as usize;
    if before.is_some_and(|limit| root_end > limit) {
        return Ok(None);
    }
    let prefixes = word_prefixes_at(&root, word_prefixes)?;
    let attributes = root
        .attributes()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let key = attributes
        .iter()
        .find(|attribute| is_field_word_attribute(attribute.key.as_ref(), b"instr", &prefixes))
        .ok_or_else(|| {
            OxmlError::MissingElement("ordered field instruction source attribute".into())
        })?
        .key
        .as_ref();
    let range = lexical_attribute_value_range(&raw[..root_end], key).ok_or_else(|| {
        OxmlError::InvalidValue("ordered field instruction source bounds are ambiguous".into())
    })?;
    FieldSourceEdit::new(
        source_bytes,
        range,
        vec![FieldSourcePiece::Escaped(instruction)],
    )
    .map(Some)
}

/// Plan the two root edits reached by the old ordered-simple flag route.
/// The result is borrowed until the caller publishes one complete source record.
fn ordered_simple_flag_source_edits<'a>(
    field: &'a Field,
    raw: &'a [u8],
    word_prefixes: &[String],
    result: &std::ops::Range<usize>,
    normalized: &[u8],
    instruction_changed: bool,
    locked_changed: bool,
) -> Result<Option<Vec<FieldSourceEdit<'a>>>> {
    let mut reader = Reader::from_reader(raw);
    let root = match reader.read_event()? {
        Event::Start(root) => root,
        _ => return Ok(None),
    };
    let header_end = reader.buffer_position() as usize;
    if header_end > result.start {
        return Ok(None);
    }
    let prefixes = word_prefixes_at(&root, word_prefixes)?;
    if !is_word_element(root.name().as_ref(), b"fldSimple", &prefixes) {
        return Ok(None);
    }
    let foreign = namespace_bindings(&prefixes)
        .iter()
        .any(|(prefix, namespace)| prefix == "w" && namespace != crate::namespace::W_NS);
    let owner_name = field_attribute_source(raw, root.name().as_ref())?;
    let mut header = vec![FieldSourcePiece::Literal(b"<")];
    header.push(if foreign {
        FieldSourcePiece::Raw(owner_name)
    } else {
        FieldSourcePiece::Literal(b"w:fldSimple")
    });
    let mut first_instruction = true;
    let mut ambiguous_lock_header = false;
    for attribute in root.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let instr = is_field_word_attribute(key, b"instr", &prefixes);
        let replace_instruction = instr && first_instruction && instruction_changed;
        if instr {
            first_instruction = false;
        }
        if is_field_word_attribute(key, b"dirty", &prefixes)
            || (!foreign && (key == b"xmlns:w" || instr))
            || (locked_changed && is_field_word_attribute(key, b"fldLock", &prefixes))
        {
            continue;
        }
        // The old metadata writer normalizes quotes but retains raw values.
        // A literal quote can make its subsequent lock reader fail. Leave that
        // historical staged error path intact instead of interpreting a recipe.
        ambiguous_lock_header |=
            locked_changed && !replace_instruction && attribute.value.contains(&b'"');
        header.push(FieldSourcePiece::Literal(b" "));
        header.push(FieldSourcePiece::Raw(field_attribute_source(raw, key)?));
        header.push(FieldSourcePiece::Literal(b"=\""));
        header.push(if replace_instruction {
            FieldSourcePiece::Escaped(&field.instruction.raw)
        } else {
            FieldSourcePiece::Raw(field_attribute_source(raw, attribute.value.as_ref())?)
        });
        header.push(FieldSourcePiece::Literal(b"\""));
    }
    if !foreign {
        header.push(FieldSourcePiece::Literal(b" xmlns:w=\""));
        header.push(FieldSourcePiece::Literal(crate::namespace::W_NS.as_bytes()));
        header.push(FieldSourcePiece::Literal(b"\" w:instr=\""));
        header.push(FieldSourcePiece::Escaped(&field.instruction.raw));
        header.push(FieldSourcePiece::Literal(b"\""));
    }
    if let Some(dirty) = field.dirty {
        let prefix = if foreign {
            std::str::from_utf8(owner_name)?
                .split_once(':')
                .map(|(prefix, _)| prefix)
                .or_else(|| {
                    prefixes
                        .iter()
                        .find(|p| !p.is_empty() && !p.starts_with('\0'))
                        .map(String::as_str)
                })
                .ok_or_else(|| {
                    OxmlError::InvalidValue("field attribute requires a Word prefix".into())
                })?
        } else {
            "w"
        };
        // Prefix is borrowed from the root or its checked inherited scope.
        header.push(FieldSourcePiece::Literal(b" "));
        if foreign && owner_name.contains(&b':') {
            header.push(FieldSourcePiece::Raw(&owner_name[..prefix.len()]));
        } else if foreign {
            // The effective scope owns this prefix for the duration of planning.
            // Emit it from the actual qualified source attribute instead below.
            let mut source_prefix = None;
            for attribute in root.attributes() {
                let attribute = attribute?;
                let key = field_attribute_source(raw, attribute.key.as_ref())?;
                if key
                    .strip_prefix(prefix.as_bytes())
                    .is_some_and(|tail| tail.starts_with(b":"))
                {
                    source_prefix = Some(&key[..prefix.len()]);
                    break;
                }
            }
            let Some(source_prefix) = source_prefix else {
                return Ok(None);
            };
            header.push(FieldSourcePiece::Raw(source_prefix));
        } else {
            header.push(FieldSourcePiece::Literal(b"w"));
        }
        header.push(FieldSourcePiece::Literal(b":dirty=\""));
        header.push(FieldSourcePiece::Literal(if dirty { b"1" } else { b"0" }));
        header.push(FieldSourcePiece::Literal(b"\""));
    }

    // This is exactly the namespace/event walk of update_simple_field_source
    // with cached_changed=false and no simple_cached_runs. The earlier lexical
    // instruction edit changes no QName or namespace declaration, so its value
    // need not be copied merely to walk this actual post-result reader input.
    let chunks = [&raw[..result.start], normalized, &raw[result.end..]];
    let mut reader = Reader::from_reader(crate::document::WordXmlInput::pieces(&chunks));
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut contexts = Vec::<Vec<String>>::new();
    let mut closing = None;
    loop {
        let start = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let current = word_prefixes_at(
                    &element,
                    contexts.last().map(Vec::as_slice).unwrap_or(word_prefixes),
                )?;
                contexts.push(current.into_owned());
            }
            Event::Empty(element) => {
                word_prefixes_at(
                    &element,
                    contexts.last().map(Vec::as_slice).unwrap_or(word_prefixes),
                )?;
            }
            Event::End(_) => {
                if contexts.pop().is_some() && contexts.is_empty() {
                    let end = reader.buffer_position() as usize;
                    let shift = |offset: usize| {
                        offset
                            .checked_sub(normalized.len())
                            .and_then(|n| n.checked_add(result.len()))
                    };
                    let Some((start, end)) = shift(start).zip(shift(end)) else {
                        return Ok(None);
                    };
                    if start != result.end || closing.replace(start..end).is_some() {
                        return Ok(None);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let Some(closing) = closing else {
        return Ok(None);
    };
    if ambiguous_lock_header {
        return Ok(None);
    }
    // Lock selection is reached only after the complete metadata reader above.
    if locked_changed && let Some(locked) = field.locked {
        let attribute_prefix = if !foreign {
            Some(&b"w"[..])
        } else if let Some(colon) = owner_name.iter().position(|byte| *byte == b':') {
            Some(&owner_name[..colon])
        } else if prefixes.iter().any(|p| p == "w") {
            Some(&b"w"[..])
        } else {
            let mut selected = None;
            for attribute in root.attributes() {
                let attribute = attribute?;
                if is_field_word_attribute(attribute.key.as_ref(), b"dirty", &prefixes) {
                    continue;
                }
                let key = field_attribute_source(raw, attribute.key.as_ref())?;
                if let Some(colon) = key.iter().position(|byte| *byte == b':')
                    && prefixes.iter().any(|p| p.as_bytes() == &key[..colon])
                {
                    selected = Some(&key[..colon]);
                    break;
                }
            }
            selected
        }
        .ok_or_else(|| OxmlError::InvalidValue("field attribute requires a Word prefix".into()))?;
        header.push(FieldSourcePiece::Literal(b" "));
        header.push(if attribute_prefix == b"w" && !foreign {
            FieldSourcePiece::Literal(b"w")
        } else {
            FieldSourcePiece::Raw(attribute_prefix)
        });
        header.push(FieldSourcePiece::Literal(b":fldLock=\""));
        header.push(FieldSourcePiece::Literal(if locked { b"1" } else { b"0" }));
        header.push(FieldSourcePiece::Literal(b"\""));
    }
    header.push(FieldSourcePiece::Literal(b">"));
    let mut edits = vec![FieldSourceEdit::new(raw.len(), 0..header_end, header)?];
    if !foreign {
        edits.push(FieldSourceEdit::new(
            raw.len(),
            closing,
            vec![FieldSourcePiece::Literal(b"</w:fldSimple>")],
        )?);
    }
    Ok(Some(edits))
}

pub(super) fn write_field_with_result_properties<W: std::io::Write>(
    writer: &mut Writer<W>,
    field: &Field,
    foreign_word_namespace: Option<&str>,
    result_properties: Option<&CT_RPr>,
    publication_scope: Option<&[(&str, &str)]>,
    source_context: Option<&[(&str, &str)]>,

    work: &mut WordOperationWork,
) -> Result<()> {
    // Only these actual producers emit an entirely Field-owned record. A
    // parsed complex source can retain outside shared-run text and uses its
    // selected source transcript instead of this complete-record scope.
    let _publication = matches!(
        field.source.as_ref(),
        FieldSource::New { .. }
            | FieldSource::Parsed {
                form: FieldForm::Simple,
                ..
            }
    )
    .then(|| work.field_publication());
    let _source_scope = publication_scope.is_none().then(|| root_binding_scope(0));
    crate::document::admit_word_model(crate::document::WordOwner::Field(field))?;
    let own_context = match field.source.as_ref() {
        FieldSource::Parsed { word_prefixes, .. } => Some(field_scope_bindings(word_prefixes)),
        FieldSource::New { .. } => None,
    };
    let source_context = if publication_scope.is_none() {
        own_context.as_deref().or(source_context)
    } else {
        None
    };
    field.normalization_work_budget()?;
    let _new_field_emission =
        matches!(field.source.as_ref(), FieldSource::New { .. }).then(|| work.new_field_emission());
    if field.ordered_result.is_some() {
        let mut prepared = if work.template_word_capture.is_some() {
            field.clone_fields(None, Some(work))?
        } else {
            field.clone()
        };
        prepared.normalize_cached_result_owner_in(work)?;
        if prepared.is_unchanged()
            && let FieldSource::Parsed { raw_xml, .. } = prepared.source.as_ref()
        {
            return write_field_raw_output(
                writer,
                raw_xml,
                field,
                foreign_word_namespace,
                publication_scope,
                work,
                None,
            );
        }
        let owner = prepared.ordered_result.take().expect("prepared result");
        let FieldSource::Parsed { raw_xml, .. } = prepared.source.as_ref() else {
            return Err(OxmlError::MissingElement(
                "ordered field original source".into(),
            ));
        };
        raw_xml
            .len()
            .checked_sub(owner.source_range.len())
            .and_then(|bytes| bytes.checked_add(owner.normalized_xml.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("ordered field publication byte budget overflow".into())
            })?;
        let instruction = instruction_for_write(&prepared);
        if let FieldSource::Parsed {
            form: FieldForm::Complex,
            raw_xml,
            word_prefixes,
            original_instruction,
            original_dirty,
            original_locked,
            original_legacy_form,
            ..
        } = prepared.source.as_ref()
        {
            if owner.source_range.end > raw_xml.len() {
                return Err(OxmlError::InvalidValue(
                    "ordered complex field source bounds changed".into(),
                ));
            }
            let emission = work.field_xml_emission();
            let mut updated = FieldXmlRecord::new(&emission);
            emission.write_xml_pieces(
                &mut updated,
                &[
                    FieldSourcePiece::Raw(&raw_xml[..owner.source_range.start]),
                    FieldSourcePiece::Raw(&owner.normalized_xml),
                    FieldSourcePiece::Raw(&raw_xml[owner.source_range.end..]),
                ],
            )?;
            // The current CT_P above has published its complete result once.
            // Only instruction-region operands belong to this source phase.
            // The legacy result-cache guards remain in their nonordered caller.
            let scan = scan_complex_source(updated.as_bytes(), word_prefixes)?;
            let mut instruction_edits = nested_instruction_source_edits(
                &prepared,
                updated.as_bytes(),
                word_prefixes,
                &scan,
                work,
            )?;
            instruction_edits.sort_by_key(|(start, _, _)| *start);
            for (start, end, replacement) in instruction_edits.into_iter().rev() {
                updated.splice(start..end, &replacement)?;
            }
            if !instruction_structure_eq(&instruction, original_instruction)
                || instruction.raw != original_instruction.raw
            {
                let next = update_complex_instruction_source(
                    &prepared,
                    updated.as_bytes(),
                    word_prefixes,
                    work,
                )?;
                updated.replace_generated(next);
            }
            if prepared.legacy_form != *original_legacy_form
                && !legacy_text_value_only_change(original_legacy_form, &prepared.legacy_form)
            {
                let form = prepared
                    .legacy_form
                    .as_ref()
                    .ok_or_else(|| OxmlError::MissingElement("typed legacy form data".into()))?;
                let next =
                    rewrite_legacy_form_source(updated.as_bytes(), word_prefixes, form, work)?;
                updated.replace_generated(next);
            }
            if prepared.dirty != *original_dirty {
                let next = update_complex_field_source(
                    &prepared,
                    updated.as_bytes(),
                    word_prefixes,
                    false,
                    false,
                    work,
                    None,
                )?;
                updated.replace_generated(next);
            }
            if prepared.locked != *original_locked {
                return write_field_lock_update(
                    writer,
                    updated.as_bytes(),
                    field,
                    foreign_word_namespace,
                    publication_scope,
                    work,
                    None,
                );
            }
            let emission = work.field_xml_emission();
            let _transfer = (publication_scope.is_none() && foreign_word_namespace.is_none())
                .then(|| emission.prepaid_xml_transport(updated.as_bytes()))
                .transpose()?;
            return write_field_raw_output(
                writer,
                updated.as_bytes(),
                field,
                foreign_word_namespace,
                publication_scope,
                work,
                None,
            );
        }
        if instruction_contains_nested(&instruction) {
            return Err(OxmlError::InvalidValue("an ordered simple-field instruction cannot acquire a nested instruction field without a source mapping".into()));
        }
        prepared.instruction = instruction;
        if prepared.source_content_metadata_unchanged() {
            let FieldSource::Parsed {
                raw_xml,
                original_instruction,
                original_dirty,
                original_locked,
                word_prefixes,
                ..
            } = prepared.source.as_ref()
            else {
                return Err(OxmlError::MissingElement(
                    "ordered field original source".into(),
                ));
            };
            if owner.source_range.end > raw_xml.len() {
                return Err(OxmlError::InvalidValue(
                    "ordered field source bounds changed".into(),
                ));
            }
            let after_result = raw_xml
                .len()
                .checked_sub(owner.source_range.len())
                .and_then(|length| length.checked_add(owner.normalized_xml.len()))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("ordered field publication byte budget overflow".into())
                })?;
            let instruction_changed =
                !instruction_structure_eq(&prepared.instruction, original_instruction)
                    || prepared.instruction.raw != original_instruction.raw;
            let edit = if instruction_changed {
                ordered_instruction_source_edit(
                    raw_xml,
                    after_result,
                    word_prefixes,
                    &prepared.instruction.raw,
                    Some(owner.source_range.start),
                )?
            } else {
                None
            };
            // An inconsistent root/result overlap retains the old staged path
            // and its actual root/error selection rather than guessing a map.
            if !instruction_changed || edit.is_some() {
                let edit = edit.map(|edit| (edit.range, edit.projected, edit.replacement_bytes));
                let stage_projected = edit
                    .as_ref()
                    .map_or(after_result, |(_, projected, _)| *projected);
                // This is the old recursive guard, before the root metadata walk.
                prepared.source_publication_summary(stage_projected, None)?;
                let flags_changed =
                    prepared.dirty != *original_dirty || prepared.locked != *original_locked;
                let flag_edits = if flags_changed && prepared.simple_cached_runs.is_none() {
                    ordered_simple_flag_source_edits(
                        &prepared,
                        raw_xml,
                        word_prefixes,
                        &owner.source_range,
                        &owner.normalized_xml,
                        instruction_changed,
                        prepared.locked != *original_locked,
                    )?
                } else if flags_changed {
                    None
                } else {
                    Some(Vec::new())
                };
                if let Some(mut flag_edits) = flag_edits {
                    let mut pieces = Vec::new();
                    if flags_changed {
                        let header = flag_edits.remove(0);
                        pieces.extend(header.replacement);
                        pieces.push(FieldSourcePiece::Raw(
                            &raw_xml[header.range.end..owner.source_range.start],
                        ));
                        pieces.push(FieldSourcePiece::Raw(&owner.normalized_xml));
                        if let Some(closing) = flag_edits.pop() {
                            pieces.push(FieldSourcePiece::Raw(
                                &raw_xml[owner.source_range.end..closing.range.start],
                            ));
                            pieces.extend(closing.replacement);
                            pieces.push(FieldSourcePiece::Raw(&raw_xml[closing.range.end..]));
                        } else {
                            pieces.push(FieldSourcePiece::Raw(&raw_xml[owner.source_range.end..]));
                        }
                    } else {
                        if let Some((range, _, _)) = &edit {
                            pieces.push(FieldSourcePiece::Raw(&raw_xml[..range.start]));
                            pieces.push(FieldSourcePiece::Escaped(&prepared.instruction.raw));
                            pieces.push(FieldSourcePiece::Raw(
                                &raw_xml[range.end..owner.source_range.start],
                            ));
                        } else {
                            pieces
                                .push(FieldSourcePiece::Raw(&raw_xml[..owner.source_range.start]));
                        }
                        pieces.push(FieldSourcePiece::Raw(&owner.normalized_xml));
                        pieces.push(FieldSourcePiece::Raw(&raw_xml[owner.source_range.end..]));
                    }
                    let (raw_map, result_map) = {
                        let origins = work
                            .field_origins
                            .lock()
                            .expect("field origin registry poisoned");
                        (
                            origins.source_map(&prepared).unwrap_or_else(|| {
                                crate::document::WordSourceMap::generated(raw_xml.len())
                            }),
                            origins
                                .result_map(&owner.normalized_xml)
                                .unwrap_or_else(|| {
                                    crate::document::WordSourceMap::generated(
                                        owner.normalized_xml.len(),
                                    )
                                }),
                        )
                    };
                    let mut maps = Vec::with_capacity(pieces.len());
                    for piece in &pieces {
                        let map = match piece {
                            FieldSourcePiece::Raw(bytes) => {
                                let raw_start = (bytes.as_ptr() as usize)
                                    .checked_sub(raw_xml.as_ptr() as usize);
                                let result_start = (bytes.as_ptr() as usize)
                                    .checked_sub(owner.normalized_xml.as_ptr() as usize);
                                if let Some(start) = raw_start.filter(|start| {
                                    start
                                        .checked_add(bytes.len())
                                        .is_some_and(|end| end <= raw_xml.len())
                                }) {
                                    raw_map.slice(start..start + bytes.len())?
                                } else if let Some(start) = result_start.filter(|start| {
                                    start
                                        .checked_add(bytes.len())
                                        .is_some_and(|end| end <= owner.normalized_xml.len())
                                }) {
                                    result_map.slice(start..start + bytes.len())?
                                } else {
                                    return Err(OxmlError::InvalidValue(
                                        "ordered field recipe source piece changed".into(),
                                    ));
                                }
                            }
                            FieldSourcePiece::Escaped(value) => {
                                crate::document::WordSourceMap::generated(escaped_source_len(
                                    value,
                                )?)
                            }
                            FieldSourcePiece::Literal(bytes) => {
                                crate::document::WordSourceMap::generated(bytes.len())
                            }
                        };
                        maps.push(map);
                    }
                    let source_map = crate::document::WordSourceMap::concat(&maps)?;
                    let record = FieldSourceEdit::new(raw_xml.len(), 0..raw_xml.len(), pieces)?;
                    let projected = record.projected;
                    let prepaid = work.reserve_inner_xml(projected)?;
                    let updated = record.emit(raw_xml)?;
                    let direct_source =
                        publication_scope.is_none() && foreign_word_namespace.is_none();
                    let _mapped = direct_source
                        .then(|| work.mapped_source_record(&updated, source_map.clone()))
                        .transpose()?;
                    let _transfer = (prepaid && direct_source)
                        .then(|| work.prepaid_xml_transport(&updated))
                        .transpose()?;
                    return write_field_raw_output(
                        writer,
                        &updated,
                        field,
                        foreign_word_namespace,
                        publication_scope,
                        work,
                        Some(&source_map),
                    );
                }
            }
        }
        let FieldSource::Parsed {
            raw_xml,
            original_instruction,
            original_cached_result,
            word_prefixes,
            ..
        } = Arc::make_mut(&mut prepared.source)
        else {
            return Err(OxmlError::MissingElement(
                "ordered field original source".into(),
            ));
        };
        if owner.source_range.end > raw_xml.len() {
            return Err(OxmlError::InvalidValue(
                "ordered field source bounds changed".into(),
            ));
        }
        work.field_xml_emission()
            .reserve_inner_xml(owner.normalized_xml.len())?;
        raw_xml.splice(owner.source_range, owner.normalized_xml.iter().copied());
        if !instruction_structure_eq(&prepared.instruction, original_instruction)
            || prepared.instruction.raw != original_instruction.raw
        {
            ordered_instruction_source_edit(
                raw_xml,
                raw_xml.len(),
                word_prefixes,
                &prepared.instruction.raw,
                None,
            )?
            .expect("unbounded root selector always returns an edit")
            .apply(raw_xml, work)?;
            original_instruction.clone_from(&prepared.instruction);
        }
        original_cached_result.clone_from(&prepared.cached_result);
        return write_field_with_result_properties(
            writer,
            &prepared,
            foreign_word_namespace,
            result_properties,
            publication_scope,
            source_context,
            work,
        );
    }
    if field.is_unchanged()
        && let FieldSource::Parsed { raw_xml, .. } = field.source.as_ref()
    {
        return write_field_raw_output(
            writer,
            raw_xml,
            field,
            foreign_word_namespace,
            publication_scope,
            work,
            None,
        );
    }

    if let FieldSource::Parsed {
        form,
        raw_xml,
        original_instruction,
        original_cached_result,
        original_dirty,
        original_locked,
        original_legacy_form,
        original_result_published,
        word_prefixes,
        ..
    } = field.source.as_ref()
        && instruction_structure_eq(&field.instruction, original_instruction)
        && instruction_source_identity_eq(&field.instruction, original_instruction)
        && field.instruction.raw == original_instruction.raw
    {
        let cached_changed = field.cached_result != *original_cached_result
            && field.nested_cached_projection().as_ref() != Some(&field.cached_result);
        let publish_missing = field.legacy_result_published && !*original_result_published;
        let legacy_form_changed = field.legacy_form != *original_legacy_form;
        let text_current_only =
            legacy_text_value_only_change(original_legacy_form, &field.legacy_form);
        let mut updated_word_map = None;
        let input_word_map = work
            .template_word_capture
            .as_ref()
            .map(|_| work.template_word_raw_map(raw_xml))
            .transpose()?;
        let updated = match form {
            FieldForm::Simple => update_simple_field_source(
                field,
                raw_xml,
                word_prefixes,
                cached_changed,
                work,
                input_word_map
                    .as_ref()
                    .map(|map| (map, &mut updated_word_map)),
            )?,
            FieldForm::Complex => {
                let mut updated = update_nested_field_sources(field, raw_xml, word_prefixes, work)?;
                if legacy_form_changed && !text_current_only {
                    let form = field.legacy_form.as_ref().ok_or_else(|| {
                        OxmlError::MissingElement("typed legacy form data".to_owned())
                    })?;
                    updated = Cow::Owned(rewrite_legacy_form_source(
                        &updated,
                        word_prefixes,
                        form,
                        work,
                    )?);
                }
                if cached_changed || publish_missing || field.dirty != *original_dirty {
                    Cow::Owned(update_complex_field_source(
                        field,
                        &updated,
                        word_prefixes,
                        cached_changed || publish_missing,
                        publish_missing,
                        work,
                        input_word_map
                            .as_ref()
                            .map(|map| (map, &mut updated_word_map)),
                    )?)
                } else {
                    updated
                }
            }
        };
        let updated = if let Some(runs) = field.typed_cache() {
            updated_word_map = None;
            Cow::Owned(replace_typed_field_cache(
                &updated,
                *form,
                word_prefixes,
                runs,
                &field.typed_cached_comment_ranges,
                publication_scope,
                work,
            )?)
        } else {
            updated
        };
        if field.locked != *original_locked
            && (*form != FieldForm::Simple
                || field_source_lock(&updated, *form, word_prefixes)? != field.locked)
        {
            return write_field_lock_update(
                writer,
                &updated,
                field,
                foreign_word_namespace,
                publication_scope,
                work,
                updated_word_map.as_ref(),
            );
        }
        // Only the actual changed-record producers above can supply this
        // immediate transport. A borrowed input or a namespace rewrite is a
        // different role and receives no produced-record receipt here.
        let emission = work.field_xml_emission();
        let _transfer = (matches!(updated, Cow::Owned(_))
            && publication_scope.is_none()
            && foreign_word_namespace.is_none())
        .then(|| emission.prepaid_xml_transport(&updated))
        .transpose()?;
        return write_field_raw_output(
            writer,
            &updated,
            field,
            foreign_word_namespace,
            publication_scope,
            work,
            updated_word_map.as_ref(),
        );
    }

    // Rebuilding a retained own-bookmarked field would omit its physical
    // start, which the paragraph deliberately leaves inside this source.
    if let FieldSource::Parsed {
        raw_xml,
        word_prefixes,
        original_legacy_form: Some(form),
        ..
    } = field.source.as_ref()
        && let Some(name) = form.name.as_deref()
        && source_contains_own_bookmark(raw_xml, word_prefixes, name)?
    {
        return Err(OxmlError::InvalidValue(
            "a source-owned legacy bookmark requires its original field instruction identity"
                .into(),
        ));
    }

    let mut field = if work.template_word_capture.is_some() {
        field.clone_fields(None, Some(work))?
    } else {
        field.clone()
    };
    field.instruction = instruction_for_write(&field);

    let form = match field.source.as_ref() {
        FieldSource::Parsed { form, .. } => *form,
        FieldSource::New { form, .. } => *form,
    };
    let form = if instruction_contains_nested(&field.instruction) {
        FieldForm::Complex
    } else {
        form
    };
    match form {
        FieldForm::Simple => write_simple_field(
            writer,
            &field,
            foreign_word_namespace,
            result_properties,
            publication_scope,
            work,
        ),
        FieldForm::Complex => write_complex_field(
            writer,
            &field,
            foreign_word_namespace,
            result_properties,
            publication_scope,
            work,
        ),
    }
}

pub(super) fn write_field_result_content_with_binding<W: std::io::Write>(
    writer: &mut Writer<W>,
    value: &str,
    canonical_word_binding: bool,
    work: &WordOperationWork,
) -> Result<()> {
    write_field_result_content_inner(writer, value, None, canonical_word_binding, work)
}

#[derive(Clone, Copy)]
pub(super) enum FieldResultPart<'a> {
    Text(&'a str),
    Tab,
    Break(BreakType),
}

impl FieldResultPart<'_> {
    pub(super) fn into_run_content(self) -> RunContent {
        match self {
            Self::Text(value) => RunContent::Text(CT_Text {
                text: value.to_owned(),
                preserve_space: needs_space_preservation(value),
            }),
            Self::Tab => RunContent::Tab,
            Self::Break(kind) => RunContent::Break(kind, BreakClear::None),
        }
    }
}

pub(super) struct FieldResultParts<'a> {
    remaining: &'a str,
    pending: Option<FieldResultPart<'a>>,
    empty: bool,
}

impl<'a> FieldResultParts<'a> {
    pub(super) fn new(value: &'a str) -> Self {
        Self {
            remaining: value,
            pending: None,
            empty: value.is_empty(),
        }
    }
}

impl<'a> Iterator for FieldResultParts<'a> {
    type Item = FieldResultPart<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(part) = self.pending.take() {
            return Some(part);
        }
        if std::mem::take(&mut self.empty) {
            return Some(FieldResultPart::Text(""));
        }
        if self.remaining.is_empty() {
            return None;
        }
        for (index, character) in self.remaining.char_indices() {
            let part = match character {
                '\t' => FieldResultPart::Tab,
                '\n' => FieldResultPart::Break(BreakType::Line),
                '\u{000c}' => FieldResultPart::Break(BreakType::Page),
                '\u{000b}' => FieldResultPart::Break(BreakType::Column),
                _ => continue,
            };
            let text = &self.remaining[..index];
            self.remaining = &self.remaining[index + character.len_utf8()..];
            if text.is_empty() {
                return Some(part);
            }
            self.pending = Some(part);
            return Some(FieldResultPart::Text(text));
        }
        Some(FieldResultPart::Text(std::mem::take(&mut self.remaining)))
    }
}

pub(super) fn write_field_result_content_inner<W: std::io::Write>(
    writer: &mut Writer<W>,
    value: &str,
    source: Option<&BytesStart<'_>>,
    canonical_word_binding: bool,
    work: &WordOperationWork,
) -> Result<()> {
    let mut used_source = false;
    for part in FieldResultParts::new(value) {
        let (name, break_type) = match part {
            FieldResultPart::Text(text) => {
                write_field_text_with_source(
                    writer,
                    text,
                    (!used_source).then_some(source).flatten(),
                    canonical_word_binding,
                    work,
                )?;
                used_source = true;
                continue;
            }
            FieldResultPart::Tab => ("w:tab", None),
            FieldResultPart::Break(BreakType::Line) => ("w:br", None),
            FieldResultPart::Break(BreakType::Page) => ("w:br", Some("page")),
            FieldResultPart::Break(BreakType::Column) => ("w:br", Some("column")),
        };
        if !used_source && source.is_some() {
            write_field_text_with_source(writer, "", source, canonical_word_binding, work)?;
            used_source = true;
        }
        let emission = work.field_xml_emission();
        let mut element = FieldXmlStart::new(name, Some(&emission))?;
        if canonical_word_binding {
            element.push("xmlns:w", crate::namespace::W_NS)?;
        }
        if let Some(break_type) = break_type {
            element.push("w:type", break_type)?;
        }
        element.write(writer, true)?;
    }
    Ok(())
}

pub(super) fn write_field_text_with_source<W: std::io::Write>(
    writer: &mut Writer<W>,
    value: &str,
    source: Option<&BytesStart<'_>>,
    canonical_word_binding: bool,
    work: &WordOperationWork,
) -> Result<()> {
    // Keep the source Word alias when canonical w would shadow foreign
    // attribute bindings. Generated text without a source gets its own scope.
    let mut preserve_source_name = canonical_word_binding;
    if let Some(source) = source {
        for attribute in source.attributes() {
            let attribute = attribute?;
            if attribute.key.as_ref() == b"xmlns:w"
                && attribute.value.as_ref() != crate::namespace::W_NS.as_bytes()
            {
                preserve_source_name = true;
            }
        }
    }
    let source_name = source
        .filter(|_| preserve_source_name)
        .map(|source| source.name());
    let source_name = source_name
        .as_ref()
        .map(|name| std::str::from_utf8(name.as_ref()))
        .transpose()?;
    let emission = work.field_xml_emission();
    let mut element = match source_name {
        Some(name) => match name.rsplit_once(':') {
            Some((prefix, _)) => FieldXmlStart::word(prefix, "t", Some(&emission))?,
            None => FieldXmlStart::new("t", Some(&emission))?,
        },
        None => FieldXmlStart::new("w:t", Some(&emission))?,
    };
    if let Some(source) = source {
        for attribute in source.attributes() {
            let attribute = attribute?;
            if attribute.key.as_ref() != b"xml:space" {
                element.push_raw_attribute(attribute)?;
            }
        }
    }
    if canonical_word_binding && source.is_none() {
        element.push("xmlns:w", crate::namespace::W_NS)?;
    }
    if needs_space_preservation(value) {
        element.push("xml:space", "preserve")?;
    }
    // The selected QName is owned only by its already-reserved event. Borrow
    // the same name for the closing event instead of allocating it again.
    let end_name = element.start.name();
    let end_name = std::str::from_utf8(end_name.as_ref())?;
    // Emit the opening event through its prepaid payload while its borrowed
    // QName remains live for the matching closing event.
    let prepaid = element
        .prepaid
        .then(|| emission.prepaid_xml_transport(element.start.as_ref()))
        .transpose()?;
    writer.write_event(Event::Start(element.start.borrow()))?;
    drop(prepaid);
    let word = (element.start.local_name().as_ref() == b"t")
        .then(|| template_word_scalar_input(work, value))
        .flatten();
    emission.write_text(writer, value, word)?;
    writer.write_event(Event::End(BytesEnd::new(end_name)))?;
    Ok(())
}

pub(super) fn needs_space_preservation(value: &str) -> bool {
    value.chars().next().is_some_and(char::is_whitespace)
        || value.chars().next_back().is_some_and(char::is_whitespace)
}

pub(super) fn instruction_for_write(field: &Field) -> FieldInstruction {
    let original = match field.source.as_ref() {
        FieldSource::New {
            original_instruction,
            ..
        }
        | FieldSource::Parsed {
            original_instruction,
            ..
        } => original_instruction,
    };
    let raw_changed = field.instruction.raw != original.raw;
    let structured_changed = !instruction_structure_eq(&field.instruction, original);

    if raw_changed && !structured_changed {
        return parse_field_instruction(&field.instruction.raw);
    }
    if structured_changed {
        let mut instruction = field.instruction.clone();
        instruction.raw = canonical_instruction_text(&instruction);
        return instruction;
    }
    field.instruction.clone()
}

pub(super) fn instruction_structure_eq(left: &FieldInstruction, right: &FieldInstruction) -> bool {
    left.name == right.name
        && left.arguments.len() == right.arguments.len()
        && left
            .arguments
            .iter()
            .zip(&right.arguments)
            .all(|(left, right)| argument_structure_eq(left, right))
        && left.switches.len() == right.switches.len()
        && left
            .switches
            .iter()
            .zip(&right.switches)
            .all(|(left, right)| {
                left.name == right.name
                    && match (&left.argument, &right.argument) {
                        (None, None) => true,
                        (Some(left), Some(right)) => argument_structure_eq(left, right),
                        _ => false,
                    }
            })
}

fn argument_structure_eq(left: &FieldArgument, right: &FieldArgument) -> bool {
    match (left, right) {
        (FieldArgument::Text(left), FieldArgument::Text(right)) => left == right,
        (FieldArgument::Nested(left), FieldArgument::Nested(right)) => {
            instruction_structure_eq(&left.instruction, &right.instruction)
        }
        (FieldArgument::Joined(left), FieldArgument::Joined(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| argument_structure_eq(left, right))
        }
        _ => false,
    }
}

pub(super) fn instruction_source_identity_eq(
    left: &FieldInstruction,
    right: &FieldInstruction,
) -> bool {
    let argument_identities_match = left
        .arguments
        .iter()
        .zip(&right.arguments)
        .all(|(left, right)| argument_identity_eq(left, right));
    let switch_identities_match = left
        .switches
        .iter()
        .zip(&right.switches)
        .all(|(left, right)| match (&left.argument, &right.argument) {
            (None, None) => true,
            (Some(left), Some(right)) => argument_identity_eq(left, right),
            _ => false,
        });
    left.arguments.len() == right.arguments.len()
        && left.switches.len() == right.switches.len()
        && argument_identities_match
        && switch_identities_match
}

fn argument_identity_eq(left: &FieldArgument, right: &FieldArgument) -> bool {
    match (left, right) {
        (FieldArgument::Nested(left), FieldArgument::Nested(right)) => {
            field_source_identity_eq(left, right)
        }
        (FieldArgument::Text(_), FieldArgument::Text(_)) => true,
        (FieldArgument::Joined(left), FieldArgument::Joined(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| argument_identity_eq(left, right))
        }
        _ => false,
    }
}

fn field_source_identity_eq(left: &Field, right: &Field) -> bool {
    match (left.source.as_ref(), right.source.as_ref()) {
        (
            FieldSource::Parsed {
                source_id: left_id, ..
            },
            FieldSource::Parsed {
                source_id: right_id,
                ..
            },
        ) => {
            left_id == right_id
                && instruction_source_identity_eq(&left.instruction, &right.instruction)
        }
        _ => false,
    }
}

pub(super) fn canonical_instruction_text(instruction: &FieldInstruction) -> String {
    let mut text = instruction.name.clone();
    for argument in &instruction.arguments {
        if let FieldArgument::Text(value) = argument {
            push_canonical_field_token(&mut text, value, false);
        }
    }
    for switch in &instruction.switches {
        text.push(' ');
        text.push('\\');
        text.push_str(&switch.name);
        if let Some(FieldArgument::Text(value)) = &switch.argument {
            push_canonical_field_token(
                &mut text,
                value,
                !switch_takes_argument(&instruction.name, &switch.name),
            );
        }
    }
    text
}

pub(super) fn instruction_contains_nested(instruction: &FieldInstruction) -> bool {
    instruction
        .arguments
        .iter()
        .any(|argument| !argument.nested_fields().is_empty())
        || instruction.switches.iter().any(|switch| {
            switch
                .argument
                .as_ref()
                .is_some_and(|argument| !argument.nested_fields().is_empty())
        })
}

fn write_simple_field<W: std::io::Write>(
    writer: &mut Writer<W>,
    field: &Field,
    foreign_word_namespace: Option<&str>,
    result_properties: Option<&CT_RPr>,
    publication_scope: Option<&[(&str, &str)]>,

    work: &mut WordOperationWork,
) -> Result<()> {
    let pieces = [
        Some(FieldSourcePiece::Literal(b"w:fldSimple")),
        foreign_word_namespace.map(|_| {
            FieldSourcePiece::Literal(
                concat!(
                    " xmlns:w=\"",
                    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
                    "\""
                )
                .as_bytes(),
            )
        }),
        Some(FieldSourcePiece::Literal(b" w:instr=\"")),
        Some(FieldSourcePiece::Escaped(&field.instruction.raw)),
        Some(FieldSourcePiece::Literal(b"\"")),
        field.dirty.map(|value| {
            FieldSourcePiece::Literal(if value {
                b" w:dirty=\"1\""
            } else {
                b" w:dirty=\"0\""
            })
        }),
        field.locked.map(|value| {
            FieldSourcePiece::Literal(if value {
                b" w:fldLock=\"1\""
            } else {
                b" w:fldLock=\"0\""
            })
        }),
    ];
    let length = pieces.iter().flatten().try_fold(0usize, |total, piece| {
        let size = match piece {
            FieldSourcePiece::Literal(bytes) | FieldSourcePiece::Raw(bytes) => bytes.len(),
            FieldSourcePiece::Escaped(value) => escaped_source_len(value)?,
        };
        total
            .checked_add(size)
            .ok_or_else(|| OxmlError::InvalidValue("field source byte overflow".into()))
    })?;
    let prepaid = work.reserve_inner_xml(length)?;
    let mut body = Vec::with_capacity(length);
    for piece in pieces.into_iter().flatten() {
        match piece {
            FieldSourcePiece::Literal(bytes) | FieldSourcePiece::Raw(bytes) => {
                body.extend_from_slice(bytes)
            }
            FieldSourcePiece::Escaped(value) => append_escaped_source(&mut body, value),
        }
    }
    let element = BytesStart::from_content(
        String::from_utf8(body).expect("field header is UTF-8"),
        "w:fldSimple".len(),
    );
    {
        let _transport = prepaid
            .then(|| work.prepaid_xml_transport(element.as_ref()))
            .transpose()?;
        writer.write_event(Event::Start(element.borrow()))?;
    }
    let mut result_scope = publication_scope.map(|scope| scope.to_vec());
    if let Some(scope) = &mut result_scope {
        scope.retain(|(prefix, _)| *prefix != "w");
        scope.push(("w", crate::namespace::W_NS));
    }
    write_field_result_run(
        writer,
        field,
        foreign_word_namespace,
        result_properties,
        result_scope.as_deref(),
        None,
        work,
    )?;
    writer.write_event(Event::End(BytesEnd::new("w:fldSimple")))?;
    Ok(())
}

fn write_complex_field<W: std::io::Write>(
    writer: &mut Writer<W>,
    field: &Field,
    foreign_word_namespace: Option<&str>,
    result_properties: Option<&CT_RPr>,
    publication_scope: Option<&[(&str, &str)]>,

    work: &mut WordOperationWork,
) -> Result<()> {
    write_field_char_run(
        writer,
        "begin",
        field.dirty,
        field.locked,
        foreign_word_namespace,
        Some(field),
        work,
    )?;
    let mut text = format!(" {}", field.instruction.name);
    for argument in &field.instruction.arguments {
        match argument {
            FieldArgument::Text(value) => push_canonical_field_token(&mut text, value, false),
            FieldArgument::Nested(field) => {
                text.push(' ');
                write_instruction_run(writer, &text, foreign_word_namespace, work)?;
                text.clear();
                write_nested_instruction_field(
                    writer,
                    field,
                    foreign_word_namespace,
                    publication_scope,
                    work,
                )?;
            }
            FieldArgument::Joined(pieces) => write_joined_operand(
                writer,
                &mut text,
                pieces,
                foreign_word_namespace,
                publication_scope,
                work,
            )?,
        }
    }
    for switch in &field.instruction.switches {
        text.push(' ');
        text.push('\\');
        text.push_str(&switch.name);
        if let Some(argument) = &switch.argument {
            match argument {
                FieldArgument::Text(value) => push_canonical_field_token(
                    &mut text,
                    value,
                    !switch_takes_argument(&field.instruction.name, &switch.name),
                ),
                FieldArgument::Nested(field) => {
                    text.push(' ');
                    write_instruction_run(writer, &text, foreign_word_namespace, work)?;
                    text.clear();
                    write_nested_instruction_field(
                        writer,
                        field,
                        foreign_word_namespace,
                        publication_scope,
                        work,
                    )?;
                }
                FieldArgument::Joined(pieces) => write_joined_operand(
                    writer,
                    &mut text,
                    pieces,
                    foreign_word_namespace,
                    publication_scope,
                    work,
                )?,
            }
        }
    }
    text.push(' ');
    if !text.trim().is_empty() {
        write_instruction_run(writer, &text, foreign_word_namespace, work)?;
    }
    write_field_char_run(
        writer,
        "separate",
        None,
        None,
        foreign_word_namespace,
        None,
        work,
    )?;
    write_field_result_run(
        writer,
        field,
        foreign_word_namespace,
        result_properties,
        publication_scope,
        None,
        work,
    )?;
    write_field_char_run(
        writer,
        "end",
        None,
        None,
        foreign_word_namespace,
        None,
        work,
    )?;
    Ok(())
}

fn write_nested_instruction_field<W: std::io::Write>(
    writer: &mut Writer<W>,
    field: &Field,
    foreign_word_namespace: Option<&str>,
    publication_scope: Option<&[(&str, &str)]>,

    work: &mut WordOperationWork,
) -> Result<()> {
    let mut field = field.clone();
    field.instruction = instruction_for_write(&field);
    write_complex_field(
        writer,
        &field,
        foreign_word_namespace,
        None,
        publication_scope,
        work,
    )
}

/// Write a joined operand quoted, its text escaped and its nested fields in
/// place, so that it reads back as the same operand.
fn write_joined_operand<W: std::io::Write>(
    writer: &mut Writer<W>,
    text: &mut String,
    pieces: &[FieldArgument],
    foreign_word_namespace: Option<&str>,
    publication_scope: Option<&[(&str, &str)]>,
    work: &mut WordOperationWork,
) -> Result<()> {
    text.push_str(" \"");
    for piece in pieces {
        match piece {
            FieldArgument::Text(value) => {
                for character in value.chars() {
                    if matches!(character, '"' | '\\') {
                        text.push('\\');
                    }
                    text.push(character);
                }
            }
            FieldArgument::Nested(field) => {
                write_instruction_run(writer, text, foreign_word_namespace, work)?;
                text.clear();
                write_nested_instruction_field(
                    writer,
                    field,
                    foreign_word_namespace,
                    publication_scope,
                    work,
                )?;
            }
            FieldArgument::Joined(_) => {
                return Err(OxmlError::InvalidValue(
                    "a joined field operand holds a joined operand".into(),
                ));
            }
        }
    }
    text.push('"');
    Ok(())
}

fn push_canonical_field_token(output: &mut String, value: &str, force_quotes: bool) {
    output.push(' ');
    if force_quotes
        || value.is_empty()
        || value.starts_with('\\')
        || value.chars().any(char::is_whitespace)
        || value.contains('"')
    {
        output.push('"');
        for character in value.chars() {
            if matches!(character, '"' | '\\') {
                output.push('\\');
            }
            output.push(character);
        }
        output.push('"');
    } else {
        output.push_str(value);
    }
}

fn write_instruction_run<W: std::io::Write>(
    writer: &mut Writer<W>,
    instruction: &str,
    foreign_word_namespace: Option<&str>,
    work: &WordOperationWork,
) -> Result<()> {
    write_word_run_start(
        writer,
        foreign_word_namespace,
        Some(&work.field_xml_emission()),
    )?;
    let emission = work.field_xml_emission();
    let mut element = FieldXmlStart::new("w:instrText", Some(&emission))?;
    element.push("xml:space", "preserve")?;
    element.write(writer, false)?;
    work.field_xml_emission()
        .write_text(writer, instruction, None)?;
    writer.write_event(Event::End(BytesEnd::new("w:instrText")))?;
    writer.write_event(Event::End(BytesEnd::new("w:r")))?;
    Ok(())
}

fn write_field_char_run<W: std::io::Write>(
    writer: &mut Writer<W>,
    kind: &str,
    dirty: Option<bool>,
    locked: Option<bool>,
    foreign_word_namespace: Option<&str>,
    legacy_field: Option<&Field>,
    work: &WordOperationWork,
) -> Result<()> {
    write_word_run_start(
        writer,
        foreign_word_namespace,
        Some(&work.field_xml_emission()),
    )?;
    let emission = work.field_xml_emission();
    let mut element = FieldXmlStart::new("w:fldChar", Some(&emission))?;
    element.push("w:fldCharType", kind)?;
    if let Some(dirty) = dirty {
        element.push("w:dirty", if dirty { "1" } else { "0" })?;
    }
    if let Some(locked) = locked {
        element.push("w:fldLock", if locked { "1" } else { "0" })?;
    }
    if kind == "begin" && legacy_field.is_some_and(|field| field.legacy_form.is_some()) {
        element.write(writer, false)?;
        write_legacy_form_ff_data(writer, legacy_field.expect("legacy field"), Some(&emission))?;
        writer.write_event(Event::End(BytesEnd::new("w:fldChar")))?;
    } else {
        element.write(writer, true)?;
    }
    writer.write_event(Event::End(BytesEnd::new("w:r")))?;
    Ok(())
}

pub(super) fn write_field_result_run<W: std::io::Write>(
    writer: &mut Writer<W>,
    field: &Field,
    foreign_word_namespace: Option<&str>,
    properties: Option<&CT_RPr>,
    publication_scope: Option<&[(&str, &str)]>,
    source_context: Option<&[(&str, &str)]>,

    work: &mut WordOperationWork,
) -> Result<()> {
    if let Some(runs) = field.typed_cache() {
        return write_typed_cached_runs(
            writer,
            runs,
            &field.typed_cached_comment_ranges,
            foreign_word_namespace,
            publication_scope,
            source_context,
            work,
        );
    }
    if let Some(runs) = field.new_result_runs() {
        let emission = work.field_xml_emission();
        for run in runs {
            if run
                .content
                .iter()
                .any(|content| matches!(content, RunContent::Field(_)))
            {
                write_mixed_field_run(
                    writer,
                    run,
                    foreign_word_namespace,
                    publication_scope,
                    source_context,
                    work,
                    Some(&emission),
                )?;
            } else {
                run.write_xml_role(
                    writer,
                    foreign_word_namespace,
                    if publication_scope.is_some() {
                        XmlWriteRole::Publication
                    } else {
                        XmlWriteRole::Source
                    },
                    source_context,
                    Some(&emission),
                    work.template_word_capture(),
                )?;
            }
        }
        return Ok(());
    }
    write_word_run_start(
        writer,
        foreign_word_namespace,
        Some(&work.field_xml_emission()),
    )?;
    let properties = properties.or_else(|| match field.source.as_ref() {
        FieldSource::New {
            cached_runs: Some(runs),
            ..
        } => runs.first().and_then(|run| run.properties.as_ref()),
        _ => None,
    });
    if let Some(properties) = properties {
        properties.write_xml_role(
            writer,
            foreign_word_namespace,
            if publication_scope.is_some() {
                XmlWriteRole::Publication
            } else {
                XmlWriteRole::Source
            },
            source_context,
            Some(&work.field_xml_emission()),
        )?;
    }
    let display = if field.cached_result.is_empty()
        && matches!(field.instruction.name.as_str(), "PAGE" | "NUMPAGES")
    {
        "1"
    } else {
        &field.cached_result
    };
    let _scalar = (std::ptr::eq(display, field.cached_result.as_str()))
        .then(|| work.template_field_scalar(field, display))
        .flatten();
    write_field_result_content_inner(writer, display, None, false, work)?;
    writer.write_event(Event::End(BytesEnd::new("w:r")))?;
    Ok(())
}

pub(super) fn write_word_run_start<W: std::io::Write>(
    writer: &mut Writer<W>,
    foreign_word_namespace: Option<&str>,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    let mut run = FieldXmlStart::new("w:r", emission)?;
    if foreign_word_namespace.is_some() {
        run.push("xmlns:w", crate::namespace::W_NS)?;
    }
    run.write(writer, false)
}
