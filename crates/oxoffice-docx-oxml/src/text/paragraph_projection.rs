//! Bookmark and hyperlink projections of a paragraph, current and accepted.

use super::*;

pub(super) fn parse_hyperlink_children(
    raw: &[u8],
    word_prefixes: &[String],
    mut source_facts: Option<&mut crate::revision::RevisionSourceFacts>,
    input: Option<&crate::document::WordFieldInput>,
    collect_texts: bool,
    collect_properties: bool,
) -> Result<ParsedHyperlinkChildren> {
    let mut ordinary_runs = Vec::new();
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let cursor = input.map(|input| input.cursor(raw)).transpose()?;
    let mut run_maps = Vec::new();
    let mut text_facts = Vec::new();
    let mut extra_maps = Vec::new();
    let mut runs = Vec::new();
    let mut run_sources = Vec::new();
    let mut revisions = Vec::new();
    let mut extra_xml = Vec::new();
    let mut revisions_at_boundary = 0usize;
    let mut buffer = Vec::new();
    let mut inside = false;
    loop {
        let event_start = reader.buffer_position();
        let before_runs = run_sources.len();
        if collect_properties {
            ordinary_runs.resize_with(before_runs, || None);
        }
        let before_extras = extra_xml.len();
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) if !inside => {
                inside = true;
                let _ = word_prefixes_at(&element, word_prefixes)?;
            }
            Event::Start(element) => {
                let prefixes = word_prefixes_at(&element, word_prefixes)?;
                if is_word_element(element.name().as_ref(), b"r", &prefixes) {
                    let (raw, mapped) = if collect_properties {
                        if let Some(cursor) = &cursor {
                            cursor.capture_element(&mut reader, &element, event_start)?
                        } else {
                            (capture_element(&mut reader, &element)?, None)
                        }
                    } else {
                        let raw = capture_element(&mut reader, &element)?;
                        let mapped = cursor.as_ref().and_then(|cursor| {
                            cursor.field_input(&reader, event_start..reader.buffer_position(), &raw)
                        });
                        (raw, mapped)
                    };
                    let mut parsed_texts = Vec::new();
                    let mut property = None;
                    runs.push(parse_run_raw_with_observation(
                        &raw,
                        &prefixes,
                        collect_texts.then_some(&mut parsed_texts),
                        mapped.as_ref(),
                        collect_properties.then_some(&mut property),
                    )?);
                    if collect_properties {
                        ordinary_runs.push(property);
                    }
                    if collect_texts {
                        text_facts.push(
                            cursor
                                .as_ref()
                                .and_then(|cursor| {
                                    cursor.captured_map(
                                        &reader,
                                        event_start..reader.buffer_position(),
                                        &raw,
                                    )
                                })
                                .map(|source_map| ParsedWordRunTexts {
                                    source: 0..raw.len(),
                                    source_map: Some(source_map),
                                    texts: parsed_texts,
                                }),
                        );
                    }
                    if let Some(facts) = source_facts.as_deref_mut() {
                        facts.push_run(runs.last().expect("just parsed hyperlink run"), &raw)?;
                    }
                    run_sources.push(Some(raw));
                    revisions_at_boundary = 0;
                } else if is_content_revision_element(element.name().as_ref(), &prefixes) {
                    let raw = capture_element(&mut reader, &element)?;
                    if let Some(revision) = CT_Revision::from_raw_with_input(
                        raw.clone(),
                        &prefixes,
                        true,
                        cursor
                            .as_ref()
                            .and_then(|cursor| {
                                cursor.field_input(
                                    &reader,
                                    event_start..reader.buffer_position(),
                                    &raw,
                                )
                            })
                            .as_ref(),
                    )? {
                        if let Some(facts) = source_facts.as_deref_mut() {
                            facts.push_revision(&revision)?;
                        }
                        revisions.push((runs.len(), revision));
                        revisions_at_boundary += 1;
                    } else {
                        extra_xml.push((runs.len(), revisions_at_boundary, raw));
                    }
                } else if is_word_element(element.name().as_ref(), b"fldSimple", &prefixes) {
                    // A simple field in a link is a field run, as it is in the
                    // paragraph, so its result reads in the link's text.
                    let prepared_header = cursor.as_ref().and_then(|cursor| {
                        cursor.simple_header(
                            &reader,
                            event_start..reader.buffer_position(),
                            &element,
                            &prefixes,
                        )
                    });
                    let raw = capture_element(&mut reader, &element)?;
                    let captured = event_start..reader.buffer_position();
                    let mapped = cursor
                        .as_ref()
                        .and_then(|cursor| cursor.field_input(&reader, captured.clone(), &raw));
                    let prepared_selector = cursor
                        .as_ref()
                        .and_then(|cursor| cursor.simple_selector(&reader, captured, &raw));
                    if let Some((field, _)) = parse_simple_field_with_summary(
                        &raw,
                        &prefixes,
                        prepared_header,
                        prepared_selector,
                        mapped.as_ref(),
                    )? {
                        runs.push(field_run(field, None));
                        if collect_texts {
                            text_facts.push(None);
                        }
                        run_sources.push(Some(raw));
                        revisions_at_boundary = 0;
                    } else {
                        extra_xml.push((runs.len(), revisions_at_boundary, raw));
                    }
                } else {
                    let raw = capture_element(&mut reader, &element)?;
                    if let Some(facts) = source_facts.as_deref_mut() {
                        if is_word_element(element.name().as_ref(), b"hyperlink", &prefixes) {
                            facts.append_opaque_hyperlink(&raw, &prefixes)?;
                        } else if crate::revision::revision_kind(element.name().as_ref(), &prefixes)
                            .is_some()
                            && let Some(revision) = CT_Revision::from_raw_with_input(
                                raw.clone(),
                                &prefixes,
                                true,
                                cursor
                                    .as_ref()
                                    .and_then(|cursor| {
                                        cursor.field_input(
                                            &reader,
                                            event_start..reader.buffer_position(),
                                            &raw,
                                        )
                                    })
                                    .as_ref(),
                            )?
                        {
                            facts.push_revision(&revision)?;
                        }
                    }
                    extra_xml.push((runs.len(), revisions_at_boundary, raw));
                }
            }
            Event::Empty(element) => {
                let prefixes = word_prefixes_at(&element, word_prefixes)?;
                let raw = capture_empty(&element, reader.empty_writing())?;
                if is_content_revision_element(element.name().as_ref(), &prefixes) {
                    if let Some(revision) = CT_Revision::from_raw_with_input(
                        raw.clone(),
                        &prefixes,
                        true,
                        cursor
                            .as_ref()
                            .and_then(|cursor| {
                                cursor.field_input(
                                    &reader,
                                    event_start..reader.buffer_position(),
                                    &raw,
                                )
                            })
                            .as_ref(),
                    )? {
                        if let Some(facts) = source_facts.as_deref_mut() {
                            facts.push_revision(&revision)?;
                        }
                        revisions.push((runs.len(), revision));
                        revisions_at_boundary += 1;
                    } else {
                        extra_xml.push((runs.len(), revisions_at_boundary, raw));
                    }
                } else if is_word_element(element.name().as_ref(), b"fldSimple", &prefixes) {
                    let mapped = cursor.as_ref().and_then(|cursor| {
                        cursor.field_input(&reader, event_start..reader.buffer_position(), &raw)
                    });
                    if let Some((field, _)) = parse_empty_simple_field(
                        &element,
                        &raw,
                        prefixes.into_owned(),
                        mapped.as_ref(),
                    )? {
                        if let Some(mapped) = mapped {
                            mapped.register(&field)?;
                        }
                        runs.push(field_run(field, None));
                        if collect_texts {
                            text_facts.push(None);
                        }
                        run_sources.push(None);
                        revisions_at_boundary = 0;
                    } else {
                        extra_xml.push((runs.len(), revisions_at_boundary, raw));
                    }
                } else {
                    extra_xml.push((runs.len(), revisions_at_boundary, raw));
                }
            }
            Event::End(element)
                if inside && matches_local_name(element.name().as_ref(), b"hyperlink") =>
            {
                break;
            }
            Event::Text(text) if is_xml_whitespace(text.as_ref()) => {
                extra_xml.push((runs.len(), revisions_at_boundary, text.as_ref().to_vec()));
            }
            Event::Comment(comment) => {
                extra_xml.push((
                    runs.len(),
                    revisions_at_boundary,
                    capture_standalone_event(Event::Comment(comment.into_owned()))?,
                ));
            }
            Event::PI(instruction) => {
                extra_xml.push((
                    runs.len(),
                    revisions_at_boundary,
                    capture_standalone_event(Event::PI(instruction.into_owned()))?,
                ));
            }
            Event::Eof => break,
            _ => {}
        }
        let captured = event_start..reader.buffer_position();
        run_maps.extend(run_sources[before_runs..].iter().map(|raw| {
            raw.as_ref().and_then(|raw| {
                cursor
                    .as_ref()?
                    .captured_map(&reader, captured.clone(), raw)
            })
        }));
        extra_maps.extend(extra_xml[before_extras..].iter().map(|(_, _, raw)| {
            cursor
                .as_ref()?
                .captured_map(&reader, captured.clone(), raw)
        }));
        buffer.clear();
    }
    if collect_properties {
        ordinary_runs.resize_with(runs.len(), || None);
    }
    let projection =
        accepted_hyperlink_bookmark_projection(&runs, &revisions, &extra_xml, word_prefixes);
    Ok(ParsedHyperlinkChildren {
        ordinary_runs,
        runs,
        run_sources,
        run_maps,
        text_facts,
        extra_maps,
        revisions,
        extra_xml,
        bookmark_markers: projection.markers,
        projected_run_count: projection.projected_run_count,
        tracked_run_count: projection.tracked_run_count,
    })
}

