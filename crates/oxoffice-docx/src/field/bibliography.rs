//! The field side of citations and bibliographies: instructions, encounter order and caches.

use super::*;

pub(crate) fn bibliography_reference_instructions(
    document: &Document,
) -> Result<Vec<FieldInstruction>> {
    let mut instructions = Vec::new();
    for story in generated_story_inventory(document, DynamicOwnerPolicy::Bibliography)? {
        for span in &story.spans {
            let field = parse_dynamic_toc_field(&story.xml, span)?;
            let instruction = field.effective_instruction();
            if instruction.name == "CITATION" {
                instructions.push(instruction);
            }
        }
    }
    Ok(instructions)
}

struct BibliographyInstructionSource {
    text: String,
    units: Vec<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    insertion: Option<usize>,
    attribute: bool,
}

fn bibliography_push_instruction_text(
    source: &mut BibliographyInstructionSource,
    raw: &[u8],
    start: usize,
) -> Result<()> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| Error::Other("bibliography instruction is not UTF-8".into()))?;
    let mut characters = text.char_indices().peekable();
    while let Some((offset, mut character)) = characters.next() {
        let mut end = offset + character.len_utf8();
        if character == '\r' {
            if characters
                .peek()
                .is_some_and(|(_, character)| *character == '\n')
            {
                end = characters.next().expect("peeked character exists").0 + 1;
            }
            character = '\n';
        }
        if source.attribute && matches!(character, '\n' | '\t') {
            character = ' ';
        }
        let begin = source.text.len();
        source.text.push(character);
        source
            .units
            .push((begin..source.text.len(), start + offset..start + end));
    }
    source.insertion = Some(start + raw.len());
    Ok(())
}

fn bibliography_push_instruction_reference(
    source: &mut BibliographyInstructionSource,
    raw: &[u8],
    start: usize,
) -> Result<()> {
    let raw_text = std::str::from_utf8(raw)
        .map_err(|_| Error::Other("bibliography reference is not UTF-8".into()))?;
    let value = quick_xml::escape::unescape(raw_text).map_err(|error| {
        Error::Other(format!("invalid bibliography character reference: {error}"))
    })?;
    if value.chars().count() != 1 {
        return Err(Error::Other(
            "ambiguous bibliography character reference".into(),
        ));
    }
    let begin = source.text.len();
    source.text.push_str(&value);
    source
        .units
        .push((begin..source.text.len(), start..start + raw.len()));
    source.insertion = Some(start + raw.len());
    Ok(())
}

