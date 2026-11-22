//! Complex fields (fldChar): events, complete results, projection and field spans.

use super::*;

#[derive(Debug)]
enum ComplexFieldEvent {
    Begin(Option<bool>, usize),
    Separate(Option<bool>),
    End(Option<bool>, usize),
    Instruction(String),
    Result(usize),
    Raw(usize),
    Display(&'static str),
    EmptyResult,
}

struct ComplexRunSources {
    start: std::ops::Range<usize>,
    properties: Option<std::ops::Range<usize>>,
    end: std::ops::Range<usize>,
    raw: Vec<Option<std::ops::Range<usize>>>,
}

// Read marker/code authority from the original raw-child slots. Typed result
// payloads, including recursive drawings, come from the one existing run parse.
fn complex_field_events(
    raw: &[u8],
    run: &CT_R,
    word_prefixes: &[String],
    legacy_consumed_owner: bool,
    publication: bool,
) -> Result<(Vec<ComplexFieldEvent>, Option<ComplexRunSources>)> {
    let mut reader = Reader::from_reader(raw);
    let run_prefixes = match reader.read_event()? {
        Event::Start(root) | Event::Empty(root) => {
            let prefixes = word_prefixes_at(&root, word_prefixes)?;
            if legacy_consumed_owner
                && matches_local_name(root.name().as_ref(), b"r")
                && !element_prefix_has_binding(root.name().as_ref(), &prefixes)
            {
                Cow::Owned(prefixes_with_assumed_word_owner(
                    root.name().as_ref(),
                    &prefixes,
                )?)
            } else {
                prefixes
            }
        }
        _ => return Ok((Vec::new(), None)),
    };
    if run.extra_xml.len() != run.extra_xml_positions.len() {
        return Err(OxmlError::InvalidValue(
            "complex run source positions changed".into(),
        ));
    }
    let mut sources = publication.then(|| ComplexRunSources {
        start: 0..reader.buffer_position() as usize,
        properties: None,
        end: raw.len()..raw.len(),
        raw: vec![None; run.extra_xml.len()],
    });
    let mut raw_children = Vec::new();
    let property = usize::from(run.properties.is_some());
    // Raw positions count source modeled children. A custom note mark's w:t
    // advances that count but is owned by the preceding note item, so derive
    // the actual content boundary from qualified direct-child ordinals.
    let mut boundaries = vec![0usize];
    let mut typed = 0usize;
    let mut note_mark_empty = false;
    loop {
        let before = reader.buffer_position() as usize;
        let ordinal = boundaries.len() - 1;
        let (root, empty) = match reader.read_event()? {
            Event::Start(root) => (root, false),
            Event::Empty(root) => (root, true),
            Event::End(_) => {
                if let Some(sources) = &mut sources {
                    sources.end = before..reader.buffer_position() as usize;
                }
                break;
            }
            Event::Eof => break,
            Event::Comment(_) | Event::PI(_) => continue,
            Event::Text(text) if text.iter().all(|byte| byte.is_ascii_whitespace()) => continue,
            _ => {
                // Other text between a reference and its mark ends the slot.
                note_mark_empty = false;
                continue;
            }
        };
        let prefixes = word_prefixes_at(&root, &run_prefixes)?;
        let name = root.name();
        // The run reader keeps one child slot for properties it models and
        // skips any other `w:rPr`, self-closing or written empty.
        let property_element = is_word_element(name.as_ref(), b"rPr", &prefixes);
        let properties = property_element && !empty && run.properties.is_some();
        if property_element && !properties {
            if !empty {
                reader.read_to_end_into(root.name(), &mut Vec::new())?;
            }
            note_mark_empty = false;
            continue;
        }
        let text = !empty && is_word_element(name.as_ref(), b"t", &prefixes);
        let modeled = properties
            || text
            || (!empty
                && [b"delText".as_slice(), b"drawing", b"commentReference"]
                    .iter()
                    .any(|local| is_word_element(name.as_ref(), local, &prefixes)))
            || (empty
                && ([
                    b"tab".as_slice(),
                    b"br",
                    b"footnoteReference",
                    b"endnoteReference",
                    b"commentReference",
                ]
                .iter()
                .any(|local| is_word_element(name.as_ref(), local, &prefixes))
                    || parse_typed_special_run_child(&root, &prefixes)?.is_some()));
        if text && note_mark_empty {
            // The first `w:t` is the mark, even an empty one.
            crate::xml_text::read_element_text(&mut reader, root.name());
            note_mark_empty = false;
        } else {
            if modeled && !properties {
                typed += 1;
                note_mark_empty = empty
                    && (is_word_element(name.as_ref(), b"footnoteReference", &prefixes)
                        || is_word_element(name.as_ref(), b"endnoteReference", &prefixes))
                    && optional_word_attribute(&root, b"customMarkFollows", &prefixes)
                        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "on"));
            }
            if !empty {
                reader.read_to_end_into(root.name(), &mut Vec::new())?;
            }
            // Any other child between a reference and its mark ends the mark
            // slot, except a rendered page break, as the note removal reads it.
            if (!modeled || properties)
                && !is_word_element(name.as_ref(), b"lastRenderedPageBreak", &prefixes)
            {
                note_mark_empty = false;
            }
        }
        if let Some(sources) = &mut sources {
            let range = before..reader.buffer_position() as usize;
            if properties {
                sources.properties = Some(range);
            } else if !modeled {
                raw_children.push((ordinal, range));
            }
        }
        if modeled {
            boundaries.push(property + typed);
        }
    }
    if let Some(captured) = &mut sources {
        let mut children = raw_children.into_iter();
        let mut complete = true;
        for (index, at) in run.extra_xml_positions.iter().enumerate() {
            if CT_R::raw_child_is_root_attributes(*at) {
                continue;
            }
            match children.next() {
                Some((ordinal, range)) if ordinal == CT_R::raw_child_position(*at) => {
                    captured.raw[index] = Some(range)
                }
                _ => {
                    complete = false;
                    break;
                }
            }
        }
        if children.next().is_some() {
            complete = false;
        }
        if !complete {
            sources = None;
        }
    }
    if typed != run.content.len() {
        return Err(OxmlError::InvalidValue(
            "complex run typed source cardinality changed".into(),
        ));
    }
    let mut children = run
        .extra_xml_positions
        .iter()
        .enumerate()
        .filter(|(_, at)| !CT_R::raw_child_is_root_attributes(**at))
        .map(|(index, at)| {
            boundaries
                .get(CT_R::raw_child_position(*at))
                .copied()
                .map(|boundary| (boundary, index))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("complex raw source boundary changed".into())
                })
        })
        .collect::<Result<Vec<_>>>()?;
    children.sort_by_key(|(boundary, index)| (*boundary, *index));
    let mut events = Vec::new();
    let mut raw_index = 0;
    let last = children
        .last()
        .map_or(property + run.content.len(), |(at, _)| {
            (*at).max(property + run.content.len())
        });
    for boundary in 0..=last {
        while let Some((at, index)) = children.get(raw_index).copied()
            && at == boundary
        {
            let mut source = Reader::from_reader(run.extra_xml[index].as_slice());
            let event = source.read_event()?;
            let paired = matches!(&event, Event::Start(_));
            match event {
                Event::Start(root) | Event::Empty(root) => {
                    let prefixes = word_prefixes_at(&root, &run_prefixes)?;
                    if is_word_element(root.name().as_ref(), b"fldChar", &prefixes) {
                        let before = events.len();
                        push_field_char_event(&mut events, &root, &prefixes, index);
                        if before == events.len() {
                            events.push(ComplexFieldEvent::Raw(index));
                        }
                    } else if is_word_element(root.name().as_ref(), b"instrText", &prefixes)
                        // Word writes the field code of a deleted field in
                        // delInstrText: it is still that field's instruction.
                        || is_word_element(root.name().as_ref(), b"delInstrText", &prefixes)
                    {
                        events.push(ComplexFieldEvent::Instruction(
                            crate::xml_text::read_element_text(&mut source, root.name()),
                        ));
                    } else {
                        // The legacy field display accepts paired tab/br even
                        // though the regular run parser preserves them raw.
                        // Keep that display policy without inventing typed facts.
                        if paired && is_word_element(root.name().as_ref(), b"tab", &prefixes) {
                            events.push(ComplexFieldEvent::Display("\t"));
                        } else if paired && is_word_element(root.name().as_ref(), b"br", &prefixes)
                        {
                            events.push(ComplexFieldEvent::Display(
                                match field_break_type(&root, &prefixes) {
                                    BreakType::Page => "\u{000c}",
                                    BreakType::Column => "\u{000b}",
                                    BreakType::Line => "\n",
                                },
                            ));
                        }
                        events.push(ComplexFieldEvent::Raw(index));
                    }
                }
                _ => events.push(ComplexFieldEvent::Raw(index)),
            }
            raw_index += 1;
        }
        if let Some(index) = boundary.checked_sub(property)
            && index < run.content.len()
        {
            events.push(ComplexFieldEvent::Result(index));
        }
    }
    if events.is_empty() {
        events.push(ComplexFieldEvent::EmptyResult);
    }
    Ok((events, sources))
}

pub(super) fn field_break_type(element: &BytesStart<'_>, word_prefixes: &[String]) -> BreakType {
    optional_word_attribute(element, b"type", word_prefixes)
        .map(|value| match value.as_str() {
            "page" => BreakType::Page,
            "column" => BreakType::Column,
            _ => BreakType::Line,
        })
        .unwrap_or(BreakType::Line)
}

fn push_field_char_event(
    events: &mut Vec<ComplexFieldEvent>,
    element: &BytesStart<'_>,
    word_prefixes: &[String],
    raw_slot: usize,
) {
    match optional_word_attribute(element, b"fldCharType", word_prefixes).as_deref() {
        Some("begin") => events.push(ComplexFieldEvent::Begin(
            optional_word_attribute(element, b"dirty", word_prefixes)
                .and_then(|value| parse_field_bool(&value)),
            raw_slot,
        )),
        Some("separate") => events.push(ComplexFieldEvent::Separate(
            optional_word_attribute(element, b"dirty", word_prefixes)
                .and_then(|value| parse_field_bool(&value)),
        )),
        Some("end") => events.push(ComplexFieldEvent::End(
            optional_word_attribute(element, b"dirty", word_prefixes)
                .and_then(|value| parse_field_bool(&value)),
            raw_slot,
        )),
        _ => {}
    }
}

// Source qualification only. No second model/parser construction is performed.
pub(crate) fn pure_complex_marker_run(
    raw: &[u8],
    inherited: &[String],
    kind: &str,
) -> Result<bool> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut scope = None;
    let mut marker = false;
    let mut properties = false;
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(root) if scope.is_none() => {
                let prefixes = word_prefixes_at(&root, inherited)?;
                if !is_word_element(root.name().as_ref(), b"r", &prefixes) {
                    return Ok(false);
                }
                scope = Some(prefixes);
            }
            Event::Start(child) | Event::Empty(child) => {
                let Some(parent) = scope.as_ref() else {
                    return Ok(false);
                };
                let prefixes = word_prefixes_at(&child, parent)?;
                let is_empty = raw.get(
                    (reader.buffer_position() as usize).saturating_sub(2)
                        ..reader.buffer_position() as usize,
                ) == Some(b"/>");
                if is_word_element(child.name().as_ref(), b"rPr", &prefixes)
                    && !properties
                    && !marker
                {
                    properties = true;
                } else if is_word_element(child.name().as_ref(), b"fldChar", &prefixes)
                    && optional_word_attribute(&child, b"fldCharType", &prefixes).as_deref()
                        == Some(kind)
                    && !marker
                {
                    marker = true;
                } else {
                    return Ok(false);
                }
                if !is_empty {
                    reader.read_to_end_into(child.name(), &mut Vec::new())?;
                }
            }
            Event::Text(text) if is_xml_whitespace(text.as_ref()) => {}
            Event::End(root)
                if scope.as_ref().is_some_and(|prefixes| {
                    is_word_element(root.name().as_ref(), b"r", prefixes)
                }) => {}
            Event::Eof => return Ok(scope.is_some() && marker),
            _ => return Ok(false),
        }
        buffer.clear();
    }
}

// The two phases prove the entire actual occurrence tree before consuming any facts.
struct CompleteResultGeometry {
    start: usize,
    end: usize,
    separate: usize,
    result_range: std::ops::Range<usize>,
    accepted_before: usize,
    tracked_before: usize,
    child_spans: Vec<(usize, usize, usize, usize)>,
}

