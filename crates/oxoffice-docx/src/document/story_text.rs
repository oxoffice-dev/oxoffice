//! Story text and hyperlinks: reading, replacing and unwrapping them, bookmarks and picture sizes.

use super::*;

pub(super) fn story_hyperlink_attributes(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<(Option<String>, Option<String>)> {
    let mut rel_id = None;
    let mut anchor = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story hyperlink attribute scan failed: {error}"))
        })?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        let target = if matches!(
            namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == drawing_ns::R.as_bytes()
        ) && local_name.as_ref() == b"id"
        {
            &mut rel_id
        } else if word_element(&namespace) && local_name.as_ref() == b"anchor" {
            &mut anchor
        } else {
            continue;
        };
        *target = Some(
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| {
                    Error::Other(format!("story hyperlink attribute decode failed: {error}"))
                })?
                .into_owned(),
        );
    }
    Ok((rel_id, anchor))
}

/// Return the start tag range of one hyperlink element and, unless it is
/// empty, its end tag range.
pub(super) fn story_hyperlink_tags(
    xml: &[u8],
    link: &Range<usize>,
) -> Result<(Range<usize>, Option<Range<usize>>)> {
    let mut reader = quick_xml::Reader::from_reader(&xml[link.clone()]);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    loop {
        let before = link.start + reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?;
        let after = link.start + reader.buffer_position() as usize;
        match event {
            Event::Empty(_) if depth == 0 => return Ok((before..after, None)),
            Event::Start(_) => {
                if depth == 0 {
                    start = Some(before..after);
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let start = start.ok_or_else(|| {
                        Error::Other("story hyperlink has no start tag".to_owned())
                    })?;
                    return Ok((start, Some(before..after)));
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "story hyperlink has no closing tag".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

/// Rebuild a hyperlink start tag so that it names `relationship_id` and has no
/// anchor, keeping every other attribute.
pub(super) fn external_story_hyperlink_start(
    tag: &[u8],
    scope: &BTreeMap<String, String>,
    relationship_id: &str,
) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(tag);
    let mut buffer = Vec::new();
    let (element, empty) = match reader
        .read_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?
    {
        Event::Start(element) => (element, false),
        Event::Empty(element) => (element, true),
        _ => {
            return Err(Error::Other(
                "story hyperlink start tag is invalid".to_owned(),
            ));
        }
    };
    let mut rewritten = Vec::with_capacity(tag.len() + relationship_id.len() + 16);
    rewritten.push(b'<');
    rewritten.extend_from_slice(element.name().as_ref());
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story hyperlink attribute scan failed: {error}"))
        })?;
        let key = attribute.key.as_ref();
        let resolved = key.iter().position(|byte| *byte == b':').and_then(|colon| {
            let prefix = std::str::from_utf8(&key[..colon]).ok()?;
            Some((scope.get(prefix)?.as_str(), &key[colon + 1..]))
        });
        if matches!(resolved, Some((namespace, b"id")) if namespace == drawing_ns::R)
            || matches!(resolved, Some((namespace, b"anchor")) if namespace == WORD_NAMESPACE)
        {
            continue;
        }
        let quote = if attribute.value.contains(&b'"') {
            b'\''
        } else {
            b'"'
        };
        rewritten.push(b' ');
        rewritten.extend_from_slice(key);
        rewritten.push(b'=');
        rewritten.push(quote);
        rewritten.extend_from_slice(&attribute.value);
        rewritten.push(quote);
    }
    let bound = scope
        .iter()
        .find(|(prefix, namespace)| !prefix.is_empty() && namespace.as_str() == drawing_ns::R)
        .map(|(prefix, _)| prefix.clone());
    let prefix = match bound {
        Some(prefix) => prefix,
        None => {
            let mut suffix = 0usize;
            let prefix = loop {
                let candidate = if suffix == 0 {
                    "r".to_owned()
                } else {
                    format!("r{suffix}")
                };
                if !scope.contains_key(&candidate) {
                    break candidate;
                }
                suffix += 1;
            };
            rewritten
                .extend_from_slice(format!(" xmlns:{prefix}=\"{}\"", drawing_ns::R).as_bytes());
            prefix
        }
    };
    rewritten.extend_from_slice(
        format!(
            " {prefix}:id=\"{}\"",
            quick_xml::escape::escape(relationship_id)
        )
        .as_bytes(),
    );
    rewritten.extend_from_slice(if empty { b"/>" } else { b">" });
    Ok(rewritten)
}

/// Remove one hyperlink element and keep its content in place.
///
/// Namespace declarations on the hyperlink move to each child element, and
/// `cleared_styles` are removed from the `w:rStyle` of the runs it held.
pub(super) fn unwrap_story_hyperlink(
    xml: &[u8],
    link: &Range<usize>,
    scope: &BTreeMap<String, String>,
    cleared_styles: &HashSet<String>,
) -> Result<Vec<u8>> {
    let (start, end) = story_hyperlink_tags(xml, link)?;
    let mut edits = vec![(start.clone(), Vec::new())];
    if let Some(end) = end {
        let mut declarations = Vec::new();
        let mut reader = quick_xml::Reader::from_reader(&xml[start.clone()]);
        let mut buffer = Vec::new();
        if let Event::Start(element) = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?
        {
            for attribute in element.attributes() {
                let attribute = attribute.map_err(|error| {
                    Error::Other(format!("story hyperlink attribute scan failed: {error}"))
                })?;
                let key = attribute.key.as_ref();
                if key == b"xmlns" || key.starts_with(b"xmlns:") {
                    declarations.push((key.to_vec(), attribute.value.into_owned()));
                }
            }
        }
        let (closed, added) = scoped_story_fragment(xml, link.clone(), scope)?;
        let map = |position: usize| link.start + position - added;
        let mut reader = NsReader::from_reader(closed.as_slice());
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        let mut stack: Vec<(bool, Vec<u8>, Option<usize>)> = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?;
            let is_word = word_element(&namespace);
            drop(namespace);
            let after = reader.buffer_position() as usize;
            match event {
                Event::Start(ref element) | Event::Empty(ref element) => {
                    if stack.len() == 1 {
                        let insert_at = map(before + 1 + element.name().as_ref().len());
                        let mut inserted = Vec::new();
                        for (key, value) in &declarations {
                            let redeclared = element
                                .attributes()
                                .flatten()
                                .any(|attribute| attribute.key.as_ref() == key.as_slice());
                            if !redeclared {
                                inserted.push(b' ');
                                inserted.extend_from_slice(key);
                                inserted.extend_from_slice(b"=\"");
                                inserted.extend_from_slice(value);
                                inserted.push(b'"');
                            }
                        }
                        if !inserted.is_empty() {
                            edits.push((insert_at..insert_at, inserted));
                        }
                    }
                    let local = element.local_name().as_ref().to_vec();
                    let in_run_properties = is_word
                        && local == b"rStyle"
                        && stack.len() >= 2
                        && matches!(&stack[stack.len() - 2..], [(true, run, _), (true, properties, _)]
                            if run == b"r" && properties == b"rPr")
                        && !stack
                            .iter()
                            .any(|(word, name, _)| *word && name == b"txbxContent");
                    let cleared = in_run_properties
                        && element.attributes().flatten().any(|attribute| {
                            let (namespace, name) =
                                reader.resolver().resolve_attribute(attribute.key);
                            word_element(&namespace)
                                && name.as_ref() == b"val"
                                && attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .is_ok_and(|value| cleared_styles.contains(value.as_ref()))
                        });
                    if matches!(event, Event::Start(_)) {
                        stack.push((is_word, local, cleared.then(|| map(before))));
                    } else if cleared {
                        edits.push((map(before)..map(after), Vec::new()));
                    }
                }
                Event::End(_) => {
                    if let Some((_, _, Some(removed_from))) = stack.pop() {
                        edits.push((removed_from..map(after), Vec::new()));
                    }
                    if stack.is_empty() {
                        break;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
        edits.push((end, Vec::new()));
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut updated = xml.to_vec();
    for (mut range, replacement) in edits.into_iter().rev() {
        // A removed tag takes its indentation with it: whitespace between
        // WordprocessingML elements outside `w:t` carries no content.
        if replacement.is_empty() && !range.is_empty() {
            let indented = xml[..range.start]
                .iter()
                .rposition(|byte| !byte.is_ascii_whitespace());
            if let Some(previous) = indented.filter(|previous| xml[*previous] == b'>') {
                range.start = previous + 1;
            }
        }
        updated.splice(range, replacement);
    }
    Ok(updated)
}

pub(super) fn bookmark_marker_spans(xml: &[u8]) -> Result<Vec<BookmarkMarkerSpan>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut markers = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("bookmark scan failed: {error}")))?;
        let namespace = story_namespace(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = story_frame_local_name(element.local_name().as_ref());
                let empty = matches!(event, Event::Empty(_));
                let modeled = stack.last().is_none_or(|parent| !parent.opaque)
                    && modeled_story_event(
                        &mut stack, namespace, &local, element, empty, xml, before, after,
                    )?;
                if modeled
                    && accepted_story_text_visible(&stack)
                    && namespace == StoryNamespace::Word
                    && matches!(local.as_ref(), b"bookmarkStart" | b"bookmarkEnd")
                {
                    let mut id = None;
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        if word_element(&namespace) && local.as_ref() == b"id" {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| Error::Other(error.to_string()))?;
                            if id.replace(value.into_owned()).is_some() {
                                return Err(Error::Other(
                                    "bookmark marker has duplicate owned id".to_owned(),
                                ));
                            }
                        }
                    }
                    if let Some(id) = id {
                        let end = if empty {
                            after
                        } else {
                            story_element_end(xml, before)?
                        };
                        if !empty {
                            let mut child = quick_xml::Reader::from_reader(&xml[after..end]);
                            if !matches!(
                                child
                                    .read_event()
                                    .map_err(|error| Error::Other(error.to_string()))?,
                                Event::End(_)
                            ) {
                                return Err(Error::Other(
                                    "a bookmark marker with child content cannot be removed"
                                        .to_owned(),
                                ));
                            }
                        }
                        markers.push(BookmarkMarkerSpan {
                            start: local.as_ref() == b"bookmarkStart",
                            id,
                            span: before..end,
                        });
                    }
                }
                if !empty {
                    let sdt_context = story_element_sdt_context(&stack, namespace, &local);
                    let inherited = stack
                        .last()
                        .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                    let word_prefixes = story_word_prefixes_at(
                        element,
                        inherited,
                        namespace == StoryNamespace::Word,
                    )?;
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: local,
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
    Ok(markers)
}

/// Resize every `pic:pic` drawing in `xml` whose blip names `relationship_id`,
/// returning the edited XML and the number of drawings resized.
///
/// `wp:extent` and the `a:ext` of `pic:spPr/a:xfrm` take the new size, and
/// `wp:effectExtent` scales with the extent on each axis.
pub(super) fn resize_story_pictures(
    xml: &[u8],
    relationship_id: &str,
    cx: i64,
    cy: i64,
) -> Result<(Vec<u8>, usize)> {
    struct Drawing {
        depth: usize,
        extent: Option<Range<usize>>,
        effect_extent: Option<Range<usize>>,
        shape_extent: Option<Range<usize>>,
        shows_image: bool,
    }
    const A: &str = drawing_ns::A;
    const PIC: &str = drawing_ns::PIC;
    let on_path = |path: &[(String, Vec<u8>)], expected: &[(&str, &[u8])]| {
        path.len() == expected.len()
            && path.iter().zip(expected).all(
                |((namespace, local), (want_namespace, want_local))| {
                    namespace == want_namespace && local.as_slice() == *want_local
                },
            )
    };
    let attribute_value = |tag: Range<usize>, name: &[u8]| -> Result<(Range<usize>, i64)> {
        let (start, end) =
            story_attribute_value_span(&xml[tag.clone()], name).ok_or_else(|| {
                Error::Other(format!(
                    "picture drawing has no {} attribute",
                    String::from_utf8_lossy(name)
                ))
            })?;
        let range = tag.start + start..tag.start + end;
        let value = std::str::from_utf8(&xml[range.clone()])
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .ok_or_else(|| {
                Error::Other(format!(
                    "picture drawing {} is not an integer",
                    String::from_utf8_lossy(name)
                ))
            })?;
        Ok((range, value))
    };
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack: Vec<(String, Vec<u8>)> = Vec::new();
    let mut drawings: Vec<Drawing> = Vec::new();
    let mut edits = Vec::new();
    let mut resized = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("picture drawing scan failed: {error}")))?;
        let namespace = match namespace {
            ResolveResult::Bound(Namespace(uri)) => String::from_utf8_lossy(uri).into_owned(),
            _ => String::new(),
        };
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = element.local_name().as_ref().to_vec();
                if let Some(drawing) = drawings.last_mut() {
                    let path = &stack[drawing.depth + 1..];
                    if path.is_empty() && namespace == drawing_ns::WP {
                        match local.as_slice() {
                            b"extent" => drawing.extent = Some(before..after),
                            b"effectExtent" => drawing.effect_extent = Some(before..after),
                            _ => {}
                        }
                    } else if namespace == A
                        && local == b"blip"
                        && on_path(
                            path,
                            &[
                                (A, b"graphic"),
                                (A, b"graphicData"),
                                (PIC, b"pic"),
                                (PIC, b"blipFill"),
                            ],
                        )
                    {
                        drawing.shows_image |= element.attributes().flatten().any(|attribute| {
                            let (attribute_namespace, name) =
                                reader.resolver().resolve_attribute(attribute.key);
                            matches!(attribute_namespace, ResolveResult::Bound(Namespace(uri)) if uri == drawing_ns::R.as_bytes())
                                && matches!(name.as_ref(), b"embed" | b"link")
                                && attribute.value.as_ref() == relationship_id.as_bytes()
                        });
                    } else if namespace == A
                        && local == b"ext"
                        && on_path(
                            path,
                            &[
                                (A, b"graphic"),
                                (A, b"graphicData"),
                                (PIC, b"pic"),
                                (PIC, b"spPr"),
                                (A, b"xfrm"),
                            ],
                        )
                    {
                        drawing.shape_extent = Some(before..after);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    if namespace == drawing_ns::WP
                        && matches!(local.as_slice(), b"inline" | b"anchor")
                    {
                        drawings.push(Drawing {
                            depth: stack.len(),
                            extent: None,
                            effect_extent: None,
                            shape_extent: None,
                            shows_image: false,
                        });
                    }
                    stack.push((namespace, local));
                }
            }
            Event::End(_) => {
                stack.pop();
                if drawings
                    .last()
                    .is_some_and(|drawing| drawing.depth == stack.len())
                    && let Some(drawing) = drawings.pop()
                    && drawing.shows_image
                    && let Some(extent) = drawing.extent
                {
                    let (cx_range, old_cx) = attribute_value(extent.clone(), b"cx")?;
                    let (cy_range, old_cy) = attribute_value(extent, b"cy")?;
                    edits.push((cx_range, cx));
                    edits.push((cy_range, cy));
                    if let Some(shape_extent) = drawing.shape_extent {
                        edits.push((attribute_value(shape_extent.clone(), b"cx")?.0, cx));
                        edits.push((attribute_value(shape_extent, b"cy")?.0, cy));
                    }
                    if let Some(effect_extent) = drawing.effect_extent {
                        for (name, new, old) in [
                            (&b"l"[..], cx, old_cx),
                            (b"t", cy, old_cy),
                            (b"r", cx, old_cx),
                            (b"b", cy, old_cy),
                        ] {
                            let (range, value) = attribute_value(effect_extent.clone(), name)?;
                            let scaled = if old == 0 {
                                0
                            } else {
                                i64::try_from(i128::from(value) * i128::from(new) / i128::from(old))
                                    .map_err(|_| {
                                        Error::Other(
                                            "scaled picture effect extent overflows".to_owned(),
                                        )
                                    })?
                            };
                            edits.push((range, scaled));
                        }
                    }
                    resized += 1;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut updated = xml.to_vec();
    for (range, value) in edits.into_iter().rev() {
        updated.splice(range, value.to_string().into_bytes());
    }
    Ok((updated, resized))
}

pub(super) fn story_item_text(
    xml: &[u8],
    item: &StoryItemSpan,
    qualified_items: &[StoryItemSpan],
    source_scan: Range<usize>,
    added: usize,
) -> Result<Option<String>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut in_text = 0usize;
    let mut pieces: Vec<(StoryFieldContext, StoryTextPiece)> = Vec::new();
    let mut fields = StoryFieldTracker::default();
    let mut active = false;
    let mut element_stack: Vec<XmlElementFrame> = Vec::new();
    let mut nested_owner_depth = 0usize;
    let mut complex_results = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(format!("story text scan failed: {error}")));
            }
        };
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
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
                let depth = element_stack.len();
                let local_name = story_frame_local_name(element.local_name().as_ref());
                // Reuse only controls independently admitted in this operation's
                // owner inventory. A typed parent can still retain untyped raw
                // descendants, which must follow ordinary qualification.
                let qualified_control = namespace_kind == StoryNamespace::Word
                    && local_name.as_ref() == b"sdt"
                    && !element_stack.last().is_some_and(|frame| frame.opaque)
                    && modeled_story_child(&element_stack, namespace_kind, &local_name)
                    && (if !item.complex_field && before == 0 {
                        Some(source_scan.start)
                    } else {
                        before
                            .checked_sub(added)
                            .and_then(|offset| source_scan.start.checked_add(offset))
                    })
                    .and_then(|start| {
                        qualified_items
                            .binary_search_by_key(&start, |item| item.full.start)
                            .ok()
                    })
                    .is_some_and(|index| {
                        let control = &qualified_items[index];
                        control.kind == StoryItemKind::ContentControl
                            && !control.complex_field
                            && control.full == control.scan
                            && source_scan.start <= control.full.start
                            && control.full.end <= source_scan.end
                            && control.sdt_context == story_sdt_context(&element_stack)
                    });
                let opaque =
                    if element_stack.is_empty() && item.kind == StoryItemKind::ContentControl {
                        false
                    } else if nested_owner_depth > 0 {
                        // The qualified nested owner excludes this content from
                        // the selected item. Keep inherited opacity and balanced
                        // owner depth while traversing its source and scopes.
                        element_stack.last().is_some_and(|frame| frame.opaque)
                    } else if qualified_control {
                        false
                    } else {
                        opaque_story_event(
                            &mut element_stack,
                            namespace_kind,
                            &local_name,
                            &element,
                            false,
                            xml,
                            before,
                            after,
                        )?
                    };
                if !opaque
                    && story_field_marker_tracked(item, before, nested_owner_depth, &element_stack)
                    && is_word_namespace
                    && local_name.as_ref() == b"fldChar"
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    if item.complex_field {
                        update_complex_field_results(&mut complex_results, marker);
                    } else {
                        fields.mark(marker);
                    }
                }
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(local_name.as_ref(), b"tc" | b"txbxContent")
                {
                    nested_owner_depth += 1;
                }
                if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                {
                    if local_name.as_ref() == b"t" {
                        in_text += 1;
                        pieces.push((fields.context(), StoryTextPiece::Text(String::new())));
                    } else if let Some(piece) =
                        story_run_character(&reader, &element, &element_stack, item)?
                    {
                        pieces.push((fields.context(), piece));
                    }
                }
                let inherited = element_stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word_namespace)?;
                element_stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word: is_word_namespace,
                    full_start: before,
                    owner_kind: None,
                    item_kind: None,
                    direct_owner_child: false,
                    sdt_context: if depth == 0 && item.kind == StoryItemKind::ContentControl {
                        item.sdt_context
                    } else {
                        story_element_sdt_context(
                            &element_stack,
                            namespace_kind,
                            element.local_name().as_ref(),
                        )
                    },
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                let local_name = element.local_name();
                let opaque = if nested_owner_depth > 0 {
                    element_stack.last().is_some_and(|frame| frame.opaque)
                } else {
                    opaque_story_event(
                        &mut element_stack,
                        namespace_kind,
                        local_name.as_ref(),
                        &element,
                        true,
                        xml,
                        before,
                        after,
                    )?
                };
                if !opaque
                    && story_field_marker_tracked(item, before, nested_owner_depth, &element_stack)
                    && is_word_namespace
                    && local_name.as_ref() == b"fldChar"
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    if item.complex_field {
                        update_complex_field_results(&mut complex_results, marker);
                    } else {
                        fields.mark(marker);
                    }
                } else if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                {
                    if local_name.as_ref() == b"t" {
                        pieces.push((fields.context(), StoryTextPiece::Text(String::new())));
                    } else if let Some(piece) =
                        story_run_character(&reader, &element, &element_stack, item)?
                    {
                        pieces.push((fields.context(), piece));
                    }
                }
            }
            Event::End(element) => {
                let opaque = element_stack.pop().is_some_and(|frame| frame.opaque);
                let depth = element_stack.len();
                if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && element.local_name().as_ref() == b"t"
                {
                    in_text = in_text.saturating_sub(1);
                }
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(element.local_name().as_ref(), b"tc" | b"txbxContent")
                {
                    nested_owner_depth = nested_owner_depth.saturating_sub(1);
                }
            }
            Event::Text(value)
                if complex_field_text_visible(item, &complex_results) && in_text > 0 =>
            {
                push_story_text(
                    &mut pieces,
                    &quick_xml::escape::unescape(&oxoffice_core::xml_text::normalize_line_ends(
                        &value.decode().map_err(|error| {
                            Error::Other(format!("story text decode failed: {error}"))
                        })?,
                    ))
                    .map_err(|error| {
                        Error::Other(format!("story text unescape failed: {error}"))
                    })?,
                )
            }
            Event::CData(value)
                if complex_field_text_visible(item, &complex_results) && in_text > 0 =>
            {
                push_story_text(
                    &mut pieces,
                    &oxoffice_core::xml_text::normalize_line_ends(&value.decode().map_err(
                        |error| Error::Other(format!("story CDATA decode failed: {error}")),
                    )?),
                )
            }
            Event::GeneralRef(value)
                if complex_field_text_visible(item, &complex_results) && in_text > 0 =>
            {
                push_story_text(
                    &mut pieces,
                    &oxoffice_core::xml_text::resolve_entity(&value),
                );
            }
            Event::Eof => break,
            _ => {}
        }
        if (item.complex_field && after >= item.full.end)
            || (!item.complex_field && after == item.scan.end)
        {
            break;
        }
        buffer.clear();
    }
    let mut text = String::new();
    let mut found = false;
    for (context, piece) in pieces {
        if fields.hides(&context) {
            continue;
        }
        match piece {
            StoryTextPiece::Text(value) => {
                text.push_str(&value);
                found = true;
            }
            StoryTextPiece::Character(plain, in_result) => {
                let character = if fields.in_result(&context) {
                    in_result
                } else {
                    plain
                };
                if let Some(character) = character {
                    text.push(character);
                    found = true;
                }
            }
        }
    }
    Ok((found || item.complex_field).then_some(text))
}