fn bibliography_instruction_source(
    xml: &[u8],
    span: &DynamicTocSpan,
) -> Result<BibliographyInstructionSource> {
    let mut source = BibliographyInstructionSource {
        text: String::new(),
        units: Vec::new(),
        insertion: None,
        attribute: span.simple_field.is_some(),
    };
    let fragments = if span.simple_field.is_some() {
        vec![(
            span.field_start,
            span.field_end,
            &span.start_paragraph_namespaces,
        )]
    } else {
        span.instruction_runs
            .iter()
            .map(|run| (run.start, run.end, &run.inherited_namespaces))
            .collect()
    };
    for (begin, end, namespaces) in fragments {
        let mut wrapper = String::from("<instructionSource");
        for (prefix, namespace) in namespaces {
            if prefix == "xml" {
                continue;
            }
            if prefix.is_empty() {
                wrapper.push_str(" xmlns=\"");
            } else {
                wrapper.push_str(&format!(" xmlns:{prefix}=\""));
            }
            wrapper.push_str(&xml_escape_attribute(namespace));
            wrapper.push('"');
        }
        wrapper.push('>');
        let wrapper_len = wrapper.len();
        let mut wrapped = wrapper.into_bytes();
        wrapped.extend_from_slice(&xml[begin..end]);
        wrapped.extend_from_slice(b"</instructionSource>");
        let mut reader = NsReader::from_reader(wrapped.as_slice());
        let mut buffer = Vec::new();
        let mut elements = Vec::<(bool, Vec<u8>)>::new();
        let mut active = None;
        loop {
            let before = reader.buffer_position() as usize;
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| Error::Other(format!("bibliography instruction XML: {error}")))?;
            let word = namespace_is_word(&namespace);
            let event = event.into_owned();
            let after = reader.buffer_position() as usize;
            let empty = matches!(event, Event::Empty(_));
            let physical = |offset: usize| -> Result<usize> {
                offset
                    .checked_sub(wrapper_len)
                    .and_then(|offset| begin.checked_add(offset))
                    .filter(|offset| *offset <= end)
                    .ok_or_else(|| {
                        Error::Other("bibliography instruction range escapes its source".into())
                    })
            };
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    if active.is_some() {
                        return Err(Error::Other("mixed bibliography instruction XML".into()));
                    }
                    let local = element.local_name().as_ref().to_vec();
                    if source.attribute && word && local == b"fldSimple" {
                        for attribute in element.attributes() {
                            let attribute =
                                attribute.map_err(|error| Error::Other(error.to_string()))?;
                            let (namespace, local) =
                                reader.resolver().resolve_attribute(attribute.key);
                            if namespace_is_word(&namespace) && local.as_ref() == b"instr" {
                                let (start, finish) = attribute_value_span(
                                    &wrapped[before..after],
                                    attribute.key.as_ref(),
                                )
                                .ok_or_else(|| {
                                    Error::Other(
                                        "bibliography instruction attribute has no source span"
                                            .into(),
                                    )
                                })?;
                                let start = before + start;
                                let finish = before + finish;
                                let mut text_reader =
                                    oxoffice_core::xml_reader::Reader::from_reader(
                                        &wrapped[start..finish],
                                    );
                                let mut text_buffer = Vec::new();
                                loop {
                                    let text_before = text_reader.buffer_position() as usize;
                                    let event = text_reader
                                        .read_event_into(&mut text_buffer)
                                        .map_err(|error| Error::Other(error.to_string()))?;
                                    let text_after = text_reader.buffer_position() as usize;
                                    match event {
                                        Event::Text(_) => bibliography_push_instruction_text(
                                            &mut source,
                                            &wrapped[start + text_before..start + text_after],
                                            physical(start + text_before)?,
                                        )?,
                                        Event::GeneralRef(_) => {
                                            bibliography_push_instruction_reference(
                                                &mut source,
                                                &wrapped[start + text_before..start + text_after],
                                                physical(start + text_before)?,
                                            )?
                                        }
                                        Event::Eof => break,
                                        _ => {
                                            return Err(Error::Other(
                                                "ambiguous bibliography instruction attribute"
                                                    .into(),
                                            ));
                                        }
                                    }
                                    text_buffer.clear();
                                }
                                source.insertion = Some(physical(finish)?);
                                let expected = attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        element.decoder(),
                                    )
                                    .map_err(|error| Error::Other(error.to_string()))?;
                                if source.text != expected {
                                    return Err(Error::Other("bibliography instruction lexical map disagrees with XML decoding".into()));
                                }
                            }
                        }
                    } else if !source.attribute
                        && word
                        && local == b"instrText"
                        && elements
                            .last()
                            .is_some_and(|(word, local)| *word && local == b"r")
                        && !empty
                    {
                        active = Some(elements.len() + 1);
                        source.insertion = Some(physical(after)?);
                    }
                    if !empty {
                        elements.push((word, local));
                    }
                }
                Event::End(_) => {
                    if active == Some(elements.len()) {
                        active = None;
                        source.insertion = Some(physical(before)?);
                    }
                    elements.pop();
                }
                Event::Text(_) if active.is_some() => bibliography_push_instruction_text(
                    &mut source,
                    &wrapped[before..after],
                    physical(before)?,
                )?,
                Event::GeneralRef(_) if active.is_some() => {
                    bibliography_push_instruction_reference(
                        &mut source,
                        &wrapped[before..after],
                        physical(before)?,
                    )?
                }
                Event::CData(value) if active.is_some() => {
                    bibliography_push_instruction_text(
                        &mut source,
                        value.as_ref(),
                        physical(before + 9)?,
                    )?;
                    source.insertion = Some(physical(after)?);
                }
                Event::DocType(_) => {
                    return Err(Error::Other("bibliography instruction DOCTYPE".into()));
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    if source.insertion.is_none() || source.text.trim() != span.instruction.trim() {
        return Err(Error::Other(
            "bibliography instruction source and owned field disagree".into(),
        ));
    }
    Ok(source)
}

