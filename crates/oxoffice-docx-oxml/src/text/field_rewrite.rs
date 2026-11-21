//! Rewriting field sources in place: nested fields, cached results, instructions and locks.

use super::*;

pub(super) fn usable_qualified_word_prefix<'a>(
    word_prefixes: &'a [String],
    tag: &[u8],
    preferred: &'a str,
) -> (Cow<'a, str>, bool) {
    if !preferred.is_empty() && word_prefixes.iter().any(|prefix| prefix == preferred) {
        return (Cow::Borrowed(preferred), false);
    }
    if let Some(prefix) = word_prefixes
        .iter()
        .find(|prefix| !prefix.is_empty() && !prefix.starts_with('\0'))
    {
        return (Cow::Borrowed(prefix), false);
    }
    for index in 0usize.. {
        let candidate = if index == 0 {
            "w".to_owned()
        } else {
            format!("w{index}")
        };
        if !tag
            .windows(candidate.len() + 6)
            .any(|window| window.starts_with(b"xmlns:") && window[6..] == *candidate.as_bytes())
        {
            return (Cow::Owned(candidate), true);
        }
    }
    unreachable!()
}

pub(super) fn lexical_attribute_value_range(
    tag: &[u8],
    wanted: &[u8],
) -> Option<std::ops::Range<usize>> {
    let mut cursor = tag.iter().position(|byte| byte.is_ascii_whitespace())?;
    while cursor < tag.len() {
        while tag.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if matches!(tag.get(cursor), Some(b'>' | b'/')) {
            return None;
        }
        let name_start = cursor;
        while tag
            .get(cursor)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'>' | b'/'))
        {
            cursor += 1;
        }
        let name = &tag[name_start..cursor];
        while tag.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if tag.get(cursor) != Some(&b'=') {
            return None;
        }
        cursor += 1;
        while tag.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let quote = *tag.get(cursor)?;
        if !matches!(quote, b'\"' | b'\'') {
            return None;
        }
        cursor += 1;
        let value_start = cursor;
        while tag.get(cursor) != Some(&quote) {
            cursor += 1;
            if cursor >= tag.len() {
                return None;
            }
        }
        let value_end = cursor;
        cursor += 1;
        if name == wanted {
            return Some(value_start..value_end);
        }
    }
    None
}

pub(super) fn nested_instruction_source_edits(
    field: &Field,
    raw: &[u8],
    word_prefixes: &[String],
    scan: &ComplexSourceScan,

    work: &mut WordOperationWork,
) -> Result<Vec<(usize, usize, Vec<u8>)>> {
    let _source_scope = root_binding_scope(0);
    let nested_fields = field.nested_fields_in_source_order();
    if scan.nested.len() != nested_fields.len() {
        return Err(OxmlError::MissingElement(
            "nested field spans in owning complex field".to_owned(),
        ));
    }
    let mut edits = Vec::new();
    for (nested, span) in nested_fields.into_iter().zip(&scan.nested) {
        if nested.is_unchanged() {
            continue;
        }
        let replacement =
            rewrite_isolated_nested_field(nested, raw, span, &scan.runs, word_prefixes, work)?;
        edits.push((span.start, span.end, replacement));
    }
    Ok(edits)
}

pub(super) fn update_nested_field_sources<'a>(
    field: &Field,
    raw: &'a [u8],
    word_prefixes: &[String],

    work: &mut WordOperationWork,
) -> Result<Cow<'a, [u8]>> {
    let _source_scope = root_binding_scope(0);
    let scan = scan_complex_source(raw, word_prefixes)?;
    let mut edits = nested_instruction_source_edits(field, raw, word_prefixes, &scan, work)?;

    let children = field.cached_fields_in_source_order();
    let complex_children = children
        .iter()
        .copied()
        .filter(|child| child.form() == FieldForm::Complex)
        .collect::<Vec<_>>();
    let simple_children = children
        .iter()
        .copied()
        .filter(|child| child.form() == FieldForm::Simple)
        .collect::<Vec<_>>();
    if field.typed_cache().is_none()
        && (scan.result_nested.len() != complex_children.len()
            || scan.result_simple.len() != simple_children.len())
    {
        return Err(OxmlError::MissingElement(
            "nested cached field spans in owning complex field".to_owned(),
        ));
    }
    for (nested, span) in complex_children.into_iter().zip(scan.result_nested) {
        if field.parsed_cached_child_runs().is_some() {
            let expected = &raw[scan.runs[span.start_run].start..scan.runs[span.end_run].end];
            if !matches!(nested.source.as_ref(), FieldSource::Parsed { raw_xml, .. } if raw_xml.as_slice() == expected)
            {
                return Err(OxmlError::InvalidValue(
                    "replacement cached field has no original complex-cache source identity"
                        .to_owned(),
                ));
            }
        }
        if nested.is_unchanged() {
            continue;
        }
        let replacement =
            rewrite_isolated_nested_field(nested, raw, &span, &scan.runs, word_prefixes, work)?;
        edits.push((span.start, span.end, replacement));
    }
    for (child, (start, end)) in simple_children.into_iter().zip(scan.result_simple) {
        if field.parsed_cached_child_runs().is_some()
            && !matches!(child.source.as_ref(), FieldSource::Parsed { raw_xml, .. } if raw_xml.as_slice() == &raw[start..end])
        {
            return Err(OxmlError::InvalidValue(
                "replacement cached field has no original simple-cache source identity".to_owned(),
            ));
        }
        if child.is_unchanged() {
            continue;
        }
        let emission = work.field_xml_emission();
        let mut writer = Writer::new(FieldXmlRecord::new(&emission));
        write_field(&mut writer, child, None, work)?;
        edits.push((start, end, writer.into_inner().into_bytes()));
    }
    if edits.is_empty() {
        return Ok(Cow::Borrowed(raw));
    }
    edits.sort_by_key(|(start, _, _)| *start);
    let mut pieces = Vec::new();
    let mut at = 0;
    for (start, end, replacement) in &edits {
        pieces.push(FieldSourcePiece::Raw(&raw[at..*start]));
        pieces.push(FieldSourcePiece::Raw(replacement));
        at = *end;
    }
    pieces.push(FieldSourcePiece::Raw(&raw[at..]));
    let mut updated = Vec::new();
    work.field_xml_emission()
        .write_xml_pieces(&mut updated, &pieces)?;
    Ok(Cow::Owned(updated))
}

#[derive(Debug)]
pub(super) struct ComplexSourceScan {
    runs: Vec<RunSourceSpan>,
    nested: Vec<NestedComplexSpan>,
    result_nested: Vec<NestedComplexSpan>,
    result_simple: Vec<(usize, usize)>,
}

#[derive(Debug)]
struct RunSourceSpan {
    start: usize,
    content_start: usize,
    content_end: usize,
    end: usize,
    field_content: Vec<(usize, usize)>,
}

#[derive(Debug)]
struct NestedComplexSpan {
    start: usize,
    end: usize,
    start_run: usize,
    end_run: usize,
}

fn is_word_source_name(name: &[u8], word_prefixes: &[String]) -> bool {
    let Some(separator) = name.iter().position(|byte| *byte == b':') else {
        return word_prefixes.iter().any(String::is_empty);
    };
    word_prefixes
        .iter()
        .any(|prefix| prefix.as_bytes() == &name[..separator])
}