/// Append decoded text to the `w:t` piece it belongs to.
fn push_story_text(pieces: &mut [(StoryFieldContext, StoryTextPiece)], value: &str) {
    if let Some((_, StoryTextPiece::Text(text))) = pieces.last_mut() {
        text.push_str(value);
    }
}

/// Return whether an element opened at this stack is run content, so a run's
/// `w:tab` counts as text and a tab stop under `w:tabs` does not.
fn story_run_child(stack: &[XmlElementFrame]) -> bool {
    stack
        .last()
        .is_some_and(|frame| !frame.opaque && frame.is_word && frame.local_name.as_ref() == b"r")
}

/// The text a visible Word element adds when it is run content other than
/// `w:t`: a tab for `w:tab` or `w:ptab`, a hyphen for `w:noBreakHyphen`, and
/// a break for `w:br` or `w:cr`, as read outside and inside a field result.
/// A field item reads inside its field.
fn story_run_character(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    stack: &[XmlElementFrame],
    item: &StoryItemSpan,
) -> Result<Option<StoryTextPiece>> {
    if !story_run_child(stack) {
        return Ok(None);
    }
    let (plain, in_result) = match element.local_name().as_ref() {
        b"tab" | b"ptab" => (Some('\t'), Some('\t')),
        b"noBreakHyphen" => (Some('-'), Some('-')),
        b"br" | b"cr" => (
            story_break_text(reader, element, false)?,
            story_break_text(reader, element, true)?,
        ),
        _ => return Ok(None),
    };
    let plain = if item.complex_field { in_result } else { plain };
    Ok(Some(StoryTextPiece::Character(plain, in_result)))
}

