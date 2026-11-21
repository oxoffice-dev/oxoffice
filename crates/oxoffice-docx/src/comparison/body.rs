//! Comparing a body: alignment of its blocks, moved content, restoring the root.

use super::*;

pub(super) fn compare_body(
    documents: [&CT_Document; 2],
    location: &str,
    original_source: Option<OriginalBodySource<'_>>,
    edited_source: OriginalBodySource<'_>,
    held: [&HeldRevisions; 2],
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let [original, edited] = documents;
    let mut frame = SourceSnapshots {
        story: location.to_owned(),
        ..SourceSnapshots::default()
    };
    if let Some(source) = original_source {
        frame.input_role = 0;
        frame.body(original, source, held[0])?;
    }
    frame.input_role = 1;
    frame.body(edited, edited_source, held[1])?;
    metadata.source_frames.push(frame);
    let result = compare_body_impl(
        original,
        edited,
        location,
        original_source,
        edited_source,
        held,
        metadata,
        diagnostics,
    );
    metadata.source_frames.pop();
    result
}

fn compare_body_impl(
    original: &CT_Document,
    edited: &CT_Document,
    location: &str,
    original_source: Option<OriginalBodySource<'_>>,
    edited_source: OriginalBodySource<'_>,
    held: [&HeldRevisions; 2],
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let [held_original, held_edited] = held;
    let source_bindings = original_source.map_or(&[][..], |(_, _, bindings)| bindings);
    let original_signatures = original
        .body
        .content
        .iter()
        .map(|content| body_signature_with_options(content, metadata.options))
        .collect::<Vec<_>>();
    let edited_signatures = edited
        .body
        .content
        .iter()
        .map(|content| body_signature_with_options(content, metadata.options))
        .collect::<Vec<_>>();
    fn paragraphs(content: &[BodyContent]) -> Vec<Option<&CT_P>> {
        content
            .iter()
            .map(|content| match content {
                BodyContent::Paragraph(paragraph) => Some(paragraph.as_ref()),
                _ => None,
            })
            .collect()
    }
    let original_paragraphs = paragraphs(&original.body.content);
    let edited_paragraphs = paragraphs(&edited.body.content);
    let (aligned, carried) = replace_changed_paragraph_runs(
        pair_gaps_by_kind(
            align(&original_signatures, &edited_signatures),
            &original_signatures,
            &edited_signatures,
        ),
        &original_paragraphs,
        &edited_paragraphs,
        &original_signatures,
        &edited_signatures,
        metadata.options,
    )?;
    let moves = pair_moves(
        &aligned,
        &original_signatures,
        &edited_signatures,
        uncarried_field_owners(&aligned, &original_paragraphs, &edited_paragraphs, &carried)?,
        &mut metadata.ids,
    )?;
    let trailing_inserted = trailing_paragraphs(&aligned, |(left, right)| {
        right
            .filter(|_| left.is_none())
            .and_then(|index| edited.body.content.get(index))
    });
    let trailing_deleted = trailing_paragraphs(&aligned, |(left, right)| {
        left.filter(|_| right.is_none())
            .and_then(|index| original.body.content.get(index))
    });
    // A final paragraph change marks the paragraph mark of an owner already
    // emitted, which must not be one that holds an existing revision.
    let refuse_held_mark = |position: Option<usize>| -> Result<()> {
        let Some((left, right)) = position.map(|position| aligned[position]) else {
            return Ok(());
        };
        if held_original.holds(left) || held_edited.holds(right) {
            return Err(Error::Other(format!(
                "comparison cannot mark the paragraph mark of {location}/paragraph[{}] for the final paragraph change after it, because that paragraph holds an existing tracked revision",
                left.or(right).unwrap_or_default()
            )));
        }
        Ok(())
    };
    // The protected-owner exception admits only actual equal content and
    // formatting. Caller ignore policies still govern ordinary comparison.
    let protected_equality_options = ComparisonOptions::default();
    let mut output: Vec<(EmittedOwner, String)> = Vec::new();
    let mut trailing = (PropertySide::default(), false, false);
    let mut owner_leaves = Vec::new();
    for (position, (original_index, edited_index)) in aligned.iter().copied().enumerate() {
        let item_leaf_mark = metadata.leaf_publications.len();
        let item = |(left, right): (Option<usize>, Option<usize>)| {
            right
                .and_then(|index| edited.body.content.get(index))
                .or_else(|| left.and_then(|index| original.body.content.get(index)))
        };
        let next_takes_mark = next_kept_owner(&aligned, position, |pair| {
            matches!(item(pair), Some(BodyContent::Table(_)))
        })
        .is_some_and(|pair| {
            matches!(
                item(pair),
                Some(BodyContent::Paragraph(_) | BodyContent::Table(_))
            )
        });
        match (original_index, edited_index) {
            (Some(left), Some(right)) => {
                let original_content = &original.body.content[left];
                let edited_content = &edited.body.content[right];
                let protected = held_original.holds(Some(left)) || held_edited.holds(Some(right));
                let same_source = held_original.same_owner_source(left, held_edited, right);
                let equal = original_content == edited_content
                    || !metadata.leaf_capture
                        && if protected {
                            coalesced_held_owner_equal(
                                original_content,
                                edited_content,
                                &protected_equality_options,
                                metadata,
                            )
                        } else {
                            match (original_content, edited_content) {
                                (BodyContent::Paragraph(left), BodyContent::Paragraph(right)) => {
                                    coalesced_paragraphs_equal(
                                        left,
                                        right,
                                        metadata.options,
                                        metadata,
                                    )
                                }
                                _ => false,
                            }
                        };
                // A protected owner both versions hold as the same owner,
                // written another way (see `same_held_owner`: the same tree,
                // or, with the same residual, what the model writes back),
                // is unchanged: the redline keeps the original's bytes.
                let rewritten = protected
                    && !(equal && same_source)
                    && !metadata.leaf_capture
                    && same_held_owner(
                        equal,
                        same_source,
                        || same_owner_tree(original_source, left, edited_source, right),
                        || {
                            same_written_owner(
                                original_content,
                                edited_content,
                                original_source.map_or(&[][..], |(_, _, bindings)| bindings),
                                edited_source.2,
                            )
                        },
                    )?;
                let equal = equal || rewritten;
                let same_source = same_source || rewritten;
                let needs_field_dispatch = equal
                    && !(protected && same_source)
                    && SourceMetadata::body(
                        original_content,
                        &SourceSnapshots::default(),
                        metadata.options,
                    )?
                    .has_fields;
                let same_physical_source = if needs_field_dispatch
                    && let Some((source, spans, bindings)) = original_source
                {
                    if let Some((left, right)) = source
                        .get(spans[left].clone())
                        .zip(edited_source.0.get(edited_source.1[right].clone()))
                        && left == right
                    {
                        let external = |bindings: &[(String, String)]| {
                            bindings
                                .iter()
                                .map(|(name, uri)| {
                                    (
                                        name.strip_prefix("xmlns:").unwrap_or("").to_owned(),
                                        uri.clone(),
                                    )
                                })
                                .collect::<BTreeMap<_, _>>()
                        };
                        required_external_bindings(left.as_bytes(), &external(bindings))?
                            == required_external_bindings(
                                right.as_bytes(),
                                &external(edited_source.2),
                            )?
                    } else {
                        false
                    }
                } else {
                    false
                };
                // Global Field equality omits representation. Exact source and
                // required bindings additionally prove a field owner's no-op.
                let field_dispatch = needs_field_dispatch && !same_physical_source;
                let compared = if equal && !field_dispatch && (!protected || same_source) {
                    if let Some((source, spans, _)) = original_source {
                        source
                            .get(spans[left].clone())
                            .ok_or_else(|| {
                                Error::Other(format!(
                                    "comparison source span is invalid at {location}[{left}]"
                                ))
                            })?
                            .to_owned()
                    } else {
                        body_content_xml(original_content)?
                    }
                } else {
                    let held_reason = match (
                        held_original.holds(Some(left)),
                        held_edited.holds(Some(right)),
                    ) {
                        (true, true) => Some("the edited document changes it"),
                        (true, false) => Some(
                            "the edited document does not hold the revision the original holds there",
                        ),
                        (false, true) => Some(
                            "the edited document holds a tracked revision there that the original does not",
                        ),
                        (false, false) => None,
                    };
                    if let Some(reason) = held_reason {
                        return Err(Error::Other(format!(
                            "comparison cannot change {}, which holds an existing tracked revision: {reason}",
                            body_location(location, original_content, left)
                        )));
                    }
                    let content_source = original_source
                        .map(|(source, spans, bindings)| {
                            source_fragment(&source[spans[left].clone()], bindings)
                        })
                        .transpose()?;
                    compare_body_content(
                        original_content,
                        edited_content,
                        content_source.as_deref(),
                        &body_location(location, edited_content, right),
                        metadata,
                        diagnostics,
                    )?
                };
                let owner = match (original_content, edited_content) {
                    (BodyContent::Table(_), BodyContent::Table(_)) => EmittedOwner::KeptTable,
                    _ => EmittedOwner::of(original_content, edited_content, None, metadata)?,
                };
                output.push((owner, compared));
            }
            (Some(left), None) => {
                let content = &original.body.content[left];
                if held_original.holds(Some(left)) {
                    let place = body_location(location, content, left);
                    return Err(Error::Other(if moves.original[left].is_some() {
                        format!(
                            "comparison cannot move {place}, which holds an existing tracked revision"
                        )
                    } else if carried.original.contains(&left) {
                        format!(
                            "comparison cannot replace {place} whole, because it holds an existing tracked revision"
                        )
                    } else {
                        format!(
                            "comparison cannot delete {place}, which holds an existing tracked revision"
                        )
                    }));
                }
                let move_id = moves.original[left];
                let kind = if move_id.is_some() { "moveFrom" } else { "del" };
                let whole = carried.original.contains(&left);
                if let BodyContent::Paragraph(paragraph) = content
                    && let Some((first, last)) =
                        trailing_deleted.filter(|&(first, _)| position >= first)
                {
                    trailing_paragraph(
                        &mut output,
                        TrailingRun {
                            content,
                            paragraph,
                            kind,
                            move_id,
                            carried: whole,
                            first: position == first,
                            single: first == last,
                            next_takes_mark,
                            location,
                        },
                        &mut trailing,
                        &refuse_held_mark,
                        source_bindings,
                        metadata,
                    )?;
                } else if let BodyContent::Paragraph(paragraph) = content
                    && !next_takes_mark
                    && !after_kept_table(&output, kind, location)
                {
                    refuse_held_mark(previous_mark_position(&output, kind))?;
                    let merged = mark_before_final_paragraph(
                        &mut output,
                        kind,
                        move_id,
                        source_bindings,
                        metadata,
                    )?;
                    output.push(final_paragraph(
                        paragraph, kind, move_id, whole, merged, metadata,
                    )?);
                } else {
                    output.push(unmatched_body_owner(
                        content, kind, move_id, whole, metadata,
                    )?);
                }
            }
            (None, Some(right)) => {
                let content = &edited.body.content[right];
                if held_edited.holds(Some(right)) {
                    let action = if moves.edited[right].is_some() {
                        "move"
                    } else {
                        "insert"
                    };
                    return Err(Error::Other(format!(
                        "comparison cannot {action} edited {}, which holds a tracked revision that the original does not hold there",
                        body_location(location, content, right)
                    )));
                }
                let move_id = moves.edited[right];
                let kind = if move_id.is_some() { "moveTo" } else { "ins" };
                let whole = carried.edited.contains(&right);
                if let BodyContent::Paragraph(paragraph) = content
                    && let Some((first, last)) =
                        trailing_inserted.filter(|&(first, _)| position >= first)
                {
                    trailing_paragraph(
                        &mut output,
                        TrailingRun {
                            content,
                            paragraph,
                            kind,
                            move_id,
                            carried: whole,
                            first: position == first,
                            single: first == last,
                            next_takes_mark,
                            location,
                        },
                        &mut trailing,
                        &refuse_held_mark,
                        source_bindings,
                        metadata,
                    )?;
                } else if let BodyContent::Paragraph(paragraph) = content
                    && !next_takes_mark
                    && !after_kept_table(&output, kind, location)
                {
                    refuse_held_mark(previous_mark_position(&output, kind))?;
                    let merged = mark_before_final_paragraph(
                        &mut output,
                        kind,
                        move_id,
                        source_bindings,
                        metadata,
                    )?;
                    output.push(final_paragraph(
                        paragraph, kind, move_id, whole, merged, metadata,
                    )?);
                } else {
                    output.push(unmatched_body_owner(
                        content, kind, move_id, whole, metadata,
                    )?);
                }
            }
            (None, None) => unreachable!(),
        }
        owner_leaves.push(metadata.leaf_publications.split_off(item_leaf_mark));
    }
    let mut output = if let Some((source, spans, bindings)) = original_source {
        let section_start = if original.body.sect_pr.is_some() {
            direct_word_child_start(source, bindings, b"sectPr")?
        } else {
            None
        };
        interleave_story_source(
            source,
            spans,
            &aligned,
            output,
            section_start,
            bindings,
            edited_source,
            &mut owner_leaves,
            &mut metadata.leaf_publications,
        )?
    } else {
        let mut result = String::new();
        for ((_, xml), mut leaves) in output.into_iter().zip(owner_leaves) {
            shift_leaf_publications(&mut leaves, result.len() as isize)?;
            metadata.leaf_publications.extend(leaves);
            result.push_str(&xml);
        }
        result
    };
    let same_section_source = held_original
        .section_source
        .as_ref()
        .zip(held_edited.section_source.as_ref())
        .is_some_and(|(original, edited)| original == edited);
    if (held_original.section || held_edited.section)
        && (original.body.sect_pr != edited.body.sect_pr || !same_section_source)
    {
        return Err(Error::Other(format!(
            "comparison cannot change the section properties of {location}, which hold an existing tracked revision"
        )));
    }
    let section_xml = section_properties_xml(
        original.body.sect_pr.as_ref(),
        edited.body.sect_pr.as_ref(),
        "section",
        metadata,
        diagnostics,
    )?;
    let section_xml = if let Some((source, _, bindings)) = original_source {
        let wrapper = source_fragment(&format!("<source>{source}</source>"), bindings)?;
        if let Some(span) = direct_word_element_spans(&wrapper, "sectPr")?.first() {
            if held_original.section {
                wrapper[span.clone()].to_owned()
            } else {
                let raw = source_child(&wrapper, span.clone())?;
                let before = original
                    .body
                    .sect_pr
                    .as_ref()
                    .map(section_property_xml)
                    .transpose()?
                    .unwrap_or_default();
                let current =
                    patch_property_source(&before, &section_xml, Some(&raw), &metadata.ids)?;
                preserve_prior_property_sources(&current, &raw)?
            }
        } else {
            section_xml
        }
    } else {
        section_xml
    };
    output.push_str(&section_xml);
    Ok(output)
}