pub(super) fn scan_complex_source(
    raw: &[u8],
    word_prefixes: &[String],
) -> Result<ComplexSourceScan> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut runs = Vec::new();
    let mut open_fields = Vec::<OpenComplexSourceField>::new();
    let mut nested = Vec::new();
    let mut result_nested = Vec::new();
    let mut result_simple = Vec::new();
    loop {
        let run_start = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let run_prefixes = word_prefixes_at(&element, word_prefixes)?;
                if is_word_element(element.name().as_ref(), b"fldSimple", &run_prefixes) {
                    reader.read_to_end_into(element.name(), &mut Vec::new())?;
                    if open_fields.len() == 1 && open_fields[0].separated {
                        result_simple.push((run_start, reader.buffer_position() as usize));
                    }
                    buffer.clear();
                    continue;
                }
                if !is_word_element(element.name().as_ref(), b"r", &run_prefixes) {
                    reader.read_to_end_into(element.name(), &mut Vec::new())?;
                    buffer.clear();
                    continue;
                }
                let run_index = runs.len();
                let content_start = reader.buffer_position() as usize;
                let mut field_content = Vec::new();
                loop {
                    buffer.clear();
                    let child_start = reader.buffer_position() as usize;
                    match reader.read_event_into(&mut buffer)? {
                        Event::Start(child) => {
                            let prefixes = word_prefixes_at(&child, &run_prefixes)?;
                            let is_field_content =
                                !is_word_element(child.name().as_ref(), b"rPr", &prefixes)
                                    && is_word_source_name(child.name().as_ref(), &prefixes);
                            if is_word_element(child.name().as_ref(), b"fldChar", &prefixes) {
                                let kind =
                                    optional_word_attribute(&child, b"fldCharType", &prefixes);
                                reader.read_to_end_into(child.name(), &mut Vec::new())?;
                                record_complex_source_marker(
                                    kind.as_deref(),
                                    child_start,
                                    reader.buffer_position() as usize,
                                    run_index,
                                    &mut open_fields,
                                    &mut nested,
                                    &mut result_nested,
                                );
                            } else {
                                reader.read_to_end_into(child.name(), &mut Vec::new())?;
                            }
                            if is_field_content {
                                field_content
                                    .push((child_start, reader.buffer_position() as usize));
                            }
                        }
                        Event::Empty(child) => {
                            let prefixes = word_prefixes_at(&child, &run_prefixes)?;
                            let is_field_content =
                                !is_word_element(child.name().as_ref(), b"rPr", &prefixes)
                                    && is_word_source_name(child.name().as_ref(), &prefixes);
                            if is_word_element(child.name().as_ref(), b"fldChar", &prefixes) {
                                let kind =
                                    optional_word_attribute(&child, b"fldCharType", &prefixes);
                                record_complex_source_marker(
                                    kind.as_deref(),
                                    child_start,
                                    reader.buffer_position() as usize,
                                    run_index,
                                    &mut open_fields,
                                    &mut nested,
                                    &mut result_nested,
                                );
                            }
                            if is_field_content {
                                field_content
                                    .push((child_start, reader.buffer_position() as usize));
                            }
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), b"r") => {
                            runs.push(RunSourceSpan {
                                start: run_start,
                                content_start,
                                content_end: child_start,
                                end: reader.buffer_position() as usize,
                                field_content,
                            });
                            break;
                        }
                        Event::Eof => {
                            return Err(OxmlError::MissingElement(
                                "end of field source run".to_owned(),
                            ));
                        }
                        _ => {}
                    }
                }
            }
            Event::Eof => {
                return Ok(ComplexSourceScan {
                    runs,
                    nested,
                    result_nested,
                    result_simple,
                });
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn field_source_has_unmodeled_semantic_attributes(
    raw: &[u8],
    form: FieldForm,
    word_prefixes: &[String],
) -> Result<bool> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) | Event::Empty(element) => {
                let prefixes = word_prefixes_at(&element, word_prefixes)?;
                let allowed = match form {
                    FieldForm::Simple
                        if is_word_element(element.name().as_ref(), b"fldSimple", &prefixes) =>
                    {
                        &[b"instr".as_slice(), b"dirty".as_slice()][..]
                    }
                    FieldForm::Complex
                        if is_word_element(element.name().as_ref(), b"fldChar", &prefixes) =>
                    {
                        &[b"fldCharType".as_slice(), b"dirty".as_slice()][..]
                    }
                    _ => {
                        buffer.clear();
                        continue;
                    }
                };

                for attribute in element.attributes() {
                    let attribute = attribute?;
                    let key = attribute.key.as_ref();
                    if key == b"xmlns" || key.starts_with(b"xmlns:") {
                        continue;
                    }
                    if allowed
                        .iter()
                        .any(|local| is_word_attribute(key, local, &prefixes))
                    {
                        continue;
                    }
                    return Ok(true);
                }

                if matches!(form, FieldForm::Simple) {
                    return Ok(false);
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn record_complex_source_marker(
    kind: Option<&str>,
    start: usize,
    end: usize,
    run_index: usize,
    open_fields: &mut Vec<OpenComplexSourceField>,
    nested: &mut Vec<NestedComplexSpan>,
    result_nested: &mut Vec<NestedComplexSpan>,
) {
    match kind {
        Some("begin") => open_fields.push(OpenComplexSourceField {
            start,
            start_run: run_index,
            separated: false,
        }),
        Some("separate") => {
            if let Some(field) = open_fields.last_mut() {
                field.separated = true;
            }
        }
        Some("end") => {
            let Some(field) = open_fields.pop() else {
                return;
            };
            if open_fields.len() == 1 {
                let target = if open_fields[0].separated {
                    result_nested
                } else {
                    nested
                };
                target.push(NestedComplexSpan {
                    start: field.start,
                    end,
                    start_run: field.start_run,
                    end_run: run_index,
                });
            }
        }
        _ => {}
    }
}

struct OpenComplexSourceField {
    start: usize,
    start_run: usize,
    separated: bool,
}

fn rewrite_isolated_nested_field(
    field: &Field,
    raw: &[u8],
    field_span: &NestedComplexSpan,
    runs: &[RunSourceSpan],
    word_prefixes: &[String],

    work: &mut WordOperationWork,
) -> Result<Vec<u8>> {
    let start_run = &runs[field_span.start_run];
    let end_run = &runs[field_span.end_run];
    let mut isolated = raw[start_run.start..start_run.content_start].to_vec();
    if field_span.start_run == field_span.end_run {
        isolated.extend_from_slice(&raw[field_span.start..field_span.end]);
    } else {
        isolated.extend_from_slice(&raw[field_span.start..start_run.end]);
        isolated.extend_from_slice(&raw[start_run.end..end_run.content_start]);
        isolated.extend_from_slice(&raw[end_run.content_start..field_span.end]);
    }
    isolated.extend_from_slice(&raw[end_run.content_end..end_run.end]);

    let FieldSource::Parsed {
        original_instruction,
        original_cached_result,
        original_dirty,
        original_legacy_form,
        original_result_published,
        ..
    } = field.source.as_ref()
    else {
        return Err(OxmlError::MissingElement(
            "parsed nested field source".to_owned(),
        ));
    };
    let raw_only_instruction_changed = field.instruction.raw != original_instruction.raw
        && instruction_structure_eq(&field.instruction, original_instruction);
    let mut updated = if raw_only_instruction_changed {
        Cow::Owned(remove_nested_field_sources(&isolated, word_prefixes, work)?)
    } else {
        update_nested_field_sources(field, &isolated, word_prefixes, work)?
    };
    if raw_only_instruction_changed {
        updated = Cow::Owned(update_complex_instruction_source(
            field,
            &updated,
            word_prefixes,
            work,
        )?);
    }
    let legacy_form_changed = field.legacy_form != *original_legacy_form;
    let text_current_only = legacy_text_value_only_change(original_legacy_form, &field.legacy_form);
    if legacy_form_changed && !text_current_only {
        let form = field
            .legacy_form
            .as_ref()
            .ok_or_else(|| OxmlError::MissingElement("typed legacy form data".to_owned()))?;
        updated = Cow::Owned(rewrite_legacy_form_source(
            &updated,
            word_prefixes,
            form,
            work,
        )?);
    }
    let cached_changed = field.cached_result != *original_cached_result
        && field.nested_cached_projection().as_ref() != Some(&field.cached_result);
    let publish_missing = field.legacy_result_published && !*original_result_published;
    if cached_changed || publish_missing || field.dirty != *original_dirty {
        updated = Cow::Owned(update_complex_field_source(
            field,
            &updated,
            word_prefixes,
            cached_changed || publish_missing,
            publish_missing,
            work,
            None,
        )?);
    }
    if let Some(runs) = field.typed_cache() {
        updated = Cow::Owned(replace_typed_field_cache(
            &updated,
            FieldForm::Complex,
            word_prefixes,
            runs,
            &field.typed_cached_comment_ranges,
            None,
            work,
        )?);
    }
    let updated_scan = scan_complex_source(&updated, word_prefixes)?;
    let (Some(first_run), Some(last_run)) = (updated_scan.runs.first(), updated_scan.runs.last())
    else {
        return Err(OxmlError::MissingElement(
            "updated nested field runs".to_owned(),
        ));
    };
    let range = first_run.content_start..last_run.content_end;
    match updated {
        Cow::Borrowed(source) => Ok(source[range].to_vec()),
        Cow::Owned(mut source) => {
            // This existing generated record is cropped in place, with no
            // additional output materialization and no refund of spent work.
            let _ = &source[range.clone()];
            source.truncate(range.end);
            source.drain(..range.start);
            Ok(source)
        }
    }
}

fn remove_nested_field_sources(
    raw: &[u8],
    word_prefixes: &[String],
    work: &WordOperationWork,
) -> Result<Vec<u8>> {
    let scan = scan_complex_source(raw, word_prefixes)?;
    let mut removals = Vec::new();
    for span in &scan.nested {
        for run in &scan.runs[span.start_run..=span.end_run] {
            removals.extend(
                run.field_content
                    .iter()
                    .copied()
                    .filter(|(start, end)| *start >= span.start && *end <= span.end),
            );
        }
    }
    removals.sort_unstable();
    removals.dedup();
    if removals.is_empty() {
        return Ok(raw.to_vec());
    }
    // Actual direct run-child intervals are disjoint. Sorting/dedup above
    // preserves the same source selection as the old reverse drains.
    let mut pieces = Vec::new();
    let mut at = 0;
    for (start, end) in removals {
        pieces.push(FieldSourcePiece::Raw(&raw[at..start]));
        at = end;
    }
    pieces.push(FieldSourcePiece::Raw(&raw[at..]));
    let mut updated = Vec::new();
    work.field_xml_emission()
        .write_xml_pieces(&mut updated, &pieces)?;
    Ok(updated)
}

struct FieldRewriteContext {
    prefixes: Vec<String>,
    word_run: bool,
    canonical_end: Option<&'static str>,
}

fn find_cached_field_source(
    raw: &[u8],
    source: &[u8],
    from: usize,
    inherited: &[String],
) -> Result<Option<usize>> {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let mut scopes = Vec::<Vec<String>>::new();
    loop {
        let start = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let prefixes =
                    word_prefixes_at(&element, scopes.last().map_or(inherited, Vec::as_slice))?;
                let field_source = is_word_element(element.name().as_ref(), b"r", &prefixes)
                    || is_word_element(element.name().as_ref(), b"fldSimple", &prefixes);
                if !scopes.is_empty()
                    && field_source
                    && start >= from
                    && raw.get(start..start.saturating_add(source.len())) == Some(source)
                {
                    return Ok(Some(start));
                }
                let wrapper = [
                    b"fldSimple".as_slice(),
                    b"hyperlink",
                    b"sdt",
                    b"sdtContent",
                    b"ins",
                    b"del",
                    b"moveFrom",
                    b"moveTo",
                    b"smartTag",
                    b"customXml",
                ]
                .iter()
                .any(|local| is_word_element(element.name().as_ref(), local, &prefixes));
                if wrapper {
                    scopes.push(prefixes.into_owned());
                } else {
                    reader.read_to_end_into(element.name(), &mut Vec::new())?;
                }
            }
            Event::Empty(element) => {
                let prefixes =
                    word_prefixes_at(&element, scopes.last().map_or(inherited, Vec::as_slice))?;
                if !scopes.is_empty()
                    && is_word_element(element.name().as_ref(), b"fldSimple", &prefixes)
                    && start >= from
                    && raw.get(start..start.saturating_add(source.len())) == Some(source)
                {
                    return Ok(Some(start));
                }
            }
            Event::End(_) => {
                scopes.pop();
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn update_simple_field_source<'a>(
    field: &'a Field,
    original_raw: &'a [u8],
    word_prefixes: &'a [String],
    cached_changed: bool,

    work: &mut WordOperationWork,
    word_map: Option<(
        &crate::document::WordSourceMap,
        &mut Option<crate::document::WordSourceMap>,
    )>,
) -> Result<Cow<'a, [u8]>> {
    let _word_scalar = work.template_field_scalar(field, &field.cached_result);
    let raw = original_raw;
    let locked = match field.source.as_ref() {
        FieldSource::Parsed {
            original_locked, ..
        } if field.locked != *original_locked => Some(field.locked),
        _ => None,
    };
    let nested_source;
    let raw = if let Some(runs) = &field.simple_cached_runs {
        if field
            .cached_fields_in_source_order()
            .iter()
            .any(|child| matches!(child.source.as_ref(), FieldSource::New { .. }))
        {
            return Err(OxmlError::MissingElement(
                "replacement cached field has no original simple-cache source identity".to_owned(),
            ));
        }
        let mut paragraph = CT_P::new();
        paragraph.runs = runs.clone();
        let mut replacements = Vec::new();
        let mut boundary = 0;
        while boundary < paragraph.runs.len() {
            boundary +=
                paragraph.field_source_replacements_at_in(boundary, &mut replacements, work)?;
        }
        let mut edits = Vec::new();
        let mut cursor = 0;
        for (source, replacement) in replacements {
            let start = find_cached_field_source(raw, &source, cursor, word_prefixes)?.ok_or_else(
                || OxmlError::MissingElement("nested simple-cache field source".to_owned()),
            )?;
            cursor = start + source.len();
            edits.push((start..cursor, replacement));
        }
        if edits.is_empty() {
            raw
        } else {
            let mut pieces = Vec::new();
            let mut at = 0;
            for (range, replacement) in &edits {
                pieces.push(FieldSourcePiece::Raw(&raw[at..range.start]));
                pieces.push(FieldSourcePiece::Raw(replacement));
                at = range.end;
            }
            pieces.push(FieldSourcePiece::Raw(&raw[at..]));
            let mut updated = Vec::new();
            work.field_xml_emission()
                .write_xml_pieces(&mut updated, &pieces)?;
            nested_source = updated;
            nested_source.as_slice()
        }
    } else {
        raw
    };
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let emission = work.field_xml_emission();
    let mut writer = Writer::new(FieldXmlRecord::new(&emission));
    let mut contexts = Vec::<FieldRewriteContext>::new();
    let mut wrote_result = false;
    let mut source_plan =
        (!cached_changed && std::ptr::eq(raw, original_raw)).then(|| FieldXmlSourceRecord {
            original: original_raw,
            edits: Vec::new(),
        });
    let bom = if raw.starts_with(b"\xef\xbb\xbf") {
        3
    } else {
        0
    };
    if let Some(plan) = &mut source_plan
        && bom != 0
    {
        plan.record(0..bom, Vec::new())?;
    }

    loop {
        // quick-xml consumes the initial UTF-8 BOM outside buffer_position.
        let event_start = reader.buffer_position() as usize + bom;
        match reader.read_event()? {
            Event::Start(element) => {
                let inherited = contexts
                    .last()
                    .map(|context| context.prefixes.as_slice())
                    .unwrap_or(word_prefixes);
                let prefixes = word_prefixes_at(&element, inherited)?;
                let depth = contexts.len();
                let parent_is_word_run = contexts.last().is_some_and(|context| context.word_run);
                if depth == 0 && is_word_element(element.name().as_ref(), b"fldSimple", &prefixes) {
                    let foreign_word_binding =
                        namespace_bindings(&prefixes)
                            .into_iter()
                            .any(|(prefix, namespace)| {
                                prefix == "w" && namespace != crate::namespace::W_NS
                            });
                    if foreign_word_binding {
                        // Keep the alias and original bindings on the cache owner.
                        // A canonical w declaration would change opaque descendants.
                        let mut retained = Vec::new();
                        for attribute in element.attributes() {
                            let attribute = attribute?;
                            if !is_field_word_attribute(attribute.key.as_ref(), b"dirty", &prefixes)
                                && !(locked.is_some()
                                    && is_field_word_attribute(
                                        attribute.key.as_ref(),
                                        b"fldLock",
                                        &prefixes,
                                    ))
                            {
                                retained.push(attribute);
                            }
                        }
                        let original_name = element.name();
                        let dirty_prefix = if field.dirty.is_some() {
                            let prefix = std::str::from_utf8(original_name.as_ref())?
                                .split_once(':')
                                .map(|(prefix, _)| prefix)
                                .or_else(|| {
                                    prefixes
                                        .iter()
                                        .find(|prefix| {
                                            !prefix.is_empty() && !prefix.starts_with('\0')
                                        })
                                        .map(String::as_str)
                                })
                                .ok_or_else(|| {
                                    OxmlError::InvalidValue(
                                        "field attribute requires a Word prefix".to_owned(),
                                    )
                                })?;
                            Some(prefix)
                        } else {
                            None
                        };
                        let source_name = element.name();
                        let source_name = std::str::from_utf8(source_name.as_ref())?;
                        let lock_prefix = if matches!(locked, Some(Some(_))) {
                            let mut selected =
                                source_name.split_once(':').map(|(prefix, _)| prefix);
                            if selected.is_none() && prefixes.iter().any(|prefix| prefix == "w") {
                                selected = Some("w");
                            }
                            if selected.is_none() {
                                for attribute in element.attributes() {
                                    let attribute = attribute?;
                                    if is_field_word_attribute(
                                        attribute.key.as_ref(),
                                        b"dirty",
                                        &prefixes,
                                    ) {
                                        continue;
                                    }
                                    if let Some(colon) =
                                        attribute.key.as_ref().iter().position(|byte| *byte == b':')
                                        && prefixes.iter().any(|prefix| {
                                            prefix.as_bytes() == &attribute.key.as_ref()[..colon]
                                        })
                                    {
                                        selected = Some(std::str::from_utf8(
                                            &field_attribute_source(
                                                element.as_ref(),
                                                attribute.key.as_ref(),
                                            )?[..colon],
                                        )?);
                                        break;
                                    }
                                }
                            }
                            Some(selected.ok_or_else(|| {
                                OxmlError::InvalidValue(
                                    "field attribute requires a Word prefix".into(),
                                )
                            })?)
                        } else {
                            None
                        };
                        if let Some(plan) = &mut source_plan {
                            let mut pieces = vec![
                                FieldSourcePiece::Literal(b"<"),
                                FieldSourcePiece::Raw(field_attribute_source(
                                    original_raw,
                                    element.name().as_ref(),
                                )?),
                            ];
                            for attribute in &retained {
                                pieces.extend([
                                    FieldSourcePiece::Literal(b" "),
                                    FieldSourcePiece::Raw(field_attribute_source(
                                        original_raw,
                                        attribute.key.as_ref(),
                                    )?),
                                    FieldSourcePiece::Literal(b"=\""),
                                    FieldSourcePiece::Raw(field_attribute_source(
                                        original_raw,
                                        attribute.value.as_ref(),
                                    )?),
                                    FieldSourcePiece::Literal(b"\""),
                                ]);
                            }
                            if let (Some(dirty), Some(prefix)) = (field.dirty, dirty_prefix) {
                                pieces.extend([
                                    FieldSourcePiece::Literal(b" "),
                                    FieldSourcePiece::Raw(field_source_prefix(
                                        prefix,
                                        original_raw,
                                        &element,
                                        word_prefixes,
                                    )?),
                                    FieldSourcePiece::Literal(if dirty {
                                        b":dirty=\"1\""
                                    } else {
                                        b":dirty=\"0\""
                                    }),
                                ]);
                            }
                            if let (Some(Some(locked)), Some(prefix)) = (locked, lock_prefix) {
                                pieces.extend([
                                    FieldSourcePiece::Literal(b" "),
                                    FieldSourcePiece::Raw(field_source_prefix(
                                        prefix,
                                        original_raw,
                                        &element,
                                        word_prefixes,
                                    )?),
                                    FieldSourcePiece::Literal(if locked {
                                        b":fldLock=\"1\""
                                    } else {
                                        b":fldLock=\"0\""
                                    }),
                                ]);
                            }
                            pieces.push(FieldSourcePiece::Literal(b">"));
                            plan.record(
                                event_start..reader.buffer_position() as usize + bom,
                                pieces,
                            )?;
                        } else {
                            let mut replacement = FieldXmlStart::new(source_name, Some(&emission))?;
                            for attribute in retained {
                                replacement.push_raw_attribute(attribute)?;
                            }
                            if let (Some(dirty), Some(prefix)) = (field.dirty, dirty_prefix) {
                                replacement.push_word(
                                    prefix,
                                    "dirty",
                                    if dirty { "1" } else { "0" },
                                )?;
                            }
                            if let (Some(Some(locked)), Some(prefix)) = (locked, lock_prefix) {
                                replacement.push_word(
                                    prefix,
                                    "fldLock",
                                    if locked { "1" } else { "0" },
                                )?;
                            }
                            replacement.write(&mut writer, false)?;
                        }
                    } else {
                        let pieces = rewritten_field_element_pieces(
                            &element,
                            Some("w:fldSimple"),
                            &prefixes,
                            &[("instr", field.instruction.raw.as_str())],
                            field.dirty,
                            true,
                            false,
                            locked,
                        )?;
                        if let Some(plan) = &mut source_plan {
                            let pieces = pieces
                                .into_iter()
                                .map(|piece| match piece {
                                    FieldSourcePiece::Literal(bytes) => {
                                        Ok(FieldSourcePiece::Literal(bytes))
                                    }
                                    FieldSourcePiece::Raw(bytes) => {
                                        field_attribute_source(original_raw, bytes)
                                            .map(FieldSourcePiece::Raw)
                                    }
                                    FieldSourcePiece::Escaped(_) => Ok(FieldSourcePiece::Escaped(
                                        field.instruction.raw.as_str(),
                                    )),
                                })
                                .collect::<Result<Vec<_>>>()?;
                            plan.record(
                                event_start..reader.buffer_position() as usize + bom,
                                pieces,
                            )?;
                        } else {
                            emission.write_xml_pieces(writer.get_mut(), &pieces)?;
                        }
                    }
                    contexts.push(FieldRewriteContext {
                        prefixes: prefixes.into_owned(),
                        word_run: false,
                        canonical_end: (!foreign_word_binding).then_some("w:fldSimple"),
                    });
                } else if depth == 2
                    && parent_is_word_run
                    && (is_word_element(element.name().as_ref(), b"t", &prefixes)
                        || is_word_element(element.name().as_ref(), b"delText", &prefixes))
                {
                    if cached_changed {
                        write_updated_text_element(
                            &mut reader,
                            &mut writer,
                            &element,
                            (!wrote_result).then_some(field.cached_result.as_str()),
                            !inherited.iter().any(|prefix| prefix == "w"),
                            work,
                        )?;
                        wrote_result = true;
                    } else {
                        if source_plan.is_none() {
                            writer.write_event(Event::Start(element))?;
                        }
                        contexts.push(FieldRewriteContext {
                            prefixes: prefixes.into_owned(),
                            word_run: false,
                            canonical_end: None,
                        });
                    }
                } else if cached_changed
                    && depth == 2
                    && parent_is_word_run
                    && is_result_character(element.name().as_ref(), &prefixes)
                {
                    reader.read_to_end(element.name())?;
                    if !wrote_result {
                        write_field_result_content_with_binding(
                            &mut writer,
                            &field.cached_result,
                            !inherited.iter().any(|prefix| prefix == "w"),
                            work,
                        )?;
                        wrote_result = true;
                    }
                } else {
                    let word_run =
                        depth == 1 && is_word_element(element.name().as_ref(), b"r", &prefixes);
                    if source_plan.is_none() {
                        writer.write_event(Event::Start(element))?;
                    }
                    contexts.push(FieldRewriteContext {
                        prefixes: prefixes.into_owned(),
                        word_run,
                        canonical_end: None,
                    });
                }
            }
            Event::Empty(element) => {
                let inherited = contexts
                    .last()
                    .map(|context| context.prefixes.as_slice())
                    .unwrap_or(word_prefixes);
                let prefixes = word_prefixes_at(&element, inherited)?;
                let depth = contexts.len();
                if depth == 2
                    && contexts.last().is_some_and(|context| context.word_run)
                    && (is_word_element(element.name().as_ref(), b"t", &prefixes)
                        || is_word_element(element.name().as_ref(), b"delText", &prefixes))
                {
                    if cached_changed {
                        write_updated_empty_text_element(
                            &mut writer,
                            &element,
                            (!wrote_result).then_some(field.cached_result.as_str()),
                            !inherited.iter().any(|prefix| prefix == "w"),
                            work,
                        )?;
                        wrote_result = true;
                    } else {
                        if source_plan.is_none() {
                            writer.write_event(Event::Empty(element))?;
                        }
                    }
                } else if cached_changed
                    && depth == 2
                    && contexts.last().is_some_and(|context| context.word_run)
                    && is_result_character(element.name().as_ref(), &prefixes)
                {
                    if !wrote_result {
                        write_field_result_content_with_binding(
                            &mut writer,
                            &field.cached_result,
                            !inherited.iter().any(|prefix| prefix == "w"),
                            work,
                        )?;
                        wrote_result = true;
                    }
                } else {
                    if source_plan.is_none() {
                        writer.write_event(Event::Empty(element))?;
                    }
                }
            }
            Event::End(element) => {
                let context = contexts.pop();
                if let Some(plan) = &mut source_plan {
                    let name = match context.as_ref().and_then(|context| context.canonical_end) {
                        Some(name) => FieldSourcePiece::Literal(name.as_bytes()),
                        None => FieldSourcePiece::Raw(field_attribute_source(
                            original_raw,
                            element.name().as_ref(),
                        )?),
                    };
                    plan.record(
                        event_start..reader.buffer_position() as usize + bom,
                        vec![
                            FieldSourcePiece::Literal(b"</"),
                            name,
                            FieldSourcePiece::Literal(b">"),
                        ],
                    )?;
                    continue;
                }
                let Some(context) = context else {
                    if source_plan.is_none() {
                        writer.write_event(Event::End(element))?;
                    }
                    continue;
                };
                if cached_changed && contexts.is_empty() && !wrote_result {
                    let foreign_word_namespace = namespace_bindings(&context.prefixes)
                        .into_iter()
                        .find(|(prefix, namespace)| {
                            prefix == "w" && namespace != crate::namespace::W_NS
                        })
                        .map(|(_, namespace)| namespace);
                    write_field_result_run(
                        &mut writer,
                        field,
                        foreign_word_namespace.as_deref(),
                        None,
                        None,
                        None,
                        work,
                    )?;
                    wrote_result = true;
                }
                if let Some(name) = context.canonical_end {
                    if source_plan.is_none() {
                        writer.write_event(Event::End(BytesEnd::new(name)))?;
                    }
                } else {
                    if source_plan.is_none() {
                        writer.write_event(Event::End(element))?;
                    }
                }
            }
            Event::DocType(value) if source_plan.is_some() => {
                source_plan.as_mut().expect("selected source plan").record(
                    event_start..reader.buffer_position() as usize + bom,
                    vec![
                        FieldSourcePiece::Literal(b"<!DOCTYPE "),
                        FieldSourcePiece::Raw(field_attribute_source(
                            original_raw,
                            value.as_ref(),
                        )?),
                        FieldSourcePiece::Literal(b">"),
                    ],
                )?;
            }
            Event::Eof => break,
            event => {
                if source_plan.is_none() {
                    writer.write_event(event)?;
                }
            }
        }
    }
    if let Some(plan) = source_plan {
        plan.finish(&emission, word_map)
    } else {
        if let Some((_, output)) = word_map {
            let (bytes, map) = writer.into_inner().into_word_mapped_parts();
            *output = map;
            Ok(Cow::Owned(bytes))
        } else {
            Ok(Cow::Owned(writer.into_inner().into_bytes()))
        }
    }
}

pub(super) fn update_complex_instruction_source(
    field: &Field,
    raw: &[u8],
    word_prefixes: &[String],
    work: &WordOperationWork,
) -> Result<Vec<u8>> {
    let effective = instruction_for_write(field);
    let instruction = format!(" {} ", canonical_instruction_text(&effective));
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let emission = work.field_xml_emission();
    let mut writer = Writer::new(FieldXmlRecord::new(&emission));
    let mut contexts = Vec::<FieldRewriteContext>::new();
    let mut field_depth = 0usize;
    let mut outer_result = false;
    let mut wrote_instruction = false;
    let canonical_word_binding = !word_prefixes.iter().any(|prefix| prefix == "w");

    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let inherited = contexts
                    .last()
                    .map(|context| context.prefixes.as_slice())
                    .unwrap_or(word_prefixes);
                let prefixes = word_prefixes_at(&element, inherited)?;
                let direct_run_child =
                    contexts.len() == 1 && contexts.last().is_some_and(|context| context.word_run);
                if direct_run_child
                    && is_word_element(element.name().as_ref(), b"fldChar", &prefixes)
                {
                    let kind = optional_word_attribute(&element, b"fldCharType", &prefixes);
                    update_complex_phase(kind.as_deref(), &mut field_depth, &mut outer_result);
                }
                if direct_run_child
                    && field_depth == 1
                    && !outer_result
                    && is_word_element(element.name().as_ref(), b"instrText", &prefixes)
                {
                    reader.read_to_end(element.name())?;
                    write_updated_instruction_text(
                        &mut writer,
                        &element,
                        (!wrote_instruction).then_some(instruction.as_str()),
                        canonical_word_binding,
                        work,
                    )?;
                    wrote_instruction = true;
                } else {
                    let word_run = contexts.is_empty()
                        && is_word_element(element.name().as_ref(), b"r", &prefixes);
                    writer.write_event(Event::Start(element))?;
                    contexts.push(FieldRewriteContext {
                        prefixes: prefixes.into_owned(),
                        word_run,
                        canonical_end: None,
                    });
                }
            }
            Event::Empty(element) => {
                let inherited = contexts
                    .last()
                    .map(|context| context.prefixes.as_slice())
                    .unwrap_or(word_prefixes);
                let prefixes = word_prefixes_at(&element, inherited)?;
                let direct_run_child =
                    contexts.len() == 1 && contexts.last().is_some_and(|context| context.word_run);
                if direct_run_child
                    && is_word_element(element.name().as_ref(), b"fldChar", &prefixes)
                {
                    let kind = optional_word_attribute(&element, b"fldCharType", &prefixes);
                    update_complex_phase(kind.as_deref(), &mut field_depth, &mut outer_result);
                    writer.write_event(Event::Empty(element))?;
                } else if direct_run_child
                    && field_depth == 1
                    && !outer_result
                    && is_word_element(element.name().as_ref(), b"instrText", &prefixes)
                {
                    write_updated_instruction_text(
                        &mut writer,
                        &element,
                        (!wrote_instruction).then_some(instruction.as_str()),
                        canonical_word_binding,
                        work,
                    )?;
                    wrote_instruction = true;
                } else {
                    writer.write_event(Event::Empty(element))?;
                }
            }
            Event::End(element) => {
                contexts.pop();
                writer.write_event(Event::End(element))?;
            }
            Event::Eof => break,
            event => writer.write_event(event)?,
        }
    }
    if !wrote_instruction {
        return Err(OxmlError::MissingElement(
            "nested complex field instruction text".to_owned(),
        ));
    }
    Ok(writer.into_inner().into_bytes())
}