fn story_text_start(element: &BytesStart<'_>, preserve_space: bool) -> Result<Vec<u8>> {
    let content: &[u8] = element;
    let mut xml_space_key = None;
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("story text attribute scan failed: {error}")))?;
        if attribute.key.as_ref() == b"xml:space" {
            let key_address = attribute.key.as_ref().as_ptr() as usize;
            let content_address = content.as_ptr() as usize;
            let key_start = key_address.checked_sub(content_address).ok_or_else(|| {
                Error::Other("story text attribute lies outside its opening tag".to_owned())
            })?;
            if key_start + attribute.key.as_ref().len() > content.len() {
                return Err(Error::Other(
                    "story text attribute lies outside its opening tag".to_owned(),
                ));
            }
            xml_space_key = Some((key_start, attribute.key.as_ref().len()));
        }
    }

    let mut updated = Vec::with_capacity(content.len() + 23);
    updated.push(b'<');
    if let Some((key_start, key_len)) = xml_space_key {
        let mut attribute_start = key_start;
        while attribute_start > element.name().as_ref().len()
            && content[attribute_start - 1].is_ascii_whitespace()
        {
            attribute_start -= 1;
        }
        let mut attribute_end = key_start + key_len;
        while content
            .get(attribute_end)
            .is_some_and(u8::is_ascii_whitespace)
        {
            attribute_end += 1;
        }
        if content.get(attribute_end) != Some(&b'=') {
            return Err(Error::Other(
                "story text xml:space attribute has no equals sign".to_owned(),
            ));
        }
        attribute_end += 1;
        while content
            .get(attribute_end)
            .is_some_and(u8::is_ascii_whitespace)
        {
            attribute_end += 1;
        }
        let quote = *content.get(attribute_end).ok_or_else(|| {
            Error::Other("story text xml:space attribute has no quoted value".to_owned())
        })?;
        if !matches!(quote, b'\'' | b'"') {
            return Err(Error::Other(
                "story text xml:space attribute has no quoted value".to_owned(),
            ));
        }
        attribute_end += 1;
        let value_end = content[attribute_end..]
            .iter()
            .position(|byte| *byte == quote)
            .map(|offset| attribute_end + offset + 1)
            .ok_or_else(|| {
                Error::Other("story text xml:space attribute has no closing quote".to_owned())
            })?;
        updated.extend_from_slice(&content[..attribute_start]);
        updated.extend_from_slice(&content[value_end..]);
    } else {
        updated.extend_from_slice(content);
    }
    if preserve_space {
        updated.extend_from_slice(b" xml:space=\"preserve\"");
    }
    updated.push(b'>');
    Ok(updated)
}