fn bookmark_marker_projection_from_raw(
    raw: &[u8],
    word_prefixes: &[String],
    run_index: usize,
    raw_before: usize,
    projected_run_index: usize,
    tracked_run_index: usize,
) -> Option<BookmarkMarker> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer).ok()?;
        let (element, has_child_content) = match event {
            Event::Start(element) => (element, raw_element_has_child_content(raw)),
            Event::Empty(element) => (element, false),
            Event::Text(text) if is_xml_whitespace(text.as_ref()) => {
                buffer.clear();
                continue;
            }
            Event::Eof => return None,
            _ => return None,
        };
        let prefixes = word_prefixes_at(&element, word_prefixes).ok()?;
        let start = is_word_element(element.name().as_ref(), b"bookmarkStart", &prefixes);
        if !start && !is_word_element(element.name().as_ref(), b"bookmarkEnd", &prefixes) {
            return None;
        }
        return Some(BookmarkMarker {
            start,
            id: optional_word_attribute(&element, b"id", &prefixes)
                .and_then(|value| value.parse().ok()),
            name: optional_word_attribute(&element, b"name", &prefixes),
            run_index,
            raw_before,
            projected_run_index,
            tracked_run_index,
            word_prefixes: prefixes.into_owned(),
            has_child_content,
        });
    }
}