fn write_updated_instruction_text<W: std::io::Write>(
    writer: &mut Writer<W>,
    source: &BytesStart<'_>,
    value: Option<&str>,
    canonical_word_binding: bool,
    work: &WordOperationWork,
) -> Result<()> {
    let value = value.unwrap_or_default();
    let mut retained = Vec::new();
    for attribute in source.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() != b"xml:space"
            && !(canonical_word_binding && attribute.key.as_ref() == b"xmlns:w")
        {
            retained.push(attribute);
        }
    }
    let emission = work.field_xml_emission();
    let mut element = FieldXmlStart::new("w:instrText", Some(&emission))?;
    for attribute in retained {
        element.push_raw_attribute(attribute)?;
    }
    if canonical_word_binding {
        element.push("xmlns:w", crate::namespace::W_NS)?;
    }
    if needs_space_preservation(value) {
        element.push("xml:space", "preserve")?;
    }
    element.write(writer, false)?;
    work.field_xml_emission().write_text(writer, value, None)?;
    writer.write_event(Event::End(BytesEnd::new("w:instrText")))?;
    Ok(())
}

/// Replace only the outer result, using its actual namespace and run boundaries.
/// A result sharing a control run is split into sibling runs with the original
/// control wrapper and properties retained on both preserved fragments.
pub(super) enum CachedFieldSource {
    EmptySimple(BytesStart<'static>),
    Simple(std::ops::Range<usize>),
    Complex {
        result: std::ops::Range<usize>,
        first_close: std::ops::Range<usize>,
        last_open: std::ops::Range<usize>,
    },
}

pub(super) fn cached_field_source(
    raw: &[u8],
    form: FieldForm,
    word_prefixes: &[String],
) -> Result<CachedFieldSource> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut contexts = Vec::<FieldRewriteContext>::new();
    let mut buffer = Vec::new();
    let mut physical_runs = Vec::<(usize, usize, usize, usize, usize)>::new();
    let mut current_run = None;
    let mut separate = None;
    let mut end = None;
    let mut depth = 0usize;
    let mut simple_open = None;
    let mut simple_close = None;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer)?;
        let event_end = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let empty = raw.get(event_end.saturating_sub(2)..event_end) == Some(b"/>");
                let inherited = contexts
                    .last()
                    .map(|context| context.prefixes.as_slice())
                    .unwrap_or(word_prefixes);
                let prefixes = word_prefixes_at(&element, inherited)?;
                let word_run = contexts.is_empty()
                    && is_word_element(element.name().as_ref(), b"r", &prefixes);
                if word_run {
                    current_run = Some(physical_runs.len());
                    physical_runs.push((start, event_end, event_end, event_end, event_end));
                }
                if contexts.is_empty()
                    && is_word_element(element.name().as_ref(), b"fldSimple", &prefixes)
                {
                    if empty {
                        return Ok(CachedFieldSource::EmptySimple(element.into_owned()));
                    }
                    simple_open = Some(event_end);
                }
                if contexts.len() == 1
                    && contexts.last().is_some_and(|context| context.word_run)
                    && is_word_element(element.name().as_ref(), b"fldChar", &prefixes)
                {
                    let kind = optional_word_attribute(&element, b"fldCharType", &prefixes);
                    let marker_end = if empty {
                        event_end
                    } else {
                        reader.read_to_end_into(element.name(), &mut Vec::new())?;
                        reader.buffer_position() as usize
                    };
                    match kind.as_deref() {
                        Some("begin") => depth += 1,
                        Some("separate") if depth == 1 => {
                            separate = current_run.map(|run| (run, marker_end))
                        }
                        Some("end") => {
                            if depth == 1 {
                                end = current_run.map(|run| (run, start));
                            }
                            depth = depth.saturating_sub(1);
                        }
                        _ => {}
                    }
                    buffer.clear();
                    continue;
                }
                if empty {
                    if contexts.len() == 1
                        && contexts.last().is_some_and(|context| context.word_run)
                        && is_word_element(element.name().as_ref(), b"rPr", &prefixes)
                        && let Some(run) = current_run
                    {
                        physical_runs[run].2 = event_end;
                    }
                } else {
                    contexts.push(FieldRewriteContext {
                        prefixes: prefixes.into_owned(),
                        word_run,
                        canonical_end: None,
                    });
                }
            }
            Event::End(element) => {
                let context = contexts.pop().ok_or_else(|| {
                    OxmlError::InvalidValue("unbalanced typed field source".into())
                })?;
                if contexts.len() == 1
                    && contexts.last().is_some_and(|context| context.word_run)
                    && is_word_element(element.name().as_ref(), b"rPr", &context.prefixes)
                    && let Some(run) = current_run
                {
                    physical_runs[run].2 = event_end;
                }
                if context.word_run {
                    let run = current_run.take().ok_or_else(|| {
                        OxmlError::MissingElement("typed cache physical run".into())
                    })?;
                    physical_runs[run].3 = start;
                    physical_runs[run].4 = event_end;
                }
                if contexts.is_empty()
                    && is_word_element(element.name().as_ref(), b"fldSimple", &context.prefixes)
                {
                    simple_close = Some(start);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    match form {
        FieldForm::Simple => {
            let (start, end) = simple_open
                .zip(simple_close)
                .ok_or_else(|| OxmlError::MissingElement("simple typed cache owner".into()))?;
            Ok(CachedFieldSource::Simple(start..end))
        }
        FieldForm::Complex => {
            let (first, start) = separate
                .ok_or_else(|| OxmlError::MissingElement("complex typed cache separator".into()))?;
            let (last, end) =
                end.ok_or_else(|| OxmlError::MissingElement("complex typed cache end".into()))?;
            if start > end || first > last {
                return Err(OxmlError::InvalidValue("inverted typed cache span".into()));
            }
            let first = physical_runs[first];
            let last = physical_runs[last];
            Ok(CachedFieldSource::Complex {
                result: start..end,
                first_close: first.3..first.4,
                last_open: last.0..last.2,
            })
        }
    }
}

pub(super) fn validate_cached_comment_ranges(
    runs: &[CT_R],
    ranges: &[CommentRangeMarker],
) -> Result<()> {
    let mut references = std::collections::BTreeMap::<i32, usize>::new();
    for (index, run) in runs.iter().enumerate() {
        for content in &run.content {
            if let RunContent::CommentReference { id, .. } = content
                && references.insert(*id, index).is_some()
            {
                return Err(OxmlError::InvalidValue(
                    "cached annotation reference IDs must be unique".into(),
                ));
            }
        }
    }
    let mut open = std::collections::BTreeMap::<i32, usize>::new();
    let mut closed = std::collections::BTreeSet::new();
    let mut last_position = 0;
    for marker in ranges {
        let (id, position, raw_before, has_child_content, start) = match marker {
            CommentRangeMarker::Start {
                id,
                run_index,
                raw_before,
                has_child_content,
            } => (*id, *run_index, *raw_before, *has_child_content, true),
            CommentRangeMarker::End {
                id,
                run_index,
                raw_before,
                has_child_content,
            } => (*id, *run_index, *raw_before, *has_child_content, false),
        };
        if position > runs.len()
            || position < last_position
            || raw_before != 0
            || has_child_content
            || !references.contains_key(&id)
        {
            return Err(OxmlError::InvalidValue(
                "cached annotation marker has no checked run boundary or reference".into(),
            ));
        }
        last_position = position;
        if start {
            if closed.contains(&id) || open.insert(id, position).is_some() {
                return Err(OxmlError::InvalidValue(
                    "cached annotation range has duplicate start".into(),
                ));
            }
        } else {
            if open.remove(&id).is_none() || !closed.insert(id) || position > references[&id] {
                return Err(OxmlError::InvalidValue(
                    "cached annotation range has no paired reference boundary".into(),
                ));
            }
        }
    }
    if !open.is_empty() || closed.len() != references.len() {
        return Err(OxmlError::InvalidValue(
            "cached annotation ranges and typed references disagree".into(),
        ));
    }
    Ok(())
}

pub(super) fn write_typed_cached_runs<W: std::io::Write>(
    writer: &mut Writer<W>,
    runs: &[CT_R],
    ranges: &[CommentRangeMarker],
    foreign_word_namespace: Option<&str>,
    publication_scope: Option<&[(&str, &str)]>,
    source_context: Option<&[(&str, &str)]>,

    work: &mut WordOperationWork,
) -> Result<()> {
    let emission = work.field_xml_emission();
    for index in 0..=runs.len() {
        for marker in ranges {
            let (id, position, name) = match marker {
                CommentRangeMarker::Start { id, run_index, .. } => {
                    (*id, *run_index, "w:commentRangeStart")
                }
                CommentRangeMarker::End { id, run_index, .. } => {
                    (*id, *run_index, "w:commentRangeEnd")
                }
            };
            if position != index {
                continue;
            }
            let mut element = FieldXmlStart::new(name, Some(&emission))?;
            let mut decimal = itoa::Buffer::new();
            element.push("w:id", decimal.format(id))?;
            if foreign_word_namespace.is_some() {
                element.push("xmlns:w", crate::namespace::W_NS)?;
            }
            element.write(writer, true)?;
        }
        if let Some(run) = runs.get(index) {
            write_mixed_field_run(
                writer,
                run,
                foreign_word_namespace,
                publication_scope,
                source_context,
                work,
                Some(&emission),
            )?;
        }
    }
    Ok(())
}

pub(super) fn replace_typed_field_cache(
    raw: &[u8],
    form: FieldForm,
    word_prefixes: &[String],
    runs: &[CT_R],
    ranges: &[CommentRangeMarker],
    publication_scope: Option<&[(&str, &str)]>,

    work: &mut WordOperationWork,
) -> Result<Vec<u8>> {
    let emission = work.field_xml_emission();
    let mut writer = Writer::new(FieldXmlRecord::new(&emission));
    let source_scope = field_scope_bindings(word_prefixes);
    write_typed_cached_runs(
        &mut writer,
        runs,
        ranges,
        Some("typed-cache"),
        publication_scope.map(|_| source_scope.as_slice()),
        Some(&source_scope),
        work,
    )?;
    let payload = writer.into_inner().into_bytes();
    let mut output = Vec::new();
    match cached_field_source(raw, form, word_prefixes)? {
        CachedFieldSource::EmptySimple(element) => {
            let name = element.name();
            let pieces = [
                FieldSourcePiece::Literal(b"<"),
                FieldSourcePiece::Raw(element.as_ref()),
                FieldSourcePiece::Literal(b">"),
                FieldSourcePiece::Raw(&payload),
                FieldSourcePiece::Literal(b"</"),
                FieldSourcePiece::Raw(name.as_ref()),
                FieldSourcePiece::Literal(b">"),
            ];
            emission.write_xml_pieces(&mut output, &pieces)?;
        }
        CachedFieldSource::Simple(result) => emission.write_xml_pieces(
            &mut output,
            &[
                FieldSourcePiece::Raw(&raw[..result.start]),
                FieldSourcePiece::Raw(&payload),
                FieldSourcePiece::Raw(&raw[result.end..]),
            ],
        )?,
        CachedFieldSource::Complex {
            result,
            first_close,
            last_open,
            ..
        } => emission.write_xml_pieces(
            &mut output,
            &[
                FieldSourcePiece::Raw(&raw[..result.start]),
                FieldSourcePiece::Raw(&raw[first_close]),
                FieldSourcePiece::Raw(&payload),
                FieldSourcePiece::Raw(&raw[last_open]),
                FieldSourcePiece::Raw(&raw[result.end..]),
            ],
        )?,
    }
    Ok(output)
}

pub(super) fn cached_runs_need_typed_projection(runs: &[CT_R]) -> bool {
    runs.iter().any(|run| {
        run.properties.is_some()
            || run.content.iter().any(|content| match content {
                RunContent::Tab
                | RunContent::Break(_)
                | RunContent::Symbol { .. }
                | RunContent::Drawing(_)
                | RunContent::DeletedText(_)
                | RunContent::FootnoteRef { .. }
                | RunContent::EndnoteRef { .. }
                | RunContent::CommentReference { .. }
                | RunContent::Field(_) => true,
                RunContent::SpecialCharacter(character) => {
                    !matches!(character, SpecialCharacter::SoftHyphen)
                }
                _ => false,
            })
    })
}

pub(super) fn source_has_note_reference_name(raw: &[u8]) -> bool {
    [
        b"footnoteReference".as_slice(),
        b"endnoteReference".as_slice(),
        b"commentReference".as_slice(),
    ]
    .iter()
    .any(|name| raw.windows(name.len()).any(|window| window == *name))
}

/// Put a new cache run before the source's still-open, pure end-marker run.
/// Its formatting and namespace scope follow that run, while owner attributes
/// remain on the original run. The original marker bytes are written normally.
fn insert_legacy_cache_before_end_run(
    writer: &mut Writer<FieldXmlRecord>,
    source_run_start: &BytesStart<'_>,
    (run_start, content_start): (usize, usize),
    display: &str,
    canonical_word_binding: bool,
    work: &WordOperationWork,
) -> Result<()> {
    let properties = writer
        .get_ref()
        .as_bytes()
        .get(content_start..)
        .ok_or_else(|| OxmlError::InvalidValue("legacy end run output boundary".into()))?;
    let mut retained = Vec::new();
    for attribute in source_run_start.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
            retained.push(attribute);
        }
    }
    let source_name = source_run_start.name();
    let source_name = std::str::from_utf8(source_name.as_ref())?;
    let emission = work.field_xml_emission();
    let mut start = FieldXmlStart::new(source_name, Some(&emission))?;
    for attribute in retained {
        start.push_raw_attribute(attribute)?;
    }
    let mut cache = Writer::new(FieldXmlRecord::new(&emission));
    start.write(&mut cache, false)?;
    std::io::Write::write_all(cache.get_mut(), properties)?;
    write_field_result_content_with_binding(&mut cache, display, canonical_word_binding, work)?;
    cache.write_event(Event::End(BytesEnd::new(source_name)))?;
    let cache = cache.into_inner().into_bytes();
    writer.get_mut().splice(run_start..run_start, &cache)?;
    Ok(())
}