pub(super) fn replace_story_item_text(
    source: &[u8],
    item: &StoryItemSpan,
    value: &str,
) -> Result<Vec<u8>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut text_depth = 0usize;
    let mut root_prefix = None;
    let mut root_end = None;
    let mut root_is_empty = false;
    let mut texts: Vec<StoryTextElement> = Vec::new();
    // Visible run tabs, non-breaking hyphens and breaks, which the new text
    // replaces as well.
    let mut removed: Vec<(StoryFieldContext, Range<usize>)> = Vec::new();
    let mut removed_start = None;
    let mut fields = StoryFieldTracker::default();
    let mut element_stack: Vec<XmlElementFrame> = Vec::new();
    let mut nested_owner_depth = 0usize;
    let mut complex_results = Vec::new();
    let mut complex_field_edit = ComplexFieldEdit::default();
    let mut active = false;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story text mutation scan failed: {error}")))?;
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
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
                let depth = element_stack.len();
                if depth == 0 {
                    let qualified_name = element.name();
                    let name = std::str::from_utf8(qualified_name.as_ref()).map_err(|error| {
                        Error::Other(format!("story item name is not UTF-8: {error}"))
                    })?;
                    root_prefix = Some(
                        name.split_once(':')
                            .map_or("", |(prefix, _)| prefix)
                            .to_owned(),
                    );
                }
                let local_name = story_frame_local_name(element.local_name().as_ref());
                let opaque =
                    if element_stack.is_empty() && item.kind == StoryItemKind::ContentControl {
                        false
                    } else {
                        opaque_story_event(
                            &mut element_stack,
                            namespace_kind,
                            &local_name,
                            &element,
                            false,
                            source,
                            before,
                            after,
                        )?
                    };
                if !opaque
                    && story_field_marker_tracked(item, before, nested_owner_depth, &element_stack)
                    && is_word_namespace
                    && local_name.as_ref() == b"fldChar"
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    track_story_field_marker(
                        item,
                        marker,
                        &mut fields,
                        &mut complex_results,
                        &mut complex_field_edit,
                        before,
                        &element_stack,
                    );
                }
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(local_name.as_ref(), b"tc" | b"txbxContent")
                {
                    nested_owner_depth += 1;
                }
                if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && local_name.as_ref() == b"t"
                {
                    texts.push(StoryTextElement {
                        context: fields.context(),
                        tag: before..after,
                        markup: story_text_markup_tags(
                            &element,
                            story_text_preserves_space(value),
                        )?,
                        empty: false,
                        content: Vec::new(),
                    });
                    text_depth += 1;
                } else if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && story_run_child(&element_stack)
                    && matches!(
                        local_name.as_ref(),
                        b"tab" | b"ptab" | b"noBreakHyphen" | b"br" | b"cr"
                    )
                {
                    removed_start = Some((element_stack.len(), before, fields.context()));
                }
                let inherited = element_stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word_namespace)?;
                element_stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word: is_word_namespace,
                    full_start: before,
                    owner_kind: None,
                    item_kind: None,
                    direct_owner_child: false,
                    sdt_context: if depth == 0 && item.kind == StoryItemKind::ContentControl {
                        item.sdt_context
                    } else {
                        story_element_sdt_context(
                            &element_stack,
                            namespace_kind,
                            element.local_name().as_ref(),
                        )
                    },
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                if element_stack.is_empty() {
                    root_is_empty = true;
                    let qualified_name = element.name();
                    let name = std::str::from_utf8(qualified_name.as_ref()).map_err(|error| {
                        Error::Other(format!("story item name is not UTF-8: {error}"))
                    })?;
                    root_prefix = Some(
                        name.split_once(':')
                            .map_or("", |(prefix, _)| prefix)
                            .to_owned(),
                    );
                }
                let local_name = element.local_name();
                let opaque = opaque_story_event(
                    &mut element_stack,
                    namespace_kind,
                    local_name.as_ref(),
                    &element,
                    true,
                    source,
                    before,
                    after,
                )?;
                if !opaque
                    && story_field_marker_tracked(item, before, nested_owner_depth, &element_stack)
                    && is_word_namespace
                    && local_name.as_ref() == b"fldChar"
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    track_story_field_marker(
                        item,
                        marker,
                        &mut fields,
                        &mut complex_results,
                        &mut complex_field_edit,
                        before,
                        &element_stack,
                    );
                } else if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && local_name.as_ref() == b"t"
                {
                    texts.push(StoryTextElement {
                        context: fields.context(),
                        tag: before..after,
                        markup: story_text_markup_tags(
                            &element,
                            story_text_preserves_space(value),
                        )?,
                        empty: true,
                        content: Vec::new(),
                    });
                } else if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && story_run_child(&element_stack)
                    && matches!(
                        local_name.as_ref(),
                        b"tab" | b"ptab" | b"noBreakHyphen" | b"br" | b"cr"
                    )
                {
                    removed.push((fields.context(), before..after));
                }
            }
            Event::End(element) => {
                let opaque = element_stack.pop().is_some_and(|frame| frame.opaque);
                let depth = element_stack.len();
                if removed_start
                    .as_ref()
                    .is_some_and(|(start_depth, _, _)| *start_depth == depth)
                    && let Some((_, start, context)) = removed_start.take()
                {
                    removed.push((context, start..after));
                }
                if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && element.local_name().as_ref() == b"t"
                {
                    text_depth = text_depth.saturating_sub(1);
                }
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(element.local_name().as_ref(), b"tc" | b"txbxContent")
                {
                    nested_owner_depth = nested_owner_depth.saturating_sub(1);
                }
                if depth == 0 {
                    root_end = Some(before);
                }
            }
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_)
                if complex_field_text_visible(item, &complex_results) && text_depth > 0 =>
            {
                if let Some(text) = texts.last_mut() {
                    text.content.push(before..after);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        if (item.complex_field && after >= item.full.end)
            || (!item.complex_field && after == item.scan.end)
        {
            break;
        }
        buffer.clear();
    }

    // Only a field closed within the item hides its instruction part.
    let texts = texts
        .into_iter()
        .filter(|text| !fields.hides(&text.context))
        .collect::<Vec<_>>();
    let removed_ranges = removed
        .into_iter()
        .filter(|(context, _)| !fields.hides(context))
        .map(|(_, range)| range)
        .collect::<Vec<_>>();
    let ComplexFieldEdit {
        separator_found: outer_complex_separator_found,
        end_marker_start: complex_end_marker_start,
        word_prefixes: complex_insertion_word_prefixes,
    } = complex_field_edit;
    let mut updated = source.to_vec();
    let space = if story_text_preserves_space(value) {
        " xml:space=\"preserve\""
    } else {
        ""
    };
    if texts.is_empty() {
        // Each insertion lies after every removed tab or break, so removing
        // them afterwards from the end keeps the insertion in place.
        if item.complex_field {
            let insert_at = complex_end_marker_start.ok_or_else(|| {
                Error::Other("complex story field has no closing marker".to_owned())
            })?;
            let nonempty_word_prefix = complex_insertion_word_prefixes
                .iter()
                .find(|prefix| !prefix.is_empty());
            let default_word_namespace =
                complex_insertion_word_prefixes.iter().any(String::is_empty);
            let (prefix, declaration, attribute_prefix, attribute_declaration) =
                if let Some(prefix) = nonempty_word_prefix {
                    (format!("{prefix}:"), String::new(), prefix.as_str(), "")
                } else if default_word_namespace {
                    (
                        String::new(),
                        String::new(),
                        "w",
                        concat!(
                            " xmlns:w=\"",
                            "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
                            "\""
                        ),
                    )
                } else {
                    (
                        "w:".to_owned(),
                        format!(" xmlns:w=\"{WORD_NAMESPACE}\""),
                        "w",
                        "",
                    )
                };
            let separator = if outer_complex_separator_found {
                String::new()
            } else {
                format!(
                    "<{prefix}fldChar{declaration}{attribute_declaration} \
                     {attribute_prefix}:fldCharType=\"separate\"/>"
                )
            };
            let markup = StoryTextMarkup::new(&prefix, &declaration, space);
            let mut replacement = separator.into_bytes();
            replacement.extend_from_slice(&markup.open);
            replacement.extend_from_slice(&story_text_content(value, &markup));
            replacement.extend_from_slice(&markup.close);
            updated.splice(insert_at..insert_at, replacement);
        } else {
            if item.kind != StoryItemKind::Paragraph {
                return Err(StoryError::NotTextBearing { kind: item.kind }.into());
            }
            let prefix = root_prefix.unwrap_or_else(|| "w".to_owned());
            let prefix = if prefix.is_empty() {
                String::new()
            } else {
                format!("{prefix}:")
            };
            let markup = StoryTextMarkup::new(&prefix, "", space);
            let mut run = format!("<{prefix}r>").into_bytes();
            run.extend_from_slice(&markup.open);
            run.extend_from_slice(&story_text_content(value, &markup));
            run.extend_from_slice(&markup.close);
            run.extend_from_slice(format!("</{prefix}r>").as_bytes());
            if let Some(end) = root_end {
                updated.splice(end..end, run);
            } else if root_is_empty {
                let slash = item.full.end.checked_sub(2).ok_or_else(|| {
                    Error::Other("self-closing paragraph story item is truncated".to_owned())
                })?;
                if source.get(slash..item.full.end) != Some(b"/>".as_slice()) {
                    return Err(Error::Other(
                        "self-closing paragraph story item has no closing slash".to_owned(),
                    ));
                }
                let mut replacement = b">".to_vec();
                replacement.extend_from_slice(&run);
                replacement.extend_from_slice(format!("</{prefix}p>").as_bytes());
                updated.splice(slash..item.full.end, replacement);
            } else {
                return Err(Error::Other(
                    "paragraph story item has no closing element".to_owned(),
                ));
            }
        }
        for range in removed_ranges.into_iter().rev() {
            updated.splice(range, []);
        }
    } else {
        let mut edits: Vec<(Range<usize>, Vec<u8>)> = removed_ranges
            .into_iter()
            .map(|range| (range, Vec::new()))
            .collect();
        for (ordinal, text) in texts.into_iter().enumerate() {
            if ordinal == 0 {
                // The first text element takes the whole new text.
                let content = story_text_content(value, &text.markup);
                if text.empty {
                    let mut replacement = text.markup.open;
                    replacement.extend_from_slice(&content);
                    replacement.extend_from_slice(&text.markup.close);
                    edits.push((text.tag, replacement));
                } else {
                    edits.push((text.tag.end..text.tag.end, content));
                    edits.push((text.tag, text.markup.open));
                }
            }
            edits.extend(text.content.into_iter().map(|range| (range, Vec::new())));
        }
        // A start tag ends where the new text is inserted, so equal starts
        // sort the start tag first and apply it last.
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        for (range, replacement) in edits.into_iter().rev() {
            updated.splice(range, replacement);
        }
    }
    Ok(updated)
}