fn complete_complex_result_geometry(
    field: &Field,
    span: std::ops::RangeInclusive<usize>,
    projection: &ComplexFieldProjection<'_>,
    complete_separators: &[(usize, usize, usize)],
    result_child_spans: &[(usize, usize, usize, usize)],
) -> Result<Option<CompleteResultGeometry>> {
    let start = *span.start();
    let end = *span.end();
    let ComplexFieldProjection {
        run_sources,
        comment_ranges,
        run_projection_ends,
        bookmark_markers,
        content_controls,
        revisions,
        hyperlinks,
        rubies,
        ..
    } = projection;
    let FieldSource::Parsed {
        raw_xml,
        word_prefixes: source_prefixes,
        ..
    } = field.source.as_ref()
    else {
        return Ok(None);
    };
    let mut bookmark_bounds =
        std::collections::BTreeMap::<i32, (usize, usize, usize, usize)>::new();
    for marker in bookmark_markers.iter() {
        if let Some(id) = marker.id {
            let facts = bookmark_bounds.entry(id).or_insert((usize::MAX, 0, 0, 0));
            facts.0 = facts.0.min(marker.run_index);
            facts.1 = facts.1.max(marker.run_index);
            if marker.start {
                facts.2 += 1;
            } else {
                facts.3 += 1;
            }
        }
    }
    let mut comment_bounds = std::collections::BTreeMap::<i32, (usize, usize, usize, usize)>::new();
    for marker in comment_ranges.iter() {
        let (id, start) = match marker {
            CommentRangeMarker::Start { id, .. } => (*id, true),
            CommentRangeMarker::End { id, .. } => (*id, false),
        };
        let facts = comment_bounds.entry(id).or_insert((usize::MAX, 0, 0, 0));
        facts.0 = facts.0.min(marker.run_index());
        facts.1 = facts.1.max(marker.run_index());
        if start {
            facts.2 += 1;
        } else {
            facts.3 += 1;
        }
    }
    let owned_geometry = if field_span_is_bare(raw_xml, source_prefixes) {
        complete_separators
            .iter()
            .find(|(first, last, _)| *first == start && *last == end)
            .and_then(|(_, _, separate)| {
                let CachedFieldSource::Complex {
                    result,
                    first_close,
                    last_open,
                    ..
                } = cached_field_source(raw_xml, FieldForm::Complex, source_prefixes).ok()?
                else {
                    return None;
                };
                let pure_marker = |index: usize, kind: &str| -> Option<bool> {
                    pure_complex_marker_run(run_sources[index].as_deref()?, source_prefixes, kind)
                        .ok()
                };
                if pure_marker(start, "begin") != Some(true)
                    || pure_marker(*separate, "separate") != Some(true)
                    || pure_marker(end, "end") != Some(true)
                    || *separate >= end
                    || result.start > first_close.start
                    || last_open.end > result.end
                    || (!raw_xml[result.start..first_close.start].is_empty()
                        && !is_xml_whitespace(&raw_xml[result.start..first_close.start]))
                    || (!raw_xml[last_open.end..result.end].is_empty()
                        && !is_xml_whitespace(&raw_xml[last_open.end..result.end]))
                    || hyperlinks.iter().any(|link| {
                        let overlaps = link.run_start <= end && link.run_end > start;
                        overlaps
                            && !(link.run_start > *separate && link.run_end <= end)
                            && !(link.run_start <= start && link.run_end > end)
                    })
                    || rubies
                        .iter()
                        .any(|ruby| ruby.base_start <= end && ruby.base_end > start)
                    || bookmark_markers.iter().any(|marker| {
                        if marker.run_index <= start || marker.run_index > end {
                            return false;
                        }
                        !marker
                            .id
                            .and_then(|id| bookmark_bounds.get(&id))
                            .is_some_and(|(first, last, starts, ends)| {
                                *first > *separate && *last <= end && *starts == 1 && *ends == 1
                            })
                    })
                    || content_controls
                        .iter()
                        .any(|(at, _, _, _)| *at > start && *at <= *separate)
                    || revisions
                        .iter()
                        .any(|(at, _, _)| *at > start && *at <= *separate)
                    || comment_ranges.iter().any(|marker| {
                        if marker.run_index() <= start || marker.run_index() > end {
                            return false;
                        }
                        let id = match marker {
                            CommentRangeMarker::Start { id, .. }
                            | CommentRangeMarker::End { id, .. } => id,
                        };
                        !comment_bounds
                            .get(id)
                            .is_some_and(|(first, last, starts, ends)| {
                                *first > *separate && *last <= end && *starts == 1 && *ends == 1
                            })
                    })
                {
                    return None;
                }
                let (_, accepted_before, tracked_before) = run_projection_ends
                    .iter()
                    .find(|(at, _, _)| at == separate)?;
                Some((
                    *separate,
                    first_close.end..last_open.start,
                    *accepted_before,
                    *tracked_before,
                ))
            })
    } else {
        None
    };
    let Some((separate, result_range, accepted_before, tracked_before)) = owned_geometry else {
        return Ok(None);
    };
    let child_spans = result_child_spans
        .iter()
        .filter(|(parent, _, _, _)| *parent == start)
        .copied()
        .collect::<Vec<_>>();
    Ok(Some(CompleteResultGeometry {
        start,
        end,
        separate,
        result_range,
        accepted_before,
        tracked_before,
        child_spans,
    }))
}

fn prepare_complete_complex_result(
    field: &Field,
    span: std::ops::RangeInclusive<usize>,
    projection: &ComplexFieldProjection<'_>,
    complete_separators: &[(usize, usize, usize)],
    result_child_spans: &[(usize, usize, usize, usize)],
) -> Result<bool> {
    let mut pending = vec![(field, span)];
    while let Some((field, span)) = pending.pop() {
        let Some(geometry) = complete_complex_result_geometry(
            field,
            span,
            projection,
            complete_separators,
            result_child_spans,
        )?
        else {
            return Ok(false);
        };
        let mut children = Vec::new();
        for (_, slot, first, last) in &geometry.child_spans {
            if *first <= geometry.separate || *last >= geometry.end {
                return Ok(false);
            }
            let child = if let Some(cache) = &field.parsed_cached_runs {
                cache
                    .iter()
                    .flat_map(|run| &run.content)
                    .filter_map(|content| match content {
                        RunContent::Field(child) => Some(child),
                        _ => None,
                    })
                    .nth(*slot)
            } else {
                field.cached_fields.get(*slot).map(|(_, child)| child)
            };
            let Some(child) = child.filter(|child| child.is_parsed_complex()) else {
                return Ok(false);
            };
            children.push((child, *first..=*last));
        }
        pending.extend(children.into_iter().rev());
    }
    Ok(true)
}

struct CompleteResultTransferFrame {
    field: Field,
    geometry: CompleteResultGeometry,
    cached_children: Vec<Option<Field>>,
    next_child: usize,
    atomic_children: Vec<(usize, usize, Field)>,
}

impl CompleteResultTransferFrame {
    pub(super) fn new(mut field: Field, geometry: CompleteResultGeometry) -> Self {
        let cached_children = if let Some(cache) = field.parsed_cached_runs.take() {
            cache
                .into_iter()
                .flat_map(|run| run.content)
                .filter_map(|content| match content {
                    RunContent::Field(child) => Some(Some(child)),
                    _ => None,
                })
                .collect()
        } else {
            std::mem::take(&mut field.cached_fields)
                .into_iter()
                .map(|(_, child)| Some(child))
                .collect()
        };
        Self {
            field,
            geometry,
            cached_children,
            next_child: 0,
            atomic_children: Vec::new(),
        }
    }
}

fn transfer_complete_complex_result(
    field: Field,
    span: std::ops::RangeInclusive<usize>,
    projection: &mut ComplexFieldProjection<'_>,
    complete_separators: &[(usize, usize, usize)],
    result_child_spans: &[(usize, usize, usize, usize)],
) -> Result<Option<Field>> {
    let Some(geometry) = complete_complex_result_geometry(
        &field,
        span,
        projection,
        complete_separators,
        result_child_spans,
    )?
    else {
        return Ok(None);
    };
    let mut pending = vec![CompleteResultTransferFrame::new(field, geometry)];
    loop {
        let parent = pending.last_mut().expect("root transfer frame");
        if let Some((_, slot, first, last)) =
            parent.geometry.child_spans.get(parent.next_child).copied()
        {
            parent.next_child += 1;
            let child = parent
                .cached_children
                .get_mut(slot)
                .and_then(Option::take)
                .ok_or_else(|| {
                    OxmlError::InvalidValue("complex result child occurrence was reused".into())
                })?;
            let Some(geometry) = complete_complex_result_geometry(
                &child,
                first..=last,
                projection,
                complete_separators,
                result_child_spans,
            )?
            else {
                return Ok(None);
            };
            pending.push(CompleteResultTransferFrame::new(child, geometry));
            continue;
        }
        let mut completed = pending.pop().expect("completed transfer frame");
        publish_complete_complex_result(
            &mut completed.field,
            &completed.geometry,
            completed.atomic_children,
            projection,
        )?;
        if let Some(parent) = pending.last_mut() {
            parent.atomic_children.push((
                completed.geometry.start,
                completed.geometry.end,
                completed.field,
            ));
        } else {
            return Ok(Some(completed.field));
        }
    }
}

fn publish_complete_complex_result(
    field: &mut Field,
    geometry: &CompleteResultGeometry,
    atomic_children: Vec<(usize, usize, Field)>,
    projection: &mut ComplexFieldProjection<'_>,
) -> Result<()> {
    let (end, separate, accepted_before, tracked_before) = (
        geometry.end,
        geometry.separate,
        geometry.accepted_before,
        geometry.tracked_before,
    );
    let result_range = geometry.result_range.clone();
    let origin_registry = projection
        .source_maps
        .as_deref()
        .map(|maps| Arc::clone(&maps.registry));
    let source_origin = origin_registry.as_ref().and_then(|registry| {
        registry
            .lock()
            .expect("field origin registry poisoned")
            .source_map(field)
    });
    let original_facts = origin_registry.as_ref().and_then(|registry| {
        registry
            .lock()
            .expect("field origin registry poisoned")
            .original_for(field)
    });
    let result_origin = source_origin
        .as_ref()
        .map(|map| map.slice(result_range.clone()))
        .transpose()?;
    let source = Arc::clone(&field.source);
    let FieldSource::Parsed { raw_xml, .. } = source.as_ref() else {
        unreachable!("qualified parsed complex field")
    };
    let ComplexFieldProjection {
        source_maps,
        runs,
        extra_xml,
        comment_ranges,
        run_projection_ends,
        bookmark_markers,
        content_controls,
        revisions,
        hyperlinks,
        ..
    } = projection;
    {
        let result_start = separate + 1;
        let mut paragraph = CT_P::default();
        // The marker run shells stay in the Field source. Only complete
        // result children become this logical paragraph's current facts.
        let mut boundaries = vec![0usize; end - result_start + 1];
        let child_intervals = atomic_children
            .iter()
            .map(|(first, last, _)| {
                let (_, first_accepted, first_tracked) = run_projection_ends
                    .iter()
                    .find(|(at, _, _)| at == first)
                    .expect("proved child begin origin");
                let (_, last_accepted, last_tracked) = run_projection_ends
                    .iter()
                    .find(|(at, _, _)| at == last)
                    .expect("proved child end origin");
                (
                    *first,
                    *last,
                    last_accepted - first_accepted,
                    last_tracked - first_tracked,
                )
            })
            .collect::<Vec<_>>();
        let mut at = result_start;
        let mut children = atomic_children.into_iter().peekable();
        while at < end {
            let logical = paragraph.runs.len();
            if children.peek().is_some_and(|(first, _, _)| *first == at) {
                let (_, last, child) = children.next().expect("proved child occurrence");
                for boundary in at..=last {
                    boundaries[boundary - result_start] = logical;
                }
                paragraph.runs.push(field_run(child, None));
                if let Some(maps) = source_maps.as_deref_mut() {
                    maps.retain_extras(extra_xml, |position| !(position > at && position <= last));
                }
                extra_xml.retain(|(position, _)| !(*position > at && *position <= last));
                at = last + 1;
            } else {
                boundaries[at - result_start] = logical;
                paragraph
                    .runs
                    .push(std::mem::replace(&mut runs[at], CT_R::new("")));
                at += 1;
            }
            boundaries[at - result_start] = paragraph.runs.len();
        }
        let remap = |position: usize| boundaries[position - result_start];
        let mut outside = Vec::new();
        if let Some(maps) = source_maps.as_deref_mut() {
            maps.extras = extra_xml
                .iter()
                .enumerate()
                .filter(|(_, (at, _))| !(*at >= result_start && *at <= end))
                .map(|(index, _)| maps.extras.get(index).cloned().flatten())
                .collect();
        }
        for (at, raw) in extra_xml.drain(..) {
            if at >= result_start && at <= end {
                paragraph.extra_xml.push((remap(at), raw));
            } else {
                outside.push((at, raw));
            }
        }
        **extra_xml = outside;
        let mut outside = Vec::new();
        let mut property_rows = source_maps
            .as_deref_mut()
            .filter(|maps| maps.collect_properties)
            .map(|maps| std::mem::take(&mut maps.ordinary_controls));
        let mut outside_properties = Vec::new();
        let control_slots = source_maps
            .as_deref_mut()
            .and_then(|maps| maps.retained_control_slots.take());
        let mut outside_control_slots = Vec::new();
        for (index, (at, before, markers, owner)) in content_controls.drain(..).enumerate() {
            if at >= result_start && at <= end {
                paragraph
                    .content_controls
                    .push((remap(at), before, markers, owner));
            } else {
                outside.push((at, before, markers, owner));
                if let Some(slots) = &control_slots {
                    outside_control_slots.push(slots[index]);
                }
                if let Some(rows) = &mut property_rows {
                    outside_properties
                        .push(rows.get_mut(index).map(std::mem::take).unwrap_or_default());
                }
            }
        }
        **content_controls = outside;
        if control_slots.is_some()
            && let Some(maps) = source_maps.as_deref_mut()
        {
            maps.retained_control_slots = Some(outside_control_slots);
        }
        if property_rows.is_some()
            && let Some(maps) = source_maps.as_deref_mut()
        {
            maps.ordinary_controls = outside_properties;
        }
        let mut outer_links = Vec::new();
        let mut link_slots = Vec::new();
        if let Some(maps) = source_maps.as_deref_mut() {
            maps.hyperlink_extras = hyperlinks
                .iter()
                .enumerate()
                .filter(|(_, link)| !(link.run_start >= result_start && link.run_end <= end))
                .map(|(index, _)| {
                    maps.hyperlink_extras
                        .get(index)
                        .cloned()
                        .unwrap_or_default()
                })
                .collect();
        }
        for mut link in hyperlinks.drain(..) {
            if link.run_start >= result_start && link.run_end <= end {
                link_slots.push((true, paragraph.hyperlinks.len()));
                link.run_start = remap(link.run_start);
                link.run_end = remap(link.run_end);
                paragraph.hyperlinks.push(link);
            } else {
                link_slots.push((false, outer_links.len()));
                outer_links.push(link);
            }
        }
        **hyperlinks = outer_links;
        let mut outside = Vec::new();
        let revision_slots = source_maps
            .as_deref_mut()
            .and_then(|maps| maps.retained_revision_slots.take());
        let mut outside_revision_slots = Vec::new();
        for (index, (at, mut before, owner)) in revisions.drain(..).enumerate() {
            if let Some(old_slot) = hyperlink_revision_index(before) {
                let (_, new_slot) = link_slots[old_slot];
                before = hyperlink_revision_slot(new_slot);
            }
            if at >= result_start && at <= end {
                paragraph.revisions.push((remap(at), before, owner));
            } else {
                outside.push((at, before, owner));
                if let Some(slots) = &revision_slots {
                    outside_revision_slots.push(slots[index]);
                }
            }
        }
        **revisions = outside;
        if revision_slots.is_some()
            && let Some(maps) = source_maps.as_deref_mut()
        {
            maps.retained_revision_slots = Some(outside_revision_slots);
        }
        let mut outside = Vec::new();
        for mut marker in comment_ranges.drain(..) {
            if marker.run_index() >= result_start && marker.run_index() <= end {
                match &mut marker {
                    CommentRangeMarker::Start { run_index, .. }
                    | CommentRangeMarker::End { run_index, .. } => *run_index = remap(*run_index),
                }
                paragraph.comment_ranges.push(marker);
            } else {
                outside.push(marker);
            }
        }
        **comment_ranges = outside;
        let mut outside = Vec::new();
        for mut marker in bookmark_markers.drain(..) {
            if marker.run_index >= result_start && marker.run_index <= end {
                let original_boundary = marker.run_index;
                marker.run_index = remap(marker.run_index);
                for (_, last, removed_accepted, removed_tracked) in &child_intervals {
                    if original_boundary > *last {
                        marker.projected_run_index -= removed_accepted;
                        marker.tracked_run_index -= removed_tracked;
                    }
                }
                marker.projected_run_index = marker
                    .projected_run_index
                    .checked_sub(accepted_before)
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("complex result accepted bookmark origin".into())
                    })?;
                marker.tracked_run_index = marker
                    .tracked_run_index
                    .checked_sub(tracked_before)
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("complex result tracked bookmark origin".into())
                    })?;
                paragraph.bookmark_markers.push(marker);
            } else {
                outside.push(marker);
            }
        }
        **bookmark_markers = outside;
        let cache = paragraph.accepted_text();
        let _publication =
            field.completed_result_publication_summary(&paragraph, result_range.len(), &cache)?;
        let normalized_xml = raw_xml[result_range.clone()].to_vec();
        // The borrowed original source has now supplied all facts and bytes.
        // Do not force COW merely by retaining this local qualification Arc.
        drop(source);
        field.cached_result = cache.clone();
        field.parsed_cached_runs = None;
        field.cached_fields.clear();
        field.derived_parent_cache = cache.clone();
        field.parsed_cached_comment_ranges.clear();
        paragraph.append_ordered_comment_ranges(&mut 0, &mut field.parsed_cached_comment_ranges);
        if let FieldSource::Parsed {
            original_cached_result,
            ..
        } = Arc::make_mut(&mut field.source)
        {
            *original_cached_result = cache.clone();
        }
        field.ordered_result = Some(OrderedFieldResult {
            paragraph: Arc::new(paragraph),
            normalized_xml: Arc::new(normalized_xml),
            source_range: result_range,
            normalized_cache: cache,
            pending: false,
            pending_payload: false,
            pending_payload_source_bytes: 0,
        });
        if let (Some(registry), Some(map)) = (origin_registry.as_ref(), source_origin) {
            let mut registry = registry.lock().expect("field origin registry poisoned");
            registry.register_forwarded(field, map, original_facts)?;
            if let Some(map) = result_origin {
                registry.register_result(
                    &field
                        .ordered_result
                        .as_ref()
                        .expect("published result")
                        .normalized_xml,
                    map,
                )?;
            }
        }
    }
    Ok(())
}