pub(super) fn update_complex_field_source(
    field: &Field,
    raw: &[u8],
    word_prefixes: &[String],
    cached_changed: bool,
    publish_missing_result: bool,
    work: &WordOperationWork,
    word_map: Option<(
        &crate::document::WordSourceMap,
        &mut Option<crate::document::WordSourceMap>,
    )>,
) -> Result<Vec<u8>> {
    let _word_scalar = work.template_field_scalar(field, &field.cached_result);
    let keep_end_run_pure = cached_changed
        && field
            .legacy_form
            .as_ref()
            .and_then(|form| form.name.as_deref())
            .map(|name| source_contains_own_bookmark(raw, word_prefixes, name))
            .transpose()?
            .unwrap_or(false);
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let emission = work.field_xml_emission();
    let mut writer = Writer::new(FieldXmlRecord::new(&emission));
    let mut contexts = Vec::<FieldRewriteContext>::new();
    let mut source_run_start: Option<BytesStart<'_>> = None;
    let mut source_run_output = None;
    let mut field_depth = 0usize;
    let mut outer_result = false;
    let mut wrote_result = false;

    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let inherited = contexts
                    .last()
                    .map(|context| context.prefixes.as_slice())
                    .unwrap_or(word_prefixes);
                let prefixes = word_prefixes_at(&element, inherited)?;
                let canonical_word_binding = !inherited.iter().any(|prefix| prefix == "w");
                let depth = contexts.len();
                let direct_run_child =
                    depth == 1 && contexts.last().is_some_and(|context| context.word_run);
                if direct_run_child
                    && is_word_element(element.name().as_ref(), b"fldChar", &prefixes)
                {
                    let kind = optional_word_attribute(&element, b"fldCharType", &prefixes);
                    let depth_before = field_depth;
                    let outer_marker = matches!(kind.as_deref(), Some("begin")) && field_depth == 0
                        || matches!(kind.as_deref(), Some("separate" | "end")) && field_depth == 1;
                    if outer_marker
                        && kind.as_deref() == Some("end")
                        && !outer_result
                        && publish_missing_result
                        && !wrote_result
                        && depth_before == 1
                    {
                        write_missing_legacy_result_region(
                            &mut writer,
                            source_run_start.as_ref().ok_or_else(|| {
                                OxmlError::MissingElement("legacy field source run".into())
                            })?,
                            &field.cached_result,
                            !word_prefixes.iter().any(|prefix| prefix == "w"),
                            work,
                        )?;
                        wrote_result = true;
                    }
                    if outer_marker
                        && kind.as_deref() == Some("end")
                        && outer_result
                        && !wrote_result
                        && cached_changed
                    {
                        if keep_end_run_pure {
                            insert_legacy_cache_before_end_run(
                                &mut writer,
                                source_run_start.as_ref().ok_or_else(|| {
                                    OxmlError::MissingElement("legacy field end run".into())
                                })?,
                                source_run_output.ok_or_else(|| {
                                    OxmlError::MissingElement("legacy field end boundary".into())
                                })?,
                                &field.cached_result,
                                canonical_word_binding,
                                work,
                            )?;
                        } else {
                            write_field_result_content_with_binding(
                                &mut writer,
                                &field.cached_result,
                                canonical_word_binding,
                                work,
                            )?;
                        }
                        wrote_result = true;
                    }
                    if cached_changed
                        && outer_result
                        && depth_before == 1
                        && kind.as_deref() == Some("begin")
                        && !wrote_result
                    {
                        write_field_result_content_with_binding(
                            &mut writer,
                            &field.cached_result,
                            canonical_word_binding,
                            work,
                        )?;
                        wrote_result = true;
                    }
                    update_complex_phase(kind.as_deref(), &mut field_depth, &mut outer_result);
                    if outer_marker {
                        let kind = kind.as_deref().unwrap_or_default();
                        write_rewritten_field_element(
                            &mut writer,
                            &element,
                            None,
                            &prefixes,
                            &[("fldCharType", kind)],
                            (kind == "begin").then_some(field.dirty).flatten(),
                            canonical_word_binding,
                            false,
                            &work.field_xml_emission(),
                        )?;
                        contexts.push(FieldRewriteContext {
                            prefixes: prefixes.into_owned(),
                            word_run: false,
                            canonical_end: None,
                        });
                        continue;
                    }
                    if cached_changed
                        && wrote_result
                        && outer_result
                        && (depth_before > 1
                            || depth_before == 1 && kind.as_deref() == Some("begin"))
                    {
                        reader.read_to_end(element.name())?;
                        continue;
                    }
                }
                if direct_run_child
                    && outer_result
                    && field_depth == 1
                    && is_word_element(element.name().as_ref(), b"t", &prefixes)
                {
                    if cached_changed {
                        reader.read_to_end(element.name())?;
                        if !wrote_result {
                            write_field_result_content_with_binding(
                                &mut writer,
                                &field.cached_result,
                                canonical_word_binding,
                                work,
                            )?;
                            wrote_result = true;
                        }
                    } else {
                        writer.write_event(Event::Start(element))?;
                        contexts.push(FieldRewriteContext {
                            prefixes: prefixes.into_owned(),
                            word_run: false,
                            canonical_end: None,
                        });
                    }
                } else if direct_run_child
                    && cached_changed
                    && ((outer_result
                        && field_depth == 1
                        && is_result_character(element.name().as_ref(), &prefixes))
                        || outer_result
                            && field_depth > 1
                            && (is_word_element(element.name().as_ref(), b"instrText", &prefixes)
                                || is_word_element(element.name().as_ref(), b"t", &prefixes)
                                || is_result_character(element.name().as_ref(), &prefixes)))
                {
                    reader.read_to_end(element.name())?;
                    if outer_result && field_depth == 1 && !wrote_result {
                        write_field_result_content_with_binding(
                            &mut writer,
                            &field.cached_result,
                            canonical_word_binding,
                            work,
                        )?;
                        wrote_result = true;
                    }
                } else {
                    let word_run =
                        depth == 0 && is_word_element(element.name().as_ref(), b"r", &prefixes);
                    let run_start = writer.get_ref().len();
                    if (publish_missing_result || keep_end_run_pure) && word_run {
                        source_run_start = Some(element.clone());
                    }
                    writer.write_event(Event::Start(element))?;
                    if keep_end_run_pure && word_run {
                        source_run_output = Some((run_start, writer.get_ref().len()));
                    }
                    contexts.push(FieldRewriteContext {
                        prefixes: prefixes.into_owned(),
                        word_run,
                        canonical_end: None,
                    });
                }
            }
            Event::Empty(element) => {
                let inherited = contexts
                    .last()
                    .map(|context| context.prefixes.as_slice())
                    .unwrap_or(word_prefixes);
                let prefixes = word_prefixes_at(&element, inherited)?;
                let canonical_word_binding = !inherited.iter().any(|prefix| prefix == "w");
                let direct_run_child =
                    contexts.len() == 1 && contexts.last().is_some_and(|context| context.word_run);
                if direct_run_child
                    && is_word_element(element.name().as_ref(), b"fldChar", &prefixes)
                {
                    let kind = optional_word_attribute(&element, b"fldCharType", &prefixes);
                    let depth_before = field_depth;
                    let outer_marker = matches!(kind.as_deref(), Some("begin")) && field_depth == 0
                        || matches!(kind.as_deref(), Some("separate" | "end")) && field_depth == 1;
                    if outer_marker
                        && kind.as_deref() == Some("end")
                        && !outer_result
                        && publish_missing_result
                        && !wrote_result
                        && depth_before == 1
                    {
                        write_missing_legacy_result_region(
                            &mut writer,
                            source_run_start.as_ref().ok_or_else(|| {
                                OxmlError::MissingElement("legacy field source run".into())
                            })?,
                            &field.cached_result,
                            !word_prefixes.iter().any(|prefix| prefix == "w"),
                            work,
                        )?;
                        wrote_result = true;
                    }
                    if outer_marker
                        && kind.as_deref() == Some("end")
                        && outer_result
                        && !wrote_result
                        && cached_changed
                    {
                        if keep_end_run_pure {
                            insert_legacy_cache_before_end_run(
                                &mut writer,
                                source_run_start.as_ref().ok_or_else(|| {
                                    OxmlError::MissingElement("legacy field end run".into())
                                })?,
                                source_run_output.ok_or_else(|| {
                                    OxmlError::MissingElement("legacy field end boundary".into())
                                })?,
                                &field.cached_result,
                                canonical_word_binding,
                                work,
                            )?;
                        } else {
                            write_field_result_content_with_binding(
                                &mut writer,
                                &field.cached_result,
                                canonical_word_binding,
                                work,
                            )?;
                        }
                        wrote_result = true;
                    }
                    if cached_changed
                        && outer_result
                        && depth_before == 1
                        && kind.as_deref() == Some("begin")
                        && !wrote_result
                    {
                        write_field_result_content_with_binding(
                            &mut writer,
                            &field.cached_result,
                            canonical_word_binding,
                            work,
                        )?;
                        wrote_result = true;
                    }
                    update_complex_phase(kind.as_deref(), &mut field_depth, &mut outer_result);
                    if outer_marker {
                        let kind = kind.as_deref().unwrap_or_default();
                        write_rewritten_field_element(
                            &mut writer,
                            &element,
                            None,
                            &prefixes,
                            &[("fldCharType", kind)],
                            (kind == "begin").then_some(field.dirty).flatten(),
                            canonical_word_binding,
                            true,
                            &work.field_xml_emission(),
                        )?;
                        continue;
                    }
                    if cached_changed
                        && wrote_result
                        && outer_result
                        && (depth_before > 1
                            || depth_before == 1 && kind.as_deref() == Some("begin"))
                    {
                        continue;
                    }
                }
                if direct_run_child
                    && outer_result
                    && field_depth == 1
                    && is_word_element(element.name().as_ref(), b"t", &prefixes)
                {
                    if cached_changed {
                        if !wrote_result {
                            write_field_result_content_with_binding(
                                &mut writer,
                                &field.cached_result,
                                canonical_word_binding,
                                work,
                            )?;
                            wrote_result = true;
                        }
                    } else {
                        writer.write_event(Event::Empty(element))?;
                    }
                } else if direct_run_child
                    && cached_changed
                    && ((outer_result
                        && field_depth == 1
                        && is_result_character(element.name().as_ref(), &prefixes))
                        || outer_result
                            && field_depth > 1
                            && (is_word_element(element.name().as_ref(), b"instrText", &prefixes)
                                || is_word_element(element.name().as_ref(), b"t", &prefixes)
                                || is_result_character(element.name().as_ref(), &prefixes)))
                {
                    if outer_result && field_depth == 1 && !wrote_result {
                        write_field_result_content_with_binding(
                            &mut writer,
                            &field.cached_result,
                            canonical_word_binding,
                            work,
                        )?;
                        wrote_result = true;
                    }
                } else {
                    writer.write_event(Event::Empty(element))?;
                }
            }
            Event::End(element) => {
                if let Some(name) = contexts.pop().and_then(|context| context.canonical_end) {
                    writer.write_event(Event::End(BytesEnd::new(name)))?;
                } else {
                    writer.write_event(Event::End(element))?;
                }
                if contexts.is_empty() {
                    source_run_start = None;
                }
            }
            Event::Eof => break,
            event => writer.write_event(event)?,
        }
    }
    if let Some((_, output)) = word_map {
        let (bytes, map) = writer.into_inner().into_word_mapped_parts();
        *output = map;
        Ok(bytes)
    } else {
        Ok(writer.into_inner().into_bytes())
    }
}