/// One accepted text element an item's new text may replace, with the
/// fields open where it was read.
struct StoryTextElement {
    context: StoryFieldContext,
    tag: Range<usize>,
    markup: StoryTextMarkup,
    empty: bool,
    content: Vec<Range<usize>>,
}

/// Where new text goes in a field item that has no text yet.
#[derive(Default)]
struct ComplexFieldEdit {
    separator_found: bool,
    end_marker_start: Option<usize>,
    word_prefixes: Vec<String>,
}

/// Track one field character for text replacement. A field item records
/// where its own field separates and ends. Any other item tracks the fields
/// of its story to learn which close within it.
fn track_story_field_marker(
    item: &StoryItemSpan,
    marker: Option<ComplexFieldMarker>,
    fields: &mut StoryFieldTracker,
    complex_results: &mut Vec<bool>,
    edit: &mut ComplexFieldEdit,
    position: usize,
    stack: &[XmlElementFrame],
) {
    if !item.complex_field {
        fields.mark(marker);
        return;
    }
    if complex_results.len() == 1 && marker == Some(ComplexFieldMarker::Separate) {
        edit.separator_found = true;
    } else if complex_results.len() == 1 && marker == Some(ComplexFieldMarker::End) {
        edit.end_marker_start = Some(position);
        edit.word_prefixes = stack
            .last()
            .map_or_else(Vec::new, |frame| frame.word_prefixes.as_ref().clone());
    }
    update_complex_field_results(complex_results, marker);
}