pub(crate) fn patch_bibliography_instruction_options(
    document: &mut Document,
    switches: &[oxoffice_docx_oxml::text::FieldSwitch],
) -> Result<()> {
    let stories = generated_story_inventory(document, DynamicOwnerPolicy::Bibliography)?;
    let mut edits = BTreeMap::<String, Vec<FieldSourceEdit>>::new();
    for story in stories {
        for span in &story.spans {
            if Field::new(&span.instruction, "").instruction.name != "BIBLIOGRAPHY" {
                continue;
            }
            let source = bibliography_instruction_source(&story.xml, span)?;
            let replacements =
                oxoffice_docx_oxml::text::bibliography_option_switch_edits(&source.text, switches)?;
            let offset = |offset: usize| -> Result<usize> {
                offset
                    .checked_sub(story.wrapper_len)
                    .and_then(|offset| story.range.start.checked_add(offset))
                    .filter(|offset| *offset <= story.range.end)
                    .ok_or_else(|| {
                        Error::Other(
                            "bibliography instruction edit escapes its physical owner".into(),
                        )
                    })
            };
            let part_edits = edits.entry(story.story.part_name().to_owned()).or_default();
            for (range, replacement) in replacements {
                if range.is_empty() {
                    let insertion = offset(source.insertion.ok_or_else(|| {
                        Error::Other("bibliography code insertion boundary disappeared".into())
                    })?)?;
                    let replacement = if source.attribute {
                        xml_escape_attribute(&replacement)
                            .replace('\r', "&#13;")
                            .replace('\n', "&#10;")
                            .replace('\t', "&#9;")
                    } else {
                        xml_escape_text(&replacement).replace('\r', "&#13;")
                    };
                    part_edits.push(FieldSourceEdit {
                        start: insertion,
                        end: insertion,
                        replacement: replacement.into_bytes(),
                    });
                } else {
                    for (logical, physical) in &source.units {
                        if logical.end <= range.start || range.end <= logical.start {
                            continue;
                        }
                        if logical.start < range.start || range.end < logical.end {
                            return Err(Error::Other(
                                "bibliography token splits an XML character".into(),
                            ));
                        }
                        part_edits.push(FieldSourceEdit {
                            start: offset(physical.start)?,
                            end: offset(physical.end)?,
                            replacement: Vec::new(),
                        });
                    }
                }
            }
        }
    }
    for (part, mut part_edits) in edits {
        part_edits.sort_by_key(|edit| (edit.start, edit.end));
        if part_edits
            .windows(2)
            .any(|pair| pair[0].end > pair[1].start)
        {
            return Err(Error::Other(
                "bibliography instruction source edits overlap".into(),
            ));
        }
        let mut xml = document
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other("bibliography physical source disappeared".into()))?
            .to_vec();
        for edit in part_edits.into_iter().rev() {
            xml.splice(edit.start..edit.end, edit.replacement);
        }
        validate_strict_xml_1_0(&xml).map_err(|error| {
            Error::Other(format!("invalid bibliography instruction edit: {error:?}"))
        })?;
        document.package.set_part(&part, xml);
    }
    Ok(())
}

// Source instructions alone establish numbering. Generated caches are excluded by the shared inventory.
pub(crate) fn bibliography_citation_encounter_tags(document: &Document) -> Result<Vec<String>> {
    use oxoffice_docx_oxml::text::FieldArgument;
    let mut tags = Vec::new();
    for story in generated_story_inventory(document, DynamicOwnerPolicy::Bibliography)? {
        for span in &story.spans {
            let field = parse_dynamic_toc_field(&story.xml, span)?;
            let instruction = field.effective_instruction();
            if instruction.name != "CITATION" {
                continue;
            }
            let Some(FieldArgument::Text(tag)) = instruction.arguments.first() else {
                return Err(Error::Other(
                    "citation source identity is missing or nested".into(),
                ));
            };
            if tag.trim().is_empty() {
                return Err(Error::Other("empty citation source identity".into()));
            }
            tags.push(tag.clone());
            for switch in instruction
                .switches
                .iter()
                .take(crate::bibliography::NUMERIC_CITATION_SWITCH_LIMIT)
            {
                if switch.name != "m" {
                    continue;
                }
                let Some(FieldArgument::Text(tag)) = &switch.argument else {
                    return Err(Error::Other(
                        "citation grouped identity is missing or nested".into(),
                    ));
                };
                if tag.trim().is_empty() {
                    return Err(Error::Other(
                        "empty grouped citation source identity".into(),
                    ));
                }
                tags.push(tag.clone());
            }
        }
    }
    Ok(tags)
}

