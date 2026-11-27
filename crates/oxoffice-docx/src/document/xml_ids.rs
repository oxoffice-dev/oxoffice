//! Relationship and drawing ids in raw Word XML: scanning, rewriting and remapping them.

use super::*;

/// Every attribute value of an XML part. A relationship named by `r:embed`,
/// `r:id`, `o:relid` or any other producer attribute counts as referenced.
pub(super) fn xml_attribute_values(xml: &[u8]) -> Result<HashSet<String>> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut values = HashSet::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute
                        .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
                    values.insert(value.into_owned());
                }
            }
            Event::Eof => return Ok(values),
            _ => {}
        }
        buffer.clear();
    }
}

pub(crate) fn xml_relationship_ids_in_order(xml: &[u8]) -> Result<Vec<String>> {
    xml_relationship_ids_in_order_with_bindings(xml, &[])
}

pub(super) fn authored_relationship_ids_in_source_order(
    xml: &[u8],
    relationship_ids: &HashSet<String>,
) -> Option<Vec<String>> {
    let mut seen = HashSet::new();
    let all = xml_relationship_ids_in_order(xml).ok()?;
    let ordered = all
        .iter()
        .filter(|id| relationship_ids.contains(*id) && seen.insert((*id).clone()))
        .cloned()
        .collect::<Vec<_>>();
    (ordered.len() == relationship_ids.len()).then_some(ordered)
}

pub(super) fn xml_relationship_ids_in_order_with_bindings(
    xml: &[u8],
    inherited_bindings: &[(String, String)],
) -> Result<Vec<String>> {
    xml_relationship_ids_in_collection_range(xml, inherited_bindings, None)
}

/// First-seen relationship ids. `collect` `None` is the whole part.
///
/// Every relationship attribute is parsed and decoded before the range filter.
/// A malformed attribute outside `collect` is still an error. Only an attribute
/// whose event lies entirely inside the range is collected.
pub(super) fn xml_relationship_ids_in_collection_range(
    xml: &[u8],
    inherited_bindings: &[(String, String)],
    collect: Option<Range<usize>>,
) -> Result<Vec<String>> {
    if let Some(range) = collect.as_ref()
        && (range.start > range.end || range.end > xml.len())
    {
        return Err(Error::Other(
            "invalid story XML: relationship collection range".to_owned(),
        ));
    }
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut ids = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (_, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute
                        .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
                    let (namespace, _) = reader.resolver().resolve_attribute(attribute.key);
                    let inherited_relationship_prefix =
                        matches!(namespace, ResolveResult::Unknown(_))
                            && attribute.key.as_ref().contains(&b':')
                            && attribute
                                .key
                                .as_ref()
                                .split(|byte| *byte == b':')
                                .next()
                                .and_then(|prefix| std::str::from_utf8(prefix).ok())
                                .is_some_and(|prefix| {
                                    let declaration = format!("xmlns:{prefix}");
                                    inherited_bindings.iter().any(|(name, value)| {
                                        name == &declaration && value == drawing_ns::R
                                    })
                                });
                    if matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == drawing_ns::R.as_bytes())
                        || inherited_relationship_prefix
                    {
                        let id = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map_err(|error| {
                                Error::Other(format!("invalid story relationship id: {error}"))
                            })?
                            .into_owned();
                        let inside = collect
                            .as_ref()
                            .is_none_or(|range| before >= range.start && after <= range.end);
                        if inside && !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                }
            }
            Event::Eof => return Ok(ids),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn rewrite_authored_doc_pr_ids(
    xml: &[u8],
    occupied: &mut HashSet<u32>,
) -> Result<Vec<u8>> {
    let edits = authored_doc_pr_edits(xml, occupied)?;
    let mut updated = xml.to_vec();
    for (start, end, replacement) in edits.into_iter().rev() {
        updated.splice(start..end, replacement);
    }
    Ok(updated)
}