/// Remove only closure added to a modeled source owner's root when it is
/// published back under identical inherited bindings. Authored declarations,
/// non-declaration attributes and every descendant byte remain untouched.
pub(super) fn retain_source_root_namespaces(
    published: &str,
    source: &str,
    destination: &[(String, String)],
) -> Result<String> {
    let scoped = source_fragment(source, destination)?;
    let mut source_reader = NsReader::from_str(&scoped);
    let (namespace, event) = source_reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let (Event::Start(root) | Event::Empty(root)) = event else {
        return Ok(published.to_owned());
    };
    if !matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == W_NS.as_bytes())
        || !matches!(
            root.local_name().as_ref(),
            b"p" | b"tbl"
                | b"sdt"
                | b"hyperlink"
                | b"r"
                | b"t"
                | b"delText"
                | b"instrText"
                | b"delInstrText"
                | b"fldChar"
                | b"drawing"
                | b"tab"
                | b"br"
                | b"sym"
                | b"footnoteReference"
                | b"endnoteReference"
                | b"commentReference"
                | b"cr"
                | b"noBreakHyphen"
                | b"softHyphen"
                | b"ptab"
        )
    {
        return Ok(published.to_owned());
    }
    restore_extracted_root(published, source, destination)
}

/// Source-only namespace restoration after an actual retained owner has been
/// established. It does not participate in comparison identity or selection.
pub(super) fn restore_extracted_root(
    published: &str,
    source: &str,
    destination: &[(String, String)],
) -> Result<String> {
    let mut original = Reader::from_str(source);
    let (Event::Start(original) | Event::Empty(original)) = original
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?
    else {
        return Ok(published.to_owned());
    };
    let authored = declared_prefixes(&original)?;
    let original_scope = source_fragment(source, destination)?;
    let mut qualified_original = NsReader::from_str(&original_scope);
    let (source_namespace, _) = qualified_original
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let source_namespace = match source_namespace {
        ResolveResult::Bound(uri) => Some(uri.as_ref().to_vec()),
        ResolveResult::Unbound => None,
        ResolveResult::Unknown(_) => {
            return Err(Error::Other(
                "retained source root namespace is unresolved".into(),
            ));
        }
    };
    let published_scope = source_fragment(published, destination)?;
    let mut qualified = NsReader::from_str(&published_scope);
    let (namespace, qualified_event) = qualified
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let namespace = match namespace {
        ResolveResult::Bound(uri) => Some(uri.as_ref().to_vec()),
        ResolveResult::Unbound => None,
        ResolveResult::Unknown(_) => {
            return Err(Error::Other(
                "published retained root namespace is unresolved".into(),
            ));
        }
    };
    if namespace != source_namespace
        || !matches!(qualified_event, Event::Start(_) | Event::Empty(_))
    {
        return Ok(published.to_owned());
    }
    let mut reader = Reader::from_str(published);
    let event = reader
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let (Event::Start(element) | Event::Empty(element)) = event else {
        return Ok(published.to_owned());
    };
    if element.local_name().as_ref() != element_local_name(source).as_bytes() {
        return Ok(published.to_owned());
    }
    let mut remove = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let name = std::str::from_utf8(attribute.key.as_ref()).map_err(utf8_error)?;
        let prefix = if name == "xmlns" {
            ""
        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
            prefix
        } else {
            continue;
        };
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(|error| Error::Other(error.to_string()))?;
        if authored.contains(prefix)
            || !destination
                .iter()
                .any(|(key, uri)| key == name && uri == value.as_ref())
        {
            continue;
        }
        // The borrowed event retains the exact source slices, including quotes.
        let key = attribute.key.as_ref().as_ptr() as usize - published.as_ptr() as usize;
        let end = attribute.value.as_ptr() as usize - published.as_ptr() as usize
            + attribute.value.len()
            + 1;
        if key == 0
            || end > published.len()
            || !matches!(published.as_bytes()[end - 1], b'\'' | b'"')
        {
            return Err(Error::Other(
                "namespace declaration has no exact source range".into(),
            ));
        }
        let start = if published.as_bytes()[key - 1].is_ascii_whitespace() {
            key - 1
        } else {
            key
        };
        remove.push(start..end);
    }
    let mut output = published.to_owned();
    for range in remove.into_iter().rev() {
        output.replace_range(range, "");
    }
    Ok(output)
}

