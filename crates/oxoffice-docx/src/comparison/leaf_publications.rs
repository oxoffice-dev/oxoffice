//! Publishing the source of each emitted leaf: runs, text and resolved occurrences.

use super::*;

pub(super) fn shift_leaf_publications(leaves: &mut [LeafPublication], delta: isize) -> Result<()> {
    for leaf in leaves {
        leaf.range.start = leaf
            .range
            .start
            .checked_add_signed(delta)
            .ok_or_else(|| Error::Other("leaf publication offset underflow".into()))?;
        leaf.range.end = leaf
            .range
            .end
            .checked_add_signed(delta)
            .ok_or_else(|| Error::Other("leaf publication offset underflow".into()))?;
    }
    Ok(())
}

/// Compose emitted child proofs at the actual replacement offsets. Existing
/// proofs outside the replaced intervals move by the same disjoint edits.
pub(super) fn splice_leaf_publications(
    metadata: &mut Metadata<'_>,
    mark: usize,
    spans: &[Range<usize>],
    replacements: &[String],
    children: Vec<Vec<LeafPublication>>,
) -> Result<()> {
    if spans.len() != replacements.len() || spans.len() != children.len() {
        return Err(Error::Other(
            "leaf replacement coverage differs from publication".into(),
        ));
    }
    let mut existing = metadata.leaf_publications.split_off(mark);
    for leaf in &mut existing {
        let mut delta = 0isize;
        for (span, replacement) in spans.iter().zip(replacements) {
            if span.end <= leaf.range.start {
                delta += replacement.len() as isize - span.len() as isize;
            } else if span.start < leaf.range.end {
                return Err(Error::Other(
                    "leaf proof overlaps another replaced owner".into(),
                ));
            }
        }
        shift_leaf_publications(std::slice::from_mut(leaf), delta)?;
    }
    let mut delta = 0isize;
    for ((span, replacement), mut leaves) in spans.iter().zip(replacements).zip(children) {
        shift_leaf_publications(&mut leaves, span.start as isize + delta)?;
        existing.extend(leaves);
        delta += replacement.len() as isize - span.len() as isize;
    }
    metadata.leaf_publications.extend(existing);
    Ok(())
}

pub(super) fn emitted_fragment_leaves(
    xml: &str,
    content: &CT_R,
    source: &RunSource,
    raw: &[(usize, usize, usize)],
    typed: &[Option<TypedSourceSelection>],
    inserted: bool,
    ignore_whitespace: bool,
    destination_parent: &NamespaceBindings,
) -> Result<Vec<LeafPublication>> {
    if !content
        .content
        .iter()
        .any(|item| matches!(item, RunContent::Text(_) | RunContent::DeletedText(_)))
    {
        return Ok(Vec::new());
    }
    let envelope = source_fragment(
        &format!("<rdocxComparisonPublication>{xml}</rdocxComparisonPublication>"),
        destination_parent,
    )?;
    let offset = element_opening(&envelope)?.0;
    let word = word_element_starts(&envelope)?;
    let mut children = direct_element_spans(&envelope)?;
    if inserted {
        children.retain(|span| {
            word.contains(&span.start) && element_local_name(&envelope[span.clone()]) == "ins"
        });
        if children.len() != 1 {
            return Err(Error::Other(
                "fragment insertion has no unique actual publication owner".into(),
            ));
        }
        let insertion = children[0].clone();
        children = direct_element_spans(&envelope[insertion.clone()])?
            .into_iter()
            .map(|span| insertion.start + span.start..insertion.start + span.end)
            .collect();
    }
    children.retain(|span| {
        word.contains(&span.start) && element_local_name(&envelope[span.clone()]) == "r"
    });
    if children.len() != 1 {
        return Err(Error::Other(
            "fragment publication has no unique actual run owner".into(),
        ));
    }
    let run = children.remove(0);
    let parent = publication_parent_at(&envelope, run.start)?;
    let closed = source_fragment(&envelope[run.clone()], &parent)?;
    let closure = element_opening(&closed)?.0 - element_opening(&envelope[run.clone()])?.0;
    let emitted_context = root_namespace_bindings(&closed, "r", &parent)?;
    let roles = run_publication_roles(&closed, content, source, raw, typed)?;
    let context = root_namespace_bindings(&source.scoped, "r", &source.parent)?;
    let mut leaves = Vec::new();
    for (span, raw, selection) in roles {
        let Some(selection) = selection.filter(|_| raw.is_none()) else {
            continue;
        };
        let Some(source_span) = source.typed_slots.get(selection.ordinal).cloned().flatten() else {
            return Err(Error::Other("fragment source leaf is unproved".into()));
        };
        if source_content_metadata(source, source_span.clone())?.is_empty() {
            continue;
        }
        let actual = source.scoped[source_span].to_owned();
        let Some(RunContent::Text(text) | RunContent::DeletedText(text)) =
            source_content_model(&actual, &context)?
        else {
            continue;
        };
        let (start, end) = selection.scalars.unwrap_or((0, text.text.chars().count()));
        let selected = source_text_fragment(&actual, &context, start..end)?;
        let Some(RunContent::Text(expected) | RunContent::DeletedText(expected)) =
            source_content_model(&selected, &context)?
        else {
            return Err(Error::Other("selected fragment is not text".into()));
        };
        let Some(RunContent::Text(emitted) | RunContent::DeletedText(emitted)) =
            source_content_model(&closed[span.clone()], &emitted_context)?
        else {
            return Err(Error::Other(
                "emitted fragment has no qualified text role".into(),
            ));
        };
        if expected.text != emitted.text
            && !(ignore_whitespace
                && expected.text.chars().all(char::is_whitespace)
                && emitted.text.chars().all(char::is_whitespace))
        {
            return Err(Error::Other(
                "emitted fragment differs from its attributed source pair".into(),
            ));
        }
        leaves.push(LeafPublication {
            range: run.start + span.start - closure - offset
                ..run.start + span.end - closure - offset,
            owner: source.owner,
            ordinal: selection.ordinal,
            scalars: start..end,
            source: actual,
            context: context.clone(),
            payload: emitted.text,
            physical: (source.owner, selection.ordinal, start..end),
            domains: [None, None],
            actions: Vec::new(),
        });
    }
    Ok(leaves)
}