fn preserve_bibliography_div_group(
    story: &GeneratedStorySource,
    span: &DynamicTocSpan,
    blocks: &mut [oxoffice_docx_oxml::document::BodyContent],
) -> Result<()> {
    use oxoffice_docx_oxml::document::BodyContent;
    if !blocks
        .iter()
        .any(|block| matches!(block, BodyContent::Table(_)))
    {
        return Ok(());
    }
    let mut reader = NsReader::from_reader(story.xml.as_slice());
    let mut buffer = Vec::new();
    let mut parents = Vec::<Vec<u8>>::new();
    let mut table_depth = 0;
    let mut group = None;
    let mut row_particle = None;
    loop {
        let start_offset = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("invalid bibliography producer identity: {error}"))
        })?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(start) | Event::Empty(start) => {
                let (namespace, local) = reader.resolver().resolve_element(start.name());
                let word = matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == W_NS.as_bytes());
                let local = if word {
                    local.as_ref().to_vec()
                } else {
                    Vec::new()
                };
                if local == b"divId"
                    && start_offset >= span.result_start
                    && start_offset < span.result_end
                {
                    let row = parents.last().is_some_and(|parent| parent == b"trPr");
                    let trailer =
                        table_depth == 0 && parents.last().is_some_and(|parent| parent == b"pPr");
                    if !row && !trailer {
                        return Err(Error::Other(
                            "ambiguous bibliography divId association".into(),
                        ));
                    }
                    let mut identity = None;
                    for attribute in start.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        if local.as_ref() == b"val"
                            && matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == W_NS.as_bytes())
                        {
                            if identity.is_some() {
                                return Err(Error::Other(
                                    "duplicate bibliography divId value".into(),
                                ));
                            }
                            identity = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        start.decoder(),
                                    )
                                    .map_err(|error| Error::Other(error.to_string()))?
                                    .parse::<u32>()
                                    .map_err(|_| {
                                        Error::Other("invalid bibliography divId value".into())
                                    })?,
                            );
                        } else if attribute.key.as_ref() != b"xmlns"
                            && !attribute.key.as_ref().starts_with(b"xmlns:")
                        {
                            return Err(Error::Other("unmodeled bibliography divId attributes have no proven group association".into()));
                        }
                    }
                    let identity = identity
                        .ok_or_else(|| Error::Other("missing bibliography divId value".into()))?;
                    if group.is_some_and(|value| value != identity) {
                        return Err(Error::Other(
                            "conflicting bibliography divId group identities".into(),
                        ));
                    }
                    group = Some(identity);
                    if !empty {
                        let content_start = reader.buffer_position() as usize;
                        let end = reader
                            .read_to_end_into(start.name(), &mut Vec::new())
                            .map_err(|error| Error::Other(error.to_string()))?;
                        if !story.xml[content_start..end.end as usize]
                            .iter()
                            .all(u8::is_ascii_whitespace)
                        {
                            return Err(Error::Other(
                                "unmodeled bibliography divId content has no proven group association".into(),
                            ));
                        }
                    }
                    if row {
                        let raw = &story.xml[start_offset..reader.buffer_position() as usize];
                        let scope =
                            crate::document::story_namespace_scope_at(&story.xml, start_offset)?;
                        let closed =
                            crate::document::close_content_fragment_namespaces(raw, &scope)?;
                        if row_particle.as_ref().is_some_and(|value| value != &closed) {
                            return Err(Error::Other("different bibliography divId row particles have no proven group association".into()));
                        }
                        row_particle = Some(closed);
                    }
                    buffer.clear();
                    continue;
                }
                if !empty {
                    if local == b"tbl" {
                        table_depth += 1;
                    }
                    parents.push(local);
                }
            }
            Event::End(_) => {
                if parents.pop().as_deref() == Some(b"tbl") {
                    table_depth -= 1;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if let Some(identity) = group {
        let particle = row_particle
            .ok_or_else(|| Error::Other("bibliography divId lacks a proven row group".into()))?;
        for block in blocks {
            match block {
                BodyContent::Table(table) => {
                    for row in &mut table.rows {
                        row.properties
                            .get_or_insert_with(Default::default)
                            .extra_xml
                            .insert(0, (0, particle.clone()));
                    }
                }
                BodyContent::Paragraph(paragraph)
                    if paragraph
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.rpr.as_ref())
                        .is_some_and(|properties| {
                            properties.font_east_asia.as_deref() == Some("Times New Roman")
                        }) =>
                {
                    paragraph.properties.as_mut().unwrap().div_id = Some(identity);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

pub(super) fn bibliography_story_text_width(
    document: &Document,
    story: &GeneratedStorySource,
    span: &DynamicTocSpan,
) -> Option<i32> {
    if story.story.kind() != crate::StoryKind::Body {
        return None;
    }
    // Physical owner ranges exclude the final section particle. Keep that existing
    // document section as the fallback behind any intervening paragraph section.
    let mut body = story.body.clone();
    body.sect_pr = document.document.body.sect_pr.clone();
    Some(toc_section_text_width(&body, span.begin_paragraph))
}

pub(crate) fn update_bibliography_caches(
    document: &mut Document,
    state: &crate::bibliography::BibliographyUpdateState,
) -> Result<crate::BibliographyUpdateReport> {
    generated_normalize_simple_tables(document, SimpleGeneratedOwnerContext::Bibliography(state))?;
    let mut report = crate::BibliographyUpdateReport {
        updated_citations: 0,
        rebuilt_bibliographies: 0,
        diagnostics: Vec::new(),
    };
    let mut edits = BTreeMap::<String, Vec<FieldSourceEdit>>::new();
    for story in generated_story_inventory(document, DynamicOwnerPolicy::Bibliography)? {
        let paragraph_properties =
            crate::bibliography::bibliography_paragraph_property_ranges(&story.xml)?;
        for span in &story.spans {
            let field = parse_dynamic_toc_field(&story.xml, span)?;
            let instruction = field.effective_instruction();
            if !matches!(instruction.name.as_str(), "CITATION" | "BIBLIOGRAPHY") {
                continue;
            }
            if field.locked() == Some(true) {
                report.diagnostics.push(format!(
                    "locked {} retains its complete cache",
                    instruction.name
                ));
                continue;
            }
            if instruction.name == "BIBLIOGRAPHY" {
                let Some(mut blocks) = state.bibliography_blocks(
                    &instruction,
                    bibliography_story_text_width(document, &story, span),
                )?
                else {
                    report
                        .diagnostics
                        .push("noncatalogue bibliography retains its complete cache".into());
                    continue;
                };
                preserve_bibliography_div_group(&story, span, &mut blocks)?;
                if span.simple_field.is_some() {
                    report.diagnostics.push("simple bibliography with unmodeled producer attributes retains its complete owner".into());
                    continue;
                }
                if !span.separator_wrapper_names.is_empty() || !span.end_wrapper_prefixes.is_empty()
                {
                    return Err(Error::Other(
                        "bibliography: a bibliography field whose separator or end is wrapped in other markup is not covered yet".into(),
                    ));
                }
                let Some(oxoffice_docx_oxml::document::BodyContent::Paragraph(first)) =
                    blocks.first()
                else {
                    return Err(Error::Other(
                        "bibliography formatter returned no entry boundary".into(),
                    ));
                };
                let mut writer = quick_xml::Writer::new(Vec::new());
                first
                    .properties
                    .as_ref()
                    .ok_or_else(|| Error::Other("bibliography first properties missing".into()))?
                    .to_xml(&mut writer)?;
                let properties = xml_fragment_with_namespaces(
                    &writer.into_inner(),
                    &BTreeMap::from([("w".into(), W_NS.into())]),
                    "native bibliography paragraph properties",
                )?;
                let paragraph_start = span.instruction_paragraph_start;
                let mut reader = oxoffice_core::xml_reader::Reader::from_reader(
                    &story.xml[paragraph_start..span.result_start],
                );
                let mut buffer = Vec::new();
                let Event::Start(start) = reader.read_event_into(&mut buffer).map_err(|error| {
                    Error::Other(format!("invalid bibliography first paragraph: {error}"))
                })?
                else {
                    return Err(Error::Other(
                        "bibliography first paragraph boundary changed".into(),
                    ));
                };
                if start.name().as_ref() != span.start_paragraph_name.as_bytes() {
                    return Err(Error::Other(
                        "bibliography first paragraph ownership changed".into(),
                    ));
                }
                let property_offset = paragraph_start + reader.buffer_position() as usize;
                let existing_properties = paragraph_properties.get(&paragraph_start).cloned();
                let relocated_properties = if let Some(range) = &existing_properties {
                    if story.xml[range.clone()] != properties {
                        if span.begin_paragraph != span.end_paragraph {
                            None
                        } else {
                            Some(crate::bibliography::bibliography_end_paragraph_properties(
                                &story.xml[range.clone()],
                                &span.start_paragraph_namespaces,
                            )?)
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                let offset = |offset: usize| {
                    offset
                        .checked_sub(story.wrapper_len)
                        .and_then(|offset| offset.checked_add(story.range.start))
                        .filter(|offset| *offset <= story.range.end)
                        .ok_or_else(|| {
                            Error::Other("bibliography cache escaped its physical owner".into())
                        })
                };
                if let Some(range) = &existing_properties {
                    let replacement = if story.xml[range.clone()] == properties {
                        properties.clone()
                    } else {
                        crate::bibliography::bibliography_first_paragraph_properties(
                            &story.xml[range.clone()],
                            &properties,
                            &span.start_paragraph_namespaces,
                        )?
                    };
                    if story.xml[range.clone()] != replacement {
                        edits
                            .entry(story.story.part_name().to_owned())
                            .or_default()
                            .push(FieldSourceEdit {
                                start: offset(range.start)?,
                                end: offset(range.end)?,
                                replacement,
                            });
                    }
                }
                if existing_properties.is_none() {
                    edits
                        .entry(story.story.part_name().to_owned())
                        .or_default()
                        .push(FieldSourceEdit {
                            start: offset(property_offset)?,
                            end: offset(property_offset)?,
                            replacement: properties,
                        });
                }
                let mut writer = quick_xml::Writer::new(Vec::new());
                for run in &first.runs {
                    run.to_xml(&mut writer)?;
                }
                let first_runs = writer.into_inner();
                let mut replacement = if first_runs.is_empty() {
                    Vec::new()
                } else {
                    xml_fragment_with_namespaces(
                        &first_runs,
                        &BTreeMap::from([("w".into(), W_NS.into())]),
                        "native bibliography first entry",
                    )?
                };
                replacement
                    .extend_from_slice(format!("</{}>", span.start_paragraph_name).as_bytes());
                for block in blocks.iter().skip(1) {
                    let mut writer = quick_xml::Writer::new(Vec::new());
                    match block {
                        oxoffice_docx_oxml::document::BodyContent::Paragraph(paragraph) => {
                            paragraph.to_xml(&mut writer)?;
                        }
                        oxoffice_docx_oxml::document::BodyContent::Table(table) => {
                            table.to_xml(&mut writer)?;
                        }
                        _ => {
                            return Err(Error::Other(
                                "bibliography formatter returned an unsupported owned block".into(),
                            ));
                        }
                    }
                    replacement.extend_from_slice(&xml_fragment_with_namespaces(
                        &writer.into_inner(),
                        &BTreeMap::from([("w".into(), W_NS.into())]),
                        "native bibliography interior entry",
                    )?);
                }
                if span.begin_paragraph == span.end_paragraph {
                    // The new end boundary has no producer paragraph identity to duplicate.
                    // Original end controls and their outside suffix remain byte-for-byte in place.
                    let mut end_start = format!("<{}", span.start_paragraph_name);
                    for (prefix, namespace) in &span.start_paragraph_namespaces {
                        if prefix == "xml" {
                            continue;
                        }
                        let name = if prefix.is_empty() {
                            "xmlns".to_owned()
                        } else {
                            format!("xmlns:{prefix}")
                        };
                        end_start
                            .push_str(&format!(" {name}=\"{}\"", xml_escape_attribute(namespace)));
                    }
                    end_start.push('>');
                    replacement.extend_from_slice(end_start.as_bytes());
                    if let Some(properties) = &relocated_properties {
                        replacement.extend_from_slice(properties);
                    }
                } else {
                    replacement.extend_from_slice(
                        &story.xml[span.end_paragraph_start..span.end_paragraph_content_start],
                    );
                }
                edits
                    .entry(story.story.part_name().to_owned())
                    .or_default()
                    .push(FieldSourceEdit {
                        start: offset(span.result_start)?,
                        end: offset(span.result_end)?,
                        replacement,
                    });
                report.rebuilt_bibliographies += 1;
                continue;
            }
            let Some(mut runs) = state.citation_runs(&instruction)? else {
                report
                    .diagnostics
                    .push("noncatalogue citation retains its complete cache".into());
                continue;
            };
            if span.begin_paragraph != span.end_paragraph {
                report.diagnostics.push(
                    "citation with producer block topology retains its complete cache".into(),
                );
                continue;
            }
            if instruction.switches.iter().any(|switch| switch.name == "*"
                && matches!(&switch.argument, Some(oxoffice_docx_oxml::text::FieldArgument::Text(value)) if value.eq_ignore_ascii_case("MERGEFORMAT"))) {
                if !span.separator_wrapper_names.is_empty() || !span.end_wrapper_prefixes.is_empty() {
                    return Err(Error::Other("ambiguous citation cache-format wrapper".into()));
                }
                let scope = crate::document::story_namespace_scope_at(&story.xml, span.result_start)?;
                let cache = crate::document::close_content_fragment_namespaces(
                    &story.xml[span.result_start..span.result_end], &scope)?;
                let mut paragraph = format!("<w:p xmlns:w=\"{W_NS}\">").into_bytes();
                paragraph.extend_from_slice(&cache);
                paragraph.extend_from_slice(b"</w:p>");
                let previous = CT_P::from_xml_fragment(&paragraph)?;
                let properties = previous.runs.first().and_then(|run| run.properties.as_ref());
                if previous.runs.is_empty() || !previous.extra_xml.is_empty() || !previous.hyperlinks.is_empty() || !previous.comment_ranges.is_empty() || !previous.bookmark_markers.is_empty() || !previous.content_controls.is_empty() || !previous.revisions.is_empty() || !previous.equations.is_empty() || !previous.rubies.is_empty() || previous.runs.iter().any(|run|
                    !run.extra_xml.is_empty() || run.properties.as_ref() != properties
                    || run.content.iter().any(|content| !matches!(content, RunContent::Text(_))))
                    || runs.windows(2).any(|pair| pair[0].properties != pair[1].properties)
                {
                    return Err(Error::Other("ambiguous citation cache-format association".into()));
                }
                let mut replacement = oxoffice_docx_oxml::text::CT_R::new(
                    &runs.iter().map(|run| run.text()).collect::<String>());
                // merge_from cascades modeled properties. Keep the original raw particles on
                // the source clone while supplying generated defaults for missing values.
                let mut merged = properties.cloned().unwrap_or_default();
                let mut effective = runs.first().and_then(|run| run.properties.clone()).unwrap_or_default();
                effective.merge_from(&merged);
                merged.merge_from(&effective);
                replacement.properties = Some(merged);
                runs = vec![replacement];
            }
            let mut writer = quick_xml::Writer::new(Vec::new());
            for run in &runs {
                run.to_xml(&mut writer)?;
            }
            let replacement = xml_fragment_with_namespaces(
                &writer.into_inner(),
                &BTreeMap::from([("w".into(), W_NS.into())]),
                "native citation cache",
            )?;
            let offset = |offset: usize| {
                offset
                    .checked_sub(story.wrapper_len)
                    .and_then(|offset| offset.checked_add(story.range.start))
                    .filter(|offset| *offset <= story.range.end)
                    .ok_or_else(|| Error::Other("citation cache escaped its physical owner".into()))
            };
            edits
                .entry(story.story.part_name().to_owned())
                .or_default()
                .push(FieldSourceEdit {
                    start: offset(span.result_start)?,
                    end: offset(span.result_end)?,
                    replacement,
                });
            report.updated_citations += 1;
        }
    }
    for (part, mut edits) in edits {
        edits.sort_by_key(|edit| edit.start);
        if edits.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err(Error::Other("citation cache edits overlap".into()));
        }
        let mut xml = document
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other("citation story disappeared".into()))?
            .to_vec();
        for edit in edits.into_iter().rev() {
            xml.splice(edit.start..edit.end, edit.replacement);
        }
        validate_strict_xml_1_0(&xml)
            .map_err(|error| Error::Other(format!("invalid citation cache XML: {error:?}")))?;
        document.package.set_part(&part, xml);
    }
    Ok(report)
}