pub(crate) fn preflight_inline_field_copies(runs: &[&CT_R]) -> Result<()> {
    for run in runs {
        for content in &run.content {
            if let RunContent::Field(field) = content {
                crate::document::admit_word_model(crate::document::WordOwner::Field(field))?;
                field.normalization_work_budget()?;
            }
        }
    }
    Ok(())
}

/// Project qualified inline-control facts through the same canonical field parser.
/// The parse-only CT_P is moved ownership, never a serialized/reparsed envelope.
// Physical slot inventories, ownership mode and source-map context are distinct
// inputs to this existing shared constructor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn inline_field_paragraph_from_facts(
    content: Vec<crate::content_control::SdtContent>,
    revisions: Vec<(usize, CT_Revision)>,
    mut physical_runs: PhysicalRunSources,
    source: &[u8],
    spans: &[std::ops::Range<usize>],
    prefixes: &[String],
    complete_only: bool,
    mapped: Option<&crate::document::WordFieldInput>,
) -> Result<InlineFieldParagraph> {
    use crate::content_control::SdtContent;
    if content.len() != spans.len() || content.len() != physical_runs.len() {
        return Err(OxmlError::InvalidValue(
            "inline physical source occurrence cardinality changed".into(),
        ));
    }
    let mut source_maps = mapped.map(|mapped| {
        let (_, registry, initial) = mapped.projection_parts();
        WordProjectionSourceMaps::new(registry, initial, false)
    });
    let mut paragraph = CT_P::default();
    let mut run_sources = Vec::new();
    let mut run_coverage = Vec::new();
    let mut control_sources = Vec::new();
    let mut comment_sources = Vec::new();
    let mut retained_comments = Vec::new();
    let mut run_projection_ends = Vec::new();
    let mut accepted = 0usize;
    let mut tracked = 0usize;
    let mut revision_slots = revisions
        .into_iter()
        .map(|(at, revision)| (at, Some(revision)))
        .collect::<Vec<_>>();
    for (index, child) in content.into_iter().enumerate() {
        let raw = source
            .get(spans[index].clone())
            .ok_or_else(|| OxmlError::InvalidValue("inline source extent changed".into()))?;
        let child_map = mapped
            .map(|mapped| mapped.projection_parts().0.slice(spans[index].clone()))
            .transpose()?
            .map(Arc::new);
        match child {
            SdtContent::Run(run) => {
                paragraph.runs.push(run);
                run_coverage.push(index..index + 1);
                if let Some(maps) = &mut source_maps {
                    maps.runs
                        .push(child_map.filter(|_| physical_runs[index].as_deref() == Some(raw)));
                }
                run_sources.push(physical_runs[index].take());
                accepted += 1;
                tracked += 1;
                run_projection_ends.push((paragraph.runs.len() - 1, accepted, tracked));
            }
            SdtContent::ContentControl(control) => {
                let at = paragraph.runs.len();
                let before = raw_xml_count_at(&paragraph.extra_xml, at);
                let markers = paragraph
                    .comment_ranges
                    .iter()
                    .filter(|marker| marker.run_index() == at && marker.raw_before() == before)
                    .count();
                let projection = accepted_control_bookmark_projection(&control);
                append_nested_bookmark_projection(
                    &mut paragraph.bookmark_markers,
                    &mut accepted,
                    &mut tracked,
                    projection,
                    at,
                    before,
                );
                control_sources.push((at, before, markers, raw.to_vec()));
                if let Some(maps) = &mut source_maps {
                    maps.controls.push(child_map);
                }
                paragraph
                    .content_controls
                    .push((at, before, markers, control));
            }
            SdtContent::RawXml(raw) => {
                let child_map =
                    child_map.filter(|_| source.get(spans[index].clone()) == Some(raw.as_slice()));
                let at = paragraph.runs.len();
                let before = raw_xml_count_at(&paragraph.extra_xml, at);
                if let Some((_, revision)) =
                    revision_slots.iter_mut().find(|(slot, _)| *slot == index)
                {
                    let revision = revision.take().ok_or_else(|| {
                        OxmlError::InvalidValue("inline revision occurrence reused".into())
                    })?;
                    let projection = accepted_revision_bookmark_projection(&revision);
                    append_nested_bookmark_projection(
                        &mut paragraph.bookmark_markers,
                        &mut accepted,
                        &mut tracked,
                        projection,
                        at,
                        before,
                    );
                    paragraph.revisions.push((at, before, revision));
                    paragraph.extra_xml.push((at, raw));
                    if let Some(maps) = &mut source_maps {
                        maps.extras.push(child_map);
                    }
                    continue;
                }
                let mut reader = Reader::from_reader(raw.as_slice());
                let root = reader.read_event()?;
                if let Event::Start(root) | Event::Empty(root) = root {
                    let scope = word_prefixes_at(&root, prefixes)?;
                    let name = root.name();
                    if is_word_element(name.as_ref(), b"commentRangeStart", &scope)
                        || is_word_element(name.as_ref(), b"commentRangeEnd", &scope)
                    {
                        let Some(id) = optional_word_attribute(&root, b"id", &scope)
                            .and_then(|value| value.parse().ok())
                        else {
                            paragraph.extra_xml.push((at, raw));
                            if let Some(maps) = &mut source_maps {
                                maps.extras.push(child_map);
                            }
                            continue;
                        };
                        push_comment_marker(
                            &mut paragraph.comment_ranges,
                            &paragraph.extra_xml,
                            at,
                            id,
                            is_word_element(name.as_ref(), b"commentRangeStart", &scope),
                            raw_element_has_child_content(&raw),
                        );
                        comment_sources.push((
                            paragraph
                                .comment_ranges
                                .last()
                                .expect("captured marker")
                                .clone(),
                            raw.clone(),
                        ));
                        retained_comments.push((index, raw));
                        if let Some(maps) = &mut source_maps {
                            maps.comments.push(child_map);
                        }
                        continue;
                    }
                    if is_word_element(name.as_ref(), b"bookmarkStart", &scope)
                        || is_word_element(name.as_ref(), b"bookmarkEnd", &scope)
                    {
                        paragraph.bookmark_markers.push(BookmarkMarker {
                            start: is_word_element(name.as_ref(), b"bookmarkStart", &scope),
                            id: optional_word_attribute(&root, b"id", &scope)
                                .and_then(|value| value.parse().ok()),
                            name: optional_word_attribute(&root, b"name", &scope),
                            run_index: at,
                            raw_before: before,
                            projected_run_index: accepted,
                            tracked_run_index: tracked,
                            word_prefixes: scope.into_owned(),
                            has_child_content: raw_element_has_child_content(&raw),
                        });
                    }
                }
                paragraph.extra_xml.push((at, raw));
                if let Some(maps) = &mut source_maps {
                    maps.extras.push(child_map);
                }
            }
            _ => {
                return Err(OxmlError::InvalidValue(
                    "block content cannot enter qualified inline field projection".into(),
                ));
            }
        }
    }
    if revision_slots
        .iter()
        .any(|(_, revision)| revision.is_some())
    {
        return Err(OxmlError::InvalidValue(
            "inline revision source occurrence is missing".into(),
        ));
    }
    project_complex_fields(ComplexFieldProjection {
        source_maps: source_maps.as_mut(),
        legacy_consumed_owner: false,
        runs: &mut paragraph.runs,
        run_sources: &mut run_sources,
        simple_field_summaries: &[],
        extra_xml: &mut paragraph.extra_xml,
        comment_ranges: &mut paragraph.comment_ranges,
        comment_sources: &comment_sources,
        control_sources: &control_sources,
        hyperlink_sources: &[],
        run_projection_ends: &run_projection_ends,
        bookmark_markers: &mut paragraph.bookmark_markers,
        content_controls: &mut paragraph.content_controls,
        revisions: &mut paragraph.revisions,
        hyperlinks: &mut paragraph.hyperlinks,
        rubies: &mut paragraph.rubies,
        word_prefixes: prefixes,
        inline_complete_only: complete_only,
        run_coverage: Some(&mut run_coverage),
    })?;
    if complete_only {
        for (run, source) in paragraph.runs.iter().zip(&mut run_sources) {
            if source.is_none() {
                let [RunContent::Field(field)] = run.content.as_slice() else {
                    return Err(OxmlError::InvalidValue(
                        "inline projection introduced an unowned split run".into(),
                    ));
                };
                let FieldSource::Parsed { raw_xml, .. } = field.source.as_ref() else {
                    return Err(OxmlError::InvalidValue(
                        "inline projected field lacks physical source".into(),
                    ));
                };
                if field.span.is_some() {
                    return Err(OxmlError::InvalidValue(
                        "shared inline field cannot replace a physical span".into(),
                    ));
                }
                *source = Some(raw_xml.clone());
            }
        }
    }
    Ok((paragraph, run_sources, retained_comments, run_coverage))
}

struct ComplexFieldBuilder {
    publication_start: Option<crate::document::WordPublicationPiece>,
    publication_shells: Vec<crate::document::WordPublicationPiece>,
    publication_local_start: Option<std::ops::Range<usize>>,
    publication_local_shells: Vec<std::ops::Range<usize>>,
    original_marker: Option<OriginalFieldKey>,
    start_run: usize,
    separate_run: Option<usize>,
    dirty: Option<bool>,
    instruction: Vec<InstructionPart>,
    cached_result: String,
    cached_segments: Vec<CachedDisplaySegment>,
    cached_child_ranges: Vec<std::ops::Range<usize>>,
    cached_child_summaries: Vec<ParsedFieldSummary>,
    cached_runs: Vec<CT_R>,
    cached_run_index: Option<usize>,
    cached_physical_runs: Vec<usize>,
    valid: bool,
}

impl ComplexFieldBuilder {
    fn physical_cache_run(&mut self, index: usize, source: &CT_R) -> &mut CT_R {
        if self.cached_run_index != Some(index) {
            let mut run = CT_R {
                properties: source.properties.clone(),
                content: Vec::new(),
                extra_xml: Vec::new(),
                extra_xml_positions: Vec::new(),
                alt_drawings: Vec::new(),
            };
            // Shared physical partitions repeat the actual run shell and rPr,
            // just as field_span_parts does when splitting its source bytes.
            for (raw, at) in source.extra_xml.iter().zip(&source.extra_xml_positions) {
                if CT_R::raw_child_is_root_attributes(*at) {
                    run.extra_xml.push(raw.clone());
                    run.extra_xml_positions.push(*at);
                }
            }
            self.cached_runs.push(run);
            self.cached_physical_runs.push(index);
            self.cached_run_index = Some(index);
        }
        self.cached_runs.last_mut().expect("physical result run")
    }
}

pub(crate) type WordExtraSourceMaps = Vec<Option<Arc<crate::document::WordSourceMap>>>;

/// Paths are recorded at the actual successful model push, never reconstructed
/// by matching model values or by a second traversal of the parsed source.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WordBlockComponent {
    TableRow(usize),
    TableControl(usize),
    RowCell(usize),
    RowControl(usize),
    CellContent(usize),
    SdtContent(usize),
    ParagraphControl(usize),
    ParagraphRevision(usize),
    RevisionParagraph,
    ControlRevision(usize),
}

#[derive(Default)]
pub(crate) struct WordParagraphReadOutput<'a> {
    pub(crate) source_facts: Option<&'a mut crate::revision::RevisionSourceFacts>,
    pub(crate) extra_maps: Option<&'a mut WordExtraSourceMaps>,
    pub(crate) text_facts: Option<&'a mut WordParagraphTextFacts>,
    pub(crate) nested_maps: Option<&'a mut WordBlockSourceMaps>,
    pub(crate) ordinary: Option<&'a mut Vec<crate::document::WordOrdinaryFact>>,
}

#[derive(Clone, Default)]
pub(crate) struct WordBlockSourceMaps {
    pub(crate) paragraphs: Vec<(Vec<WordBlockComponent>, WordExtraSourceMaps)>,
    pub(crate) word_texts: Vec<(Vec<WordBlockComponent>, Option<WordParagraphReadFacts>)>,
    pub(crate) word_runs: Vec<(Vec<WordBlockComponent>, Option<ParsedWordRunTexts>)>,
    pub(crate) word_raws: Vec<(Vec<WordBlockComponent>, Arc<crate::document::WordSourceMap>)>,
    pub(crate) collect_word_texts: bool,
    pub(crate) collect_properties: bool,
    pub(crate) ordinary: Vec<crate::document::WordOrdinaryFact>,
}