fn append_projection(
    output: &mut AcceptedBookmarkProjection,
    projection: AcceptedBookmarkProjection,
    direct_run_offset: usize,
    raw_before: usize,
) {
    let projected_offset = output.projected_run_count;
    let tracked_offset = output.tracked_run_count;
    output
        .markers
        .extend(projection.markers.into_iter().map(|mut marker| {
            marker.run_index += direct_run_offset;
            marker.raw_before = raw_before;
            marker.projected_run_index += projected_offset;
            marker.tracked_run_index += tracked_offset;
            marker
        }));
    output.projected_run_count += projection.projected_run_count;
    output.tracked_run_count += projection.tracked_run_count;
}

pub(super) fn append_nested_bookmark_projection(
    markers: &mut Vec<BookmarkMarker>,
    projected_run_count: &mut usize,
    tracked_run_count: &mut usize,
    projection: AcceptedBookmarkProjection,
    direct_run_offset: usize,
    raw_before: usize,
) {
    let projected_offset = *projected_run_count;
    let tracked_offset = *tracked_run_count;
    markers.extend(projection.markers.into_iter().map(|mut marker| {
        marker.run_index += direct_run_offset;
        marker.raw_before = raw_before;
        marker.projected_run_index += projected_offset;
        marker.tracked_run_index += tracked_offset;
        marker
    }));
    *projected_run_count += projection.projected_run_count;
    *tracked_run_count += projection.tracked_run_count;
}