fn update_complex_phase(kind: Option<&str>, field_depth: &mut usize, outer_result: &mut bool) {
    match kind {
        Some("begin") => *field_depth += 1,
        Some("separate") if *field_depth == 1 => *outer_result = true,
        Some("end") if *field_depth == 1 => {
            *outer_result = false;
            *field_depth = 0;
        }
        Some("end") if *field_depth > 1 => *field_depth -= 1,
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewritten_field_element_pieces<'a>(
    source: &'a BytesStart<'_>,
    name: Option<&'static str>,
    word_prefixes: &'a [String],
    replacements: &[(&str, &'a str)],
    dirty: Option<bool>,
    canonical_word_binding: bool,
    empty: bool,
    locked: Option<Option<bool>>,
) -> Result<Vec<FieldSourcePiece<'a>>> {
    let canonical_word_binding = name.is_some() && canonical_word_binding;
    let attribute_prefix = if name.is_some() {
        "w"
    } else {
        word_prefixes
            .iter()
            .find(|prefix| !prefix.is_empty() && !prefix.starts_with('\0'))
            .map(String::as_str)
            .ok_or_else(|| {
                OxmlError::InvalidValue("field attribute requires a Word prefix".into())
            })?
    };
    let mut retained = Vec::new();
    for attribute in source.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if canonical_word_binding && key == b"xmlns:w" {
            continue;
        }
        if replacements
            .iter()
            .any(|(local, _)| is_field_word_attribute(key, local.as_bytes(), word_prefixes))
            || is_field_word_attribute(key, b"dirty", word_prefixes)
            || locked.is_some() && is_field_word_attribute(key, b"fldLock", word_prefixes)
        {
            continue;
        }
        retained.push(attribute);
    }
    let replacements = replacements
        .iter()
        .map(|(local, value)| {
            let local = match *local {
                "instr" => "instr",
                "fldCharType" => "fldCharType",
                _ => {
                    return Err(OxmlError::MissingElement(format!(
                        "field attribute {local}"
                    )));
                }
            };
            Ok((local, *value))
        })
        .collect::<Result<Vec<_>>>()?;
    let chosen_name = match name {
        Some(name) => name,
        None => std::str::from_utf8(&source.as_ref()[..source.name().as_ref().len()])?,
    };
    let mut pieces = vec![
        FieldSourcePiece::Literal(b"<"),
        match name {
            Some(name) => FieldSourcePiece::Literal(name.as_bytes()),
            None => FieldSourcePiece::Raw(chosen_name.as_bytes()),
        },
    ];
    for attribute in retained {
        pieces.extend([
            FieldSourcePiece::Literal(b" "),
            FieldSourcePiece::Raw(field_attribute_source(
                source.as_ref(),
                attribute.key.as_ref(),
            )?),
            FieldSourcePiece::Literal(b"=\""),
            FieldSourcePiece::Raw(field_attribute_source(
                source.as_ref(),
                attribute.value.as_ref(),
            )?),
            FieldSourcePiece::Literal(b"\""),
        ]);
    }
    if canonical_word_binding {
        pieces.push(FieldSourcePiece::Literal(
            concat!(
                " xmlns:w=\"",
                "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
                "\""
            )
            .as_bytes(),
        ));
    }
    for (local, value) in replacements {
        pieces.extend([
            FieldSourcePiece::Literal(b" "),
            if name.is_some() {
                FieldSourcePiece::Literal(b"w")
            } else {
                FieldSourcePiece::Raw(attribute_prefix.as_bytes())
            },
            FieldSourcePiece::Literal(b":"),
            FieldSourcePiece::Literal(local.as_bytes()),
            FieldSourcePiece::Literal(b"=\""),
            FieldSourcePiece::Escaped(value),
            FieldSourcePiece::Literal(b"\""),
        ]);
    }
    if let Some(dirty) = dirty {
        pieces.extend([
            FieldSourcePiece::Literal(b" "),
            if name.is_some() {
                FieldSourcePiece::Literal(b"w")
            } else {
                FieldSourcePiece::Raw(attribute_prefix.as_bytes())
            },
            FieldSourcePiece::Literal(if dirty {
                b":dirty=\"1\""
            } else {
                b":dirty=\"0\""
            }),
        ]);
    }
    if let Some(Some(locked)) = locked {
        pieces.extend([
            FieldSourcePiece::Literal(b" "),
            if name.is_some() {
                FieldSourcePiece::Literal(b"w")
            } else {
                FieldSourcePiece::Raw(attribute_prefix.as_bytes())
            },
            FieldSourcePiece::Literal(if locked {
                b":fldLock=\"1\""
            } else {
                b":fldLock=\"0\""
            }),
        ]);
    }
    pieces.push(FieldSourcePiece::Literal(if empty { b"/>" } else { b">" }));
    Ok(pieces)
}

#[allow(clippy::too_many_arguments)]
fn write_rewritten_field_element<W: std::io::Write>(
    writer: &mut Writer<W>,
    source: &BytesStart<'_>,
    name: Option<&'static str>,
    word_prefixes: &[String],
    replacements: &[(&str, &str)],
    dirty: Option<bool>,
    canonical_word_binding: bool,
    empty: bool,
    emission: &FieldXmlEmission,
) -> Result<()> {
    let pieces = rewritten_field_element_pieces(
        source,
        name,
        word_prefixes,
        replacements,
        dirty,
        canonical_word_binding,
        empty,
        None,
    )?;
    emission.write_xml_pieces(writer.get_mut(), &pieces)
}

pub(super) fn is_field_word_attribute(key: &[u8], local: &[u8], word_prefixes: &[String]) -> bool {
    let Some(separator) = key.iter().position(|byte| *byte == b':') else {
        return false;
    };
    key.get(separator + 1..) == Some(local)
        && word_prefixes
            .iter()
            .any(|prefix| prefix.as_bytes() == &key[..separator])
}

fn write_updated_text_element<W: std::io::Write>(
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<W>,
    source: &BytesStart<'_>,
    value: Option<&str>,
    canonical_word_binding: bool,
    work: &WordOperationWork,
) -> Result<()> {
    reader.read_to_end_into(source.name(), &mut Vec::new())?;
    if let Some(value) = value {
        write_field_result_content_inner(
            writer,
            value,
            Some(source),
            canonical_word_binding,
            work,
        )?;
    } else {
        write_field_text_with_source(writer, "", Some(source), canonical_word_binding, work)?;
    }
    Ok(())
}

fn write_updated_empty_text_element<W: std::io::Write>(
    writer: &mut Writer<W>,
    source: &BytesStart<'_>,
    value: Option<&str>,
    canonical_word_binding: bool,
    work: &WordOperationWork,
) -> Result<()> {
    if let Some(value) = value {
        write_field_result_content_inner(
            writer,
            value,
            Some(source),
            canonical_word_binding,
            work,
        )?;
    } else {
        write_field_text_with_source(writer, "", Some(source), canonical_word_binding, work)?;
    }
    Ok(())
}

struct FieldLockOwner<'a> {
    start: usize,
    end: usize,
    element: BytesStart<'a>,
    prefixes: Vec<String>,
    empty: bool,
}