impl WordBlockSourceMaps {
    pub(crate) fn paragraph(&mut self, maps: WordExtraSourceMaps) {
        self.paragraphs.push((Vec::new(), maps));
    }

    pub(crate) fn append(&mut self, component: WordBlockComponent, child: Self) {
        self.ordinary.extend(child.ordinary);
        for (mut path, maps) in child.paragraphs {
            path.insert(0, component.clone());
            self.paragraphs.push((path, maps));
        }
        for (mut path, facts) in child.word_texts {
            path.insert(0, component.clone());
            self.word_texts.push((path, facts));
        }
        for (mut path, map) in child.word_raws {
            path.insert(0, component.clone());
            self.word_raws.push((path, map));
        }
        for (mut path, facts) in child.word_runs {
            path.insert(0, component.clone());
            self.word_runs.push((path, facts));
        }
    }
}

impl WordProjectionSourceMaps {
    pub(super) fn new(
        registry: Arc<std::sync::Mutex<FieldOriginRegistry>>,
        initial_delivery: bool,
        text_facts_enabled: bool,
    ) -> Self {
        Self {
            text_facts_enabled,
            collect_properties: false,
            ordinary_runs: Vec::new(),
            ordinary_controls: Vec::new(),
            retained_control_slots: None,
            retained_revision_slots: None,
            texts: Vec::new(),
            runs: Vec::new(),
            extras: Vec::new(),
            comments: Vec::new(),
            controls: Vec::new(),
            hyperlink_sources: Vec::new(),
            hyperlink_extras: Vec::new(),
            registry,
            initial_delivery,
        }
    }

    pub(super) fn fill_gaps(&mut self, lengths: [usize; 5]) {
        self.runs.resize(lengths[0], None);
        if self.collect_properties {
            self.ordinary_runs.resize_with(lengths[0], || None);
        }
        if self.text_facts_enabled {
            self.texts.resize_with(lengths[0], || None);
        }
        self.extras.resize(lengths[1], None);
        self.comments.resize(lengths[2], None);
        self.controls.resize(lengths[3], None);
        if self.collect_properties {
            self.ordinary_controls.resize_with(lengths[3], Vec::new);
        }
        self.hyperlink_sources.resize(lengths[4], None);
    }

    fn retain_extras(&mut self, extras: &[(usize, Vec<u8>)], keep: impl Fn(usize) -> bool) {
        self.extras = extras
            .iter()
            .enumerate()
            .filter(|(_, (at, _))| keep(*at))
            .map(|(index, _)| self.extras.get(index).cloned().flatten())
            .collect();
    }
}