// Match each actual marker occurrence once, while retaining the original source
// slots, namespace scope and malformed/duplicate evidence verbatim.
fn install_current_bookmark(
    original: &mut [BookmarkMarker],
    assigned: &mut [bool],
    current: &BookmarkMarker,
) {
    if let Some((index, marker)) = original.iter_mut().enumerate().find(|(index, marker)| {
        !assigned[*index]
            && marker.start == current.start
            && marker.id == current.id
            && marker.name == current.name
            && marker.raw_before == current.raw_before
            && marker.has_child_content == current.has_child_content
    }) {
        marker.projected_run_index = current.projected_run_index;
        marker.tracked_run_index = current.tracked_run_index;
        assigned[index] = true;
    }
}

fn current_raw_bookmark(
    raw: &[u8],
    originals: &[BookmarkMarker],
    boundary: usize,
    slot: usize,
    accepted: usize,
    tracked: usize,
) -> Option<BookmarkMarker> {
    originals
        .iter()
        .filter(|marker| marker.run_index == boundary && marker.raw_before == slot)
        .find_map(|marker| {
            let parsed = bookmark_marker_projection_from_raw(
                raw,
                &marker.word_prefixes,
                boundary,
                slot,
                accepted,
                tracked,
            )?;
            (parsed.start == marker.start && parsed.id == marker.id && parsed.name == marker.name)
                .then_some(parsed)
        })
}