/// The tags that write new story text into a run: the text element's start
/// and end tags, and the tab and break elements a tab or line feed becomes.
struct StoryTextMarkup {
    open: Vec<u8>,
    close: Vec<u8>,
    tab: Vec<u8>,
    br: Vec<u8>,
}

impl StoryTextMarkup {
    fn new(prefix: &str, declaration: &str, space: &str) -> Self {
        Self {
            open: format!("<{prefix}t{declaration}{space}>").into_bytes(),
            close: format!("</{prefix}t>").into_bytes(),
            tab: format!("<{prefix}tab{declaration}/>").into_bytes(),
            br: format!("<{prefix}br{declaration}/>").into_bytes(),
        }
    }
}

/// Build the markup for new text written into an existing `w:t`, keeping its
/// attributes and namespace declarations for any further text element.
fn story_text_markup_tags(
    element: &BytesStart<'_>,
    preserve_space: bool,
) -> Result<StoryTextMarkup> {
    let name = element.name();
    let name = std::str::from_utf8(name.as_ref())
        .map_err(|error| Error::Other(format!("story text name is not UTF-8: {error}")))?;
    let prefix = name.strip_suffix('t').unwrap_or_default();
    let mut declarations = String::new();
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("story text attribute scan failed: {error}")))?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            declarations.push(' ');
            declarations.push_str(&String::from_utf8_lossy(key));
            declarations.push_str("=\"");
            declarations.push_str(&String::from_utf8_lossy(&attribute.value));
            declarations.push('"');
        }
    }
    let mut markup = StoryTextMarkup::new(prefix, &declarations, "");
    markup.open = story_text_start(element, preserve_space)?;
    markup.close = format!("</{name}>").into_bytes();
    Ok(markup)
}