pub(super) fn restore_resolved_leaf_occurrences(
    resolved: &[u8],
    publications: &[LeafPublication],
    mapped: &[Option<Range<usize>>],
) -> Result<String> {
    let xml = std::str::from_utf8(resolved).map_err(utf8_error)?;
    let mut groups = BTreeMap::<(usize, usize), Vec<(&LeafPublication, Range<usize>)>>::new();
    for (leaf, range) in publications.iter().zip(mapped) {
        if let Some(range) = range {
            groups
                .entry((leaf.owner, leaf.ordinal))
                .or_default()
                .push((leaf, range.clone()));
        }
    }
    let mut physical = HashMap::<(usize, usize), Vec<Range<usize>>>::new();
    for group in groups.values() {
        for (leaf, _) in group {
            let ranges = physical
                .entry((leaf.physical.0, leaf.physical.1))
                .or_default();
            if ranges.iter().any(|range| {
                *range == leaf.physical.2
                    || (range.start < leaf.physical.2.end && leaf.physical.2.start < range.end)
            }) {
                return Err(Error::Other(
                    "projection repeats physical emitted scalar provenance".into(),
                ));
            }
            ranges.push(leaf.physical.2.clone());
        }
    }
    let mut replacements = Vec::new();
    for group in groups.values() {
        let first = group[0].0;
        let Some(RunContent::Text(_) | RunContent::DeletedText(_)) =
            source_content_model(&first.source, &first.context)?
        else {
            return Err(Error::Other(
                "fragment restoration has no source text".into(),
            ));
        };
        validate_projection_actions(&first.domains, &first.actions)?;
        let mut value = String::new();
        let mut runs = Vec::<Range<usize>>::new();
        let mut properties = None;
        let mut root_facts = None;
        let mut root_scope = None;
        for (leaf, range) in group {
            if leaf.source != first.source
                || leaf.context != first.context
                || leaf.domains != first.domains
                || leaf.actions != first.actions
            {
                return Err(Error::Other(
                    "fragment restoration has incomplete source occurrence coverage".into(),
                ));
            }
            let parent = publication_parent_at(xml, range.start)?;
            let Some(RunContent::Text(text) | RunContent::DeletedText(text)) =
                source_content_model(&xml[range.clone()], &parent)?
            else {
                return Err(Error::Other("resolved fragment lost its text owner".into()));
            };
            if text.text != leaf.payload {
                return Err(Error::Other(
                    "resolved fragment changed its actual emitted scalar payload".into(),
                ));
            }
            value.push_str(&text.text);
            let (start, end) = containing_run(xml, range.start)?;
            let run = start..end;
            if !word_element_starts(xml)?.contains(&start) {
                return Err(Error::Other(
                    "resolved fragment has a foreign run owner".into(),
                ));
            }
            let props = direct_property(&xml[run.clone()], "rPr")?.unwrap_or_default();
            let parent_scope = publication_parent_at(xml, start)?;
            let facts = source_root_metadata(&xml[run.clone()], &parent_scope)?;
            let scope = root_namespace_bindings(&xml[run.clone()], "r", &parent_scope)?
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            if root_facts.as_ref().is_some_and(|actual| actual != &facts)
                || root_scope.as_ref().is_some_and(|actual| actual != &scope)
                || properties.as_ref().is_some_and(|actual| actual != &props)
            {
                return Err(Error::Other(
                    "fragment restoration crosses different run shells or properties".into(),
                ));
            }
            root_facts = Some(facts);
            root_scope = Some(scope);
            properties = Some(props);
            if runs.last() != Some(&run) {
                runs.push(run);
            }
        }
        if group.iter().any(|(leaf, _)| {
            leaf.scalars != leaf.physical.2 || leaf.physical.2.start > leaf.physical.2.end
        }) {
            return Err(Error::Other(
                "fragment restoration omits a source scalar interval".into(),
            ));
        }
        if group.len() == 1 {
            continue;
        }
        for pair in runs.windows(2) {
            if pair[0].end != pair[1].start
                || publication_parent_at(xml, pair[0].start)?
                    != publication_parent_at(xml, pair[1].start)?
            {
                return Err(Error::Other(
                    "fragment restoration crosses an authored source barrier".into(),
                ));
            }
        }
        // Retire only complete member runs. The terminal carrier stays in place,
        // including any independent raw suffix after its selected text children.
        let terminal = runs.last().unwrap();
        for run in &runs {
            let mut cursor = element_opening(&xml[run.clone()])?.0 + run.start;
            let mut seen_member = false;
            let mut suffix = false;
            for span in run_source_events(&xml[run.clone()])? {
                let child = run.start + span.start..run.start + span.end;
                if cursor != child.start {
                    return Err(Error::Other(
                        "fragment restoration crosses an unmodelled run event".into(),
                    ));
                }
                cursor = child.end;
                if element_local_name(&xml[child.clone()]) == "rPr"
                    && word_element_starts(xml)?.contains(&child.start)
                    && !seen_member
                    && !suffix
                {
                    continue;
                }
                if group.iter().any(|(_, range)| *range == child) {
                    if suffix {
                        return Err(Error::Other(
                            "fragment restoration crosses an independent authored leaf".into(),
                        ));
                    }
                    seen_member = true;
                } else if run == terminal && seen_member {
                    suffix = true;
                } else {
                    return Err(Error::Other(
                        "fragment restoration crosses an independent authored leaf".into(),
                    ));
                }
            }
            if cursor != run.start + paragraph_close_start(&xml[run.clone()])? {
                return Err(Error::Other(
                    "fragment restoration crosses trailing run events".into(),
                ));
            }
            if !seen_member {
                return Err(Error::Other(
                    "restored carrier has no selected child".into(),
                ));
            }
        }
        let (opening, _) = paragraph_shell(Some(&first.source))?;
        let leaf = format!(
            "{opening}{}</{}>",
            quick_xml::escape::escape(&value),
            element_opening(&first.source)?.2
        );
        for run in &runs[..runs.len() - 1] {
            replacements.push((run.clone(), String::new()));
        }
        let terminal_members = group
            .iter()
            .filter(|(_, range)| terminal.start < range.start && range.end < terminal.end)
            .map(|(_, range)| range.clone())
            .collect::<Vec<_>>();
        let Some(last) = terminal_members.last() else {
            return Err(Error::Other(
                "terminal carrier has no selected child".into(),
            ));
        };
        for member in &terminal_members[..terminal_members.len() - 1] {
            replacements.push((member.clone(), String::new()));
        }
        replacements.push((last.clone(), leaf));
    }
    replacements.sort_by_key(|(span, _)| span.start);
    if replacements
        .windows(2)
        .any(|pair| pair[0].0.end > pair[1].0.start)
    {
        return Err(Error::Other("restored source occurrences overlap".into()));
    }
    let mut result = xml.to_owned();
    for (span, replacement) in replacements.into_iter().rev() {
        result.replace_range(span, &replacement);
    }
    Ok(result)
}