pub(super) fn authored_doc_pr_edits(
    xml: &[u8],
    occupied: &mut HashSet<u32>,
) -> Result<Vec<(usize, usize, Vec<u8>)>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut edits = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let namespace = reader.resolver().resolve_element(element.name()).0;
                let is_doc_pr = matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == drawing_ns::WP.as_bytes())
                    && element.local_name().as_ref() == b"docPr";
                let mut replaced = false;
                for attribute in element.attributes() {
                    let attribute = attribute
                        .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
                    let (attribute_namespace, local) =
                        reader.resolver().resolve_attribute(attribute.key);
                    if is_doc_pr
                        && matches!(attribute_namespace, ResolveResult::Unbound)
                        && local.as_ref() == b"id"
                    {
                        let id = reserve_u32(occupied, 1, "drawing")?;
                        let Some((start, end)) =
                            story_attribute_value_span(&xml[before..after], attribute.key.as_ref())
                        else {
                            return Err(Error::Other(
                                "story drawing id source was not found".to_owned(),
                            ));
                        };
                        edits.push((before + start, before + end, id.to_string().into_bytes()));
                        replaced = true;
                    }
                }
                if is_doc_pr && !replaced {
                    return Err(Error::Other(
                        "wp:docPr requires an unqualified id attribute".to_owned(),
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    edits.sort_by_key(|(start, _, _)| *start);
    Ok(edits)
}

pub(crate) fn apply_template_source_edits(
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    source: oxoffice_docx_oxml::document::ScopedWordSource,
    edits: Vec<(usize, usize, Vec<u8>)>,
) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
    if edits.is_empty() {
        return Ok(source);
    }
    apply_borrowed_template_source_edits(operation, &source, edits)
}

pub(super) fn apply_borrowed_template_source_edits(
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    source: &oxoffice_docx_oxml::document::ScopedWordSource,
    mut edits: Vec<(usize, usize, Vec<u8>)>,
) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
    edits.sort_by_key(|(start, _, _)| *start);
    let mut pieces = Vec::new();
    let mut at = 0;
    for (start, end, replacement) in &edits {
        if *start < at || start > end || *end > source.as_bytes().len() {
            return Err(Error::Other("invalid template source edit ranges".into()));
        }
        pieces.push((Some(source), at..*start, &[][..]));
        pieces.push((None, 0..0, replacement.as_slice()));
        at = *end;
    }
    pieces.push((Some(source), at..source.as_bytes().len(), &[][..]));
    Ok(operation.compose_sources(&pieces)?)
}

/// Outermost drawings in order. A scan error keeps the drawings completed
/// before it and does not turn that error into an empty success.
pub(super) fn drawing_occurrences(xml: &[u8]) -> (Vec<DrawingOccurrence>, Option<Error>) {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut drawings = Vec::new();
    let mut depth = 0usize;
    let mut current_depth = None;
    let mut current_doc_pr = None;
    let mut current_embeds = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = match reader.read_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return (
                    drawings,
                    Some(Error::Other(format!("invalid story XML: {error}"))),
                );
            }
        };
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let namespace = reader.resolver().resolve_element(element.name()).0;
                let local = element.local_name();
                let word_drawing = matches!(namespace, ResolveResult::Bound(Namespace(value)) if value == drawing_ns::WP.as_bytes());
                let drawing = matches!(namespace, ResolveResult::Bound(Namespace(value)) if value == drawing_ns::A.as_bytes());
                if matches!(&event, Event::Start(_))
                    && word_drawing
                    && matches!(local.as_ref(), b"inline" | b"anchor")
                    && current_depth.is_none()
                {
                    current_depth = Some(depth);
                }
                if current_depth.is_some() {
                    if word_drawing && local.as_ref() == b"docPr" {
                        for attribute in element.attributes() {
                            let attribute = match attribute {
                                Ok(attribute) => attribute,
                                Err(error) => {
                                    return (
                                        drawings,
                                        Some(Error::Other(format!("invalid story XML: {error}"))),
                                    );
                                }
                            };
                            let (namespace, local) =
                                reader.resolver().resolve_attribute(attribute.key);
                            if matches!(namespace, ResolveResult::Unbound)
                                && local.as_ref() == b"id"
                            {
                                let id = match attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .map_err(|error| {
                                        Error::Other(format!("invalid story drawing id: {error}"))
                                    })
                                    .and_then(|id| {
                                        id.parse::<u32>().map_err(|_| {
                                            Error::Other("invalid story drawing id".to_owned())
                                        })
                                    }) {
                                    Ok(id) => id,
                                    Err(error) => return (drawings, Some(error)),
                                };
                                let Some((start, end)) = story_attribute_value_span(
                                    &xml[before..after],
                                    attribute.key.as_ref(),
                                ) else {
                                    return (
                                        drawings,
                                        Some(Error::Other(
                                            "story drawing id source was not found".to_owned(),
                                        )),
                                    );
                                };
                                current_doc_pr = Some((id, before + start..before + end));
                            }
                        }
                    }
                    if drawing && local.as_ref() == b"blip" {
                        for attribute in element.attributes() {
                            let attribute = match attribute {
                                Ok(attribute) => attribute,
                                Err(error) => {
                                    return (
                                        drawings,
                                        Some(Error::Other(format!("invalid story XML: {error}"))),
                                    );
                                }
                            };
                            let (namespace, local) =
                                reader.resolver().resolve_attribute(attribute.key);
                            if matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == drawing_ns::R.as_bytes())
                                && local.as_ref() == b"embed"
                            {
                                let id = match attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .map_err(|error| {
                                        Error::Other(format!(
                                            "invalid story relationship id: {error}"
                                        ))
                                    }) {
                                    Ok(id) => id.into_owned(),
                                    Err(error) => return (drawings, Some(error)),
                                };
                                current_embeds.push(id);
                            }
                        }
                    }
                }
                if matches!(&event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(element) => {
                depth = depth.saturating_sub(1);
                let namespace = reader.resolver().resolve_element(element.name()).0;
                let word_drawing = matches!(namespace, ResolveResult::Bound(Namespace(value)) if value == drawing_ns::WP.as_bytes());
                if word_drawing
                    && matches!(element.local_name().as_ref(), b"inline" | b"anchor")
                    && current_depth == Some(depth)
                {
                    current_depth = None;
                    drawings.push(DrawingOccurrence {
                        doc_pr: current_doc_pr.take(),
                        embeds: std::mem::take(&mut current_embeds),
                    });
                }
            }
            Event::Eof => return (drawings, None),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn drawing_ids_in_xml(xml: &[u8]) -> Result<HashSet<u32>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut ids = HashSet::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                let namespace = reader.resolver().resolve_element(element.name()).0;
                if matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == drawing_ns::WP.as_bytes())
                    && element.local_name().as_ref() == b"docPr"
                {
                    for attribute in element.attributes() {
                        let attribute = attribute
                            .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
                        let (attribute_namespace, local) =
                            reader.resolver().resolve_attribute(attribute.key);
                        if matches!(attribute_namespace, ResolveResult::Unbound)
                            && local.as_ref() == b"id"
                        {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| {
                                    Error::Other(format!("invalid drawing id: {error}"))
                                })?
                                .parse::<u32>()
                                .map_err(|_| Error::Other("invalid drawing id".to_owned()))?;
                            ids.insert(value);
                        }
                    }
                }
            }
            Event::Eof => return Ok(ids),
            _ => {}
        }
        buffer.clear();
    }
}

