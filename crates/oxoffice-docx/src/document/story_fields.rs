//! Complex fields of a story item: their XML and run shells, and how story events are modeled and admitted.

use super::*;

pub(super) fn complex_story_field_xml(xml: &[u8], item: &StoryItemSpan) -> Result<Vec<u8>> {
    let scope = story_namespace_scope_at(xml, item.scan.start)?;
    let word_prefix = story_word_prefix_for_scope(&scope);
    let mut bindings = scope;
    bindings
        .entry(word_prefix.clone())
        .or_insert_with(|| WORD_NAMESPACE.to_owned());
    let qualified_paragraph = if word_prefix.is_empty() {
        "p".to_owned()
    } else {
        format!("{word_prefix}:p")
    };
    let mut projection = format!("<{qualified_paragraph}").into_bytes();
    for (prefix, namespace) in bindings {
        projection.extend_from_slice(b" xmlns");
        if !prefix.is_empty() {
            projection.push(b':');
            projection.extend_from_slice(prefix.as_bytes());
        }
        projection.extend_from_slice(b"=\"");
        projection.extend_from_slice(quick_xml::escape::escape(&namespace).as_bytes());
        projection.push(b'"');
    }
    projection.push(b'>');
    projection.extend_from_slice(&complex_story_field_runs(xml, item)?);
    projection.extend_from_slice(format!("</{qualified_paragraph}>").as_bytes());
    Ok(projection)
}

struct StoryRunShell {
    open_end: usize,
    close_start: usize,
    close_end: usize,
}

fn story_run_shell(xml: &[u8], start: usize) -> Result<StoryRunShell> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut active = false;
    let mut depth = 0usize;
    let mut open_end = None;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (_, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story run shell scan failed: {error}")))?;
        let after = reader.buffer_position() as usize;
        if !active {
            if before != start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            if !matches!(event, Event::Start(_)) {
                return Err(Error::Other(
                    "complex story field does not begin in a run".to_owned(),
                ));
            }
            active = true;
            depth = 1;
            open_end = Some(after);
        } else {
            match event {
                Event::Start(_) => depth += 1,
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(StoryRunShell {
                            open_end: open_end.unwrap_or(after),
                            close_start: before,
                            close_end: after,
                        });
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        buffer.clear();
    }
    Err(Error::Other(
        "complex story field run has no matching close".to_owned(),
    ))
}

fn complex_story_field_runs(xml: &[u8], item: &StoryItemSpan) -> Result<Vec<u8>> {
    let first = story_run_shell(xml, item.scan.start)?;
    let mut last_start = item.scan.start;
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut active = false;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("complex story run scan failed: {error}")))?;
        let is_word_namespace = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != item.scan.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(element) => {
                if depth == 0 && is_word_namespace && element.local_name().as_ref() == b"r" {
                    last_start = before;
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
        if after == item.scan.end {
            break;
        }
        buffer.clear();
    }
    let last = story_run_shell(xml, last_start)?;
    let mut result = Vec::new();
    if last_start == item.scan.start {
        result.extend_from_slice(&xml[item.scan.start..first.open_end]);
        result.extend_from_slice(&xml[item.full.clone()]);
        result.extend_from_slice(&xml[first.close_start..first.close_end]);
        return Ok(result);
    }
    result.extend_from_slice(&xml[item.scan.start..first.open_end]);
    result.extend_from_slice(&xml[item.full.start..first.close_end]);
    result.extend_from_slice(&xml[first.close_end..last_start]);
    result.extend_from_slice(&xml[last_start..last.open_end]);
    result.extend_from_slice(&xml[last.open_end..item.full.end]);
    result.extend_from_slice(&xml[last.close_start..last.close_end]);
    Ok(result)
}

// Match CT_Sdt's actual first nonempty content slot. Empty content and
// subsequent content are retained raw, never visible modeled descendants.
pub(super) fn admit_story_sdt_content_slot(
    stack: &mut [XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
    empty: bool,
) -> bool {
    if namespace != StoryNamespace::Word || local_name != b"sdtContent" {
        return true;
    }
    let Some(parent) = stack.last_mut() else {
        return true;
    };
    if parent.namespace != StoryNamespace::Word || parent.local_name.as_ref() != b"sdt" {
        return true;
    }
    if empty || parent.typed_sdt_content_seen {
        return false;
    }
    parent.typed_sdt_content_seen = true;
    true
}

pub(super) fn modeled_story_event(
    stack: &mut [XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
    element: &BytesStart<'_>,
    empty: bool,
    xml: &[u8],
    before: usize,
    after: usize,
) -> Result<bool> {
    if !modeled_story_child(stack, namespace, local_name) {
        return Ok(false);
    }
    if !admit_story_sdt_content_slot(stack, namespace, local_name, empty) {
        return Ok(false);
    }
    if namespace == StoryNamespace::MarkupCompatibility && local_name == b"Choice" {
        // The text box is read from the first Choice that holds one, as layout
        // draws the first Choice drawing. Any other Choice stays opaque, as the
        // Fallback does.
        let Some(alternate) = stack.last() else {
            return Ok(false);
        };
        let Some(text_box) = first_text_box_start(xml, alternate.full_start)? else {
            return Ok(false);
        };
        return Ok(!empty && before < text_box && text_box < story_element_end(xml, before)?);
    }
    if namespace != StoryNamespace::Word {
        return Ok(true);
    }
    if matches!(
        local_name,
        b"sdt" | b"ins" | b"del" | b"moveFrom" | b"moveTo" | b"fldSimple"
    ) {
        let inherited = stack
            .last()
            .map(|frame| Arc::clone(&frame.word_prefixes))
            .unwrap_or_else(|| Arc::new(Vec::new()));
        let word_prefixes = story_word_prefixes_at(element, &inherited, true)?;
        let raw_end = if empty {
            after
        } else {
            story_element_end(xml, before)?
        };
        let raw = xml
            .get(before..raw_end)
            .ok_or_else(|| Error::Other("story element lies outside its source part".to_owned()))?;
        if local_name == b"sdt" {
            let owner = match story_sdt_context(stack) {
                Some(StorySdtContext::Block) => StorySdtOwner::Block,
                Some(StorySdtContext::Table) => StorySdtOwner::Table,
                Some(StorySdtContext::Row) => StorySdtOwner::Row,
                Some(StorySdtContext::Inline) => StorySdtOwner::Inline,
                None => return Ok(false),
            };
            return Ok(CT_Sdt::story_raw_is_typed(raw, &word_prefixes, owner));
        }
        if matches!(local_name, b"ins" | b"del" | b"moveFrom" | b"moveTo") {
            return Ok(CT_Revision::story_raw_is_typed(raw, &word_prefixes));
        }
        return Ok(!empty && story_simple_field_is_typed(raw, &word_prefixes));
    }
    if empty && matches!(local_name, b"drawing" | b"pict" | b"sdt" | b"hyperlink") {
        return Ok(false);
    }
    Ok(true)
}