pub(super) fn complete_run_selection(
    run: &CT_R,
) -> (Vec<RawSourceSelection>, Vec<Option<TypedSourceSelection>>) {
    let raw = run
        .extra_xml_positions
        .iter()
        .enumerate()
        .filter(|(_, encoded)| !CT_R::raw_child_is_root_attributes(**encoded))
        .map(|(ordinal, encoded)| (ordinal, ordinal, CT_R::raw_child_position(*encoded)))
        .collect();
    (
        raw,
        (0..run.content.len())
            .map(|ordinal| {
                Some(TypedSourceSelection {
                    ordinal,
                    scalars: None,
                })
            })
            .collect(),
    )
}

/// Generated spans receive roles from the emitted model and selected original
/// slots. The source is never searched by bytes, QName spelling or closure.
fn run_publication_roles(
    generated: &str,
    run: &CT_R,
    source: &RunSource,
    selected_raw: &[(usize, usize, usize)],
    selected_typed: &[Option<TypedSourceSelection>],
) -> Result<Vec<PublishedRunRole>> {
    if source.projected_raw_slots.iter().any(|ordinal| {
        !selected_raw
            .iter()
            .any(|(_, selected, _)| ordinal == selected)
    }) {
        return Err(Error::Other(
            "projected textbox escaped its selected source role".into(),
        ));
    }
    if source.projected_typed_slots.iter().any(|ordinal| {
        !selected_typed
            .iter()
            .flatten()
            .any(|selected| selected.ordinal == *ordinal && selected.scalars.is_none())
    }) {
        return Err(Error::Other(
            "projected drawing escaped its selected typed source role".into(),
        ));
    }
    if selected_typed.len() != run.content.len() {
        return Err(Error::Other(
            "typed publication selection is incomplete".into(),
        ));
    }
    let properties = usize::from(direct_property(generated, "rPr")?.is_some());
    let total = properties + run.content.len();
    let mut ordinary = vec![Vec::new(); total + 1];
    let mut comments = vec![Vec::new(); total + 1];
    let mut covered = vec![false; run.extra_xml.len()];
    let mut previous = None;
    for &(local, ordinal, original_boundary) in selected_raw {
        if local >= covered.len()
            || covered[local]
            || previous.is_some_and(|previous| ordinal <= previous)
        {
            return Err(Error::Other(
                "raw publication selection repeats or reorders a source occurrence".into(),
            ));
        }
        let Some(Some((_, boundary, comment))) = source.raw_slots.get(ordinal) else {
            return Err(Error::Other(
                "raw publication source ordinal is absent".into(),
            ));
        };
        if *boundary != original_boundary
            || CT_R::raw_child_is_root_attributes(run.extra_xml_positions[local])
        {
            return Err(Error::Other(
                "raw publication source boundary changed".into(),
            ));
        }
        let output_boundary = CT_R::raw_child_position(run.extra_xml_positions[local]);
        if output_boundary > total {
            return Err(Error::Other(
                "raw publication boundary is outside the emitted owner".into(),
            ));
        }
        if *comment {
            comments[output_boundary].push(ordinal);
        } else {
            ordinary[output_boundary].push(ordinal);
        }
        covered[local] = true;
        previous = Some(ordinal);
    }
    if covered
        .iter()
        .zip(&run.extra_xml_positions)
        .any(|(covered, encoded)| !*covered && !CT_R::raw_child_is_root_attributes(*encoded))
    {
        return Err(Error::Other(
            "raw publication selection omits emitted content".into(),
        ));
    }
    let events = run_source_events(generated)?;
    let mut cursor = 0;
    let mut roles = Vec::with_capacity(events.len());
    for boundary in 0..=total {
        for &ordinal in &ordinary[boundary] {
            let span = events
                .get(cursor)
                .ok_or_else(|| Error::Other("generated raw role has no event".into()))?
                .clone();
            roles.push((span, Some(ordinal), None));
            cursor += 1;
        }
        if boundary == total {
            if !comments[boundary].is_empty() {
                return Err(Error::Other("comment source has no typed reference".into()));
            }
            break;
        }
        if properties == 1 && boundary == 0 {
            if !comments[boundary].is_empty() {
                return Err(Error::Other(
                    "comment source precedes the emitted property role".into(),
                ));
            }
            let span = events
                .get(cursor)
                .ok_or_else(|| Error::Other("generated properties have no event".into()))?
                .clone();
            roles.push((span, None, None));
            cursor += 1;
            continue;
        }
        let index = boundary - properties;
        let width = run_content_source_width(&run.content[index]);
        if !comments[boundary].is_empty() {
            if comments[boundary].len() != 1
                || !matches!(run.content[index], RunContent::CommentReference { .. })
            {
                return Err(Error::Other(
                    "comment raw source has a different typed owner".into(),
                ));
            }
            let span = events
                .get(cursor)
                .ok_or_else(|| Error::Other("generated comment has no event".into()))?
                .clone();
            roles.push((span, Some(comments[boundary][0]), None));
            cursor += 1;
        } else {
            for _ in 0..width {
                let span = events
                    .get(cursor)
                    .ok_or_else(|| Error::Other("generated typed role has no event".into()))?
                    .clone();
                roles.push((
                    span,
                    None,
                    (width == 1).then_some(selected_typed[index]).flatten(),
                ));
                cursor += 1;
            }
        }
    }
    if cursor != events.len() {
        return Err(Error::Other(
            "generated run roles do not cover its events".into(),
        ));
    }
    Ok(roles)
}