/// One actual surgical source edit. Escaping is sized before payload emission.
/// Original source slices and generated punctuation remain separate pieces.
pub(super) enum FieldSourcePiece<'a> {
    Raw(&'a [u8]),
    Escaped(&'a str),
    Literal(&'static [u8]),
}

impl<'a> FieldSourceEdit<'a> {
    pub(super) fn new(
        source_bytes: usize,
        range: std::ops::Range<usize>,
        replacement: Vec<FieldSourcePiece<'a>>,
    ) -> Result<Self> {
        if range.start > range.end || range.end > source_bytes {
            return Err(OxmlError::InvalidValue(
                "field source edit range is invalid".into(),
            ));
        }
        let overflow = || OxmlError::InvalidValue("field source byte overflow".into());
        let mut replacement_bytes = 0usize;
        for piece in &replacement {
            let bytes = match piece {
                FieldSourcePiece::Raw(bytes) | FieldSourcePiece::Literal(bytes) => bytes.len(),
                FieldSourcePiece::Escaped(value) => escaped_source_len(value)?,
            };
            replacement_bytes = replacement_bytes.checked_add(bytes).ok_or_else(overflow)?;
        }
        let projected = source_bytes
            .checked_sub(range.len())
            .and_then(|bytes| bytes.checked_add(replacement_bytes))
            .ok_or_else(overflow)?;
        Ok(Self {
            source_bytes,
            range,
            replacement,
            replacement_bytes,
            projected,
        })
    }

    fn replacement_bytes(&self, output: &mut Vec<u8>) {
        for piece in &self.replacement {
            match piece {
                FieldSourcePiece::Raw(bytes) | FieldSourcePiece::Literal(bytes) => {
                    output.extend_from_slice(bytes)
                }
                FieldSourcePiece::Escaped(value) => append_escaped_source(output, value),
            }
        }
    }

    pub(super) fn emit(self, raw: &[u8]) -> Result<Vec<u8>> {
        if raw.len() != self.source_bytes {
            return Err(OxmlError::InvalidValue(
                "field source edit input changed".into(),
            ));
        }
        let mut output = Vec::with_capacity(self.projected);
        output.extend_from_slice(&raw[..self.range.start]);
        self.replacement_bytes(&mut output);
        output.extend_from_slice(&raw[self.range.end..]);
        debug_assert_eq!(output.len(), self.projected);
        Ok(output)
    }

    pub(super) fn apply(self, raw: &mut Vec<u8>, work: &WordOperationWork) -> Result<()> {
        if raw.len() != self.source_bytes {
            return Err(OxmlError::InvalidValue(
                "field source edit input changed".into(),
            ));
        }
        work.reserve_inner_xml(self.projected)?;
        let mut replacement = Vec::with_capacity(self.replacement_bytes);
        self.replacement_bytes(&mut replacement);
        raw.splice(self.range, replacement);
        debug_assert_eq!(raw.len(), self.projected);
        Ok(())
    }
}

/// The selected Attribute is borrowed by the actual reader over this raw input.
/// Convert its view to a source slice without retaining a local Attribute borrow.
pub(super) fn field_attribute_source<'a>(raw: &'a [u8], value: &[u8]) -> Result<&'a [u8]> {
    if value.is_empty() {
        return Ok(&raw[..0]);
    }
    let start = (value.as_ptr() as usize)
        .checked_sub(raw.as_ptr() as usize)
        .ok_or_else(|| {
            OxmlError::InvalidValue("field source attribute bounds are invalid".into())
        })?;
    let end = start.checked_add(value.len()).ok_or_else(|| {
        OxmlError::InvalidValue("field source attribute bounds are invalid".into())
    })?;
    raw.get(start..end)
        .ok_or_else(|| OxmlError::InvalidValue("field source attribute bounds are invalid".into()))
}