/// Replayed bytes stay authoritative. Block proof is used only when it is the
/// same byte string.
pub(super) fn relationship_xml_from_replayed(
    replayed: Vec<u8>,
    blocked: Result<(Vec<u8>, Option<MainPartBlocks>)>,
) -> (Vec<u8>, Option<MainPartBlocks>) {
    match blocked {
        Ok((xml, Some(record))) if xml == replayed => (replayed, Some(record)),
        _ => (replayed, None),
    }
}

pub(super) fn relationship_ids_by_blocks(
    xml: &[u8],
    blocks: Option<&MainPartBlocks>,
    bindings: &[(String, String)],
) -> Result<Vec<String>> {
    if let Some(ids) = blocks.and_then(|blocks| blocks.proved_relationship_ids(xml, bindings)) {
        #[cfg(debug_assertions)]
        {
            let scanned = xml_relationship_ids_in_order_with_bindings(xml, bindings).ok();
            assert_eq!(
                scanned.as_deref(),
                Some(ids.as_slice()),
                "relationship proof disagrees with a whole scan"
            );
        }
        return Ok(ids);
    }
    xml_relationship_ids_in_order_with_bindings(xml, bindings)
}

pub(crate) fn remap_xml_relationship_ids(
    xml: &[u8],
    remap: &HashMap<String, String>,
) -> Result<Vec<u8>> {
    remap_xml_relationship_ids_with_bindings(xml, remap, &[])
}

pub(crate) fn uniquify_drawing_ids_in_xml(xml: &[u8]) -> Result<Vec<u8>> {
    let edits = unique_drawing_id_edits(xml)?;
    let mut updated = xml.to_vec();
    for (start, end, replacement) in edits.into_iter().rev() {
        updated.splice(start..end, replacement);
    }
    Ok(updated)
}