pub(super) fn current_paragraph_bookmark_projection(
    paragraph: &CT_P,
) -> AcceptedBookmarkProjection {
    let mut output = AcceptedBookmarkProjection {
        markers: paragraph.bookmark_markers.clone(),
        ..Default::default()
    };
    let mut assigned = vec![false; output.markers.len()];
    let mut revision_positions = vec![None; paragraph.revisions.len()];
    let mut boundary_positions = Vec::new();
    let mut raw_positions = Vec::new();
    for boundary in 0..=paragraph.runs.len() {
        let before_owners = (output.projected_run_count, output.tracked_run_count);
        let raws = paragraph
            .extra_xml
            .iter()
            .filter(|(at, _)| *at == boundary)
            .collect::<Vec<_>>();
        // Only typed owners contribute to the bookmark run axis. In particular,
        // no opaque wrapper is reparsed to discover a speculative run owner.
        let mut owners = paragraph
            .content_controls
            .iter()
            .filter(|(at, _, _, _)| *at == boundary)
            .map(|(_, slot, _, control)| {
                (
                    AcceptedOwnerOrder::Raw(*slot),
                    0u8,
                    None,
                    current_control_bookmark_projection(control),
                )
            })
            .chain(
                paragraph
                    .revisions
                    .iter()
                    .enumerate()
                    .filter(|(_, (at, _, _))| *at == boundary)
                    .map(|(index, (_, slot, revision))| {
                        let order = if let Some(link) = hyperlink_revision_index(*slot) {
                            let link = paragraph.hyperlinks.get(link);
                            match link.and_then(|link| link.preserved_raw_before) {
                                Some(slot) => AcceptedOwnerOrder::Raw(slot),
                                None if link.is_some_and(|link| boundary == link.run_end) => {
                                    AcceptedOwnerOrder::BeforeRaw
                                }
                                None => AcceptedOwnerOrder::AfterRaw,
                            }
                        } else {
                            AcceptedOwnerOrder::Raw(*slot)
                        };
                        (
                            order,
                            1u8,
                            Some(index),
                            current_revision_bookmark_projection(revision),
                        )
                    }),
            )
            .collect::<Vec<_>>();
        owners.sort_by_key(|(order, kind, _, _)| (*order, *kind));
        let mut owners = owners.into_iter().peekable();
        for slot in 0..=raws.len() {
            while owners
                .peek()
                .is_some_and(|(order, _, _, _)| *order <= AcceptedOwnerOrder::Raw(slot))
            {
                let (order, _, revision_index, projection) = owners.next().expect("peeked owner");
                let before = (output.projected_run_count, output.tracked_run_count);
                let raw_before = if revision_index.is_some_and(|index| {
                    hyperlink_revision_index(paragraph.revisions[index].1).is_some()
                }) {
                    0
                } else {
                    match order {
                        AcceptedOwnerOrder::Raw(slot) => slot,
                        AcceptedOwnerOrder::BeforeRaw | AcceptedOwnerOrder::AfterRaw => 0,
                    }
                };
                for mut marker in projection.markers {
                    marker.raw_before = raw_before;
                    marker.projected_run_index += output.projected_run_count;
                    marker.tracked_run_index += output.tracked_run_count;
                    install_current_bookmark(&mut output.markers, &mut assigned, &marker);
                }
                output.projected_run_count += projection.projected_run_count;
                output.tracked_run_count += projection.tracked_run_count;
                if let Some(index) = revision_index {
                    revision_positions[index] = Some((
                        before,
                        (output.projected_run_count, output.tracked_run_count),
                    ));
                }
            }
            raw_positions.push((
                boundary,
                slot,
                output.projected_run_count,
                output.tracked_run_count,
            ));
            if let Some((_, raw)) = raws.get(slot)
                && let Some(marker) = current_raw_bookmark(
                    raw,
                    &paragraph.bookmark_markers,
                    boundary,
                    slot,
                    output.projected_run_count,
                    output.tracked_run_count,
                )
            {
                install_current_bookmark(&mut output.markers, &mut assigned, &marker);
            }
        }
        for (_, _, revision_index, projection) in owners {
            let before = (output.projected_run_count, output.tracked_run_count);
            for mut marker in projection.markers {
                marker.raw_before = 0;
                marker.projected_run_index += output.projected_run_count;
                marker.tracked_run_index += output.tracked_run_count;
                install_current_bookmark(&mut output.markers, &mut assigned, &marker);
            }
            output.projected_run_count += projection.projected_run_count;
            output.tracked_run_count += projection.tracked_run_count;
            if let Some(index) = revision_index {
                revision_positions[index] = Some((
                    before,
                    (output.projected_run_count, output.tracked_run_count),
                ));
            }
        }
        boundary_positions.push((
            before_owners,
            (output.projected_run_count, output.tracked_run_count),
        ));
        if boundary < paragraph.runs.len() {
            output.projected_run_count += 1;
            output.tracked_run_count += 1;
        }
    }
    // Hyperlink raw children already retain their exact relative boundary and
    // revision ordinal. Reuse those source slots around the actual typed owners.
    for (link_index, link) in paragraph.hyperlinks.iter().enumerate() {
        for (relative, before, raw) in &link.extra_xml {
            let boundary = link.run_start + relative;
            let Some((start, end)) = boundary_positions.get(boundary) else {
                continue;
            };
            let revisions = paragraph
                .revisions
                .iter()
                .enumerate()
                .filter(|(_, (at, slot, _))| {
                    *at == boundary && hyperlink_revision_index(*slot) == Some(link_index)
                })
                .filter_map(|(index, _)| revision_positions[index])
                .collect::<Vec<_>>();
            let position = if let Some((start, _)) = revisions.get(*before) {
                *start
            } else if let Some((_, end)) = revisions.last() {
                *end
            } else if let Some(slot) = link.preserved_raw_before {
                raw_positions
                    .iter()
                    .find(|(at, raw_slot, _, _)| *at == boundary && *raw_slot == slot)
                    .map_or(*end, |(_, _, accepted, tracked)| (*accepted, *tracked))
            } else if boundary == link.run_end {
                *start
            } else {
                *end
            };
            if let Some(marker) = current_raw_bookmark(
                raw,
                &paragraph.bookmark_markers,
                boundary,
                0,
                position.0,
                position.1,
            ) {
                install_current_bookmark(&mut output.markers, &mut assigned, &marker);
            }
        }
    }
    output
}

fn current_revision_bookmark_projection(revision: &CT_Revision) -> AcceptedBookmarkProjection {
    if let Some(paragraph) = revision.content_paragraph() {
        let mut projection = current_paragraph_bookmark_projection(paragraph);
        if !matches!(
            revision.kind(),
            RevisionKind::Insertion | RevisionKind::MoveTo
        ) {
            projection.markers.clear();
            projection.projected_run_count = 0;
        }
        return projection;
    }
    let mut tracked_run_count = 0;
    if let crate::revision::RevisionContent::Runs(runs) = revision.content() {
        tracked_run_count = runs.len();
        for (_, nested) in revision
            .nested_revisions()
            .iter()
            .filter(|(boundary, _)| *boundary <= runs.len())
        {
            tracked_run_count += current_revision_bookmark_projection(nested).tracked_run_count;
        }
    }
    AcceptedBookmarkProjection {
        markers: Vec::new(),
        projected_run_count: 0,
        tracked_run_count,
    }
}