fn field_lock_owner<'a>(
    raw: &'a [u8],
    form: FieldForm,
    word_prefixes: &[String],
) -> Result<Option<FieldLockOwner<'a>>> {
    let mut reader = Reader::from_reader(raw);
    let mut contexts = Vec::<(Vec<String>, bool)>::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event()?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let prefixes = word_prefixes_at(
                    element,
                    contexts
                        .last()
                        .map(|context| context.0.as_slice())
                        .unwrap_or(word_prefixes),
                )?;
                let owner = match form {
                    FieldForm::Simple => {
                        contexts.is_empty()
                            && is_word_element(element.name().as_ref(), b"fldSimple", &prefixes)
                    }
                    FieldForm::Complex => {
                        contexts.len() == 1
                            && contexts[0].1
                            && is_word_element(element.name().as_ref(), b"fldChar", &prefixes)
                            && optional_word_attribute(element, b"fldCharType", &prefixes)
                                .as_deref()
                                == Some("begin")
                    }
                };
                if owner {
                    return Ok(Some(FieldLockOwner {
                        start,
                        end: reader.buffer_position() as usize,
                        element: element.clone(),
                        prefixes: prefixes.into_owned(),
                        empty: matches!(event, Event::Empty(_)),
                    }));
                }
                if matches!(event, Event::Start(_)) {
                    let word_run = contexts.is_empty()
                        && is_word_element(element.name().as_ref(), b"r", &prefixes);
                    contexts.push((prefixes.into_owned(), word_run));
                }
            }
            Event::End(_) => {
                contexts.pop();
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
    }
}