/// The byte range and value of each drawing's `wp:docPr` id in `xml`.
pub(crate) fn drawing_id_spans(xml: &[u8]) -> Result<Vec<(usize, usize, u32)>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut occurrences = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let namespace = reader.resolver().resolve_element(element.name()).0;
                if matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == drawing_ns::WP.as_bytes())
                    && element.local_name().as_ref() == b"docPr"
                {
                    for attribute in element.attributes() {
                        let attribute = attribute
                            .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
                        let (attribute_namespace, local) =
                            reader.resolver().resolve_attribute(attribute.key);
                        if matches!(attribute_namespace, ResolveResult::Unbound)
                            && local.as_ref() == b"id"
                        {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| {
                                    Error::Other(format!("invalid drawing id: {error}"))
                                })?
                                .parse::<u32>()
                                .map_err(|_| Error::Other("invalid drawing id".to_owned()))?;
                            let Some((start, end)) = story_attribute_value_span(
                                &xml[before..after],
                                attribute.key.as_ref(),
                            ) else {
                                return Err(Error::Other(
                                    "story drawing id source was not found".to_owned(),
                                ));
                            };
                            occurrences.push((before + start, before + end, value));
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    Ok(occurrences)
}

pub(crate) fn unique_drawing_id_edits(xml: &[u8]) -> Result<Vec<(usize, usize, Vec<u8>)>> {
    let occurrences = drawing_id_spans(xml)?;
    let mut next = occurrences
        .iter()
        .map(|(_, _, value)| *value)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| Error::Other("drawing id range is exhausted".to_owned()))?;
    let mut seen = HashSet::new();
    let mut edits = Vec::new();
    for (start, end, value) in occurrences {
        if seen.insert(value) {
            continue;
        }
        while !seen.insert(next) {
            next = next
                .checked_add(1)
                .ok_or_else(|| Error::Other("drawing id range is exhausted".to_owned()))?;
        }
        edits.push((start, end, next.to_string().into_bytes()));
        next = next
            .checked_add(1)
            .ok_or_else(|| Error::Other("drawing id range is exhausted".to_owned()))?;
    }
    Ok(edits)
}

pub(super) fn remap_xml_relationship_ids_with_bindings(
    xml: &[u8],
    remap: &HashMap<String, String>,
    inherited_bindings: &[(String, String)],
) -> Result<Vec<u8>> {
    if remap.is_empty() {
        return Ok(xml.to_vec());
    }
    let mut updated = xml.to_vec();
    for (start, end, value) in relationship_remap_edits(xml, remap, inherited_bindings)?
        .into_iter()
        .rev()
    {
        updated.splice(start..end, value);
    }
    Ok(updated)
}

pub(super) fn relationship_remap_edits(
    xml: &[u8],
    remap: &HashMap<String, String>,
    inherited_bindings: &[(String, String)],
) -> Result<Vec<(usize, usize, Vec<u8>)>> {
    if remap.is_empty() {
        return Ok(Vec::new());
    }
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut edits = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute
                        .map_err(|error| Error::Other(format!("invalid story XML: {error}")))?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .map_err(|error| {
                            Error::Other(format!("invalid story relationship id: {error}"))
                        })?;
                    let inherited_relationship_prefix =
                        matches!(namespace, ResolveResult::Unknown(_))
                            && attribute.key.as_ref().contains(&b':')
                            && attribute
                                .key
                                .as_ref()
                                .split(|byte| *byte == b':')
                                .next()
                                .and_then(|prefix| std::str::from_utf8(prefix).ok())
                                .is_some_and(|prefix| {
                                    let declaration = format!("xmlns:{prefix}");
                                    inherited_bindings.iter().any(|(name, value)| {
                                        name == &declaration && value == drawing_ns::R
                                    })
                                });
                    if (matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == drawing_ns::R.as_bytes())
                        || inherited_relationship_prefix)
                        && matches!(local.as_ref(), b"id" | b"embed" | b"link")
                        && let Some(updated) = remap.get(value.as_ref())
                    {
                        let Some((start, end)) =
                            story_attribute_value_span(&xml[before..after], attribute.key.as_ref())
                        else {
                            return Err(Error::Other(
                                "story relationship attribute source was not found".to_owned(),
                            ));
                        };
                        edits.push((before + start, before + end, updated.as_bytes().to_vec()));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    edits.sort_by_key(|(start, _, _)| *start);
    Ok(edits)
}

pub(super) fn story_attribute_value_span(
    element: &[u8],
    attribute_name: &[u8],
) -> Option<(usize, usize)> {
    let mut index = 1usize;
    while index < element.len() && !element[index].is_ascii_whitespace() {
        index += 1;
    }
    while index < element.len() {
        while index < element.len() && element[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= element.len() || matches!(element[index], b'>' | b'/') {
            return None;
        }
        let name_start = index;
        while index < element.len()
            && !element[index].is_ascii_whitespace()
            && element[index] != b'='
        {
            index += 1;
        }
        let name_end = index;
        while index < element.len() && element[index].is_ascii_whitespace() {
            index += 1;
        }
        if element.get(index) != Some(&b'=') {
            return None;
        }
        index += 1;
        while index < element.len() && element[index].is_ascii_whitespace() {
            index += 1;
        }
        let quote = *element.get(index)?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        index += 1;
        let value_start = index;
        while index < element.len() && element[index] != quote {
            index += 1;
        }
        let value_end = index;
        if &element[name_start..name_end] == attribute_name {
            return Some((value_start, value_end));
        }
        index += 1;
    }
    None
}