/// Supply only the actual run grammar and inherited context for one associated
/// content slot. Neither this read envelope nor its namespace declarations publish.
fn source_content_envelope(xml: &str, context: &[(String, String)]) -> Result<Vec<u8>> {
    let (binding, _) = context
        .iter()
        .find(|(_, uri)| uri == W_NS)
        .ok_or_else(|| Error::Other("content source has no qualified run context".into()))?;
    let name = if binding == "xmlns" {
        "r".to_owned()
    } else {
        format!(
            "{}:r",
            binding
                .strip_prefix("xmlns:")
                .ok_or_else(|| Error::Other("content context has an invalid binding".into()))?
        )
    };
    let mut writer = Writer::new(Vec::new());
    let mut root = BytesStart::new(name.as_str());
    for (key, value) in context {
        root.push_attribute((key.as_str(), value.as_str()));
    }
    writer.write_event(Event::Start(root))?;
    writer.get_mut().extend_from_slice(xml.as_bytes());
    writer.write_event(Event::End(BytesEnd::new(name.as_str())))?;
    Ok(writer.into_inner())
}

/// Interpret the actual selected content child, independently of its spelling.
pub(super) fn source_content_model(
    xml: &str,
    context: &[(String, String)],
) -> Result<Option<RunContent>> {
    let run = CT_R::from_xml_fragment_checked(&source_content_envelope(xml, context)?)?;
    Ok(if run.content.len() == 1 {
        Some(run.content[0].clone())
    } else {
        None
    })
}

