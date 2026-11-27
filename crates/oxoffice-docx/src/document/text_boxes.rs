//! Text boxes and their alternate content copies, kept paired.

use super::*;

/// Where the first `txbxContent` element inside the element that starts at
/// `start` begins, whatever its prefix.
pub(super) fn first_text_box_start(xml: &[u8], start: usize) -> Result<Option<usize>> {
    let fragment = xml
        .get(start..)
        .ok_or_else(|| Error::Other("story element begins outside its source part".to_owned()))?;
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(fragment);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = start + reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story text box scan failed: {error}")))?
        {
            Event::Start(element) | Event::Empty(element)
                if depth > 0 && element.local_name().as_ref() == b"txbxContent" =>
            {
                return Ok(Some(before));
            }
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => return Ok(None),
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(None);
                }
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn scoped_alternate_text_box_copies(xml: &[u8]) -> Result<Vec<PairedTextBoxCopy>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut copies = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("alternate text-box scan failed: {error}")))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let namespace = story_namespace(&namespace);
                let local = local.as_ref();
                // Inspect both representations using the same qualified structural
                // routes as story discovery, without selecting or rewriting Fallback.
                let alternate_branch = namespace == StoryNamespace::MarkupCompatibility
                    && matches!(local, b"Choice" | b"Fallback")
                    && stack.last().is_some_and(|parent| {
                        parent.namespace == StoryNamespace::MarkupCompatibility
                            && parent.local_name.as_ref() == b"AlternateContent"
                    });
                let branch_content = namespace == StoryNamespace::Word
                    && matches!(local, b"drawing" | b"pict")
                    && stack.last().is_some_and(|parent| {
                        parent.namespace == StoryNamespace::MarkupCompatibility
                            && matches!(parent.local_name.as_ref(), b"Choice" | b"Fallback")
                    });
                let modeled = stack.last().is_none_or(|parent| !parent.opaque)
                    && (alternate_branch
                        || branch_content
                        || modeled_story_child(&stack, namespace, local))
                    && admit_story_sdt_content_slot(&mut stack, namespace, local, empty);
                if modeled && namespace == StoryNamespace::Word && local == b"txbxContent" {
                    let end = if empty {
                        reader.buffer_position() as usize
                    } else {
                        story_element_end(xml, before)?
                    };
                    for (index, alternate) in stack.iter().enumerate() {
                        if alternate.namespace != StoryNamespace::MarkupCompatibility
                            || alternate.local_name.as_ref() != b"AlternateContent"
                            || alternate.opaque
                        {
                            continue;
                        }
                        if let Some(branch) = stack.get(index + 1)
                            && branch.namespace == StoryNamespace::MarkupCompatibility
                            && matches!(branch.local_name.as_ref(), b"Choice" | b"Fallback")
                        {
                            copies.push(PairedTextBoxCopy {
                                alternate: alternate.full_start,
                                branch: branch.full_start,
                                range: before..end,
                            });
                        }
                    }
                }
                if !empty {
                    let sdt_context = story_element_sdt_context(&stack, namespace, local);
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: story_frame_local_name(local),
                        is_word: namespace == StoryNamespace::Word,
                        full_start: before,
                        owner_kind: None,
                        item_kind: None,
                        direct_owner_child: false,
                        sdt_context,
                        word_prefixes: Arc::clone(&empty_word_prefixes),
                        opaque: !modeled,
                        typed_sdt_content_seen: false,
                    });
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(copies)
}