/// Escape new story text, writing a tab as `w:tab` and a line feed, a
/// carriage return or a Windows line ending as `w:br` between text elements,
/// as `Paragraph::set_text` does.
fn story_text_content(value: &str, markup: &StoryTextMarkup) -> Vec<u8> {
    let value = if value.contains("\r\n") {
        std::borrow::Cow::Owned(value.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(value)
    };
    let mut content = Vec::with_capacity(value.len());
    let mut separators = value
        .chars()
        .filter(|character| matches!(character, '\t' | '\n' | '\r'));
    for (index, segment) in value.split(['\t', '\n', '\r']).enumerate() {
        if index > 0 {
            content.extend_from_slice(&markup.close);
            if separators.next() == Some('\t') {
                content.extend_from_slice(&markup.tab);
            } else {
                content.extend_from_slice(&markup.br);
            }
            content.extend_from_slice(&markup.open);
        }
        content.extend_from_slice(quick_xml::escape::escape(segment).as_bytes());
    }
    content
}

/// Return whether any text element of new story text must keep its spaces.
fn story_text_preserves_space(value: &str) -> bool {
    value
        .split(['\t', '\n', '\r'])
        .any(|segment| segment.starts_with(' ') || segment.ends_with(' '))
}

/// The heading level of a paragraph, from its style: the built-in name
/// `heading N` in any case, which Word keeps whatever language it saves in
/// (the id is localised, `Titre1` in French), else an id `HeadingN`.
pub(crate) fn paragraph_heading_level(para: &CT_P, styles: &CT_Styles) -> Option<u32> {
    let style_id = para.properties.as_ref()?.style_id.as_deref()?;
    let level = |rest: &str| {
        rest.trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=9).contains(n))
    };
    let by_name = styles
        .get_by_id(style_id)
        .and_then(|style| style.name.as_deref())
        .and_then(|name| {
            name.to_ascii_lowercase()
                .strip_prefix("heading ")
                .and_then(level)
        });
    by_name.or_else(|| style_id.strip_prefix("Heading").and_then(level))
}