/// Use the existing lower nonformatting content classifier, excluding rPr/pPr.
pub(super) fn source_content_metadata(
    source: &RunSource,
    span: Range<usize>,
) -> Result<Vec<String>> {
    let context = root_namespace_bindings(&source.scoped, "r", &source.parent)?;
    if !word_element_starts(&source.scoped)?.contains(&span.start) {
        return Ok(Vec::new());
    }
    Ok(oxoffice_docx_oxml::properties::protected_source_residual(
        &source_content_envelope(&source.scoped[span], &context)?,
        &[],
    )?)
}

fn source_text_fragment(
    xml: &str,
    context: &NamespaceBindings,
    scalars: Range<usize>,
) -> Result<String> {
    let Some(RunContent::Text(text) | RunContent::DeletedText(text)) =
        source_content_model(xml, context)?
    else {
        return Err(Error::Other(
            "split typed role has no actual text model".into(),
        ));
    };
    let mut reader = Reader::from_str(xml);
    let mut depth = 0;
    loop {
        match reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?
        {
            Event::Start(_) => {
                depth += 1;
                if depth != 1 {
                    return Err(Error::Other("split text has opaque mixed events".into()));
                }
            }
            Event::Empty(_) if depth == 0 => {}
            Event::End(_) => {
                depth -= 1;
            }
            // The reader splits text around each character or entity
            // reference, such as `&amp;`. The value comes from the model.
            Event::Text(_) | Event::GeneralRef(_) if depth == 1 => {}
            Event::Eof => break,
            _ => return Err(Error::Other("split text has opaque mixed events".into())),
        }
    }
    if scalars.start > scalars.end || scalars.end > text.text.chars().count() {
        return Err(Error::Other(
            "split text scalar range is outside its source".into(),
        ));
    }
    let value = text
        .text
        .chars()
        .skip(scalars.start)
        .take(scalars.len())
        .collect::<String>();
    let (end, empty, name) = element_opening(xml)?;
    let mut opening = xml[..end].to_owned();
    if empty {
        opening.truncate(end - 2);
        opening.push('>');
    }
    if has_edge_whitespace(&value) && !text.preserve_space {
        // The actual leaf shell is retained. Only the standard serialization
        // space policy is added for the selected text's new boundary.
        let mut start = match Reader::from_str(&opening)
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?
        {
            Event::Start(start) => start.into_owned(),
            _ => unreachable!(),
        };
        let attributes = start
            .attributes()
            .map(|attribute| {
                attribute.map(|attribute| {
                    (
                        attribute.key.as_ref().to_vec(),
                        attribute.value.as_ref().to_vec(),
                    )
                })
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| Error::Other(error.to_string()))?;
        start.clear_attributes();
        for (key, value) in &attributes {
            if key.as_slice() != b"xml:space" {
                start.push_attribute((key.as_slice(), value.as_slice()));
            }
        }
        start.push_attribute(("xml:space", "preserve"));
        let mut writer = Writer::new(Vec::new());
        writer.write_event(Event::Start(start))?;
        opening = String::from_utf8(writer.into_inner()).map_err(utf8_error)?;
    }
    Ok(format!(
        "{opening}{}</{name}>",
        quick_xml::escape::escape(&value)
    ))
}

pub(super) fn preserve_run_source(
    generated: &str,
    original_properties: &str,
    sources: [&RunSource; 2],
    content_run: &CT_R,
    selected_raw: &[(usize, usize, usize)],
    selected_typed: &[Option<TypedSourceSelection>],
    destination_parent: &NamespaceBindings,
    ids: &IdAllocator,
) -> Result<String> {
    let [shell_source, content_source] = sources;
    let source = shell_source.scoped.as_str();
    if element_local_name(generated) != "r" || element_local_name(source) != "r" {
        return ids.generated(generated.to_owned());
    }
    let raw_properties = direct_property(source, "rPr")?;
    let generated_properties = direct_property(generated, "rPr")?;
    let current = patch_property_source(
        original_properties,
        generated_properties.as_deref().unwrap_or(""),
        raw_properties.as_deref(),
        ids,
    )?;
    let current = if let Some(prior) = raw_properties.as_deref() {
        preserve_prior_property_sources(&current, prior)?
    } else {
        current
    };
    let (mut output, closing) = paragraph_shell(Some(source))?;
    let destination = root_namespace_bindings(&output, "r", destination_parent)?;
    let context = root_namespace_bindings(&content_source.scoped, "r", &content_source.parent)?;
    let same_scope = destination
        .iter()
        .map(|(key, uri)| (key, uri))
        .collect::<BTreeMap<_, _>>()
        == context
            .iter()
            .map(|(key, uri)| (key, uri))
            .collect::<BTreeMap<_, _>>();
    let current =
        if let Some(actual) = direct_word_element_spans(&shell_source.scoped, "rPr")?.first() {
            let scoped_open = element_opening(&shell_source.scoped)?.0;
            let raw_open = element_opening(&shell_source.raw)?.0;
            if shell_source.scoped[scoped_open..] != shell_source.raw[raw_open..] {
                return Err(Error::Other(
                    "retained run extraction changed descendant source".into(),
                ));
            }
            let added = scoped_open
                .checked_sub(raw_open)
                .ok_or_else(|| Error::Other("retained run extraction shortened its root".into()))?;
            restore_extracted_root(
                &current,
                &shell_source.raw[actual.start - added..actual.end - added],
                &destination,
            )?
        } else {
            current
        };
    let roles = run_publication_roles(
        generated,
        content_run,
        content_source,
        selected_raw,
        selected_typed,
    )?;
    let word = word_element_starts(generated)?;
    if generated_properties.is_none() && !current.is_empty() {
        output.push_str(&current);
    }
    let mut cursor = element_opening(generated)?.0;
    for (span, raw_ordinal, typed_ordinal) in roles {
        output.push_str(&generated[cursor..span.start]);
        let generated_child = &generated[span.clone()];
        if word.contains(&span.start) && element_local_name(generated_child) == "rPr" {
            output.push_str(&current);
        } else {
            let actual = raw_ordinal
                .and_then(|ordinal| {
                    content_source.raw_slots[ordinal]
                        .as_ref()
                        .map(|(span, _, comment)| (span.clone(), *comment))
                })
                .or_else(|| {
                    typed_ordinal
                        .and_then(|ordinal| {
                            content_source
                                .typed_slots
                                .get(ordinal.ordinal)
                                .cloned()
                                .flatten()
                        })
                        .map(|span| (span, true))
                });
            let mut restored = false;
            if let Some((source_span, typed_authority)) = actual {
                let mut source_child_xml = content_source.scoped[source_span.clone()].to_owned();
                if let Some((start, end)) = typed_ordinal.and_then(|selected| selected.scalars) {
                    source_child_xml =
                        source_text_fragment(&source_child_xml, &context, start..end)?;
                }
                let deletion_name = match (
                    raw_ordinal.is_some(),
                    element_local_name(generated_child),
                    element_local_name(&source_child_xml),
                ) {
                    (true, "delInstrText", "instrText") => Some("delInstrText"),
                    (false, "delText", "t") => Some("delText"),
                    _ => None,
                };
                if let Some(target) = deletion_name
                    && word.contains(&span.start)
                    && word_element_starts(&content_source.scoped)?.contains(&source_span.start)
                {
                    let (_, empty, name) = element_opening(&source_child_xml)?;
                    let replacement = name
                        .rsplit_once(':')
                        .map_or(target.to_owned(), |(prefix, _)| {
                            format!("{prefix}:{target}")
                        });
                    if !empty {
                        let close = paragraph_close_start(&source_child_xml)?;
                        source_child_xml
                            .replace_range(close + 2..close + 2 + name.len(), &replacement);
                    }
                    source_child_xml.replace_range(1..1 + name.len(), &replacement);
                }
                let lexical = raw_occurrence_representation(&source_child_xml, &context)?
                    == raw_occurrence_representation(generated_child, &context)?;
                let modeled = if raw_ordinal.is_none() && !lexical {
                    let actual = source_content_model(&source_child_xml, &context)?;
                    // These generated typed Word events come from CT_R's writer,
                    // independently of a producer's retained w binding.
                    let mut generated_context = destination.clone();
                    if word.contains(&span.start) {
                        generated_context.retain(|(key, _)| key != "xmlns:w");
                        generated_context.push(("xmlns:w".into(), W_NS.into()));
                    }
                    let generated = source_content_model(generated_child, &generated_context)?;
                    match (actual, generated) {
                        (Some(actual), Some(generated)) => {
                            run_content_signature(&actual) == run_content_signature(&generated)
                        }
                        _ => false,
                    }
                } else {
                    false
                };
                let derived = typed_ordinal.is_some_and(|ordinal| {
                    content_source
                        .projected_typed_slots
                        .contains(&ordinal.ordinal)
                });
                let consistent = lexical || modeled || derived;
                if consistent {
                    if same_scope
                        || !source_child_xml.starts_with('<')
                        || source_child_xml.starts_with("<?")
                        || source_child_xml.starts_with("<!--")
                    {
                        output.push_str(&source_child_xml);
                    } else if raw_ordinal.is_some() {
                        output.push_str(&source_child(&content_source.scoped, source_span)?);
                    } else {
                        output.push_str(&source_fragment(&source_child_xml, &context)?);
                    }
                    restored = true;
                } else if !typed_authority {
                    return Err(Error::Other(format!(
                        "generated raw source occurrence {:?} changed its payload",
                        raw_ordinal
                    )));
                }
            }
            if !restored {
                let closed = source_child(generated, span.clone())?;
                output.push_str(&retain_source_root_namespaces(
                    &closed,
                    generated_child,
                    &destination,
                )?);
            }
        }
        cursor = span.end;
    }
    let end = if element_opening(generated)?.1 {
        element_opening(generated)?.0
    } else {
        paragraph_close_start(generated)?
    };
    output.push_str(&generated[cursor..end]);
    output.push_str(&closing);
    retain_source_root_namespaces(&output, &shell_source.raw, destination_parent)
}