pub(super) fn project_complex_fields(projection: ComplexFieldProjection<'_>) -> Result<bool> {
    let ComplexFieldProjection {
        mut source_maps,
        inline_complete_only,
        legacy_consumed_owner,
        mut run_coverage,
        runs,
        run_sources,
        simple_field_summaries,
        extra_xml,
        comment_ranges,
        comment_sources,
        control_sources,
        hyperlink_sources,
        run_projection_ends,
        bookmark_markers,
        content_controls,
        revisions,
        hyperlinks,
        rubies,
        word_prefixes,
    } = projection;
    if runs.len() != run_sources.len() {
        return Ok(false);
    }
    let mut retain_namespace_frame = false;
    let mut stack = Vec::<ComplexFieldBuilder>::new();
    let mut completed = Vec::<(
        usize,
        usize,
        Field,
        Option<CT_RPr>,
        Option<Vec<ComplexSourcePart>>,
    )>::new();
    let mut complete_separators = Vec::new();
    let mut result_child_spans = Vec::new();
    let mut hyperlink_cursor = 0usize;
    // Original wrapper contexts remain immutable while fields and final shared
    // groups capture their physical parent. Current HyperlinkSpan is mutable.
    let mut hyperlink_scopes = vec![None; hyperlink_sources.len()];
    for run_index in 0..runs.len() {
        let Some(raw) = run_sources[run_index].as_deref() else {
            continue;
        };
        if let Some(child) = runs[run_index]
            .content
            .iter()
            .find_map(|content| match content {
                RunContent::Field(field) => Some(field),
                _ => None,
            })
        {
            for parent in &mut stack {
                if parent.separate_run.is_some() {
                    parent.cached_result.push_str(&child.cached_result);
                    for (text, properties) in child.cached_display_segments() {
                        push_cached_display_segment(
                            &mut parent.cached_segments,
                            run_index,
                            text,
                            properties,
                        );
                    }
                }
            }
            if let Some(parent) = stack.last_mut()
                && parent.separate_run.is_some()
            {
                let summary = match simple_field_summaries
                    .binary_search_by_key(&run_index, |(index, _)| *index)
                {
                    Ok(index) => simple_field_summaries[index].1,
                    Err(_) => ParsedFieldSummary::from_model(child)?,
                };
                parent.cached_child_summaries.push(summary);
                let end = parent.cached_result.len();
                parent
                    .cached_child_ranges
                    .push(end - child.cached_result.len()..end);
                parent
                    .cached_runs
                    .push(field_run(child.clone(), runs[run_index].properties.clone()));
                parent.cached_physical_runs.push(run_index);
                parent.cached_run_index = None;
            }
            continue;
        }

        // A flattened hyperlink run retains the hyperlink's source scope even
        // when that owner locally shadows a paragraph namespace declaration.
        while hyperlink_sources
            .get(hyperlink_cursor)
            .is_some_and(|(_, end, _)| *end <= run_index)
        {
            hyperlink_cursor += 1;
        }
        let owner = hyperlink_sources
            .get(hyperlink_cursor)
            .filter(|(start, end, _)| *start <= run_index && run_index < *end);
        if let Some((_, _, source)) = owner
            && hyperlink_scopes[hyperlink_cursor].is_none()
        {
            crate::document::admit_word_xml(source, word_prefixes, None)?;
            let mut reader = Reader::from_reader(source.as_slice());
            let prefixes = match reader.read_event()? {
                Event::Start(root) | Event::Empty(root) => word_prefixes_at(&root, word_prefixes)?,
                _ => {
                    return Err(OxmlError::InvalidValue(
                        "hyperlink source namespace root changed".into(),
                    ));
                }
            };
            hyperlink_scopes[hyperlink_cursor] = Some(prefixes);
        }
        let prefixes = if owner.is_some() {
            hyperlink_scopes[hyperlink_cursor].as_deref()
        } else {
            None
        };
        let publication_map = source_maps
            .as_deref()
            .and_then(|maps| maps.runs.get(run_index))
            .and_then(Option::as_ref)
            .filter(|map| map.has_publication())
            .cloned();
        let (events, publication_sources) = complex_field_events(
            raw,
            &runs[run_index],
            prefixes.unwrap_or(word_prefixes),
            legacy_consumed_owner,
            true,
        )?;
        for event in events {
            match event {
                ComplexFieldEvent::Begin(dirty, raw_slot) => stack.push(ComplexFieldBuilder {
                    publication_local_start: publication_sources
                        .as_ref()
                        .and_then(|sources| sources.raw.get(raw_slot))
                        .cloned()
                        .flatten(),
                    publication_local_shells: publication_sources
                        .as_ref()
                        .map(|sources| {
                            let mut shells = vec![sources.start.clone()];
                            shells.extend(sources.properties.iter().cloned());
                            shells
                        })
                        .unwrap_or_default(),
                    publication_start: publication_map
                        .as_ref()
                        .zip(publication_sources.as_ref())
                        .and_then(|(map, sources)| {
                            sources
                                .raw
                                .get(raw_slot)
                                .and_then(Option::as_ref)
                                .and_then(|range| map.publication_boundary(range.clone()))
                        }),
                    publication_shells: publication_map
                        .as_ref()
                        .zip(publication_sources.as_ref())
                        .map(|(map, sources)| {
                            let mut shells =
                                map.publication_selection(std::slice::from_ref(&sources.start));
                            if let Some(properties) = &sources.properties {
                                shells.extend(
                                    map.publication_selection(std::slice::from_ref(properties)),
                                );
                            }
                            shells
                        })
                        .unwrap_or_default(),
                    original_marker: source_maps
                        .as_deref()
                        .and_then(|maps| maps.runs.get(run_index))
                        .and_then(Option::as_deref)
                        .filter(|map| map.entirely_original())
                        .map(|map| OriginalFieldKey {
                            physical: map.original_pieces(),
                            marker: Some(raw_slot),
                        }),
                    start_run: run_index,
                    separate_run: None,
                    dirty,
                    instruction: Vec::new(),
                    cached_result: String::new(),
                    cached_segments: Vec::new(),
                    cached_child_ranges: Vec::new(),
                    cached_child_summaries: Vec::new(),
                    cached_runs: Vec::new(),
                    cached_run_index: None,
                    cached_physical_runs: Vec::new(),
                    valid: true,
                }),
                ComplexFieldEvent::Instruction(text) => {
                    if let Some(field) = stack.last_mut()
                        && field.separate_run.is_none()
                    {
                        field.instruction.push(InstructionPart::Text(text));
                    }
                }
                ComplexFieldEvent::Result(content_index) => {
                    let content = &runs[run_index].content[content_index];
                    let display = match content {
                        // The old field scanner does not publish unwrapped
                        // deletion text as a stored field display.
                        RunContent::DeletedText(_) => "",
                        RunContent::Break(BreakType::Page) => "\u{000c}",
                        RunContent::Break(BreakType::Column) => "\u{000b}",
                        _ => CT_R::content_text(content),
                    };
                    for field in &mut stack {
                        if field.separate_run.is_some() {
                            field.cached_result.push_str(display);
                            push_cached_display_segment(
                                &mut field.cached_segments,
                                run_index,
                                display,
                                runs[run_index].properties.as_ref(),
                            );
                        }
                    }
                    if let Some(field) = stack.last_mut()
                        && field.separate_run.is_some()
                    {
                        let cached = field.physical_cache_run(run_index, &runs[run_index]);
                        let mut content = content.clone();
                        if let RunContent::CommentReference { raw_before, .. } = &mut content {
                            *raw_before = cached.extra_xml.len();
                        }
                        cached.content.push(content);
                    }
                }
                ComplexFieldEvent::Display(display) => {
                    for field in &mut stack {
                        if field.separate_run.is_some() {
                            field.cached_result.push_str(display);
                            push_cached_display_segment(
                                &mut field.cached_segments,
                                run_index,
                                display,
                                runs[run_index].properties.as_ref(),
                            );
                        }
                    }
                }
                ComplexFieldEvent::EmptyResult => {
                    if let Some(field) = stack.last_mut()
                        && field.separate_run.is_some()
                    {
                        field.physical_cache_run(run_index, &runs[run_index]);
                    }
                }
                ComplexFieldEvent::Raw(index) => {
                    if let Some(field) = stack.last_mut()
                        && field.separate_run.is_some()
                    {
                        let source = &runs[run_index];
                        let cached = field.physical_cache_run(run_index, source);
                        let mut at = source.extra_xml_positions[index];
                        CT_R::set_raw_child_position(
                            &mut at,
                            usize::from(cached.properties.is_some()) + cached.content.len(),
                        );
                        cached.extra_xml.push(source.extra_xml[index].clone());
                        cached.extra_xml_positions.push(at);
                        if CT_R::raw_child_has_alternate_content_drawing(at) {
                            let drawing_index = source.extra_xml_positions[..index]
                                .iter()
                                .filter(|at| CT_R::raw_child_has_alternate_content_drawing(**at))
                                .count();
                            if let Some(drawing) = source.alt_drawings.get(drawing_index) {
                                cached.alt_drawings.push(drawing.clone());
                            }
                        }
                    }
                }
                ComplexFieldEvent::Separate(dirty) => {
                    if let Some(field) = stack.last_mut() {
                        merge_field_dirty(&mut field.dirty, dirty);
                        if field.separate_run.replace(run_index).is_some() {
                            field.valid = false;
                        }
                    }
                }
                ComplexFieldEvent::End(dirty, raw_slot) => {
                    let Some(mut field) = stack.pop() else {
                        continue;
                    };
                    merge_field_dirty(&mut field.dirty, dirty);
                    let (instruction_syntax, nested_fields, nested_summaries) =
                        parse_instruction_syntax(field.instruction);
                    let ComplexSourceAssembly {
                        parts: source_parts,
                        maps: source_part_maps,
                        transcript: source_transcript,
                    } = complex_field_source_parts(
                        field.start_run..=run_index,
                        run_sources,
                        extra_xml,
                        hyperlinks,
                        comment_sources,
                        control_sources,
                        hyperlink_sources,
                        source_maps.as_deref(),
                    );
                    let source_prefixes = if owner.is_some_and(|(first, last, _)| {
                        *first <= field.start_run && *last > run_index
                    }) {
                        prefixes.unwrap_or(word_prefixes)
                    } else {
                        word_prefixes
                    };
                    crate::document::admit_word_field_input(
                        crate::document::WordXmlInput::pieces(&source_parts),
                        source_prefixes,
                        source_maps.as_deref().map(|maps| &maps.registry),
                    )?;
                    let raw_bytes = source_parts
                        .iter()
                        .try_fold(0usize, |total, part| total.checked_add(part.len()))
                        .ok_or_else(|| {
                            OxmlError::InvalidValue("field source byte overflow".into())
                        })?;
                    let metadata = ParsedFieldMetadata::read(
                        crate::document::WordXmlInput::pieces(&source_parts),
                        FieldForm::Complex,
                        source_prefixes,
                        &instruction_syntax.name,
                        &field.cached_result,
                    );
                    let initial_summary = admit_initial_field_capture(
                        &instruction_syntax,
                        &nested_summaries,
                        &field.cached_result,
                        raw_bytes,
                        source_prefixes,
                        &metadata,
                        ParsedFieldCache::Runs {
                            runs: &field.cached_runs,
                            summaries: Some(&field.cached_child_summaries),
                        },
                        source_maps.as_deref().map(|maps| &maps.registry),
                    )?;
                    let origin = source_part_maps
                        .map(|parts| crate::document::WordSourceMap::concat(&parts))
                        .transpose()?;
                    let publication_owned = origin.as_ref().and_then(|origin| {
                        let start = field.publication_start.as_ref()?;
                        let (map, sources) =
                            publication_map.as_ref().zip(publication_sources.as_ref())?;
                        let end = map.publication_boundary(sources.raw.get(raw_slot)?.clone()?)?;
                        let mut shells = field.publication_shells;
                        shells
                            .extend(map.publication_selection(std::slice::from_ref(&sources.end)));
                        origin.publication_owner_ranges(start, &end, &shells)
                    });
                    let source = assemble_field_source(&source_parts)?;
                    let (instruction, nested_order) = instruction_syntax.materialize(nested_fields);
                    let mut parsed = Field::parsed(
                        instruction,
                        field.cached_result,
                        field.cached_segments,
                        field.dirty,
                        FieldForm::Complex,
                        (source, source_prefixes.to_vec()),
                        Some(metadata),
                    );
                    if let (
                        Some(start),
                        Some(sources),
                        FieldSource::Parsed {
                            publication_owned, ..
                        },
                    ) = (
                        field.publication_local_start.as_ref(),
                        publication_sources.as_ref(),
                        Arc::get_mut(&mut parsed.source).expect("newly constructed Field source"),
                    ) && let Some(end) = sources.raw.get(raw_slot).and_then(Option::as_ref)
                        && let Some(end_base) = raw_bytes.checked_sub(raw.len())
                    {
                        let mut ranges = field.publication_local_shells;
                        ranges.push(start.start..end_base + end.end);
                        ranges.push(end_base + sources.end.start..end_base + sources.end.end);
                        *publication_owned = Some(ranges);
                    }
                    parsed.normalization_work_budget()?;
                    parsed.nested_order = nested_order;
                    if let (Some(map), Some(maps)) = (origin, source_maps.as_deref()) {
                        maps.registry
                            .lock()
                            .expect("field origin registry poisoned")
                            .register_occurrence(
                                &parsed,
                                map,
                                field.original_marker.clone(),
                                maps.initial_delivery,
                            )?;
                        let publication_owned =
                            publication_owned.or_else(|| match parsed.source.as_ref() {
                                FieldSource::Parsed {
                                    publication_owned, ..
                                } => publication_owned.clone(),
                                FieldSource::New { .. } => None,
                            });
                        if let Some(ranges) = publication_owned {
                            maps.registry
                                .lock()
                                .expect("field origin registry poisoned")
                                .register_publication_ranges(&parsed, ranges);
                        }
                    }
                    if let Some(separate) = field.separate_run {
                        for marker in comment_ranges.iter().filter(|marker| {
                            marker.run_index() > separate
                                && marker.run_index() <= run_index
                                && !result_child_spans.iter().any(|(parent, _, first, last)| {
                                    *parent == field.start_run
                                        && marker.run_index() > *first
                                        && marker.run_index() <= *last
                                })
                        }) {
                            let mut marker = marker.clone();
                            let boundary = field
                                .cached_physical_runs
                                .partition_point(|at| *at < marker.run_index());
                            match &mut marker {
                                CommentRangeMarker::Start {
                                    run_index,
                                    raw_before,
                                    ..
                                }
                                | CommentRangeMarker::End {
                                    run_index,
                                    raw_before,
                                    ..
                                } => {
                                    *run_index = boundary;
                                    *raw_before = 0;
                                }
                            }
                            parsed.parsed_cached_comment_ranges.push(marker);
                        }
                    }
                    // A result that carries annotation markers keeps the
                    // legacy annotation range axis and its physical typed
                    // runs. The markers are not typed run content, so a flat
                    // run projection would lose the owner boundaries they
                    // delimit. Reuse the already-projected physical runs
                    // rather than reparsing a result interval that may cross
                    // a sibling field boundary.
                    let annotation_runs = field.separate_run.is_some()
                        && field.cached_runs.iter().any(|run| {
                            run.content.iter().any(|content| {
                                matches!(
                                    content,
                                    RunContent::FootnoteRef { .. }
                                        | RunContent::EndnoteRef { .. }
                                        | RunContent::CommentReference { .. }
                                )
                            })
                        });
                    if field.separate_run.is_some() {
                        if annotation_runs {
                            parsed.parsed_cached_runs = Some(field.cached_runs.clone());
                            parsed.cached_fields = field
                                .cached_child_ranges
                                .into_iter()
                                .zip(
                                    field
                                        .cached_runs
                                        .into_iter()
                                        .flat_map(|run| run.content)
                                        .filter_map(|content| match content {
                                            RunContent::Field(child) => Some(child),
                                            _ => None,
                                        }),
                                )
                                .collect();
                        } else {
                            parsed.parsed_cached_runs = Some(field.cached_runs);
                        }
                    } else {
                        parsed.cached_fields = field
                            .cached_child_ranges
                            .into_iter()
                            .zip(
                                field
                                    .cached_runs
                                    .into_iter()
                                    .flat_map(|run| run.content)
                                    .filter_map(|content| match content {
                                        RunContent::Field(child) => Some(child),
                                        _ => None,
                                    }),
                            )
                            .collect();
                    }
                    let valid = field.valid
                        && !parsed.instruction.name.is_empty()
                        && run_sources[field.start_run..=run_index]
                            .iter()
                            .all(Option::is_some);
                    if valid && let Some(separate) = field.separate_run {
                        complete_separators.push((field.start_run, run_index, separate));
                    }
                    if let Some(parent) = stack.last_mut() {
                        if !valid {
                            parent.valid = false;
                        } else if parent.separate_run.is_none() {
                            parent
                                .instruction
                                .push(InstructionPart::Nested(parsed, initial_summary));
                        } else {
                            result_child_spans.push((
                                parent.start_run,
                                parent.cached_child_ranges.len(),
                                field.start_run,
                                run_index,
                            ));
                            parent.cached_child_summaries.push(initial_summary);
                            let end = parent.cached_result.len();
                            let start = end.saturating_sub(parsed.cached_result.len());
                            parent.cached_child_ranges.push(start..end);
                            parent.cached_runs.push(field_run(parsed, None));
                            parent.cached_physical_runs.push(field.start_run);
                            parent.cached_run_index = None;
                        }
                    } else if valid {
                        completed.push((
                            field.start_run,
                            run_index,
                            parsed,
                            None,
                            source_transcript,
                        ));
                    }
                }
            }
        }
    }

    let mut grouped = Vec::<(
        usize,
        usize,
        Vec<(Field, Option<CT_RPr>)>,
        Vec<Option<Vec<ComplexSourcePart>>>,
    )>::new();
    for (start, end, field, properties, transcript) in completed {
        // Fields that share a physical run form one span, so the splice of
        // one never removes the run that holds the next.
        if let Some((_, group_end, fields, transcripts)) = grouped.last_mut()
            && start <= *group_end
        {
            *group_end = end.max(*group_end);
            fields.push((field, properties));
            transcripts.push(transcript);
        } else {
            grouped.push((start, end, vec![(field, properties)], vec![transcript]));
        }
    }
    for (start, end, mut fields, transcripts) in grouped.into_iter().rev() {
        let group_prefixes = hyperlink_sources
            .iter()
            .enumerate()
            .find(|(_, (first, last, _))| *first <= start && *last > end)
            .and_then(|(index, _)| hyperlink_scopes[index].as_deref())
            .unwrap_or(word_prefixes);
        checked_field_scope_bytes(group_prefixes)?;
        let mut owned_comment_ids = std::collections::BTreeSet::new();
        let mut cache_owners = fields.iter().map(|(field, _)| field).collect::<Vec<_>>();
        while let Some(field) = cache_owners.pop() {
            cache_owners.extend(field.all_nested_fields_in_source_order());
            let mut ranges = field.parsed_cached_comment_ranges.clone();
            for marker in &mut ranges {
                match marker {
                    CommentRangeMarker::Start { raw_before, .. }
                    | CommentRangeMarker::End { raw_before, .. } => *raw_before = 0,
                }
            }
            if !ranges.is_empty()
                && field
                    .cached_result_runs()
                    .is_some_and(|runs| validate_cached_comment_ranges(runs, &ranges).is_ok())
            {
                for marker in ranges {
                    match marker {
                        CommentRangeMarker::Start { id, .. }
                        | CommentRangeMarker::End { id, .. } => {
                            owned_comment_ids.insert(id);
                        }
                    }
                }
            }
        }
        let unrelated_comments = comment_ranges
            .iter()
            .filter(|marker| match marker {
                CommentRangeMarker::Start { id, .. } | CommentRangeMarker::End { id, .. } => {
                    !owned_comment_ids.contains(id)
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        let owned_bookmark = if fields.len() == 1 {
            legacy_form_own_bookmark_start(
                start,
                end,
                &fields[0].0,
                run_sources,
                bookmark_markers,
                word_prefixes,
            )
        } else {
            None
        };
        let ComplexSourceAssembly {
            parts: raw_parts,
            maps: raw_part_maps,
            transcript: group_transcript,
        } = complex_field_source_parts(
            start..=end,
            run_sources,
            extra_xml,
            hyperlinks,
            comment_sources,
            control_sources,
            hyperlink_sources,
            source_maps.as_deref(),
        );

        let raw_bytes = raw_parts
            .iter()
            .try_fold(0usize, |total, part| total.checked_add(part.len()))
            .ok_or_else(|| OxmlError::InvalidValue("field grouped source byte overflow".into()))?;
        for (field, _) in &fields {
            field.source_publication_summary(raw_bytes, None)?;
        }
        let group_origin = raw_part_maps
            .map(|parts| crate::document::WordSourceMap::concat(&parts))
            .transpose()?;
        let raw_xml = assemble_field_source(&raw_parts)?;
        if inline_complete_only {
            let Some((_, _, separate)) = complete_separators
                .iter()
                .find(|(first, last, _)| *first == start && *last == end)
            else {
                continue;
            };
            if fields.len() != 1
                || !field_span_is_bare(&raw_xml, word_prefixes)
                || !pure_complex_marker_run(
                    run_sources[start].as_deref().unwrap_or_default(),
                    word_prefixes,
                    "begin",
                )?
                || !pure_complex_marker_run(
                    run_sources[*separate].as_deref().unwrap_or_default(),
                    word_prefixes,
                    "separate",
                )?
                || !pure_complex_marker_run(
                    run_sources[end].as_deref().unwrap_or_default(),
                    word_prefixes,
                    "end",
                )?
            {
                continue;
            }
        }
        let leading_bookmarks = if fields.len() == 1 {
            leading_field_bookmarks(
                start,
                end,
                runs,
                bookmark_markers,
                comment_ranges,
                content_controls,
                revisions,
                hyperlinks,
            )
        } else {
            Vec::new()
        };
        let needs_current_owner = fields.len() == 1
            && (content_controls
                .iter()
                .any(|(at, _, _, _)| *at > start && *at <= end)
                || revisions.iter().any(|(at, _, _)| *at > start && *at <= end)
                || hyperlinks
                    .iter()
                    .any(|link| link.run_start > start && link.run_end <= end)
                || bookmark_markers
                    .iter()
                    .any(|marker| marker.run_index > start && marker.run_index <= end));
        let mut owner_projection = ComplexFieldProjection {
            source_maps: source_maps.as_deref_mut(),
            inline_complete_only,
            legacy_consumed_owner: false,
            run_coverage: None,
            runs,
            run_sources,
            simple_field_summaries,
            extra_xml,
            comment_ranges,
            comment_sources,
            control_sources,
            hyperlink_sources,
            run_projection_ends,
            bookmark_markers,
            content_controls,
            revisions,
            hyperlinks,
            rubies,
            word_prefixes,
        };
        let owns_current_result = needs_current_owner
            && prepare_complete_complex_result(
                &fields[0].0,
                start..=end,
                &owner_projection,
                &complete_separators,
                &result_child_spans,
            )?;
        if inline_complete_only && needs_current_owner && !owns_current_result {
            continue;
        }
        if owns_current_result {
            let (field, properties) = fields.pop().expect("one qualified field");
            let field = transfer_complete_complex_result(
                field,
                start..=end,
                &mut owner_projection,
                &complete_separators,
                &result_child_spans,
            )?
            .ok_or_else(|| {
                OxmlError::InvalidValue("proved complex ownership changed before transfer".into())
            })?;
            fields.push((field, properties));
        }
        if !owns_current_result
            && (has_typed_boundary_inside(
                start,
                end,
                &unrelated_comments,
                &[],
                content_controls,
                revisions,
                hyperlinks,
            ) || bookmark_markers.iter().enumerate().any(|(index, marker)| {
                marker.run_index > start
                    && marker.run_index <= end
                    && Some(index) != owned_bookmark
                    && !leading_bookmarks.contains(&index)
            }))
        {
            continue;
        }
        comment_ranges.retain(|marker| match marker {
            CommentRangeMarker::Start { id, .. } | CommentRangeMarker::End { id, .. } => {
                !owned_comment_ids.contains(id)
                    || !(marker.run_index() > start && marker.run_index() <= end)
            }
        });
        if let Some(maps) = source_maps.as_deref_mut() {
            maps.retain_extras(extra_xml, |at| !(at > start && at <= end));
        }
        extra_xml.retain(|(at, _)| !(*at > start && *at <= end));
        for (link_index, hyperlink) in hyperlinks.iter_mut().enumerate() {
            let hyperlink_start = hyperlink.run_start;
            if let Some(maps) = source_maps.as_deref_mut()
                && let Some(row) = maps.hyperlink_extras.get_mut(link_index)
            {
                *row = hyperlink
                    .extra_xml
                    .iter()
                    .enumerate()
                    .filter(|(_, (boundary, _, _))| {
                        let absolute = hyperlink_start + *boundary;
                        !(absolute > start && absolute <= end)
                    })
                    .map(|(index, _)| row.get(index).cloned().flatten())
                    .collect();
            }
            hyperlink.extra_xml.retain(|(boundary, _, _)| {
                let absolute = hyperlink_start + *boundary;
                !(absolute > start && absolute <= end)
            });
        }

        let destination = group_transcript.as_ref().map(|parts| {
            parts
                .iter()
                .map(|part| (part.kind, part.output.clone()))
                .collect::<std::collections::HashMap<_, _>>()
        });
        let publication = fields
            .iter()
            .zip(transcripts)
            .map(|((field, _), transcript)| {
                let FieldSource::Parsed {
                    publication_owned: Some(owned),
                    ..
                } = field.source.as_ref()
                else {
                    return None;
                };
                transcript
                    .as_ref()
                    .zip(destination.as_ref())
                    .and_then(|(source, destination)| {
                        relocate_complex_publication(owned, source, destination)
                    })
            })
            .collect();
        let origin_context = group_origin
            .as_ref()
            .zip(source_maps.as_deref().map(|maps| &maps.registry));
        let mut replacement_texts = Vec::new();
        let mut replacement_properties = Vec::new();
        let replacement = field_span_runs(
            &raw_xml,
            fields,
            group_prefixes,
            publication,
            origin_context,
            source_maps
                .as_deref()
                .is_some_and(|maps| maps.text_facts_enabled)
                .then_some(&mut replacement_texts),
            source_maps
                .as_deref()
                .is_some_and(|maps| maps.collect_properties)
                .then_some(&mut replacement_properties),
        )?;
        if let (Some(map), Some(maps)) = (group_origin, source_maps.as_deref()) {
            let mut registry = maps
                .registry
                .lock()
                .expect("field origin registry poisoned");
            for run in &replacement {
                for content in &run.content {
                    if let RunContent::Field(field) = content {
                        registry.register(field, map.clone())?;
                    }
                }
            }
        }
        let replacement_count = replacement.len();
        if let Some(coverage) = run_coverage.as_deref_mut() {
            let physical = coverage[start].start..coverage[end].end;
            coverage.splice(
                start..=end,
                std::iter::repeat_n(physical, replacement_count),
            );
        }
        runs.splice(start..=end, replacement);
        run_sources.splice(start..=end, std::iter::repeat_n(None, replacement_count));
        if let Some(maps) = source_maps.as_deref_mut() {
            maps.runs
                .splice(start..=end, std::iter::repeat_n(None, replacement_count));
            if maps.collect_properties {
                maps.ordinary_runs
                    .splice(start..=end, replacement_properties);
            }
            if maps.text_facts_enabled {
                maps.texts.splice(start..=end, replacement_texts);
            }
        }
        if let Some(index) = owned_bookmark {
            retain_namespace_frame = true;
            // The physical start remains inside the field's retained source.
            // Its semantic boundary precedes the virtual field run. The pure
            // begin contributed exactly one accepted and tracked run before it.
            let marker = &mut bookmark_markers[index];
            marker.run_index = start;
            marker.projected_run_index -= 1;
            marker.tracked_run_index -= 1;
        }
        for &index in leading_bookmarks.iter().filter(|_| !owns_current_result) {
            if Some(index) == owned_bookmark {
                continue;
            }
            // A marker before anything the field displays has no position of
            // its own: it reads at the field's start, as Word shows it. Each
            // physical run before it contributed one accepted and one
            // tracked run, now part of the single field slot.
            let marker = &mut bookmark_markers[index];
            let consumed = marker.run_index - start;
            marker.run_index = start;
            marker.projected_run_index = marker
                .projected_run_index
                .checked_sub(consumed)
                .ok_or_else(|| {
                    OxmlError::InvalidValue("complex leading accepted bookmark origin".into())
                })?;
            marker.tracked_run_index =
                marker
                    .tracked_run_index
                    .checked_sub(consumed)
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("complex leading tracked bookmark origin".into())
                    })?;
        }
        if owns_current_result {
            let (_, first_accepted, first_tracked) = run_projection_ends
                .iter()
                .find(|(at, _, _)| *at == start)
                .ok_or_else(|| {
                    OxmlError::InvalidValue("complex begin projection origin is missing".into())
                })?;
            let (_, last_accepted, last_tracked) = run_projection_ends
                .iter()
                .find(|(at, _, _)| *at == end)
                .ok_or_else(|| {
                    OxmlError::InvalidValue("complex end projection origin is missing".into())
                })?;
            let physical_after_begin = end - start;
            let accepted_extra = (last_accepted - first_accepted)
                .checked_sub(physical_after_begin)
                .ok_or_else(|| {
                    OxmlError::InvalidValue("complex accepted physical projection changed".into())
                })?;
            let tracked_extra = (last_tracked - first_tracked)
                .checked_sub(physical_after_begin)
                .ok_or_else(|| {
                    OxmlError::InvalidValue("complex tracked physical projection changed".into())
                })?;
            // The legacy remapper below already accounts for direct marker/run
            // compaction. Transferred boundary owners contributed additional
            // accepted/tracked runs that now live inside the single Field slot.
            for marker in bookmark_markers
                .iter_mut()
                .filter(|marker| marker.run_index > end)
            {
                marker.projected_run_index = marker
                    .projected_run_index
                    .checked_sub(accepted_extra)
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("complex outside accepted bookmark origin".into())
                    })?;
                marker.tracked_run_index = marker
                    .tracked_run_index
                    .checked_sub(tracked_extra)
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("complex outside tracked bookmark origin".into())
                    })?;
            }
        }
        remap_complex_field_boundaries(
            start,
            end,
            replacement_count,
            ComplexFieldBoundariesMut {
                extra_xml,
                comment_ranges,
                bookmark_markers,
                content_controls,
                revisions,
                hyperlinks,
                rubies,
            },
        );
    }
    Ok(retain_namespace_frame)
}