pub(super) fn current_control_bookmark_projection(control: &CT_Sdt) -> AcceptedBookmarkProjection {
    let mut output = AcceptedBookmarkProjection::default();
    for boundary in 0..=control.content.len() {
        for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
            let projection = current_revision_bookmark_projection(revision);
            append_projection(&mut output, projection, 0, 0);
        }
        let Some(content) = control.content.get(boundary) else {
            continue;
        };
        match content {
            SdtContent::Run(_) => {
                output.projected_run_count += 1;
                output.tracked_run_count += 1;
            }
            SdtContent::RawXml(raw) => {
                if let Some(marker) = bookmark_marker_projection_from_raw(
                    raw,
                    control.word_prefixes(),
                    0,
                    0,
                    output.projected_run_count,
                    output.tracked_run_count,
                ) {
                    output.markers.push(marker);
                }
            }
            SdtContent::ContentControl(control) => {
                let projection = current_control_bookmark_projection(control);
                append_projection(&mut output, projection, 0, 0);
            }
            SdtContent::Paragraph(paragraph) => {
                let projection = current_paragraph_bookmark_projection(paragraph);
                append_projection(&mut output, projection, 0, 0);
            }
            SdtContent::Table(table) => {
                append_current_control_table_bookmark_projection(&mut output, table)
            }
            SdtContent::Row(row) => {
                append_current_control_row_bookmark_projection(&mut output, row)
            }
            SdtContent::Cell(cell) => {
                append_current_control_cell_bookmark_projection(&mut output, cell)
            }
        }
    }
    output
}

/// The bookmarks that start in one child of a content control that is not
/// a run, a control or a paragraph (a raw marker, or a table, row or cell
/// read as its runs), each at its run boundary in that child.
pub(super) fn sdt_content_bookmark_starts(
    content: &SdtContent,
    word_prefixes: &[String],
) -> Vec<(usize, String)> {
    let mut output = AcceptedBookmarkProjection::default();
    match content {
        SdtContent::RawXml(raw) => {
            output.markers.extend(bookmark_marker_projection_from_raw(
                raw,
                word_prefixes,
                0,
                0,
                0,
                0,
            ));
        }
        SdtContent::Table(table) => {
            append_current_control_table_bookmark_projection(&mut output, table)
        }
        SdtContent::Row(row) => append_current_control_row_bookmark_projection(&mut output, row),
        SdtContent::Cell(cell) => {
            append_current_control_cell_bookmark_projection(&mut output, cell)
        }
        SdtContent::Run(_) | SdtContent::ContentControl(_) | SdtContent::Paragraph(_) => {}
    }
    output
        .markers
        .into_iter()
        .filter(|marker| marker.is_start())
        .filter_map(|marker| Some((marker.projected_run_index(), marker.name()?.to_owned())))
        .collect()
}

fn append_current_control_table_bookmark_projection(
    output: &mut AcceptedBookmarkProjection,
    table: &CT_Tbl,
) {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            let projection = current_control_bookmark_projection(control);
            append_projection(output, projection, 0, 0);
        }
        if let Some(row) = table.rows.get(boundary) {
            append_current_control_row_bookmark_projection(output, row);
        }
    }
}

fn append_current_control_row_bookmark_projection(
    output: &mut AcceptedBookmarkProjection,
    row: &CT_Row,
) {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            let projection = current_control_bookmark_projection(control);
            append_projection(output, projection, 0, 0);
        }
        if let Some(cell) = row.cells.get(boundary) {
            append_current_control_cell_bookmark_projection(output, cell);
        }
    }
}

fn append_current_control_cell_bookmark_projection(
    output: &mut AcceptedBookmarkProjection,
    cell: &CT_Tc,
) {
    for content in &cell.content {
        let projection = match content {
            CellContent::Paragraph(paragraph) => current_paragraph_bookmark_projection(paragraph),
            CellContent::Table(table) => {
                append_current_control_table_bookmark_projection(output, table);
                continue;
            }
            CellContent::ContentControl(control) => current_control_bookmark_projection(control),
        };
        append_projection(output, projection, 0, 0);
    }
}