pub(super) fn paired_text_box_projection(
    xml: &[u8],
    range: Range<usize>,
    mut operation: Option<&mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>>,
) -> Result<PairedTextBoxProjection> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut paths = Vec::<Option<Vec<usize>>>::new();
    let mut children = Vec::<usize>::new();
    let mut signature = Vec::new();
    let mut slots = Vec::new();
    let mut active = false;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("paired text-box projection failed: {error}")))?;
        let after = reader.buffer_position() as usize;
        if !active {
            if before != range.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let namespace = story_namespace(&namespace);
                let local = local.as_ref();
                let inherited = if stack.is_empty() {
                    Arc::new(
                        story_namespace_scope_at(xml, before)?
                            .into_iter()
                            .filter(|(_, uri)| uri == oxoffice_docx_oxml::namespace::W_NS)
                            .map(|(prefix, _)| prefix)
                            .collect::<Vec<_>>(),
                    )
                } else {
                    stack.last().unwrap().word_prefixes.clone()
                };
                let word_prefixes = story_word_prefixes_at(
                    &element,
                    &inherited,
                    namespace == StoryNamespace::Word,
                )?;
                let modeled = !stack.last().is_some_and(|frame| frame.opaque)
                    && modeled_story_event(
                        &mut stack, namespace, local, &element, empty, xml, before, after,
                    )?;
                if !stack.is_empty()
                    && modeled
                    && ((namespace == StoryNamespace::Word && local == b"txbxContent")
                        || namespace == StoryNamespace::MarkupCompatibility)
                {
                    return Err(Error::Other(
                        "nested alternate/text-box correspondence is unsupported".into(),
                    ));
                }
                let structural = modeled
                    && namespace == StoryNamespace::Word
                    && matches!(
                        local,
                        b"txbxContent" | b"p" | b"tbl" | b"tr" | b"tc" | b"sdt" | b"sdtContent"
                    );
                let path = if structural {
                    let parent = paths.iter().rposition(Option::is_some);
                    let mut path = parent.and_then(|i| paths[i].clone()).unwrap_or_default();
                    if let Some(parent) = parent {
                        path.push(children[parent]);
                        children[parent] += 1;
                    }
                    let end = if empty {
                        after
                    } else {
                        story_element_end(xml, before)?
                    };
                    slots.push((path.clone(), before..end));
                    signature.push(format!("+{}", String::from_utf8_lossy(local)));
                    if local == b"tbl" {
                        let scope = story_namespace_scope_at(xml, before)?;
                        let parsed = if let Some(operation) = operation.as_deref_mut() {
                            operation.document_block(before..end, &scope)?
                        } else {
                            let closed =
                                close_content_fragment_namespaces(&xml[before..end], &scope)?;
                            let mut wrapped =
                                format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#)
                                    .into_bytes();
                            wrapped.extend_from_slice(&closed);
                            wrapped.extend_from_slice(b"</w:body></w:document>");
                            CT_Document::from_xml(&wrapped)?
                        };
                        let Some(BodyContent::Table(table)) = parsed.body.content.first() else {
                            return Err(Error::Other(
                                "paired table is not a qualified table".into(),
                            ));
                        };
                        validate_table_topology(table)?;
                        let columns = table.grid.as_ref().map_or(0, |grid| grid.columns.len());
                        signature.push(format!("grid:{columns}"));
                        for row in &table.rows {
                            let ranges = crate::table::row_cell_ranges(row, columns)?;
                            signature.push(format!("row:{:?}", ranges));
                            signature.push(format!(
                                "omissions:{:?}:{:?}",
                                row.properties.as_ref().and_then(|p| p.grid_before),
                                row.properties.as_ref().and_then(|p| p.grid_after)
                            ));
                            for cell in &row.cells {
                                signature.push(format!(
                                    "merge:{:?}:{:?}",
                                    cell.properties.as_ref().and_then(|p| p.v_merge),
                                    cell.properties.as_ref().and_then(|p| p.h_merge.as_ref())
                                ));
                            }
                        }
                    }
                    if local == b"p" {
                        let scope = story_namespace_scope_at(xml, before)?;
                        let paragraph = if let Some(operation) = operation.as_deref_mut() {
                            operation.paragraph(before..end, &scope)?
                        } else {
                            let closed =
                                close_content_fragment_namespaces(&xml[before..end], &scope)?;
                            CT_P::from_xml_fragment(&closed)?
                        };
                        signature.push(paragraph.accepted_text());
                    }
                    Some(path)
                } else {
                    None
                };
                if modeled
                    && namespace == StoryNamespace::Word
                    && matches!(
                        local,
                        b"gridCol"
                            | b"gridBefore"
                            | b"gridAfter"
                            | b"gridSpan"
                            | b"vMerge"
                            | b"hMerge"
                    )
                {
                    let mut fact = String::from_utf8_lossy(local).into_owned();
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (attribute_namespace, attribute_local) =
                            reader.resolver().resolve_attribute(attribute.key);
                        if word_element(&attribute_namespace) && attribute_local.as_ref() == b"val"
                        {
                            fact.push(':');
                            fact.push_str(
                                &attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .map_err(|error| Error::Other(error.to_string()))?,
                            );
                        }
                    }
                    signature.push(fact);
                }
                if empty {
                    if structural {
                        signature.push("-".into());
                    }
                } else {
                    let sdt_context = story_element_sdt_context(&stack, namespace, local);
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: story_frame_local_name(local),
                        is_word: namespace == StoryNamespace::Word,
                        full_start: before,
                        owner_kind: None,
                        item_kind: None,
                        direct_owner_child: false,
                        sdt_context,
                        word_prefixes,
                        opaque: !modeled,
                        typed_sdt_content_seen: false,
                    });
                    paths.push(path);
                    children.push(0);
                }
            }
            Event::End(_) => {
                if paths.pop().flatten().is_some() {
                    signature.push("-".into());
                }
                children.pop();
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        if after == range.end {
            return Ok(PairedTextBoxProjection { signature, slots });
        }
        buffer.clear();
    }
    Err(Error::Other("paired text-box source is incomplete".into()))
}
