//! Namespace scopes and items of a story at a position, and the content items an owner holds.

use super::*;

pub(crate) fn story_namespace_scope_at(
    xml: &[u8],
    offset: usize,
) -> Result<BTreeMap<String, String>> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut scopes = vec![BTreeMap::<String, String>::new()];
    let mut changed_frames = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story namespace scope scan failed: {error}")))?
        {
            Event::Start(element) => {
                let inherited = scopes.last().expect("initial namespace scope");
                let changed = changed_story_namespace_scope(inherited, &element)?;
                if before == offset {
                    return Ok(changed.unwrap_or_else(|| inherited.clone()));
                }
                changed_frames.push(changed.is_some());
                if let Some(scope) = changed {
                    scopes.push(scope);
                }
            }
            Event::Empty(element) => {
                if before == offset {
                    let inherited = scopes.last().expect("initial namespace scope");
                    return Ok(changed_story_namespace_scope(inherited, &element)?
                        .unwrap_or_else(|| inherited.clone()));
                }
            }
            Event::End(_) => {
                if changed_frames.pop() == Some(true) {
                    scopes.pop();
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "story item namespace scope was not found".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

// Pooled local declaration frames and exact target offsets form one lexical proof.
#[allow(clippy::type_complexity)]
pub(crate) fn story_namespace_ancestors_at(
    xml: &[u8],
    offsets: &[usize],
) -> Result<(Vec<Vec<(String, String)>>, HashMap<usize, Vec<usize>>)> {
    let requested = offsets.iter().copied().collect::<HashSet<_>>();
    if requested.len() != offsets.len() {
        return Err(Error::Other(
            "field-cache paragraphs share a canonical start".into(),
        ));
    }
    let mut frames = Vec::new();
    let mut found = HashMap::with_capacity(requested.len());
    let mut active = Vec::<Option<usize>>::new();
    let mut decoded_bytes = 0usize;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("field-cache ancestor scan failed: {error}")))?;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                // Snapshot BEFORE the target's own declarations. The root is
                // never pooled: the original model rebuilds its writer-time root.
                if requested.contains(&before) {
                    found.insert(before, active.iter().flatten().copied().collect());
                }
                let empty = xml.get(
                    (reader.buffer_position() as usize).saturating_sub(2)
                        ..reader.buffer_position() as usize,
                ) == Some(b"/>");
                if !empty {
                    let mut local = Vec::new();
                    if !active.is_empty() {
                        for attribute in element.attributes() {
                            let attribute = attribute.map_err(|error| {
                                Error::Other(format!(
                                    "story namespace scope attribute failed: {error}"
                                ))
                            })?;
                            if let Some((prefix, namespace)) =
                                decoded_story_namespace_declaration(&element, &attribute)?
                            {
                                decoded_bytes = decoded_bytes
                                    .checked_add(prefix.len())
                                    .and_then(|bytes| bytes.checked_add(namespace.len()))
                                    .filter(|bytes| *bytes <= xml.len())
                                    .ok_or_else(|| {
                                        Error::Other(
                                            "field-cache declaration source work overflow".into(),
                                        )
                                    })?;
                                local.push((prefix, namespace));
                            }
                        }
                    }
                    if local.is_empty() {
                        active.push(None);
                    } else {
                        let index = frames.len();
                        frames.push(local);
                        active.push(Some(index));
                    }
                }
            }
            Event::End(_) => {
                active.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if found.len() != requested.len() || !active.is_empty() {
        return Err(Error::Other(
            "field-cache ancestor inventory is incomplete".into(),
        ));
    }
    Ok((frames, found))
}

pub(super) fn story_namespace_scopes_at(
    xml: &[u8],
    offsets: impl IntoIterator<Item = usize>,
) -> Result<HashMap<usize, BTreeMap<String, String>>> {
    let requested = offsets.into_iter().collect::<HashSet<_>>();
    let mut found = HashMap::with_capacity(requested.len());
    if requested.is_empty() {
        return Ok(found);
    }
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut scopes = vec![BTreeMap::<String, String>::new()];
    let mut changed_frames = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story namespace scope scan failed: {error}")))?
        {
            Event::Start(element) => {
                let inherited = scopes.last().expect("initial namespace scope");
                // Snapshot inherited context and preserve the original early
                // return before this target's declarations are validated.
                if requested.contains(&before) {
                    found.insert(before, inherited.clone());
                    if found.len() == requested.len() {
                        return Ok(found);
                    }
                }
                let changed = changed_story_namespace_scope(inherited, &element)?;
                changed_frames.push(changed.is_some());
                if let Some(scope) = changed {
                    scopes.push(scope);
                }
            }
            Event::Empty(_) => {
                if requested.contains(&before) {
                    found.insert(
                        before,
                        scopes.last().expect("initial namespace scope").clone(),
                    );
                    if found.len() == requested.len() {
                        return Ok(found);
                    }
                }
            }
            Event::End(_) => {
                if changed_frames.pop() == Some(true) {
                    scopes.pop();
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let missing = requested
        .into_iter()
        .filter(|offset| !found.contains_key(offset))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(found)
    } else {
        Err(Error::Other(format!(
            "story namespace scopes were not found at offsets {missing:?}"
        )))
    }
}

pub(super) fn scoped_story_fragment(
    xml: &[u8],
    range: Range<usize>,
    scope: &BTreeMap<String, String>,
) -> Result<(Vec<u8>, usize)> {
    let original = xml
        .get(range)
        .ok_or_else(|| Error::Other("story fragment lies outside its source part".to_owned()))?;
    let closed = close_content_fragment_namespaces(original, scope)?;
    let added = closed
        .len()
        .checked_sub(original.len())
        .ok_or_else(|| Error::Other("namespace closure shortened a story fragment".to_owned()))?;
    Ok((closed, added))
}

pub(super) fn scan_story_items_with_scope(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    scope: &BTreeMap<String, String>,
    template_field_ranges: Option<&mut Vec<Range<usize>>>,
) -> Result<Vec<StoryItemSpan>> {
    let (closed, added) = scoped_story_fragment(xml, owner.full.clone(), scope)?;
    let mut local_owner = owner.clone();
    local_owner.full = 0..closed.len();
    let mut local_fields = Vec::new();
    let mut items = scan_story_items(
        &closed,
        &local_owner,
        template_field_ranges.as_ref().map(|_| &mut local_fields),
    )?;
    let map = |position: usize| {
        if position == 0 {
            owner.full.start
        } else {
            owner.full.start + position - added
        }
    };
    if let Some(ranges) = template_field_ranges {
        ranges.extend(
            local_fields
                .into_iter()
                .map(|range| map(range.start)..map(range.end)),
        );
    }
    for item in &mut items {
        item.full = map(item.full.start)..map(item.full.end);
        item.scan = map(item.scan.start)..map(item.scan.end);
        for ancestor in &mut item.complex_ancestors {
            *ancestor = map(*ancestor);
        }
    }
    Ok(items)
}

fn scoped_story_item(
    xml: &[u8],
    item: &StoryItemSpan,
    scope: &BTreeMap<String, String>,
) -> Result<(Vec<u8>, StoryItemSpan, usize)> {
    let (closed, added, scan) = if item.complex_field {
        // A complex field spans sibling runs. Bind their shared context on an
        // inert envelope, leaving every source byte and local declaration intact.
        let original = xml.get(item.scan.clone()).ok_or_else(|| {
            Error::Other("complex story field lies outside its source part".to_owned())
        })?;
        let mut closed = b"<rdocx_story_excerpt".to_vec();
        for (prefix, namespace) in scope {
            closed.extend_from_slice(b" xmlns");
            if !prefix.is_empty() {
                closed.push(b':');
                closed.extend_from_slice(prefix.as_bytes());
            }
            closed.extend_from_slice(b"=\"");
            closed.extend_from_slice(quick_xml::escape::escape(namespace).as_bytes());
            closed.push(b'"');
        }
        closed.push(b'>');
        let added = closed.len();
        closed.extend_from_slice(original);
        let end = closed.len();
        closed.extend_from_slice(b"</rdocx_story_excerpt>");
        (closed, added, added..end)
    } else {
        let (closed, added) = scoped_story_fragment(xml, item.scan.clone(), scope)?;
        let end = closed.len();
        (closed, added, 0..end)
    };
    let source_len = item
        .scan
        .end
        .checked_sub(item.scan.start)
        .ok_or_else(|| Error::Other("story item has an inverted scan span".to_owned()))?;
    let local = |position: usize| -> Result<usize> {
        if !item.complex_field && position == item.scan.start {
            return Ok(0);
        }
        position
            .checked_sub(item.scan.start)
            .filter(|relative| *relative <= source_len)
            .and_then(|relative| relative.checked_add(added))
            .ok_or_else(|| Error::Other("story item lies outside its scan span".to_owned()))
    };
    let full = local(item.full.start)?..local(item.full.end)?;
    if full.start > full.end {
        return Err(Error::Other(
            "story item has an inverted source span".to_owned(),
        ));
    }
    let local = StoryItemSpan {
        kind: item.kind,
        full,
        scan,
        direct_owner_child: item.direct_owner_child,
        complex_field: item.complex_field,
        // Ancestors gate typed discovery in scan_story_items. Neither excerpt
        // scanner consumes them, and an external ancestor has no local offset.
        complex_ancestors: Vec::new(),
        sdt_context: item.sdt_context,
    };
    Ok((closed, local, added))
}

pub(super) fn story_item_text_with_scope(
    xml: &[u8],
    item: &StoryItemSpan,
    scope: &BTreeMap<String, String>,
    qualified_items: &[StoryItemSpan],
) -> Result<Option<String>> {
    let (closed, local, added) = scoped_story_item(xml, item, scope)?;
    story_item_text(&closed, &local, qualified_items, item.scan.clone(), added)
}

pub(super) fn scan_story_item_links_with_scope(
    xml: &[u8],
    item: &StoryItemSpan,
    scope: &BTreeMap<String, String>,
) -> Result<Vec<StoryLinkSpan>> {
    let (closed, local, added) = scoped_story_item(xml, item, scope)?;
    let links = scan_story_item_links(&closed, &local)?;
    let map = |position: usize| -> Result<usize> {
        if !item.complex_field && position == 0 {
            return Ok(item.scan.start);
        }
        position
            .checked_sub(added)
            .and_then(|relative| item.scan.start.checked_add(relative))
            .filter(|source| item.scan.start <= *source && *source <= item.scan.end)
            .ok_or_else(|| Error::Other("story hyperlink lies outside its source span".to_owned()))
    };
    links
        .into_iter()
        .map(|link| {
            let full = map(link.full.start)?..map(link.full.end)?;
            if full.start > full.end {
                return Err(Error::Other(
                    "story hyperlink has an inverted source span".to_owned(),
                ));
            }
            Ok(StoryLinkSpan {
                full,
                rel_id: link.rel_id,
                anchor: link.anchor,
            })
        })
        .collect()
}

/// Scan one story's hyperlinks in physical source order, each paired with the
/// smallest item that owns it. [`Document::story_links`] numbers this list.
pub(super) fn story_link_spans(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    story: &StoryId,
) -> Result<Vec<(ContentLocation, StoryLinkSpan)>> {
    let items = scan_story_items(xml, owner, None)?;
    let item_scopes = story_namespace_scopes_at(xml, items.iter().map(|item| item.scan.start))?;
    ordered_story_link_spans(xml, story, items, &item_scopes)
}

/// Pair the hyperlinks of one story's scanned items with the smallest item
/// that owns each, in physical source order.
pub(super) fn ordered_story_link_spans(
    xml: &[u8],
    story: &StoryId,
    items: Vec<StoryItemSpan>,
    item_scopes: &HashMap<usize, BTreeMap<String, String>>,
) -> Result<Vec<(ContentLocation, StoryLinkSpan)>> {
    let mut links = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let scope = item_scopes.get(&item.scan.start).ok_or_else(|| {
            Error::Other("story item namespace scope was not inventoried".to_owned())
        })?;
        let owner_width = item.full.end - item.full.start;
        for link in scan_story_item_links_with_scope(xml, item, scope)? {
            let location = ContentLocation {
                story: story.clone(),
                item_kind: item.kind,
                index_path: vec![index],
                is_end: false,
            };
            links.push((owner_width, location, link));
        }
    }
    links.sort_by_key(|(owner_width, _, link)| (link.full.start, *owner_width));
    links.dedup_by(|left, right| left.2.full == right.2.full);
    Ok(links
        .into_iter()
        .map(|(_, location, link)| (location, link))
        .collect())
}

pub(super) fn content_fragment_root_is_section_properties(
    xml: &[u8],
    item: &StoryItemSpan,
) -> Result<bool> {
    let mut scope = story_namespace_scope_at(xml, item.full.start)?;
    let mut reader = quick_xml::Reader::from_reader(&xml[item.full.clone()]);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("content fragment scan failed: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                update_story_namespace_scope(&mut scope, &element)?;
                let name = element.name();
                let bytes = name.as_ref();
                let (prefix, local) =
                    if let Some(colon) = bytes.iter().position(|byte| *byte == b':') {
                        (
                            std::str::from_utf8(&bytes[..colon]).map_err(|error| {
                                Error::Other(format!(
                                    "content fragment namespace prefix is invalid: {error}"
                                ))
                            })?,
                            &bytes[colon + 1..],
                        )
                    } else {
                        ("", bytes)
                    };
                return Ok(local == b"sectPr"
                    && scope
                        .get(prefix)
                        .is_some_and(|value| value == WORD_NAMESPACE));
            }
            Event::Eof => {
                return Err(Error::Other(
                    "content fragment has no root element".to_owned(),
                ));
            }
            _ => buffer.clear(),
        }
    }
}

pub(super) fn direct_story_content_items(
    xml: &[u8],
    owner: &StoryOwnerSpan,
) -> Result<Vec<StoryItemSpan>> {
    direct_story_content_items_in(xml, owner, &scan_story_items(xml, owner, None)?)
}

pub(super) fn direct_story_content_items_in(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    story_items: &[StoryItemSpan],
) -> Result<Vec<StoryItemSpan>> {
    let mut items = Vec::new();
    for item in story_items {
        if !item.direct_owner_child {
            continue;
        }
        if owner.kind == StoryKind::Body
            && item.kind == StoryItemKind::PreservedNode
            && content_fragment_root_is_section_properties(xml, item)?
        {
            continue;
        }
        items.push(item.clone());
    }
    Ok(items)
}
