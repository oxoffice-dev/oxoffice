//! Comparing a paragraph by words or characters.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GranularAction {
    Preserve,
    Equal,
    Replace,
    Delete,
    Insert,
    Drop,
}

pub(super) fn compare_granular_paragraph(
    original: &CT_P,
    edited: &CT_P,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let actual_source = original_source
        .map(str::to_owned)
        .map_or_else(|| paragraph_xml(original), Ok)?;
    let actual_spans = modeled_paragraph_run_spans(original, &actual_source)?;
    // One read of the paragraph answers the scope of every run, on first use.
    // Reading it again for each written piece is quadratic in its runs.
    let run_parents = ParentScopes::new(
        &actual_source,
        actual_spans
            .iter()
            .map(|span| span.start)
            .chain(element_opening(&actual_source).ok().map(|(end, ..)| end))
            .collect(),
    );
    let parent_for = |owner: usize| {
        let at = actual_spans
            .get(owner)
            .map_or(element_opening(&actual_source)?.0, |span| span.start);
        run_parents.at(at)
    };
    let boundary_error = || {
        Error::Other(format!(
            "comparison cannot revise paragraph boundary structures at {location}"
        ))
    };
    if paragraph_boundaries_differ(original, edited, metadata.options) {
        return Err(boundary_error());
    }
    let original_boundaries = shell_run_boundaries(original, metadata.options);
    let edited_boundaries = shell_run_boundaries(edited, metadata.options);

    let original_run_signatures = original
        .runs
        .iter()
        .map(attributed_run_signature)
        .collect::<Vec<_>>();
    let edited_run_signatures = edited
        .runs
        .iter()
        .map(attributed_run_signature)
        .collect::<Vec<_>>();
    // Runs that all match stay whole even when a control differs, so a
    // control's metadata or content never moves run-indexed markers.
    if original_run_signatures == edited_run_signatures
        && original_boundaries == edited_boundaries
        && !SourceMetadata::paragraph_contains_fields(original, metadata.options)?
    {
        let properties =
            paragraph_properties_xml(original, edited, location, metadata, diagnostics)?;
        let spans = original_source
            .map(|source| modeled_paragraph_run_spans(original, source))
            .transpose()?
            .unwrap_or_default();
        if original_source.is_some() && spans.len() != original.runs.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate granular run owners at {location}"
            )));
        }
        let replacements = original
            .runs
            .iter()
            .zip(&edited.runs)
            .enumerate()
            .map(|(index, (left, right))| {
                if left == right
                    && let Some(source) = original_source
                {
                    Ok(source[spans[index].clone()].to_owned())
                } else {
                    compared_run_xml(
                        left,
                        right,
                        None,
                        None,
                        &format!("{location}/run[{index}]"),
                        &parent_for(index)?,
                        metadata,
                        diagnostics,
                    )
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let output = replace_paragraph_properties_and_runs(
            original,
            original_source,
            &properties,
            &replacements,
        )?;
        if original.content_controls == edited.content_controls {
            return Ok(output);
        }
        return compare_granular_controls(
            original,
            edited,
            &output,
            location,
            metadata,
            diagnostics,
            leaf_mark,
        );
    }

    let projection_pairs = metadata.leaf_capture.then(|| {
        projection_leaf_pairs(
            [&original.runs[..], &edited.runs[..]],
            &original_boundaries
                .iter()
                .copied()
                .zip(edited_boundaries.iter().copied())
                .collect::<Vec<_>>(),
        )
    });
    let projection_sources = [&original.runs[..], &edited.runs[..]]
        .into_iter()
        .enumerate()
        .map(|(side, runs)| {
            runs.iter()
                .enumerate()
                .map(|(owner, run)| {
                    let _ = (side, owner);
                    metadata.run_source(run).cloned()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let projection_sources = [projection_sources[0].clone(), projection_sources[1].clone()];
    let mut projection_ledgers = if let Some(pairs) = &projection_pairs {
        projection_action_ledgers(
            pairs,
            [&original.runs[..], &edited.runs[..]],
            &projection_sources,
        )?
    } else {
        Vec::new()
    };
    let mut coalesced_words =
        !metadata.leaf_capture && metadata.options.granularity == ComparisonGranularity::Word;
    let unit_options = ComparisonOptions {
        granularity: if coalesced_words {
            ComparisonGranularity::Character
        } else {
            metadata.options.granularity
        },
        ..metadata.options.clone()
    };
    let mut original_units = attributed_run_units(&original.runs, &unit_options);
    let mut edited_units = attributed_run_units(&edited.runs, &unit_options);
    validate_raw_attribution(&original.runs, &original_units)?;
    validate_raw_attribution(&edited.runs, &edited_units)?;
    let mut original_signatures = original_units
        .iter()
        .map(|unit| {
            if coalesced_words
                && metadata.options.ignore_formatting
                && unit_plain_text(unit).is_some()
            {
                run_signature_with_options(&unit.run, metadata.options)
            } else {
                attributed_unit_signature(unit)
            }
        })
        .collect::<Vec<_>>();
    let mut edited_signatures = edited_units
        .iter()
        .map(|unit| {
            if coalesced_words
                && metadata.options.ignore_formatting
                && unit_plain_text(unit).is_some()
            {
                run_signature_with_options(&unit.run, metadata.options)
            } else {
                attributed_unit_signature(unit)
            }
        })
        .collect::<Vec<_>>();

    let properties = paragraph_properties_xml(original, edited, location, metadata, diagnostics)?;
    let Some(mut cuts) = shell_unit_cuts(
        &original_units,
        &edited_units,
        original_boundaries
            .iter()
            .copied()
            .zip(edited_boundaries.iter().copied()),
    ) else {
        return Err(boundary_error());
    };
    // Actual original formatting owner for a paired logical word. This is
    // independent of every edited scalar's physical content/source owner.
    let mut word_format_owners = vec![None; edited_units.len()];
    let (aligned, segments) = if let Some(pairs) = &projection_pairs {
        projection_unit_alignment(
            pairs,
            [&original_units, &edited_units],
            [&original_signatures, &edited_signatures],
        )?
    } else {
        if !metadata.leaf_capture && metadata.options.granularity == ComparisonGranularity::Word {
            if let Some(alignment) = align_words_between_shells(
                &original_units,
                &edited_units,
                &original_signatures,
                &edited_signatures,
                &cuts,
                metadata.options.ignore_formatting,
                &mut word_format_owners,
            ) {
                alignment
            } else {
                coalesced_words = false;
                word_format_owners.clear();
                // A barrier paired with text uses the existing whole Word unit,
                // never a newly invented scalar-to-object correspondence.
                original_units = attributed_run_units(&original.runs, metadata.options);
                edited_units = attributed_run_units(&edited.runs, metadata.options);
                validate_raw_attribution(&original.runs, &original_units)?;
                validate_raw_attribution(&edited.runs, &edited_units)?;
                original_signatures = original_units
                    .iter()
                    .map(attributed_unit_signature)
                    .collect();
                edited_signatures = edited_units.iter().map(attributed_unit_signature).collect();
                cuts = shell_unit_cuts(
                    &original_units,
                    &edited_units,
                    original_boundaries
                        .iter()
                        .copied()
                        .zip(edited_boundaries.iter().copied()),
                )
                .ok_or_else(boundary_error)?;
                align_between_shells(&original_signatures, &edited_signatures, &cuts)
            }
        } else {
            align_between_shells(&original_signatures, &edited_signatures, &cuts)
        }
    };
    // The original run each shell segment starts at, before which the
    // redline copies the original bytes, shell tags included.
    let segment_runs = if let Some(pairs) = &projection_pairs {
        pairs
            .iter()
            .enumerate()
            .map(|(at, pair)| {
                pair[0]
                    .map(|(owner, _)| owner)
                    .or_else(|| {
                        pairs[at..]
                            .iter()
                            .find_map(|next| next[0].map(|(owner, _)| owner))
                    })
                    .unwrap_or(original.runs.len())
            })
            .collect::<Vec<_>>()
    } else {
        std::iter::once(0)
            .chain(cuts.iter().map(|&(cut, _)| {
                original_units
                    .get(cut)
                    .map_or(original.runs.len(), |unit| unit.owner)
            }))
            .collect::<Vec<_>>()
    };
    if !metadata.options.ignore_fields {
        validate_field_alignment(
            &aligned,
            &original_units
                .iter()
                .map(|unit| unit.run.clone())
                .collect::<Vec<_>>(),
            &edited_units
                .iter()
                .map(|unit| unit.run.clone())
                .collect::<Vec<_>>(),
            &original_signatures,
            &edited_signatures,
            location,
        )?;
    }
    let (aligned, segments) = grouped_text_alignment(
        &aligned,
        &segments,
        &original_units,
        &edited_units,
        &original_signatures,
        &edited_signatures,
        metadata.options,
    );
    let mut grouped = coalesced_granular_alignment(
        &aligned,
        &original_units,
        &edited_units,
        &original_signatures,
        &edited_signatures,
    );
    if coalesced_words && metadata.options.ignore_formatting {
        grouped = grouped
            .into_iter()
            .flat_map(|(action, members)| {
                let mut pieces = Vec::new();
                let mut start = members.start;
                let owner = |at: usize| aligned[at].1.and_then(|index| word_format_owners[index]);
                for at in members.start + 1..members.end {
                    if owner(at) != owner(start) {
                        pieces.push((action, start..at));
                        start = at;
                    }
                }
                pieces.push((action, start..members.end));
                pieces
            })
            .collect();
    }
    if let Some(pairs) = &projection_pairs {
        retain_projection_separators(
            &mut grouped,
            &aligned,
            &segments,
            pairs,
            [&original_units, &edited_units],
            &projection_sources,
            metadata.options,
        )?;
    }
    validate_fragment_partition(&aligned, &grouped)?;
    let mut grouped_alignment = Vec::with_capacity(grouped.len());
    let mut grouped_segment_runs = Vec::with_capacity(grouped.len());
    let mut replacements = Vec::with_capacity(grouped.len());
    let mut fragment_publications = Vec::with_capacity(grouped.len());
    let mut word_revisions = Vec::<
        Option<(
            GranularAction,
            usize,
            Option<oxoffice_docx_oxml::properties::CT_RPr>,
            i32,
        )>,
    >::with_capacity(grouped.len());
    for (action, members) in grouped {
        if metadata.leaf_capture {
            record_projection_actions(
                &mut projection_ledgers,
                action,
                members.clone(),
                &aligned,
                [&original_units, &edited_units],
                &projection_sources,
            )?;
        }
        let members_start = members.start;
        let first = aligned[members.start];
        grouped_alignment.push(first);
        grouped_segment_runs.push(segment_runs[segments[members.start]]);
        let left_indices = members
            .clone()
            .filter_map(|index| aligned[index].0)
            .collect::<Vec<_>>();
        let right_indices = members
            .filter_map(|index| aligned[index].1)
            .collect::<Vec<_>>();
        let word_key = if coalesced_words
            && matches!(action, GranularAction::Delete | GranularAction::Insert)
        {
            let (indices, units) = if action == GranularAction::Delete {
                (&left_indices, &original_units)
            } else {
                (&right_indices, &edited_units)
            };
            if indices.iter().all(|index| {
                unit_plain_text(&units[*index]).is_some_and(|text| {
                    !text.is_empty()
                        && text
                            .chars()
                            .all(|character| character.is_alphanumeric() || character == '_')
                })
            }) {
                Some((
                    action,
                    segments[members_start],
                    if action == GranularAction::Insert && metadata.options.ignore_formatting {
                        word_format_owners[indices[0]]
                            .map(|owner| original.runs[owner].properties.clone())
                            .unwrap_or_else(|| units[indices[0]].run.properties.clone())
                    } else {
                        units[indices[0]].run.properties.clone()
                    },
                ))
            } else {
                None
            }
        } else {
            None
        };
        let mut word_revision_id = None;
        let mut left_raw = Vec::new();
        let mut left_typed = Vec::new();
        let mut right_raw = Vec::new();
        let mut right_typed = Vec::new();
        let left = merge_unit_runs(
            &original.runs,
            &original_units,
            &left_indices,
            &mut left_raw,
            &mut left_typed,
        )?;
        let right = merge_unit_runs(
            &edited.runs,
            &edited_units,
            &right_indices,
            &mut right_raw,
            &mut right_typed,
        )?;
        let left_source = left_indices
            .first()
            .and_then(|index| metadata.run_source(&original.runs[original_units[*index].owner]))
            .cloned();
        let right_source = right_indices
            .first()
            .and_then(|index| metadata.run_source(&edited.runs[edited_units[*index].owner]))
            .cloned();
        let destination_owner = left_indices
            .first()
            .map(|index| original_units[*index].owner)
            .unwrap_or(segment_runs[segments[members_start]]);
        let destination_parent = parent_for(destination_owner)?;
        let revision_parent = metadata.ids.revision_parent_scope(&destination_parent);
        let policy_shell = projection_pairs
            .as_ref()
            .and_then(|pairs| {
                right_indices.first().and_then(|index| {
                    let unit = &edited_units[*index];
                    if !matches!(
                        unit.run.content.first(),
                        Some(RunContent::Text(_) | RunContent::DeletedText(_))
                    ) {
                        return None;
                    }
                    let ordinal = unit.typed_sources.first().and_then(|slot| *slot)?.ordinal;
                    pairs.iter().find_map(|pair| {
                        (pair[1] == Some((unit.owner, ordinal)))
                            .then_some(pair[0])
                            .flatten()
                    })
                })
            })
            .map(|(owner, ordinal)| {
                let run = &original.runs[owner];
                let source = metadata.run_source(run).cloned().ok_or_else(|| {
                    Error::Other("paired projection shell has no actual source".into())
                })?;
                if source
                    .typed_slots
                    .get(ordinal)
                    .and_then(Option::as_ref)
                    .is_none()
                {
                    return Err(Error::Other(
                        "paired projection shell leaf has no physical slot".into(),
                    ));
                }
                Ok((run, source))
            })
            .transpose()?;
        let policy_shell =
            if policy_shell.is_none() && coalesced_words && metadata.options.ignore_formatting {
                right_indices
                    .first()
                    .and_then(|index| word_format_owners[*index])
                    .map(|owner| -> Result<_> {
                        let run = &original.runs[owner];
                        let source = metadata.run_source(run).cloned().ok_or_else(|| {
                            Error::Other("logical word formatting has no actual source".into())
                        })?;
                        Ok((run, source))
                    })
                    .transpose()?
            } else {
                policy_shell
            };
        let mut field_bindings = Vec::new();
        for (indices, units, originals, cloned, selected) in [
            (
                &left_indices,
                &original_units,
                &original.runs,
                left.as_ref(),
                &left_typed,
            ),
            (
                &right_indices,
                &edited_units,
                &edited.runs,
                right.as_ref(),
                &right_typed,
            ),
        ] {
            if let Some(cloned) = cloned
                && run_is_field(cloned)
            {
                let owner = units[indices[0]].owner;
                match metadata.bind_selected_run_fields(&originals[owner], cloned, selected) {
                    Ok(bindings) => field_bindings.extend(bindings),
                    Err(error) => {
                        metadata.unbind_cloned_control_runs(field_bindings)?;
                        return Err(error);
                    }
                }
            }
        }
        let replacement = (|| -> Result<String> {
            Ok(match action {
                GranularAction::Preserve => publish_run_source(
                    left.as_ref().unwrap(),
                    left.as_ref().unwrap(),
                    paragraph_owned_run_xml(left.as_ref().unwrap())?,
                    left_source.as_ref(),
                    None,
                    Some((&left_raw, &left_typed)),
                    &destination_parent,
                    metadata,
                )?,
                GranularAction::Equal => compared_run_xml(
                    left.as_ref().unwrap(),
                    right.as_ref().unwrap(),
                    left_source.as_ref(),
                    Some((&left_raw, &left_typed)),
                    &format!("{location}/run-unit[{}]", right_indices[0]),
                    &destination_parent,
                    metadata,
                    diagnostics,
                )?,
                GranularAction::Replace => paired_run_replacement(
                    [left.as_ref().unwrap(), right.as_ref().unwrap()],
                    [left_source.as_ref(), right_source.as_ref()],
                    [
                        Some((&left_raw, &left_typed)),
                        Some((&right_raw, &right_typed)),
                    ],
                    location,
                    &destination_parent,
                    metadata,
                    diagnostics,
                )?,
                GranularAction::Delete => {
                    let payload = publish_run_source(
                        left.as_ref().unwrap(),
                        left.as_ref().unwrap(),
                        deleted_run_xml(left.as_ref().unwrap())?,
                        left_source.as_ref(),
                        None,
                        Some((&left_raw, &left_typed)),
                        &revision_parent,
                        metadata,
                    )?;
                    if let Some((kind, segment, properties)) = &word_key {
                        let id = match word_revisions.last().and_then(Option::as_ref) {
                            Some((previous_kind, previous_segment, previous_properties, id))
                                if previous_kind == kind
                                    && previous_segment == segment
                                    && previous_properties == properties =>
                            {
                                *id
                            }
                            _ => metadata.ids.allocate()?,
                        };
                        word_revision_id = Some(id);
                        payload
                    } else {
                        metadata.ids.revision(
                            "del",
                            metadata.author,
                            metadata.timestamp,
                            &payload,
                        )?
                    }
                }
                GranularAction::Insert => {
                    let mut content = right.as_ref().unwrap().clone();
                    let (shell, shell_source) = if let Some((shell, source)) = &policy_shell {
                        let before = usize::from(content.properties.is_some());
                        content.properties = shell.properties.clone();
                        let after = usize::from(content.properties.is_some());
                        for encoded in &mut content.extra_xml_positions {
                            if !CT_R::raw_child_is_root_attributes(*encoded) {
                                let at = CT_R::raw_child_position(*encoded).saturating_sub(before)
                                    + after;
                                CT_R::set_raw_child_position(encoded, at);
                            }
                        }
                        (*shell, Some(source))
                    } else {
                        (right.as_ref().unwrap(), right_source.as_ref())
                    };
                    let payload = publish_run_source(
                        shell,
                        &content,
                        paragraph_owned_run_xml(&content)?,
                        shell_source,
                        right_source.as_ref(),
                        Some((&right_raw, &right_typed)),
                        &revision_parent,
                        metadata,
                    )?;
                    if let Some((kind, segment, properties)) = &word_key {
                        let id = match word_revisions.last().and_then(Option::as_ref) {
                            Some((previous_kind, previous_segment, previous_properties, id))
                                if previous_kind == kind
                                    && previous_segment == segment
                                    && previous_properties == properties =>
                            {
                                *id
                            }
                            _ => metadata.ids.allocate()?,
                        };
                        word_revision_id = Some(id);
                        payload
                    } else {
                        metadata.ids.revision(
                            "ins",
                            metadata.author,
                            metadata.timestamp,
                            &payload,
                        )?
                    }
                }
                GranularAction::Drop => String::new(),
            })
        })();
        metadata.unbind_cloned_control_runs(field_bindings)?;
        replacements.push(replacement?);
        word_revisions.push(
            word_key
                .zip(word_revision_id)
                .map(|((kind, segment, properties), id)| (kind, segment, properties, id)),
        );
        let mut leaves = Vec::new();
        if metadata.leaf_capture {
            let selected = match action {
                GranularAction::Replace | GranularAction::Insert => right
                    .as_ref()
                    .zip(right_source.as_ref())
                    .map(|(run, source)| {
                        (
                            run,
                            source,
                            &right_raw[..],
                            &right_typed[..],
                            matches!(action, GranularAction::Replace | GranularAction::Insert),
                        )
                    }),
                GranularAction::Equal | GranularAction::Preserve => left
                    .as_ref()
                    .zip(left_source.as_ref())
                    .map(|(run, source)| (run, source, &left_raw[..], &left_typed[..], false)),
                _ => None,
            };
            if let Some((run, source, raw, typed, inserted)) = selected {
                leaves = emitted_fragment_leaves(
                    replacements.last().unwrap(),
                    run,
                    source,
                    raw,
                    typed,
                    inserted,
                    metadata.options.ignore_whitespace,
                    &destination_parent,
                )?;
            }
        }
        fragment_publications.push(leaves);
    }
    // Payloads retain their separate physical owners and source publication.
    // Only the enclosing generated revision is shared by adjacent pieces of
    // one ordinary word, without crossing a shell, formatting or token boundary.
    let mut start = 0;
    while start < replacements.len() {
        let Some((action, _, _, id)) = &word_revisions[start] else {
            start += 1;
            continue;
        };
        let id = *id;
        let kind = if *action == GranularAction::Delete {
            "del"
        } else {
            "ins"
        };
        let mut end = start + 1;
        while end < word_revisions.len()
            && word_revisions[end]
                .as_ref()
                .is_some_and(|(_, _, _, other)| *other == id)
        {
            end += 1;
        }
        let mut payload = std::mem::take(&mut replacements[start]);
        for replacement in &mut replacements[start + 1..end] {
            payload.push_str(&std::mem::take(replacement));
        }
        replacements[start] = metadata.ids.generated(IdAllocator::revision_with_id(
            kind,
            metadata.author,
            metadata.timestamp,
            &payload,
            id,
        ))?;
        start = end;
    }
    if metadata.leaf_capture {
        attach_projection_actions(&mut fragment_publications, &projection_ledgers)?;
    }
    interleave_granular_paragraph(
        original,
        edited,
        original_source,
        &original_units,
        &edited_units,
        &grouped_alignment,
        &grouped_segment_runs,
        &replacements,
        &fragment_publications,
        &properties,
        location,
        metadata,
        diagnostics,
    )
}

/// The hyperlink owners without their place in the paragraph, which
/// [`shell_unit_cuts`] follows through the unit alignment.
pub(super) fn hyperlink_shells(paragraph: &CT_P, options: &ComparisonOptions) -> Vec<String> {
    paragraph
        .hyperlinks
        .iter()
        .map(|link| {
            format!(
                "{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}",
                link.rel_id,
                link.anchor,
                link.tooltip,
                link.doc_location,
                link.extra_attributes,
                hyperlink_raw_boundaries(paragraph, link, options),
                link.preserved_raw_before,
            )
        })
        .collect()
}

/// The run boundaries of the shells that the attributed path keeps from the
/// original: where each hyperlink starts and ends and where each inline
/// control or opaque child sits.
pub(super) fn shell_run_boundaries(paragraph: &CT_P, options: &ComparisonOptions) -> Vec<usize> {
    paragraph
        .hyperlinks
        .iter()
        .flat_map(|link| [link.run_start, link.run_end])
        .chain(paragraph.content_controls.iter().map(|(at, ..)| *at))
        .chain(paragraph.extra_xml.iter().filter_map(|(at, raw)| {
            (options.granularity != ComparisonGranularity::Run
                && !CT_P::raw_is_root_attributes(*at, raw))
            .then_some(*at)
        }))
        .chain(
            paragraph
                .bookmark_markers
                .iter()
                .map(|marker| marker.run_index()),
        )
        .chain(
            paragraph
                .comment_ranges
                .iter()
                .filter(|_| !options.ignore_comments)
                .map(|marker| match marker {
                    CommentRangeMarker::Start { run_index, .. }
                    | CommentRangeMarker::End { run_index, .. } => *run_index,
                }),
        )
        .collect()
}

/// The `(original, edited)` unit indices where the shell boundaries fall, in
/// document order, for `(original, edited)` run boundaries.
///
/// The units between two consecutive boundaries form a segment, and the
/// redline copies the original bytes before the first run of a segment,
/// shell tags included, before anything of that segment, so each boundary
/// moves with the words inserted or deleted around it. `None` when the
/// shells are not in the same order on both sides, or when the edited side
/// writes a unit between two boundaries that fall between the same two
/// original runs, because the original bytes there are not split. Ignorable
/// and empty units are left out, as in the accept and reject postconditions.
pub(super) fn shell_unit_cuts(
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    boundaries: impl IntoIterator<Item = (usize, usize)>,
) -> Option<Vec<(usize, usize)>> {
    let mut boundaries = boundaries.into_iter().collect::<Vec<_>>();
    boundaries.sort_unstable();
    if boundaries.windows(2).any(|pair| pair[0].1 > pair[1].1) {
        return None;
    }
    let cuts = boundaries
        .into_iter()
        .map(|(original, edited)| {
            (
                original_units.partition_point(|unit| unit.owner < original),
                edited_units.partition_point(|unit| unit.owner < edited),
            )
        })
        .collect::<Vec<_>>();
    let mut start = (0, 0);
    cuts.iter()
        .all(|&end| {
            let writable = start.0 < end.0
                || edited_units[start.1..end.1]
                    .iter()
                    .all(|unit| unit_is_ignorable(unit) || unit_is_empty(unit));
            start = end;
            writable
        })
        .then_some(cuts)
}

/// The unit alignment made segment by segment between the shell boundaries
/// at `cuts`, so no unit is matched across a hyperlink or an inline control,
/// and the segment of each pair.
#[allow(clippy::type_complexity)]
pub(super) fn align_between_shells(
    original: &[String],
    edited: &[String],
    cuts: &[(usize, usize)],
) -> (Vec<(Option<usize>, Option<usize>)>, Vec<usize>) {
    let mut aligned = Vec::with_capacity(original.len().max(edited.len()));
    let mut segments = Vec::with_capacity(aligned.capacity());
    let mut start = (0, 0);
    for (segment, &end) in cuts
        .iter()
        .chain(std::iter::once(&(original.len(), edited.len())))
        .enumerate()
    {
        for (left, right) in align(&original[start.0..end.0], &edited[start.1..end.1]) {
            aligned.push((
                left.map(|index| index + start.0),
                right.map(|index| index + start.1),
            ));
            segments.push(segment);
        }
        start = end;
    }
    (aligned, segments)
}

/// Align logical words across plain physical runs, retaining each scalar's real
/// owner for source publication. Shell cuts and nontext units stay barriers.
fn align_words_between_shells(
    original: &[AttributedRunUnit],
    edited: &[AttributedRunUnit],
    original_signatures: &[String],
    edited_signatures: &[String],
    cuts: &[(usize, usize)],
    ignore_formatting: bool,
    format_owners: &mut [Option<usize>],
) -> Option<(Alignment, Vec<usize>)> {
    fn groups(
        units: &[AttributedRunUnit],
        signatures: &[String],
        span: Range<usize>,
    ) -> Vec<(String, Range<usize>)> {
        let class = |unit: &AttributedRunUnit| {
            let text = unit_plain_text(unit)?;
            let character = text.chars().next()?;
            Some(if character.is_alphanumeric() || character == '_' {
                0
            } else if character.is_whitespace() {
                1
            } else {
                2
            })
        };
        let mut result = Vec::new();
        let mut at = span.start;
        while at < span.end {
            let mut end = at + 1;
            if let Some(kind) = class(&units[at]) {
                while end < span.end
                    && class(&units[end]) == Some(kind)
                    && run_formatting(&units[end].run) == run_formatting(&units[at].run)
                {
                    end += 1;
                }
            }
            let signature = format!("{:?}", &signatures[at..end]);
            result.push((signature, at..end));
            at = end;
        }
        result
    }
    let mut result = Vec::new();
    let mut segments = Vec::new();
    let mut start = (0, 0);
    for (segment, &end) in cuts
        .iter()
        .chain(std::iter::once(&(original.len(), edited.len())))
        .enumerate()
    {
        let left = groups(original, original_signatures, start.0..end.0);
        let right = groups(edited, edited_signatures, start.1..end.1);
        let left_signatures = left
            .iter()
            .map(|(signature, _)| signature.clone())
            .collect::<Vec<_>>();
        let right_signatures = right
            .iter()
            .map(|(signature, _)| signature.clone())
            .collect::<Vec<_>>();
        for (a, b) in align(&left_signatures, &right_signatures) {
            if !ignore_formatting
                && let (Some(a), Some(b)) = (a, b)
                && run_formatting(&original[left[a].1.start].run)
                    != run_formatting(&edited[right[b].1.start].run)
            {
                // Formatting transitions retain the existing Word replacement
                // owner and its ignore-formatting left bias.
                return None;
            }

            if ignore_formatting
                && let (Some(a), Some(b)) = (a, b)
                && unit_plain_text(&original[left[a].1.start]).is_some()
                && unit_plain_text(&edited[right[b].1.start]).is_some()
            {
                for index in right[b].1.clone() {
                    format_owners[index] = Some(original[left[a].1.start].owner);
                }
            }
            if let (Some(a), Some(b)) = (a, b)
                && left[a].0 == right[b].0
            {
                result.extend(
                    left[a]
                        .1
                        .clone()
                        .zip(right[b].1.clone())
                        .map(|(a, b)| (Some(a), Some(b))),
                );
            } else if let (Some(a), Some(b)) = (a, b)
                && (unit_plain_text(&original[left[a].1.start]).is_none()
                    || unit_plain_text(&edited[right[b].1.start]).is_none())
            {
                if unit_plain_text(&original[left[a].1.start]).is_some()
                    || unit_plain_text(&edited[right[b].1.start]).is_some()
                {
                    return None;
                }
                // Both groups are one actual atomic unit. Retain Replace so
                // the existing Field/textbox/token publisher owns its change.
                result.push((Some(left[a].1.start), Some(right[b].1.start)));
            } else {
                if let Some(a) = a {
                    result.extend(left[a].1.clone().map(|a| (Some(a), None)));
                }
                if let Some(b) = b {
                    result.extend(right[b].1.clone().map(|b| (None, Some(b))));
                }
            }
            segments.resize(result.len(), segment);
        }
        start = end;
    }
    Some((result, segments))
}

pub(super) fn hyperlink_raw_boundaries(
    paragraph: &CT_P,
    link: &oxoffice_docx_oxml::text::HyperlinkSpan,
    options: &ComparisonOptions,
) -> Vec<(usize, usize, Vec<u8>)> {
    let logical_start = policy_run_boundary(paragraph, link.run_start, options);
    link.extra_xml
        .iter()
        .map(|(boundary, revisions_before, raw)| {
            let absolute = link.run_start.saturating_add(*boundary);
            (
                policy_run_boundary(paragraph, absolute, options).saturating_sub(logical_start),
                *revisions_before,
                raw.clone(),
            )
        })
        .collect()
}

pub(super) fn policy_run_boundary(
    paragraph: &CT_P,
    run_boundary: usize,
    options: &ComparisonOptions,
) -> usize {
    attributed_run_units(
        &paragraph.runs[..run_boundary.min(paragraph.runs.len())],
        options,
    )
    .iter()
    .filter(|unit| !unit_is_ignorable(unit) && !unit_is_empty(unit))
    .count()
}

pub(super) fn unit_is_ignorable(unit: &AttributedRunUnit) -> bool {
    unit.ignored && semantic_run_raw(&unit.run).is_empty() && unit.run.alt_drawings.is_empty()
}

pub(super) fn unit_is_empty(unit: &AttributedRunUnit) -> bool {
    unit.run.content.is_empty()
        && semantic_run_raw(&unit.run).is_empty()
        && unit.run.alt_drawings.is_empty()
        && run_formatting(&unit.run).is_none()
}

/// The properties of a run, none when empty: LibreOffice writes
/// `<w:rPr></w:rPr>`, which the model writes back as nothing, and which
/// carries no format.
pub(super) fn run_formatting(run: &CT_R) -> Option<&oxoffice_docx_oxml::properties::CT_RPr> {
    run.properties
        .as_ref()
        .filter(|properties| **properties != oxoffice_docx_oxml::properties::CT_RPr::default())
}

pub(super) fn granular_action(
    pair: (Option<usize>, Option<usize>),
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    original_signatures: &[String],
    edited_signatures: &[String],
) -> GranularAction {
    match pair {
        (Some(i), Some(j))
            if original_units[i].ignored
                && edited_units[j].ignored
                && original_signatures[i] == edited_signatures[j] =>
        {
            GranularAction::Preserve
        }
        (Some(i), Some(j)) if original_signatures[i] == edited_signatures[j] => {
            GranularAction::Equal
        }
        (Some(_), Some(_)) => GranularAction::Replace,
        (Some(i), None) if unit_is_ignorable(&original_units[i]) => GranularAction::Preserve,
        (None, Some(j)) if unit_is_ignorable(&edited_units[j]) => GranularAction::Drop,
        (Some(_), None) => GranularAction::Delete,
        (None, Some(_)) => GranularAction::Insert,
        (None, None) => unreachable!(),
    }
}

pub(super) fn validate_fragment_partition(
    aligned: &[(Option<usize>, Option<usize>)],
    groups: &[(GranularAction, Range<usize>)],
) -> Result<()> {
    let mut end = 0;
    for (_, range) in groups {
        if range.start != end || range.start >= range.end || range.end > aligned.len() {
            return Err(Error::Other(
                "fragment groups do not partition their input roles".into(),
            ));
        }
        end = range.end;
    }
    if end != aligned.len() {
        return Err(Error::Other(
            "fragment groups omit aligned input roles".into(),
        ));
    }
    for side in [0, 1] {
        let mut next = 0;
        for &(left, right) in aligned {
            if let Some(index) = if side == 0 { left } else { right } {
                if index != next {
                    return Err(Error::Other(
                        "fragment input role repeats, reorders or loses a unit".into(),
                    ));
                }
                next += 1;
            }
        }
    }
    Ok(())
}