fn accepted_paragraph_bookmark_projection(paragraph: &CT_P) -> AcceptedBookmarkProjection {
    let mut projection = AcceptedBookmarkProjection {
        markers: paragraph.bookmark_markers.clone(),
        projected_run_count: accepted_paragraph_runs(paragraph).len(),
        tracked_run_count: tracked_paragraph_runs(paragraph).len(),
    };
    projection
        .markers
        .sort_by_key(BookmarkMarker::projected_run_index);
    projection
}

pub(super) fn accepted_revision_bookmark_projection(
    revision: &CT_Revision,
) -> AcceptedBookmarkProjection {
    if matches!(
        revision.kind(),
        RevisionKind::Insertion | RevisionKind::MoveTo
    ) && let Some(paragraph) = revision.content_paragraph()
    {
        return accepted_paragraph_bookmark_projection(paragraph);
    }
    let mut runs = Vec::new();
    append_tracked_revision_runs(revision, &mut runs);
    AcceptedBookmarkProjection {
        markers: Vec::new(),
        projected_run_count: 0,
        tracked_run_count: runs.len(),
    }
}

pub(super) fn accepted_control_bookmark_projection(control: &CT_Sdt) -> AcceptedBookmarkProjection {
    let mut output = AcceptedBookmarkProjection::default();
    for boundary in 0..=control.content.len() {
        for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
            let projection = accepted_revision_bookmark_projection(revision);
            append_projection(&mut output, projection, 0, 0);
        }
        let Some(content) = control.content.get(boundary) else {
            continue;
        };
        match content {
            SdtContent::Run(_) => {
                output.projected_run_count += 1;
                output.tracked_run_count += 1;
            }
            SdtContent::RawXml(raw) => {
                if let Some(marker) = bookmark_marker_projection_from_raw(
                    raw,
                    control.word_prefixes(),
                    0,
                    0,
                    output.projected_run_count,
                    output.tracked_run_count,
                ) {
                    output.markers.push(marker);
                }
            }
            SdtContent::ContentControl(control) => {
                let projection = accepted_control_bookmark_projection(control);
                append_projection(&mut output, projection, 0, 0);
            }
            SdtContent::Paragraph(paragraph) => {
                let projection = accepted_paragraph_bookmark_projection(paragraph);
                append_projection(&mut output, projection, 0, 0);
            }
            SdtContent::Table(table) => {
                append_control_table_bookmark_projection(&mut output, table)
            }
            SdtContent::Row(row) => append_control_row_bookmark_projection(&mut output, row),
            SdtContent::Cell(cell) => append_control_cell_bookmark_projection(&mut output, cell),
        }
    }
    output
}

fn append_control_table_bookmark_projection(
    output: &mut AcceptedBookmarkProjection,
    table: &CT_Tbl,
) {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            let projection = accepted_control_bookmark_projection(control);
            append_projection(output, projection, 0, 0);
        }
        if let Some(row) = table.rows.get(boundary) {
            append_control_row_bookmark_projection(output, row);
        }
    }
}

fn append_control_row_bookmark_projection(output: &mut AcceptedBookmarkProjection, row: &CT_Row) {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            let projection = accepted_control_bookmark_projection(control);
            append_projection(output, projection, 0, 0);
        }
        if let Some(cell) = row.cells.get(boundary) {
            append_control_cell_bookmark_projection(output, cell);
        }
    }
}

fn append_control_cell_bookmark_projection(output: &mut AcceptedBookmarkProjection, cell: &CT_Tc) {
    for content in &cell.content {
        let projection = match content {
            CellContent::Paragraph(paragraph) => accepted_paragraph_bookmark_projection(paragraph),
            CellContent::Table(table) => {
                append_control_table_bookmark_projection(output, table);
                continue;
            }
            CellContent::ContentControl(control) => accepted_control_bookmark_projection(control),
        };
        append_projection(output, projection, 0, 0);
    }
}