pub(super) fn field_source_lock(
    raw: &[u8],
    form: FieldForm,
    word_prefixes: &[String],
) -> Result<Option<bool>> {
    Ok(
        field_lock_owner(raw, form, word_prefixes)?.and_then(|owner| {
            optional_word_attribute(&owner.element, b"fldLock", &owner.prefixes)
                .and_then(|value| parse_field_bool(&value))
        }),
    )
}

fn plan_field_lock<'a>(
    raw: &'a [u8],
    form: FieldForm,
    word_prefixes: &[String],
    locked: Option<bool>,
) -> Result<FieldSourceEdit<'a>> {
    let owner = field_lock_owner(raw, form, word_prefixes)?
        .ok_or_else(|| OxmlError::MissingElement("field lock owner".into()))?;
    let owner_name =
        std::str::from_utf8(field_attribute_source(raw, owner.element.name().as_ref())?)?;
    let mut attribute_prefix = owner_name
        .split_once(':')
        .map(|(prefix, _)| prefix.as_bytes());
    if attribute_prefix.is_none() && owner.prefixes.iter().any(|prefix| prefix == "w") {
        attribute_prefix = Some(b"w");
    }
    let mut replacement = vec![
        FieldSourcePiece::Literal(b"<"),
        FieldSourcePiece::Raw(owner_name.as_bytes()),
    ];
    for attribute in owner.element.attributes() {
        let attribute = attribute?;
        if attribute_prefix.is_none()
            && let Some(colon) = attribute.key.as_ref().iter().position(|byte| *byte == b':')
            && owner
                .prefixes
                .iter()
                .any(|prefix| prefix.as_bytes() == &attribute.key.as_ref()[..colon])
        {
            attribute_prefix = Some(&field_attribute_source(raw, attribute.key.as_ref())?[..colon]);
        }
        if !is_field_word_attribute(attribute.key.as_ref(), b"fldLock", &owner.prefixes) {
            replacement.push(FieldSourcePiece::Literal(b" "));
            replacement.push(FieldSourcePiece::Raw(field_attribute_source(
                raw,
                attribute.key.as_ref(),
            )?));
            replacement.push(FieldSourcePiece::Literal(b"=\""));
            replacement.push(FieldSourcePiece::Raw(field_attribute_source(
                raw,
                attribute.value.as_ref(),
            )?));
            replacement.push(FieldSourcePiece::Literal(b"\""));
        }
    }
    if let Some(value) = locked {
        // A parsed owner already has a Word-qualified instruction/type attribute.
        // Reuse its actual binding instead of redeclaring a possibly foreign w.
        let attribute_prefix = attribute_prefix.ok_or_else(|| {
            OxmlError::InvalidValue("field attribute requires a Word prefix".into())
        })?;
        replacement.push(FieldSourcePiece::Literal(b" "));
        replacement.push(FieldSourcePiece::Raw(attribute_prefix));
        replacement.push(FieldSourcePiece::Literal(b":fldLock=\""));
        replacement.push(FieldSourcePiece::Literal(if value { b"1" } else { b"0" }));
        replacement.push(FieldSourcePiece::Literal(b"\""));
    }
    replacement.push(FieldSourcePiece::Literal(if owner.empty {
        b"/>"
    } else {
        b">"
    }));
    FieldSourceEdit::new(raw.len(), owner.start..owner.end, replacement)
}

pub(super) fn write_field_lock_update<W: std::io::Write>(
    writer: &mut Writer<W>,
    raw: &[u8],
    field: &Field,
    foreign_word_namespace: Option<&str>,
    publication_scope: Option<&[(&str, &str)]>,
    work: &WordOperationWork,
    word_map: Option<&crate::document::WordSourceMap>,
) -> Result<()> {
    let FieldSource::Parsed {
        form,
        word_prefixes,
        ..
    } = field.source.as_ref()
    else {
        return Err(OxmlError::MissingElement(
            "field lock original source".into(),
        ));
    };
    let plan = plan_field_lock(raw, *form, word_prefixes, field.locked)?;
    let prepaid = work.reserve_inner_xml(plan.projected)?;
    let mapped = word_map
        .map(|map| {
            crate::document::WordSourceMap::concat(&[
                map.slice(0..plan.range.start)?.word_only(),
                crate::document::WordSourceMap::generated(plan.replacement_bytes),
                map.slice(plan.range.end..raw.len())?.word_only(),
            ])
        })
        .transpose()?;
    let updated = plan.emit(raw)?;
    // This is the exact synchronous transfer of the record just produced. A
    // namespace-transforming publication is a different producer.
    let _transfer = (prepaid && publication_scope.is_none() && foreign_word_namespace.is_none())
        .then(|| work.prepaid_xml_transport(&updated))
        .transpose()?;
    write_field_raw_output(
        writer,
        &updated,
        field,
        foreign_word_namespace,
        publication_scope,
        work,
        mapped.as_ref(),
    )
}

/// A run child a field's cached result reads as a character besides `w:t`,
/// as `CT_R::content_text` does: it goes with the old result when the cache
/// changes.
fn is_result_character(name: &[u8], prefixes: &[String]) -> bool {
    [b"tab".as_slice(), b"ptab", b"br", b"cr", b"noBreakHyphen"]
        .iter()
        .any(|local| is_word_element(name, local, prefixes))
}