/// Current pPr and its mark rPr retain their actual owner context. Prior
/// change payloads have different roles and deliberately keep their closure.
pub(super) fn restore_current_property_roots(
    published: &str,
    actual: &str,
    destination: &[(String, String)],
) -> Result<String> {
    let mut output = restore_extracted_root(published, actual, destination)?;
    if element_local_name(actual) == "pPr" {
        let scoped = source_fragment(actual, destination)?;
        if let (Some(source_span), Some(output_span)) = (
            direct_word_element_spans(&scoped, "rPr")?.first(),
            direct_word_element_spans(&output, "rPr")?.first(),
        ) {
            let delta = element_opening(&scoped)?
                .0
                .checked_sub(element_opening(actual)?.0)
                .ok_or_else(|| {
                    Error::Other("property extraction shortened its source root".into())
                })?;
            if scoped[element_opening(&scoped)?.0..] != actual[element_opening(actual)?.0..] {
                return Err(Error::Other(
                    "property extraction changed descendant bytes".into(),
                ));
            }
            let context = root_namespace_bindings(&output, "pPr", destination)?;
            let restored = restore_extracted_root(
                &output[output_span.clone()],
                &actual[source_span.start - delta..source_span.end - delta],
                &context,
            )?;
            output.replace_range(output_span.clone(), &restored);
        }
    }
    Ok(output)
}