// A native TextInput can own one ordinary complex child in either region.
// Instruction ownership requires the single direct operand, never both regions.
fn legacy_ordinary_bookmarked_child(field: &Field) -> bool {
    if field.instruction.name != "FORMTEXT"
        || !field
            .legacy_form
            .as_ref()
            .is_some_and(|form| form.kind == LegacyFormFieldKind::TextInput)
    {
        return false;
    }
    let instruction = field.nested_fields_in_source_order();
    let result = field.cached_fields_in_source_order();
    let child = match (instruction.as_slice(), result.as_slice()) {
        ([], [child]) => *child,
        ([child], [])
            if matches!(field.instruction.arguments.as_slice(),
            [FieldArgument::Nested(argument)] if std::ptr::eq(argument.as_ref(), *child)) =>
        {
            *child
        }
        _ => return false,
    };
    if !matches!(
        child.source.as_ref(),
        FieldSource::Parsed {
            form: FieldForm::Complex,
            ..
        }
    ) || !(matches!(child.instruction.name.as_str(), "TIME" | "DATE")
        || child.instruction.name.starts_with('='))
        || child.legacy_form.is_some()
        || child.validate_legacy_form_owner().is_err()
        || !child.all_nested_fields_in_source_order().is_empty()
    {
        return false;
    }
    [field, child].into_iter().all(|owner| {
        owner.parsed_cached_comment_ranges.is_empty()
            && owner.typed_cached_comment_ranges.is_empty()
            && owner.cached_result_runs().is_none_or(|runs| {
                runs.iter()
                    .flat_map(|run| &run.content)
                    .all(|content| match content {
                        RunContent::Text(_) | RunContent::Tab | RunContent::Break(_) => true,
                        RunContent::Field(nested) => std::ptr::eq(nested, child),
                        _ => false,
                    })
            })
    })
}

// Word's own-name bookmark surrounds the result and instruction, with its
// start just after the pure begin run and its end just after the pure end run.
// Only this complete isolated pair can project onto one virtual field run.
fn legacy_form_own_bookmark_start(
    start: usize,
    end: usize,
    field: &Field,
    sources: &[Option<Vec<u8>>],
    markers: &[BookmarkMarker],
    word_prefixes: &[String],
) -> Option<usize> {
    field.validate_legacy_form_owner().ok()?;
    let name = field.legacy_form.as_ref()?.name.as_deref()?;
    if name.is_empty()
        || (!field.all_nested_fields_in_source_order().is_empty()
            && !legacy_ordinary_bookmarked_child(field))
    {
        return None;
    }
    // A simple instruction child is not modeled as an operand by the generic
    // parser. Inspect actual sources rather than treating that omission as zero
    // children. The bounded complex case also excludes private run owners in
    // either region, including instruction content without projected display.
    let nested = !field.all_nested_fields_in_source_order().is_empty();
    for raw in sources.get(start..=end)? {
        let raw = raw.as_deref()?;
        if parse_simple_field(raw, word_prefixes, None).ok()?.is_some() {
            return None;
        }
        if nested {
            let run = parse_run_raw(raw, word_prefixes, None).ok()?;
            // AlternateContent drawings and classified VML rules are typed
            // owners outside content. Scope records and opaque raw children
            // carry no additional owner and remain eligible.
            if !run.alt_drawings.is_empty()
                || run
                    .extra_xml_positions
                    .iter()
                    .any(|position| CT_R::raw_child_is_legacy_horizontal_rule(*position))
                || run.content.iter().any(|content| {
                    !matches!(
                        content,
                        RunContent::Text(_) | RunContent::Tab | RunContent::Break(_)
                    )
                })
            {
                return None;
            }
        }
    }
    for (index, begin) in [(start, true), (end, false)] {
        let raw = sources.get(index)?.as_deref()?;
        let run = parse_run_raw(raw, word_prefixes, None).ok()?;
        let raw_children = run
            .extra_xml_positions
            .iter()
            .filter(|encoded| !CT_R::raw_child_is_root_attributes(**encoded))
            .count();
        if !run.content.is_empty() || raw_children != 1 || !run.alt_drawings.is_empty() {
            return None;
        }
        let events = complex_field_events(raw, &run, word_prefixes, false, false)
            .ok()?
            .0;
        if !matches!(
            (begin, events.as_slice()),
            (true, [ComplexFieldEvent::Begin(_, _)]) | (false, [ComplexFieldEvent::End(_, _)])
        ) {
            return None;
        }
    }
    let mut starts = markers.iter().enumerate().filter(|(_, marker)| {
        marker.start
            && marker.name.as_deref() == Some(name)
            && marker.run_index == start + 1
            && !marker.has_child_content
    });
    let (index, marker) = starts.next()?;
    if starts.next().is_some() || marker.projected_run_index == 0 || marker.tracked_run_index == 0 {
        return None;
    }
    let id = marker.id?;
    let pair = markers
        .iter()
        .filter(|marker| marker.id == Some(id))
        .collect::<Vec<_>>();
    if pair.len() != 2
        || !pair
            .iter()
            .any(|marker| !marker.start && marker.run_index == end + 1 && !marker.has_child_content)
    {
        return None;
    }
    // A carried paragraph frame must not rebind a generated writer prefix.
    // The shared cache scope names current generated inline namespaces. SDT
    // repeating-section types additionally emit w15 without a local binding.
    let mut reader = Reader::from_reader(FIELD_CACHE_VALIDATION_SCOPE);
    let Event::Start(scope) = reader.read_event().ok()? else {
        return None;
    };
    let mut reserved = raw_namespace_declarations(&scope).ok()?;
    reserved.push((
        "w15".into(),
        "http://schemas.microsoft.com/office/word/2012/wordml".into(),
    ));
    if namespace_bindings(word_prefixes)
        .iter()
        .any(|(prefix, uri)| {
            reserved
                .iter()
                .any(|(reserved, expected)| prefix == reserved && uri != expected)
        })
    {
        return None;
    }
    Some(index)
}