fn accepted_hyperlink_bookmark_projection(
    runs: &[CT_R],
    revisions: &[(usize, CT_Revision)],
    extra_xml: &[(usize, usize, Vec<u8>)],
    word_prefixes: &[String],
) -> AcceptedBookmarkProjection {
    let mut output = AcceptedBookmarkProjection::default();
    for boundary in 0..=runs.len() {
        let boundary_revisions = revisions
            .iter()
            .filter(|(at, _)| *at == boundary)
            .map(|(_, revision)| revision)
            .collect::<Vec<_>>();
        for revision_before in 0..=boundary_revisions.len() {
            for (_, _, raw) in extra_xml
                .iter()
                .filter(|(at, before, _)| *at == boundary && *before == revision_before)
            {
                if let Some(marker) = bookmark_marker_projection_from_raw(
                    raw,
                    word_prefixes,
                    boundary,
                    0,
                    output.projected_run_count,
                    output.tracked_run_count,
                ) {
                    output.markers.push(marker);
                }
            }
            if let Some(revision) = boundary_revisions.get(revision_before) {
                let projection = accepted_revision_bookmark_projection(revision);
                append_projection(&mut output, projection, boundary, 0);
            }
        }
        if boundary < runs.len() {
            output.projected_run_count += 1;
            output.tracked_run_count += 1;
        }
    }
    output
}

pub(super) fn parse_hyperlink_attributes(
    element: &BytesStart<'_>,
    scope: &[String],
) -> Result<ParsedHyperlinkAttributes> {
    let mut rel_id = None;
    let mut anchor = None;
    let mut tooltip = None;
    let mut doc_location = None;
    let mut extra = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute?;
        let name = attribute.key.as_ref();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
            .into_owned();
        if attribute_in_namespace(name, b"id", R_NS, scope) {
            rel_id = Some(value);
        } else if attribute_in_namespace(name, b"anchor", crate::namespace::W_NS, scope) {
            anchor = Some(value);
        } else if attribute_in_namespace(name, b"tooltip", crate::namespace::W_NS, scope) {
            tooltip = Some(value);
        } else if attribute_in_namespace(name, b"docLocation", crate::namespace::W_NS, scope) {
            doc_location = Some(value);
        } else {
            extra.push((std::str::from_utf8(name)?.to_owned(), value));
        }
    }
    Ok((rel_id, anchor, tooltip, doc_location, extra))
}

pub(super) fn is_element_in_namespace(
    name: &[u8],
    local: &[u8],
    namespace: &str,
    scope: &[String],
) -> bool {
    let (prefix, actual_local) = name
        .iter()
        .position(|byte| *byte == b':')
        .map_or((&b""[..], name), |separator| {
            (&name[..separator], &name[separator + 1..])
        });
    actual_local == local
        && scope.iter().any(|binding| {
            binding
                .strip_prefix('\0')
                .and_then(|binding| binding.split_once('\0'))
                .is_some_and(|(candidate, value)| {
                    candidate.as_bytes() == prefix && value == namespace
                })
        })
}

pub(super) fn element_prefix_has_binding(name: &[u8], scope: &[String]) -> bool {
    let prefix = name
        .iter()
        .position(|byte| *byte == b':')
        .map_or(&b""[..], |separator| &name[..separator]);
    scope.iter().any(|binding| {
        binding
            .strip_prefix('\0')
            .and_then(|binding| binding.split_once('\0'))
            .is_some_and(|(candidate, _)| candidate.as_bytes() == prefix)
    })
}

pub(super) fn prefixes_with_assumed_word_owner(
    name: &[u8],
    scope: &[String],
) -> Result<Vec<String>> {
    let prefix = name
        .iter()
        .position(|byte| *byte == b':')
        .map_or(&b""[..], |separator| &name[..separator]);
    let mut prefixes = scope.to_vec();
    prefixes.push(std::str::from_utf8(prefix)?.to_owned());
    Ok(prefixes)
}

pub(super) fn is_content_revision_element(name: &[u8], word_prefixes: &[String]) -> bool {
    is_word_element(name, b"ins", word_prefixes)
        || is_word_element(name, b"del", word_prefixes)
        || is_word_element(name, b"moveFrom", word_prefixes)
        || is_word_element(name, b"moveTo", word_prefixes)
}