fn hyperlink_has_visible_content(source: &[u8], range: Range<usize>) -> Result<bool> {
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut word_text_depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("hyperlink content scan failed: {error}")))?;
        let is_word = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if after <= range.start {
            buffer.clear();
            continue;
        }
        if before >= range.end {
            return Ok(false);
        }
        match event {
            Event::Start(element) if is_word && element.local_name().as_ref() == b"t" => {
                word_text_depth += 1;
            }
            Event::End(element) if is_word && element.local_name().as_ref() == b"t" => {
                word_text_depth = word_text_depth.saturating_sub(1);
            }
            Event::Text(text) if word_text_depth > 0 => {
                let bytes: &[u8] = text.as_ref();
                if !bytes.is_empty() {
                    return Ok(true);
                }
            }
            Event::Start(element) | Event::Empty(element)
                if is_word
                    && matches!(
                        element.local_name().as_ref(),
                        b"tab"
                            | b"br"
                            | b"cr"
                            | b"sym"
                            | b"drawing"
                            | b"pict"
                            | b"object"
                            | b"footnoteReference"
                            | b"endnoteReference"
                    ) =>
            {
                return Ok(true);
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn story_hyperlinks(source: &[u8], item_range: Range<usize>) -> Result<Vec<(Range<usize>, bool)>> {
    source
        .get(item_range.clone())
        .ok_or_else(|| Error::Other("story item range is outside updated XML".to_owned()))?;
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut ranges = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("empty hyperlink scan failed: {error}")))?;
        let is_word = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if after <= item_range.start {
            buffer.clear();
            continue;
        }
        if before >= item_range.end {
            break;
        }
        if let Event::Start(element) = event {
            if is_word && element.local_name().as_ref() == b"hyperlink" {
                let end = story_element_end(source, before)?;
                let visible = hyperlink_has_visible_content(source, before..end)?;
                ranges.push((before..end, visible));
            }
        } else if matches!(event, Event::Eof) {
            break;
        }
        buffer.clear();
    }
    Ok(ranges)
}

pub(super) fn remove_newly_empty_hyperlinks(
    before: &[u8],
    before_item: Range<usize>,
    after: &[u8],
    after_item: Range<usize>,
) -> Result<Vec<u8>> {
    let old_links = story_hyperlinks(before, before_item)?;
    let new_links = story_hyperlinks(after, after_item)?;
    let ranges = new_links
        .into_iter()
        .enumerate()
        .filter_map(|(index, (range, visible))| {
            (!visible
                && old_links
                    .get(index)
                    .is_some_and(|(_, old_visible)| *old_visible))
            .then_some(range)
        })
        .collect::<Vec<_>>();
    if ranges.is_empty() {
        return Ok(after.to_vec());
    }
    let mut updated = Vec::with_capacity(after.len());
    let mut cursor = 0usize;
    for range in ranges {
        updated.extend_from_slice(&after[cursor..range.start]);
        cursor = range.end;
    }
    updated.extend_from_slice(&after[cursor..]);
    Ok(updated)
}