// Bookmark markers inside a complex field before any run it displays: among
// its instruction runs, or at the start of its result, as LibreOffice writes
// a form field's bookmarks. Only a span with no other typed boundary there
// qualifies: each physical run before such a marker is then one accepted and
// one tracked run.
#[allow(clippy::too_many_arguments)]
fn leading_field_bookmarks(
    start: usize,
    end: usize,
    runs: &[CT_R],
    bookmark_markers: &[BookmarkMarker],
    comment_ranges: &[CommentRangeMarker],
    content_controls: &[(usize, usize, usize, CT_Sdt)],
    revisions: &[(usize, usize, CT_Revision)],
    hyperlinks: &[HyperlinkSpan],
) -> Vec<usize> {
    let first_displayed = (start + 1..end)
        .find(|index| runs.get(*index).is_none_or(|run| !run_shows_nothing(run)))
        .unwrap_or(end);
    let markers = bookmark_markers
        .iter()
        .enumerate()
        .filter(|(_, marker)| {
            marker.run_index > start
                && marker.run_index <= first_displayed
                && !marker.has_child_content
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if markers.is_empty()
        || has_typed_boundary_inside(
            start,
            first_displayed,
            comment_ranges,
            &[],
            content_controls,
            revisions,
            hyperlinks,
        )
    {
        return Vec::new();
    }
    markers
}

// A run with no typed content, no drawing and no raw child but field
// characters and instruction text: a field's own marker or instruction run,
// or an empty run.
fn run_shows_nothing(run: &CT_R) -> bool {
    run.content.is_empty()
        && run.alt_drawings.is_empty()
        && run
            .extra_xml
            .iter()
            .zip(&run.extra_xml_positions)
            .all(|(raw, at)| {
                CT_R::raw_child_is_root_attributes(*at) || {
                    let mut reader = Reader::from_reader(raw.as_slice());
                    matches!(
                        reader.read_event(),
                        Ok(Event::Start(element) | Event::Empty(element))
                            if matches!(
                                element.local_name().as_ref(),
                                b"fldChar" | b"instrText" | b"delInstrText"
                            )
                    )
                }
            })
}

fn merge_field_dirty(current: &mut Option<bool>, next: Option<bool>) {
    if next == Some(true) || current.is_none() {
        *current = next;
    }
}

fn push_cached_display_segment(
    segments: &mut Vec<CachedDisplaySegment>,
    run_index: usize,
    text: &str,
    properties: Option<&CT_RPr>,
) {
    if let Some(segment) = segments.last_mut()
        && segment.run_index == run_index
    {
        segment.text.push_str(text);
        return;
    }
    segments.push(CachedDisplaySegment {
        run_index,
        text: text.to_owned(),
        properties: properties.cloned(),
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ComplexSourceKind {
    Run(usize),
    Hyperlink(usize),
    Control(usize),
    Comment(usize),
    Extra(usize),
    HyperlinkExtra(usize, usize),
}

struct ComplexSourcePart {
    kind: ComplexSourceKind,
    output: std::ops::Range<usize>,
}

// These are coordinates from the actual assembly calls in this parser pass.
// Component indices never escape into a later model/context identity lookup.
fn relocate_complex_publication(
    owned: &[std::ops::Range<usize>],
    source: &[ComplexSourcePart],
    destination: &std::collections::HashMap<ComplexSourceKind, std::ops::Range<usize>>,
) -> Option<Vec<std::ops::Range<usize>>> {
    let mut ranges = Vec::new();
    for range in owned {
        let mut covered = 0usize;
        for part in source {
            let start = range.start.max(part.output.start);
            let end = range.end.min(part.output.end);
            if start < end {
                let target = destination.get(&part.kind)?;
                if target.len() != part.output.len() {
                    return None;
                }
                ranges.push(
                    target.start + start - part.output.start
                        ..target.start + end - part.output.start,
                );
                covered = covered.checked_add(end - start)?;
            }
        }
        if covered != range.len() {
            return None;
        }
    }
    Some(ranges)
}

struct ComplexSourceAssembly<'a> {
    parts: Vec<&'a [u8]>,
    maps: Option<Vec<crate::document::WordSourceMap>>,
    transcript: Option<Vec<ComplexSourcePart>>,
}

#[allow(clippy::too_many_arguments)]
fn complex_field_source_parts<'a>(
    span: std::ops::RangeInclusive<usize>,
    run_sources: &'a [Option<Vec<u8>>],
    extra_xml: &'a [(usize, Vec<u8>)],
    hyperlinks: &'a [HyperlinkSpan],
    comment_sources: &'a [(CommentRangeMarker, Vec<u8>)],
    control_sources: &'a [(usize, usize, usize, Vec<u8>)],
    hyperlink_sources: &'a [(usize, usize, Vec<u8>)],
    maps: Option<&WordProjectionSourceMaps>,
) -> ComplexSourceAssembly<'a> {
    let (start, end) = span.into_inner();
    let mut source = Vec::<&'a [u8]>::new();
    let mut selected_maps = maps.map(|_| Vec::new());
    let mut transcript = Some(Vec::new());
    let mut position = Some(0usize);
    let mut push = |bytes: &'a [u8], map: Option<&Arc<crate::document::WordSourceMap>>, kind| {
        if let Some(start) = position {
            if let Some(end) = start.checked_add(bytes.len()) {
                transcript
                    .as_mut()
                    .expect("live assembly transcript")
                    .push(ComplexSourcePart {
                        kind,
                        output: start..end,
                    });
                position = Some(end);
            } else {
                position = None;
                transcript = None;
            }
        }
        source.push(bytes);
        if let Some(selected) = &mut selected_maps {
            selected.push(map.map_or_else(
                || crate::document::WordSourceMap::generated(bytes.len()),
                |map| (**map).clone(),
            ));
        }
    };
    for (run_index, raw_source) in run_sources.iter().enumerate().take(end + 1).skip(start) {
        let owned_hyperlink = hyperlink_sources
            .iter()
            .enumerate()
            .find(|(_, (first, last, _))| {
                *first > start && *last <= end && run_index >= *first && run_index < *last
            });
        if let Some((index, (first, last, raw))) = owned_hyperlink {
            if run_index == *first {
                push(
                    raw,
                    maps.and_then(|maps| maps.hyperlink_sources.get(index))
                        .and_then(Option::as_ref),
                    ComplexSourceKind::Hyperlink(index),
                );
            }
            if run_index + 1 < *last {
                continue;
            }
        } else if let Some(raw) = raw_source.as_deref() {
            push(
                raw,
                maps.and_then(|maps| maps.runs.get(run_index))
                    .and_then(Option::as_ref),
                ComplexSourceKind::Run(run_index),
            );
        }
        if run_index < end {
            let extras = extra_xml
                .iter()
                .enumerate()
                .filter(|(_, (at, _))| *at == run_index + 1)
                .collect::<Vec<_>>();
            for slot in 0..=extras.len() {
                let markers = comment_sources
                    .iter()
                    .enumerate()
                    .filter(|(_, (marker, _))| {
                        marker.run_index() == run_index + 1 && marker.raw_before() == slot
                    })
                    .collect::<Vec<_>>();
                for marker_slot in 0..=markers.len() {
                    for (index, (_, _, _, raw)) in control_sources.iter().enumerate().filter(
                        |(_, (at, before, markers_before, _))| {
                            *at == run_index + 1
                                && *before == slot
                                && *markers_before == marker_slot
                        },
                    ) {
                        push(
                            raw,
                            maps.and_then(|maps| maps.controls.get(index))
                                .and_then(Option::as_ref),
                            ComplexSourceKind::Control(index),
                        );
                    }
                    if let Some((index, (_, raw))) = markers.get(marker_slot) {
                        push(
                            raw,
                            maps.and_then(|maps| maps.comments.get(*index))
                                .and_then(Option::as_ref),
                            ComplexSourceKind::Comment(*index),
                        );
                    }
                }
                if let Some((index, (_, raw))) = extras.get(slot) {
                    push(
                        raw,
                        maps.and_then(|maps| maps.extras.get(*index))
                            .and_then(Option::as_ref),
                        ComplexSourceKind::Extra(*index),
                    );
                }
            }
            for (index, hyperlink) in hyperlinks
                .iter()
                .enumerate()
                .filter(|(_, hyperlink)| hyperlink.run_start <= start && hyperlink.run_end > end)
            {
                let relative = run_index + 1 - hyperlink.run_start;
                for (child, (_, _, raw)) in hyperlink
                    .extra_xml
                    .iter()
                    .enumerate()
                    .filter(|(_, (boundary, _, _))| *boundary == relative)
                {
                    push(
                        raw,
                        maps.and_then(|maps| maps.hyperlink_extras.get(index))
                            .and_then(|row| row.get(child))
                            .and_then(Option::as_ref),
                        ComplexSourceKind::HyperlinkExtra(index, child),
                    );
                }
            }
        }
    }
    ComplexSourceAssembly {
        parts: source,
        maps: selected_maps,
        transcript,
    }
}

fn assemble_field_source(parts: &[&[u8]]) -> Result<Vec<u8>> {
    let bytes = parts
        .iter()
        .try_fold(0usize, |total, part| total.checked_add(part.len()))
        .ok_or_else(|| OxmlError::InvalidValue("field source byte overflow".into()))?;
    let mut output = Vec::with_capacity(bytes);
    for part in parts {
        output.extend_from_slice(part);
    }
    Ok(output)
}

/// One part of a physical field span. Repeated shells retain the same physical
/// ranges instead of turning assembled bytes into a fresh source identity.
pub(super) enum FieldSpanPart {
    Field(Vec<std::ops::Range<usize>>),
    Outside(CT_R, Vec<std::ops::Range<usize>>),
}

impl FieldSpanPart {
    pub(super) fn materialize_source(
        raw: &[u8],
        ranges: &[std::ops::Range<usize>],
    ) -> Result<Vec<u8>> {
        let parts = ranges
            .iter()
            .map(|range| {
                raw.get(range.clone()).ok_or_else(|| {
                    OxmlError::InvalidValue("field span source range is invalid".into())
                })
            })
            .collect::<Result<Vec<_>>>()?;
        assemble_field_source(&parts)
    }
}

pub(super) struct FieldSpanPropertyRead<'a> {
    map: &'a crate::document::WordSourceMap,
    registry: &'a Arc<std::sync::Mutex<FieldOriginRegistry>>,
    output: &'a mut Vec<Option<crate::document::WordOrdinaryFact>>,
}