fn interleave_story_source(
    source: &str,
    spans: &[Range<usize>],
    aligned: &[(Option<usize>, Option<usize>)],
    output: Vec<(EmittedOwner<'_>, String)>,
    section_start: Option<usize>,
    bindings: &[(String, String)],
    edited_source: OriginalBodySource<'_>,
    owner_leaves: &mut [Vec<LeafPublication>],
    publications: &mut Vec<LeafPublication>,
) -> Result<String> {
    if spans.len() != aligned.iter().filter(|(left, _)| left.is_some()).count()
        || aligned.len() != output.len()
    {
        return Err(Error::Other(
            "comparison source spans do not match related-story owners".to_owned(),
        ));
    }
    let content_end = section_start.unwrap_or(source.len());
    let mut tracked = String::new();
    if let Some(first) = spans.first() {
        tracked.push_str(&source[..first.start]);
    } else {
        tracked.push_str(&source[..content_end]);
    }
    let mut next_original = 0usize;
    for (owner_index, ((left, right), (_, xml))) in aligned.iter().zip(output).enumerate() {
        if let Some(index) = left {
            if *index != next_original {
                return Err(Error::Other(
                    "comparison related-story owner order is not monotonic".to_owned(),
                ));
            }
            if *index > 0 {
                tracked.push_str(&source[spans[index - 1].end..spans[*index].start]);
            }
            next_original += 1;
        }
        let template = left
            .map(|index| &source[spans[index].clone()])
            .or_else(|| right.map(|index| &edited_source.0[edited_source.1[index].clone()]));
        let published = if let Some(template) = template {
            retain_source_root_namespaces(&xml, template, bindings)?
        } else {
            xml.clone()
        };
        let delta = element_opening(&published)?.0 as isize - element_opening(&xml)?.0 as isize;
        let mut leaves = std::mem::take(&mut owner_leaves[owner_index]);
        shift_leaf_publications(&mut leaves, tracked.len() as isize + delta)?;
        publications.extend(leaves);
        tracked.push_str(&published);
    }
    if let Some(last) = spans.last() {
        if last.end > content_end {
            return Err(Error::Other(
                "comparison section properties overlap body content".to_owned(),
            ));
        }
        tracked.push_str(&source[last.end..content_end]);
    }
    Ok(tracked)
}

fn direct_word_child_start(
    xml: &str,
    bindings: &[(String, String)],
    local_name: &[u8],
) -> Result<Option<usize>> {
    let (wrapped, offset) = story_wrapper(xml, bindings)?;
    let mut reader = NsReader::from_reader(wrapped.as_slice());
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let is_word = matches!(
            namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()
        );
        match event {
            Event::Start(element) => {
                if depth == 2 && is_word && element.local_name().as_ref() == local_name {
                    return Ok(Some(before.saturating_sub(offset)));
                }
                depth += 1;
            }
            Event::Empty(element)
                if depth == 2 && is_word && element.local_name().as_ref() == local_name =>
            {
                return Ok(Some(before.saturating_sub(offset)));
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn moved_body_content(
    content: &BodyContent,
    kind: &str,
    id: i32,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    match content {
        BodyContent::Paragraph(paragraph) => moved_paragraph(paragraph, kind, id, metadata),
        BodyContent::Table(table) => moved_table(table, kind, id, metadata),
        BodyContent::ContentControl(_) | BodyContent::RawXml(_) => Err(Error::Other(
            "comparison cannot move a content-control or opaque body node".to_owned(),
        )),
    }
}

fn moved_paragraph(
    paragraph: &CT_P,
    kind: &str,
    id: i32,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    refuse_moved_markers(paragraph)?;
    let marker = metadata.ids.generated(IdAllocator::marker_with_id(
        kind,
        metadata.author,
        metadata.timestamp,
        id,
    ))?;
    let source = PropertySide::of(paragraph, metadata)?;
    let properties = marked_paragraph_properties_xml(
        &source
            .xml
            .unwrap_or(metadata.ids.generated("<w:pPr/>".to_owned())?),
        &marker,
        &metadata.ids,
    )?;
    let (mut output, closing) = paragraph_shell(metadata.paragraph_source(paragraph))?;
    output.push_str(&properties);
    let (children, published) = paragraph_children_xml(paragraph, false, false, metadata)?;
    output.push_str(&crate::revision::tracked_paragraph_children(
        &children,
        false,
        false,
        &published,
        &mut |content| {
            metadata.ids.generated(IdAllocator::revision_with_id(
                kind,
                metadata.author,
                metadata.timestamp,
                content,
                id,
            ))
        },
    )?);
    output.push_str(&closing);
    metadata.ids.generated(output)
}

/// Both ends of a move would hold the paragraph's bookmarks and comment
/// ranges, and each may appear once.
fn refuse_moved_markers(paragraph: &CT_P) -> Result<()> {
    if paragraph.bookmark_markers.is_empty() && paragraph.comment_ranges.is_empty() {
        return Ok(());
    }
    Err(Error::Other(
        "comparison cannot move a paragraph that holds a bookmark or comment range".to_owned(),
    ))
}

fn moved_table(table: &CT_Tbl, kind: &str, id: i32, metadata: &Metadata<'_>) -> Result<String> {
    let source = metadata
        .table_source(table)
        .map(str::to_owned)
        .map_or_else(|| table_xml(table), Ok)?;
    let replacements = table
        .rows
        .iter()
        .map(|row| moved_row(row, kind, id, metadata))
        .collect::<Result<Vec<_>>>()?;
    metadata
        .ids
        .generated(replace_direct_word_elements(&source, "tr", &replacements)?)
}

fn moved_row(row: &CT_Row, kind: &str, id: i32, metadata: &Metadata<'_>) -> Result<String> {
    let marker = metadata.ids.generated(IdAllocator::marker_with_id(
        kind,
        metadata.author,
        metadata.timestamp,
        id,
    ))?;
    let mut xml = metadata
        .row_source(row)
        .map(str::to_owned)
        .map_or_else(|| row_xml(row), Ok)?;
    let properties = direct_word_element_spans(&xml, "trPr")?;
    if let Some(span) = properties.first() {
        let updated = append_word_child(&xml[span.clone()], "trPr", &marker)?;
        xml.replace_range(span.clone(), &updated);
        metadata.ids.generated(xml)
    } else {
        metadata.ids.generated(insert_first_child(
            &xml,
            &metadata
                .ids
                .generated(format!("<w:trPr>{marker}</w:trPr>"))?,
        )?)
    }
}
