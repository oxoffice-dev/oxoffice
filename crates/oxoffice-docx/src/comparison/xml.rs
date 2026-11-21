//! Serializing model items and editing spans of raw Word XML.

use super::*;

pub(super) fn direct_element_spans(xml: &str) -> Result<Vec<Range<usize>>> {
    let mut reader = Reader::from_reader(xml.as_bytes());
    reader.config_mut().trim_text(false);
    let mut spans = Vec::new();
    let mut open = Vec::new();
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(_) => {
                open.push((depth == 1).then_some(before));
                depth += 1;
            }
            Event::Empty(_) if depth == 1 => spans.push(before..after),
            Event::Empty(_) => {}
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if let Some(Some(start)) = open.pop() {
                    spans.push(start..after);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(spans)
}

pub(super) fn body_content_xml(content: &BodyContent) -> Result<String> {
    match content {
        BodyContent::Paragraph(paragraph) => paragraph_xml(paragraph),
        BodyContent::Table(table) => table_xml(table),
        BodyContent::ContentControl(control) => control_xml(control),
        BodyContent::RawXml(raw) => String::from_utf8(raw.as_ref().clone()).map_err(utf8_error),
    }
}

/// The paragraph source with each run that the reader split around a field
/// written as separate physical runs, one per modeled run.
pub(super) fn detach_field_spans(paragraph: &CT_P, source: Option<&str>) -> Result<Option<String>> {
    let mut spans = Vec::new();
    let mut index = 0;
    while index < paragraph.runs.len() {
        if let Some((coverage, fragments)) = paragraph.field_source_run_fragments_at(index)? {
            let mut owners = Vec::new();
            if paragraph.field_source_replacements_at(index, &mut owners)? != coverage
                || owners.len() != 1
            {
                return Err(Error::Other(
                    "detached source span coverage is unproved".into(),
                ));
            }
            let mut detached = Vec::new();
            for (run, fragment) in paragraph.runs[index..index + coverage]
                .iter()
                .zip(fragments)
            {
                if let Some(fragment) = fragment {
                    detached.extend_from_slice(&fragment);
                } else if let Some(field) = run_field(run) {
                    let payload = field.detached_source()?.ok_or_else(|| {
                        Error::Other("shared field has no individual source".into())
                    })?;
                    detached.extend_from_slice(&payload);
                } else {
                    return Err(Error::Other(
                        "shared source field slot has no typed owner".into(),
                    ));
                }
            }
            spans.push((owners.remove(0).0, detached));
            index += coverage;
        } else {
            index += 1;
        }
    }
    if spans.is_empty() {
        return Ok(None);
    }
    let mut source =
        source.map_or_else(|| paragraph_xml(paragraph), |source| Ok(source.to_owned()))?;
    // Each span starts at a physical run of the paragraph, in order.
    let runs = paragraph_run_spans(&source)?;
    let mut edits = Vec::with_capacity(spans.len());
    let mut cursor = 0;
    for (raw, detached) in spans {
        let raw = std::str::from_utf8(&raw).map_err(utf8_error)?;
        let start = runs
            .iter()
            .map(|run| run.start)
            .find(|start| *start >= cursor && source[*start..].starts_with(raw))
            .ok_or_else(|| {
                Error::Other(
                    "comparison could not find a split field run in its paragraph".to_owned(),
                )
            })?;
        cursor = start + raw.len();
        edits.push((
            start..cursor,
            String::from_utf8(detached).map_err(utf8_error)?,
        ));
    }
    for (range, detached) in edits.into_iter().rev() {
        source.replace_range(range, &detached);
    }
    Ok(Some(source))
}

pub(super) fn paragraph_xml(paragraph: &CT_P) -> Result<String> {
    let mut bytes = Vec::new();
    paragraph.to_xml(&mut Writer::new(&mut bytes))?;
    String::from_utf8(bytes).map_err(utf8_error)
}

pub(super) fn property_xml(properties: &CT_PPr) -> Result<String> {
    let mut bytes = Vec::new();
    properties.to_xml(&mut Writer::new(&mut bytes))?;
    String::from_utf8(bytes).map_err(utf8_error)
}

pub(super) fn run_xml(run: &CT_R) -> Result<String> {
    let mut bytes = Vec::new();
    run.to_xml(&mut Writer::new(&mut bytes))?;
    String::from_utf8(bytes).map_err(utf8_error)
}

pub(super) fn paragraph_owned_run_xml(run: &CT_R) -> Result<String> {
    if !run_is_field(run) {
        return run_xml(run);
    }
    let mut paragraph = CT_P::new();
    paragraph.runs.push(run.clone());
    let xml = paragraph_xml(&paragraph)?;
    let start = element_opening(&xml)?.0;
    let end = xml
        .rfind("</w:p>")
        .ok_or_else(|| Error::Other("serialized paragraph has no end".to_owned()))?;
    Ok(xml[start..end].to_owned())
}

/// A run as deleted content. The field instruction of a field that spans
/// paragraphs is a preserved child, so it is renamed in the written run.
pub(super) fn deleted_run_xml(run: &CT_R) -> Result<String> {
    let mut xml = run_xml(&deleted_run(run))?;
    let spans = direct_word_element_spans(&xml, "instrText")?;
    for span in spans.into_iter().rev() {
        let child = &xml[span.clone()];
        let (_, _, name) = element_opening(child)?;
        let replacement = name
            .rsplit_once(':')
            .map_or("delInstrText".to_owned(), |(prefix, _)| {
                format!("{prefix}:delInstrText")
            });
        let mut renamed = child.to_owned();
        if !element_opening(child)?.1 {
            let close = paragraph_close_start(child)?;
            renamed.replace_range(close + 2..close + 2 + name.len(), &replacement);
        }
        let name_start = 1;
        renamed.replace_range(name_start..name_start + name.len(), &replacement);
        xml.replace_range(span, &renamed);
    }
    Ok(xml)
}

pub(super) fn deleted_run(run: &CT_R) -> CT_R {
    let mut deleted = run.clone();
    for content in &mut deleted.content {
        if let RunContent::Text(text) = content {
            *content = RunContent::DeletedText(text.clone());
        }
    }
    deleted
}

pub(super) fn table_xml(table: &CT_Tbl) -> Result<String> {
    let mut bytes = Vec::new();
    table.to_xml(&mut Writer::new(&mut bytes))?;
    String::from_utf8(bytes).map_err(utf8_error)
}

pub(super) fn row_xml(row: &CT_Row) -> Result<String> {
    let mut bytes = Vec::new();
    row.to_xml(&mut Writer::new(&mut bytes))?;
    String::from_utf8(bytes).map_err(utf8_error)
}

pub(super) fn cell_xml(cell: &CT_Tc) -> Result<String> {
    let mut bytes = Vec::new();
    cell.to_xml(&mut Writer::new(&mut bytes))?;
    String::from_utf8(bytes).map_err(utf8_error)
}

pub(super) fn control_xml(control: &CT_Sdt) -> Result<String> {
    let document = CT_Document {
        body: oxoffice_docx_oxml::document::CT_Body {
            content: vec![BodyContent::ContentControl(Box::new(control.clone()))],
            sect_pr: None,
        },
        ..CT_Document::new()
    };
    let xml = document.to_xml()?;
    extract_body_inner(&xml).map(str::to_owned)
}

pub(super) fn section_property_xml(
    section: &oxoffice_docx_oxml::document::CT_SectPr,
) -> Result<String> {
    let mut bytes = Vec::new();
    section.to_xml(&mut Writer::new(&mut bytes))?;
    String::from_utf8(bytes).map_err(utf8_error)
}

/// The direct Word body and its effective inherited/local namespace scope.
pub(super) fn main_body_source(document: &[u8]) -> Result<(Range<usize>, NamespaceBindings)> {
    let mut reader = NsReader::from_reader(document);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut body = None;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison body scan failed: {error}")))?;
        let word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        match event {
            Event::Start(element) => {
                if depth == 0 && !(word && element.local_name().as_ref() == b"document") {
                    return Err(Error::Other(
                        "comparison main source has no Word document root".into(),
                    ));
                }
                if depth == 1 && word && element.local_name().as_ref() == b"body" {
                    let bindings = reader
                        .resolver()
                        .bindings()
                        .map(|(prefix, namespace)| {
                            let name = match prefix {
                                quick_xml::name::PrefixDeclaration::Default => "xmlns".to_owned(),
                                quick_xml::name::PrefixDeclaration::Named(prefix) => format!(
                                    "xmlns:{}",
                                    std::str::from_utf8(prefix).map_err(utf8_error)?
                                ),
                            };
                            Ok((
                                name,
                                std::str::from_utf8(namespace.as_ref())
                                    .map_err(utf8_error)?
                                    .to_owned(),
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    body = Some((reader.buffer_position() as usize, bindings));
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::Other("comparison body has an unmatched close".into()))?;
                if depth == 1
                    && let Some((start, bindings)) = body.take()
                {
                    return Ok((start..before, bindings));
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "comparison source has no expanded Word body".into(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn replace_body_inner(document: &[u8], body: &str) -> Result<String> {
    let xml = std::str::from_utf8(document).map_err(utf8_error)?;
    let (range, _) = main_body_source(document)?;
    Ok(format!(
        "{}{}{}",
        &xml[..range.start],
        body,
        &xml[range.end..]
    ))
}

pub(super) fn extract_body_inner(document: &[u8]) -> Result<&str> {
    let xml = std::str::from_utf8(document).map_err(utf8_error)?;
    let (range, _) = main_body_source(document)?;
    Ok(&xml[range])
}

pub(super) fn replace_element_inner(xml: &str, name: &str, inner: &str) -> Result<String> {
    let local = name.rsplit(':').next().unwrap_or(name);
    let spans = direct_word_element_spans(xml, local)?;
    let span = spans
        .first()
        .ok_or_else(|| Error::Other(format!("serialized XML has no Word {local} child")))?;
    let (opening, closing) = paragraph_shell(Some(&xml[span.clone()]))?;
    let mut output = xml.to_owned();
    output.replace_range(span.clone(), &format!("{opening}{inner}{closing}"));
    Ok(output)
}

pub(super) fn inject_before_close(xml: &str, closing: &str, addition: &str) -> Result<String> {
    let at = xml
        .rfind(closing)
        .ok_or_else(|| Error::Other(format!("serialized XML has no {closing}")))?;
    Ok(format!("{}{}{}", &xml[..at], addition, &xml[at..]))
}

pub(super) fn append_word_child(xml: &str, local: &str, addition: &str) -> Result<String> {
    if element_local_name(xml) != local {
        return Err(Error::Other(format!("serialized owner is not {local}")));
    }
    let (opening, closing) = paragraph_shell(Some(xml))?;
    if xml.trim_end().ends_with("/>") {
        return Ok(format!("{opening}{addition}{closing}"));
    }
    inject_before_close(xml, &closing, addition)
}

pub(super) fn with_mark_revision(run_properties: &str, marker: &str) -> Result<String> {
    const ORDER: [&str; 4] = ["ins", "del", "moveFrom", "moveTo"];
    let rank = |xml: &str| {
        ORDER
            .iter()
            .position(|local| *local == element_local_name(xml))
    };
    let children = direct_element_spans(run_properties)?;
    let Some(marker_rank) = rank(marker) else {
        return append_word_child(run_properties, "rPr", marker);
    };
    let open = element_opening(run_properties)?.0;
    if children.is_empty() {
        return append_word_child(run_properties, "rPr", marker);
    }
    let at = children
        .iter()
        .take_while(|span| {
            rank(&run_properties[(*span).clone()]).is_some_and(|rank| rank <= marker_rank)
        })
        .last()
        .map_or(open, |span| span.end);
    let mut marked = run_properties.to_owned();
    marked.insert_str(at, marker);
    Ok(marked)
}

/// The local name of the element that `xml` starts with.
pub(super) fn element_local_name(xml: &str) -> &str {
    let name_end = xml
        .find(|character: char| character.is_ascii_whitespace() || matches!(character, '/' | '>'))
        .unwrap_or(xml.len());
    xml.get(1..name_end)
        .and_then(|name| name.rsplit(':').next())
        .unwrap_or_default()
}

pub(super) fn direct_word_element_spans(xml: &str, local: &str) -> Result<Vec<Range<usize>>> {
    direct_word_element_spans_in_scope(xml, local, &vec![("xmlns:w".into(), W_NS.into())])
}

pub(super) fn direct_word_element_spans_in_scope(
    xml: &str,
    local: &str,
    scope: &NamespaceBindings,
) -> Result<Vec<Range<usize>>> {
    let mut root = BytesStart::new("scope");
    for (name, uri) in scope {
        root.push_attribute((name.as_str(), uri.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Start(root))
        .map_err(|error| Error::Other(error.to_string()))?;
    let opening = String::from_utf8(writer.into_inner()).map_err(utf8_error)?;
    let wrapped = format!("{opening}{xml}</scope>");
    let offset = opening.len();
    let mut reader = NsReader::from_reader(wrapped.as_bytes());
    reader.config_mut().trim_text(false);
    let mut spans = Vec::new();
    let mut open = Vec::new();
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let target =
                    depth == 2 && word && element.local_name().as_ref() == local.as_bytes();
                open.push(target.then_some(before));
                depth += 1;
            }
            Event::Empty(element) => {
                if depth == 2 && word && element.local_name().as_ref() == local.as_bytes() {
                    spans.push(before.saturating_sub(offset)..after.saturating_sub(offset));
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if let Some(Some(start)) = open.pop() {
                    spans.push(start.saturating_sub(offset)..after.saturating_sub(offset));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(spans)
}

pub(super) fn paragraph_close_start(xml: &str) -> Result<usize> {
    let (_, closing) = paragraph_shell(Some(xml))?;
    xml.rfind(&closing)
        .ok_or_else(|| Error::Other("serialized owner has no end".to_owned()))
}

pub(super) fn paragraph_run_spans(xml: &str) -> Result<Vec<Range<usize>>> {
    let open = format!(r#"<rdocxcmp:root xmlns:rdocxcmp="urn:rdocx-compare" xmlns:w="{W_NS}">"#);
    let wrapped = format!("{open}{xml}</rdocxcmp:root>");
    let offset = open.len();
    let mut reader = NsReader::from_reader(wrapped.as_bytes());
    reader.config_mut().trim_text(false);
    let mut spans = Vec::new();
    let mut stack = Vec::<(Vec<u8>, bool, Option<usize>)>::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let is_word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = element.local_name().as_ref().to_vec();
                let parent = stack
                    .last()
                    .map(|(local, word, _)| (local.as_slice(), *word));
                let target = is_word
                    && matches!(local.as_slice(), b"r" | b"fldSimple")
                    && parent.is_some_and(|(value, word)| {
                        word && ((value == b"p" && stack.len() == 2)
                            || (value == b"hyperlink"
                                && stack.len() == 3
                                && local.as_slice() == b"r"))
                    });
                stack.push((local, is_word, target.then_some(before)));
            }
            Event::Empty(element) => {
                let local = element.local_name();
                let parent = stack
                    .last()
                    .map(|(local, word, _)| (local.as_slice(), *word));
                if is_word
                    && local.as_ref() == b"fldSimple"
                    && parent.is_some_and(|(value, word)| word && value == b"p" && stack.len() == 2)
                {
                    spans.push(before.saturating_sub(offset)..after.saturating_sub(offset));
                }
            }
            Event::End(_) => {
                if let Some((_, _, Some(start))) = stack.pop() {
                    spans.push(start.saturating_sub(offset)..after.saturating_sub(offset));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    spans.sort_by_key(|span| span.start);
    Ok(spans)
}

pub(super) fn replace_paragraph_run_elements(
    paragraph: &CT_P,
    xml: &str,
    replacements: &[String],
) -> Result<String> {
    let spans = modeled_paragraph_run_spans(paragraph, xml)?;
    if spans.len() != replacements.len() {
        return Err(Error::Other(format!(
            "comparison expected {} paragraph run owners, found {}",
            replacements.len(),
            spans.len()
        )));
    }
    let mut output = xml.to_owned();
    for (span, replacement) in spans.into_iter().zip(replacements).rev() {
        output.replace_range(span, replacement);
    }
    Ok(output)
}

pub(super) fn modeled_paragraph_run_spans(
    paragraph: &CT_P,
    xml: &str,
) -> Result<Vec<Range<usize>>> {
    let physical = paragraph_run_spans(xml)?;
    let (opening, closing) = paragraph_shell(Some(xml))?;
    let mut projected = Vec::with_capacity(paragraph.runs.len());
    let mut cursor = 0usize;
    let mut previous_field_owner = None;
    let mut modeled_index = 0;
    while modeled_index < paragraph.runs.len() {
        if let Some((coverage, _)) = paragraph.field_source_run_fragments_at(modeled_index)? {
            let mut sources = Vec::new();
            let actual_coverage =
                paragraph.field_source_replacements_at(modeled_index, &mut sources)?;
            if actual_coverage != coverage || sources.len() != 1 {
                return Err(Error::Other(
                    "shared field source coverage changed during correlation".into(),
                ));
            }
            let raw = std::str::from_utf8(&sources[0].0).map_err(utf8_error)?;
            if let Some(at) = physical.get(cursor).map(|span| span.start)
                && let Some(relative) = xml[at..].find(raw)
            {
                let start = at + relative;
                let end = start + raw.len();
                let count = physical[cursor..]
                    .iter()
                    .take_while(|span| span.start < end)
                    .count();
                if count > 0
                    && physical[cursor].start >= start
                    && physical[cursor + count - 1].end <= end
                {
                    projected.push(start..end);
                    projected.extend(std::iter::repeat_n(end..end, coverage - 1));
                    cursor += count;
                    modeled_index += coverage;
                    previous_field_owner = None;
                    continue;
                }
            }
        }
        let run = &paragraph.runs[modeled_index];
        let (physical_count, field_owner) = match run.content.as_slice() {
            [RunContent::Field(field)] if field.is_complex() => {
                if let Some(source) = field.detached_source()? {
                    let source = String::from_utf8(source).map_err(utf8_error)?;
                    let count = paragraph_run_spans(&format!("{opening}{source}{closing}"))?.len();
                    if count == 0 || cursor + count > physical.len() {
                        return Err(Error::Other(format!(
                            "comparison could not correlate split field run {modeled_index}"
                        )));
                    }
                    projected.push(physical[cursor].start..physical[cursor + count - 1].end);
                    cursor += count;
                    modeled_index += 1;
                    previous_field_owner = None;
                    continue;
                }
                let field_owner = field.source_owner_id().ok_or_else(|| {
                    Error::Other("parsed complex field has no source owner".to_owned())
                })?;
                if previous_field_owner == Some(field_owner) {
                    let end = physical[cursor - 1].end;
                    projected.push(end..end);
                    modeled_index += 1;
                    continue;
                }
                let (source, _) = field.source_replacement()?.ok_or_else(|| {
                    Error::Other("parsed complex field has no source ownership".to_owned())
                })?;
                let source = std::str::from_utf8(source).map_err(|error| {
                    Error::Other(format!("complex field source is not UTF-8: {error}"))
                })?;
                (
                    paragraph_run_spans(&format!("{opening}{source}{closing}"))?.len(),
                    Some(field_owner),
                )
            }
            _ => (1, None),
        };
        if physical_count == 0 || cursor + physical_count > physical.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate paragraph run owner {modeled_index}: \
                 {physical_count} physical runs from cursor {cursor}, {} available",
                physical.len()
            )));
        }
        projected.push(physical[cursor].start..physical[cursor + physical_count - 1].end);
        cursor += physical_count;
        previous_field_owner = field_owner;
        modeled_index += 1;
    }
    if cursor != physical.len() {
        return Err(Error::Other(format!(
            "comparison correlated {cursor} of {} physical paragraph runs",
            physical.len()
        )));
    }
    Ok(projected)
}

pub(super) fn replace_direct_word_elements(
    xml: &str,
    local: &str,
    replacements: &[String],
) -> Result<String> {
    let spans = direct_word_element_spans(xml, local)?;
    if spans.len() != replacements.len() {
        return Err(Error::Other(format!(
            "comparison expected {} direct w:{local} elements, found {}",
            replacements.len(),
            spans.len()
        )));
    }
    let mut output = xml.to_owned();
    for (span, replacement) in spans.into_iter().zip(replacements).rev() {
        output.replace_range(span, replacement);
    }
    Ok(output)
}

pub(super) fn cell_child_spans(
    cell: &CT_Tc,
    xml: &str,
    location: &str,
) -> Result<Vec<(String, Range<usize>)>> {
    let candidates = direct_modeled_child_spans(xml)?;
    let mut spans = Vec::with_capacity(cell.content.len());
    let mut candidate_index = 0usize;
    for (index, child) in cell.content.iter().enumerate() {
        for (_, raw) in cell.extra_xml.iter().filter(|(at, _)| *at == index) {
            let raw = std::str::from_utf8(raw).map_err(utf8_error)?;
            if candidates
                .get(candidate_index)
                .is_some_and(|(_, span)| &xml[span.clone()] == raw)
            {
                candidate_index += 1;
            }
        }
        let expected_name = match child {
            CellContent::Paragraph(_) => "w:p",
            CellContent::Table(_) => "w:tbl",
            CellContent::ContentControl(_) => "w:sdt",
        };
        let (name, span) = candidates.get(candidate_index).ok_or_else(|| {
            Error::Other(format!(
                "comparison could not correlate serialized cell content at {location}"
            ))
        })?;
        if name != expected_name {
            return Err(Error::Other(format!(
                "comparison found {name} instead of {expected_name} at {location}"
            )));
        }
        spans.push((name.clone(), span.clone()));
        candidate_index += 1;
    }
    Ok(spans)
}

fn direct_modeled_child_spans(xml: &str) -> Result<Vec<(String, Range<usize>)>> {
    let mut spans = Vec::new();
    for local in ["p", "tbl", "sdt"] {
        spans.extend(
            direct_word_element_spans(xml, local)?
                .into_iter()
                .map(|span| (format!("w:{local}"), span)),
        );
    }
    spans.sort_by_key(|(_, span)| span.start);
    Ok(spans)
}

pub(super) fn replace_ranges(
    xml: &str,
    spans: &[(String, Range<usize>)],
    replacements: &[String],
) -> Result<String> {
    if spans.len() != replacements.len() {
        return Err(Error::Other(
            "comparison replacement count does not match modeled children".to_owned(),
        ));
    }
    let mut output = xml.to_owned();
    for ((_, span), replacement) in spans.iter().zip(replacements).rev() {
        output.replace_range(span.clone(), replacement);
    }
    Ok(output)
}

pub(super) fn word_ids(xml: &[u8]) -> Result<HashSet<i32>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut ids = HashSet::new();
    let mut buffer = Vec::new();
    loop {
        let (_, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    if local.as_ref() == b"id"
                        && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                        && let Ok(value) = std::str::from_utf8(&attribute.value)
                        && let Ok(id) = value.parse::<i32>()
                    {
                        ids.insert(id);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(ids)
}

pub(super) fn utf8_error(error: impl std::fmt::Display) -> Error {
    Error::Other(format!("comparison XML is not UTF-8: {error}"))
}