/// Split the exact bytes of a complex field span into its fields and the run
/// content outside them, in order.
///
/// Each part is written as its own runs, so a part taken from a shared run
/// repeats that run's start tag and `w:rPr`. Returns `None` for a shape this
/// does not model, which leaves the span as it was read.
pub(super) fn field_span_parts(
    raw: &[u8],
    word_prefixes: &[String],
    text_source: Option<(&crate::document::WordSourceMap, &mut WordParagraphTextFacts)>,
    ordinary_source: Option<FieldSpanPropertyRead<'_>>,
) -> Result<Option<Vec<FieldSpanPart>>> {
    use std::ops::Range;

    let text_map = text_source.as_ref().map(|(map, _)| *map);
    let property_map = ordinary_source.as_ref().map(|source| source.map);
    let current_map = text_map.or(property_map);
    let mut outside_properties = Vec::new();
    // The ordinary None consumer keeps its original enum and Vec layout.
    let mut outside_texts = Vec::new();
    // A run's start tag, `w:rPr` and end tag.
    let mut runs = Vec::<(Range<usize>, Option<Range<usize>>, Range<usize>)>::new();
    // Each part: whether it is a field, and its children (run index, bytes).
    let mut parts = Vec::<(bool, Vec<(Option<usize>, Range<usize>)>)>::new();
    let mut current: Option<usize> = None;
    let mut depth = 0usize;
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer)?;
        let (run_start, run_prefixes) = match event {
            Event::Start(start) => {
                let prefixes = word_prefixes_at(&start, word_prefixes)?;
                if is_word_element(start.name().as_ref(), b"r", &prefixes) {
                    (before..reader.buffer_position() as usize, prefixes)
                } else {
                    reader.read_to_end_into(start.name(), &mut Vec::new())?;
                    let range = before..reader.buffer_position() as usize;
                    let Some(part) = current.filter(|_| depth > 0) else {
                        return Ok(None);
                    };
                    parts[part].1.push((None, range));
                    buffer.clear();
                    continue;
                }
            }
            Event::Eof => break,
            _ => {
                let range = before..reader.buffer_position() as usize;
                let Some(part) = current.filter(|_| depth > 0) else {
                    return Ok(None);
                };
                parts[part].1.push((None, range));
                buffer.clear();
                continue;
            }
        };
        let run = runs.len();
        runs.push((run_start, None, 0..0));
        let mut children = 0usize;
        loop {
            buffer.clear();
            let child_start = reader.buffer_position() as usize;
            let kind = match reader.read_event_into(&mut buffer)? {
                Event::Start(child) => {
                    let prefixes = word_prefixes_at(&child, &run_prefixes)?;
                    let kind = field_span_child_kind(&child, &prefixes);
                    reader.read_to_end_into(child.name(), &mut Vec::new())?;
                    kind
                }
                Event::Empty(child) => {
                    let prefixes = word_prefixes_at(&child, &run_prefixes)?;
                    field_span_child_kind(&child, &prefixes)
                }
                Event::End(_) => {
                    let end = reader.buffer_position() as usize;
                    if children == 0 {
                        let Some(part) = current.filter(|_| depth > 0) else {
                            return Ok(None);
                        };
                        parts[part].1.push((Some(run), child_start..child_start));
                    }
                    runs[run].2 = child_start..end;
                    break;
                }
                Event::Eof => return Ok(None),
                _ => FieldSpanChild::Other,
            };
            let range = child_start..reader.buffer_position() as usize;
            if kind == FieldSpanChild::Properties {
                runs[run].1 = Some(range);
                continue;
            }
            children += 1;
            let inside = match kind {
                FieldSpanChild::Begin => {
                    if depth == 0 {
                        parts.push((true, Vec::new()));
                        current = Some(parts.len() - 1);
                    }
                    depth += 1;
                    true
                }
                FieldSpanChild::End => {
                    if depth == 0 {
                        return Ok(None);
                    }
                    depth -= 1;
                    true
                }
                FieldSpanChild::FieldMarker => {
                    if depth == 0 {
                        return Ok(None);
                    }
                    true
                }
                FieldSpanChild::Other | FieldSpanChild::Properties => depth > 0,
            };
            if !inside && current.is_none_or(|part| parts[part].0) {
                parts.push((false, Vec::new()));
                current = Some(parts.len() - 1);
            }
            let part = current.expect("a part is open");
            parts[part].1.push((Some(run), range));
            if kind == FieldSpanChild::End && depth == 0 {
                current = None;
            }
        }
        buffer.clear();
    }
    if depth != 0 {
        return Ok(None);
    }

    let source_ranges = |children: &[(Option<usize>, Range<usize>)]| {
        let mut ranges = Vec::new();
        let mut open = None;
        for (run, range) in children {
            if *run != open {
                if let Some(open) = open {
                    ranges.push(runs[open].2.clone());
                }
                if let Some(run) = *run {
                    ranges.push(runs[run].0.clone());
                    if let Some(properties) = runs[run].1.clone() {
                        ranges.push(properties);
                    }
                }
                open = *run;
            }
            ranges.push(range.clone());
        }
        if let Some(open) = open {
            ranges.push(runs[open].2.clone());
        }
        ranges
    };

    // Content outside every field that no reader sees, such as whitespace or
    // an unknown element, stays with its neighbouring field as before.
    let mut outside = Vec::with_capacity(parts.len());
    let mut index = 0;
    while index < parts.len() {
        if parts[index].0 {
            outside.push(None);
            if text_map.is_some() {
                outside_texts.push(None);
            }
            if ordinary_source.is_some() {
                outside_properties.push(None);
            }
            index += 1;
            continue;
        }
        let ranges = source_ranges(&parts[index].1);
        let raw_run = FieldSpanPart::materialize_source(raw, &ranges)?;
        let mut texts = Vec::new();
        let map = current_map.and_then(|map| {
            let pieces = ranges
                .iter()
                .map(|range| map.slice(range.clone()))
                .collect::<Result<Vec<_>>>()
                .ok()?;
            crate::document::WordSourceMap::concat(&pieces).ok()
        });
        let input = map
            .as_ref()
            .zip(ordinary_source.as_ref())
            .map(|(map, source)| {
                crate::document::WordFieldInput::produced(map.clone(), Arc::clone(source.registry))
            });
        let mut property = None;
        if let Ok(run) = parse_run_raw_with_observation(
            &raw_run,
            word_prefixes,
            text_map.map(|_| &mut texts),
            input.as_ref(),
            ordinary_source.as_ref().map(|_| &mut property),
        ) && !run.content.is_empty()
        {
            let facts = text_map.and_then(|_| {
                map.map(|map| ParsedWordRunTexts {
                    source: 0..raw_run.len(),
                    source_map: Some(Arc::new(map)),
                    texts,
                })
            });
            outside.push(Some(run));
            if ordinary_source.is_some() {
                outside_properties.push(property);
            }
            if text_map.is_some() {
                outside_texts.push(facts);
            }
            index += 1;
            continue;
        }
        let (_, children) = parts.remove(index);
        if let Some(next) = parts.get_mut(index) {
            next.1.splice(0..0, children);
        } else if let Some(previous) = index.checked_sub(1).and_then(|at| parts.get_mut(at)) {
            previous.1.extend(children);
        }
    }
    if let Some((_, destination)) = text_source {
        *destination = outside_texts;
    }
    if let Some(source) = ordinary_source {
        *source.output = outside_properties;
    }
    Ok(Some(
        parts
            .iter()
            .zip(outside)
            .map(|((_, children), run)| match run {
                Some(run) => FieldSpanPart::Outside(run, source_ranges(children)),
                None => FieldSpanPart::Field(source_ranges(children)),
            })
            .collect(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldSpanChild {
    Properties,
    Begin,
    End,
    /// A separator or instruction text, which only ever sits inside a field.
    FieldMarker,
    Other,
}

fn field_span_child_kind(element: &BytesStart<'_>, word_prefixes: &[String]) -> FieldSpanChild {
    let name = element.name();
    if is_word_element(name.as_ref(), b"rPr", word_prefixes) {
        return FieldSpanChild::Properties;
    }
    if is_word_element(name.as_ref(), b"instrText", word_prefixes)
        || is_word_element(name.as_ref(), b"delInstrText", word_prefixes)
    {
        return FieldSpanChild::FieldMarker;
    }
    if !is_word_element(name.as_ref(), b"fldChar", word_prefixes) {
        return FieldSpanChild::Other;
    }
    match optional_word_attribute(element, b"fldCharType", word_prefixes).as_deref() {
        Some("begin") => FieldSpanChild::Begin,
        Some("end") => FieldSpanChild::End,
        _ => FieldSpanChild::FieldMarker,
    }
}

/// Whether one field's span starts with its begin marker and ends with its
/// end marker, the shape Word writes, so no run content sits outside it.
///
/// Only the first and last run children are read. Any other shape answers
/// `false` and takes the full split.
fn field_span_is_bare(raw: &[u8], word_prefixes: &[String]) -> bool {
    let marker = |element: &BytesStart<'_>, prefixes: &[String], kind: &str| {
        is_word_element(element.name().as_ref(), b"fldChar", prefixes)
            && optional_word_attribute(element, b"fldCharType", prefixes).as_deref() == Some(kind)
    };
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut run_prefixes = None;
    let begins = loop {
        let Ok(event) = reader.read_event_into(&mut buffer) else {
            return false;
        };
        match event {
            Event::Start(element) => {
                let Ok(prefixes) =
                    word_prefixes_at(&element, run_prefixes.as_deref().unwrap_or(word_prefixes))
                else {
                    return false;
                };
                if run_prefixes.is_none() {
                    if !is_word_element(element.name().as_ref(), b"r", &prefixes) {
                        return false;
                    }
                    run_prefixes = Some(prefixes.into_owned());
                } else if marker(&element, &prefixes, "begin") {
                    break true;
                } else if is_word_element(element.name().as_ref(), b"rPr", &prefixes) {
                    if reader
                        .read_to_end_into(element.name(), &mut Vec::new())
                        .is_err()
                    {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            Event::Empty(element) => {
                let Some(inherited) = run_prefixes.as_deref() else {
                    return false;
                };
                let Ok(prefixes) = word_prefixes_at(&element, inherited) else {
                    return false;
                };
                break marker(&element, &prefixes, "begin");
            }
            _ => return false,
        }
        buffer.clear();
    };
    if !begins {
        return false;
    }
    // The last run child is the empty element before the closing run tag.
    let body = raw.trim_ascii_end();
    let Some(close) = body.iter().rposition(|byte| *byte == b'<') else {
        return false;
    };
    let Some(last) = body[..close].iter().rposition(|byte| *byte == b'<') else {
        return false;
    };
    let mut reader = Reader::from_reader(&body[last..close]);
    let mut buffer = Vec::new();
    let Some(prefixes) = run_prefixes else {
        return false;
    };
    matches!(
        reader.read_event_into(&mut buffer),
        Ok(Event::Empty(element))
            if word_prefixes_at(&element, &prefixes)
                .is_ok_and(|prefixes| marker(&element, &prefixes, "end"))
    ) && body[last..close].trim_ascii_end().ends_with(b"/>")
}

/// Turn a projected field span into model runs, splitting out the run
/// content that sits outside its fields.
///
/// Without such content the span stays one run per field, as before.
fn field_span_runs(
    raw: &[u8],
    mut fields: Vec<(Field, Option<CT_RPr>)>,
    word_prefixes: &[String],
    publication: Vec<Option<Vec<std::ops::Range<usize>>>>,
    origins: Option<(
        &crate::document::WordSourceMap,
        &Arc<std::sync::Mutex<FieldOriginRegistry>>,
    )>,
    text_facts: Option<&mut WordParagraphTextFacts>,
    ordinary_facts: Option<&mut Vec<Option<crate::document::WordOrdinaryFact>>>,
) -> Result<Vec<CT_R>> {
    let mut outside_texts = Vec::new();
    let mut outside_properties = Vec::new();
    let mut publication_ranges = None;
    let shape = if fields.len() == 1 && field_span_is_bare(raw, word_prefixes) {
        None
    } else {
        match field_span_parts(
            raw,
            word_prefixes,
            text_facts
                .as_ref()
                .and(origins.map(|(map, _)| (map, &mut outside_texts))),
            ordinary_facts
                .as_ref()
                .and(origins.map(|(map, registry)| FieldSpanPropertyRead {
                    map,
                    registry,
                    output: &mut outside_properties,
                })),
        ) {
            Ok(Some(parts))
                if (fields.len() > 1
                    || parts
                        .iter()
                        .any(|part| matches!(part, FieldSpanPart::Outside(_, _))))
                    && parts
                        .iter()
                        .filter(|part| matches!(part, FieldSpanPart::Field(_)))
                        .count()
                        == fields.len() =>
            {
                let mut ranges = Vec::new();
                let shape = parts
                    .into_iter()
                    .map(|part| {
                        let (run, source) = match part {
                            FieldSpanPart::Field(source) => (None, source),
                            FieldSpanPart::Outside(run, source) => (Some(run), source),
                        };
                        ranges.push(source);
                        run
                    })
                    .collect::<Vec<_>>();
                publication_ranges = Some(ranges);
                Some(shape)
            }
            _ => None,
        }
    };
    if let Some(destination) = ordinary_facts {
        if shape.is_some() {
            *destination = outside_properties;
        } else {
            destination.resize_with(fields.len(), || None);
        }
    }
    if let Some(destination) = text_facts {
        if shape.is_some() {
            *destination = outside_texts;
        } else {
            // Bare and legacy fallback publish only Fields, never hypothetical
            // outside runs discarded by the real splitter outcome.
            destination.resize_with(fields.len(), || None);
        }
    }
    let summaries = fields
        .iter()
        .map(|(field, _)| field.source_publication_summary(raw.len(), shape.as_deref()))
        .collect::<Result<Vec<_>>>()?;
    let owner_id = fields
        .iter()
        .find_map(|(field, _)| match field.source.as_ref() {
            FieldSource::Parsed { source_id, .. } => Some(*source_id),
            FieldSource::New { .. } => None,
        });
    for (field_index, (field, _)) in fields.iter_mut().enumerate() {
        let publication_selected =
            origins
                .filter(|(map, _)| map.has_publication())
                .and_then(|(_, registry)| {
                    registry
                        .lock()
                        .expect("field origin registry poisoned")
                        .publication_selection(field)
                });
        let original_facts = origins.and_then(|(_, registry)| {
            registry
                .lock()
                .expect("field origin registry poisoned")
                .original_for(field)
        });
        if let FieldSource::Parsed {
            raw_xml: source,
            publication_owned,
            owner_id: field_owner_id,
            ..
        } = Arc::make_mut(&mut field.source)
        {
            *source = raw.to_vec();
            if let (Some(shape), Some(ranges)) = (&shape, &publication_ranges) {
                *publication_owned = shape
                    .iter()
                    .enumerate()
                    .filter(|(_, run)| run.is_none())
                    .nth(field_index)
                    .and_then(|(position, _)| ranges.get(position))
                    .cloned();
            } else {
                *publication_owned = publication.get(field_index).cloned().flatten();
            }
            if let Some(owner_id) = owner_id {
                *field_owner_id = owner_id;
            }
        }
        if let Some((map, registry)) = origins {
            registry
                .lock()
                .expect("field origin registry poisoned")
                .register_forwarded(field, map.clone(), original_facts)?;
            // The actual successful splitter or fallback scanner has just
            // bound these numeric ranges to this exact replaced raw record.
            // They remain valid when multiple CURRENT domains share its map.
            let owned = match field.source.as_ref() {
                FieldSource::Parsed {
                    publication_owned: Some(owned),
                    ..
                } => Some(owned.clone()),
                _ => {
                    publication_selected.map(|selected| map.publication_relocated_ranges(&selected))
                }
            };
            if let Some(owned) = owned {
                registry
                    .lock()
                    .expect("field origin registry poisoned")
                    .register_publication_ranges(field, owned);
            }
        }
    }
    let Some(shape) = shape else {
        return Ok(fields
            .into_iter()
            .zip(summaries)
            .map(|((field, properties), _summary)| field_run(field, properties))
            .collect());
    };
    let mut fields = fields.into_iter().zip(summaries).enumerate();
    Ok(shape
        .iter()
        .enumerate()
        .map(|(position, run)| match run {
            Some(run) => run.clone(),
            None => {
                let (field_index, ((mut field, properties), _summary)) =
                    fields.next().expect("one field per field part");
                field.span = Some(Arc::new(FieldRunSpan {
                    runs: shape.clone(),
                    position,
                    field_index,
                }));
                if let Some(ranges) = publication_ranges
                    .as_mut()
                    .and_then(|ranges| ranges.get_mut(position))
                    && let Some((_, registry)) = origins
                {
                    registry
                        .lock()
                        .expect("field origin registry poisoned")
                        .register_publication_ranges(&field, std::mem::take(ranges));
                }
                field_run(field, properties)
            }
        })
        .collect())
}

pub(super) fn has_typed_boundary_inside(
    start: usize,
    end: usize,
    comment_ranges: &[CommentRangeMarker],
    bookmark_markers: &[BookmarkMarker],
    content_controls: &[(usize, usize, usize, CT_Sdt)],
    revisions: &[(usize, usize, CT_Revision)],
    hyperlinks: &[HyperlinkSpan],
) -> bool {
    let inside = |at: usize| at > start && at <= end;
    comment_ranges
        .iter()
        .any(|marker| inside(marker.run_index()))
        || bookmark_markers
            .iter()
            .any(|marker| inside(marker.run_index))
        || content_controls.iter().any(|(at, _, _, _)| inside(*at))
        || revisions.iter().any(|(at, _, _)| inside(*at))
        || hyperlinks.iter().any(|hyperlink| {
            let overlaps = hyperlink.run_start <= end && hyperlink.run_end > start;
            let contains_field = hyperlink.run_start <= start && hyperlink.run_end > end;
            overlaps && !contains_field
        })
}

struct ComplexFieldBoundariesMut<'a> {
    extra_xml: &'a mut [(usize, Vec<u8>)],
    comment_ranges: &'a mut [CommentRangeMarker],
    bookmark_markers: &'a mut [BookmarkMarker],
    content_controls: &'a mut [(usize, usize, usize, CT_Sdt)],
    revisions: &'a mut [(usize, usize, CT_Revision)],
    hyperlinks: &'a mut [HyperlinkSpan],
    rubies: &'a mut [CT_Ruby],
}

fn remap_complex_field_boundaries(
    start: usize,
    end: usize,
    replacement_count: usize,
    boundaries: ComplexFieldBoundariesMut<'_>,
) {
    let ComplexFieldBoundariesMut {
        extra_xml,
        comment_ranges,
        bookmark_markers,
        content_controls,
        revisions,
        hyperlinks,
        rubies,
    } = boundaries;
    let source_count = end - start + 1;
    let remap = |at: &mut usize| {
        if *at > end {
            if replacement_count >= source_count {
                *at += replacement_count - source_count;
            } else {
                *at -= source_count - replacement_count;
            }
        } else if *at > start {
            *at = start + replacement_count;
        }
    };
    for (at, _) in extra_xml {
        if *at != ROOT_ATTRIBUTES_POSITION {
            remap(at);
        }
    }
    for marker in comment_ranges {
        match marker {
            CommentRangeMarker::Start { run_index, .. }
            | CommentRangeMarker::End { run_index, .. } => remap(run_index),
        }
    }
    for marker in bookmark_markers {
        if marker.run_index > end {
            if replacement_count >= source_count {
                let inserted = replacement_count - source_count;
                marker.projected_run_index += inserted;
                marker.tracked_run_index += inserted;
            } else {
                let removed = source_count - replacement_count;
                marker.projected_run_index = marker.projected_run_index.saturating_sub(removed);
                marker.tracked_run_index = marker.tracked_run_index.saturating_sub(removed);
            }
        }
        remap(&mut marker.run_index);
    }
    for (at, _, _, _) in content_controls {
        remap(at);
    }
    for (at, _, _) in revisions {
        remap(at);
    }
    for ruby in rubies {
        remap(&mut ruby.base_start);
        remap(&mut ruby.base_end);
    }
    for hyperlink in hyperlinks {
        let old_start = hyperlink.run_start;
        let old_boundaries = hyperlink
            .extra_xml
            .iter()
            .map(|(boundary, _, _)| old_start + *boundary)
            .collect::<Vec<_>>();
        remap(&mut hyperlink.run_start);
        remap(&mut hyperlink.run_end);
        for ((boundary, _, _), mut absolute) in hyperlink.extra_xml.iter_mut().zip(old_boundaries) {
            remap(&mut absolute);
            *boundary = absolute.saturating_sub(hyperlink.run_start);
        }
    }
}

#[cfg(test)]
mod deleted_field_tests {
    use super::*;

    /// The fields of every deletion of a Word paragraph, as the tracked view
    /// reads them.
    fn deleted_fields(body: &str) -> Vec<Field> {
        let xml = format!(
            r#"<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{body}</w:p>"#
        );
        let paragraph = CT_P::from_xml_fragment(xml.as_bytes()).unwrap();
        let mut fields = Vec::new();
        for (_, _, revision) in &paragraph.revisions {
            let mut revision = revision.clone();
            let content = revision.physical_content_paragraph_mut().unwrap();
            for run in content.into_iter().flat_map(|content| &content.runs) {
                fields.extend(run.content.iter().filter_map(|item| match item {
                    RunContent::Field(field) => Some(field.clone()),
                    _ => None,
                }));
            }
        }
        fields
    }

    #[test]
    fn a_field_deleted_in_word_reads_its_code_from_its_deleted_instruction_text() {
        // Word writes the code of a deleted field in w:delInstrText, here split
        // over two runs, and its result in w:delText.
        let body = r#"<w:del w:id="1" w:author="Ada" w:date="2026-01-01T00:00:00Z"><w:r w:rsidDel="00A1B2C3"><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Check1"/><w:enabled/><w:calcOnExit w:val="0"/><w:checkBox><w:sizeAuto/><w:default w:val="0"/><w:checked/></w:checkBox></w:ffData></w:fldChar></w:r><w:r w:rsidDel="00A1B2C3"><w:delInstrText xml:space="preserve"> FORMCHECKBOX </w:delInstrText></w:r><w:r w:rsidDel="00A1B2C3"><w:fldChar w:fldCharType="separate"/></w:r><w:r w:rsidDel="00A1B2C3"><w:fldChar w:fldCharType="end"/></w:r><w:r w:rsidDel="00A1B2C3"><w:fldChar w:fldCharType="begin"/></w:r><w:r w:rsidDel="00A1B2C3"><w:delInstrText xml:space="preserve"> PA</w:delInstrText></w:r><w:r w:rsidDel="00A1B2C3"><w:delInstrText xml:space="preserve">GE </w:delInstrText></w:r><w:r w:rsidDel="00A1B2C3"><w:fldChar w:fldCharType="separate"/></w:r><w:r w:rsidDel="00A1B2C3"><w:delText>3</w:delText></w:r><w:r w:rsidDel="00A1B2C3"><w:fldChar w:fldCharType="end"/></w:r></w:del>"#;
        let read: Vec<_> = deleted_fields(body)
            .iter()
            .map(|field| {
                (
                    field.effective_instruction_text(),
                    field.legacy_form.as_ref().map(|form| form.value.clone()),
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                (
                    "FORMCHECKBOX".to_owned(),
                    Some(LegacyFormFieldValue::Checked(true))
                ),
                ("PAGE".to_owned(), None),
            ]
        );
    }
}
