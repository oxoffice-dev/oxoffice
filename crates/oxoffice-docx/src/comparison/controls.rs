//! Comparing content controls and their runs.

use super::*;

pub(super) fn compare_control(
    original: &CT_Sdt,
    edited: &CT_Sdt,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let original_xml = metadata
        .control_source(original)
        .map(str::to_owned)
        .map_or_else(|| control_xml(original), Ok)?;
    let result = compare_control_from_xml(
        original,
        edited,
        location,
        metadata,
        diagnostics,
        &original_xml,
    )?;
    metadata.ids.generated(result)
}

pub(super) fn compare_control_from_xml(
    original: &CT_Sdt,
    edited: &CT_Sdt,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
    original_xml: &str,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let output = (|| -> Result<String> {
        if control_property_signature(original) != control_property_signature(edited) {
            return Err(Error::Other(format!(
                "comparison cannot revise content-control properties at {location}"
            )));
        }
        control_metadata_diagnostics(original, edited, location, diagnostics)?;
        let original_content = modeled_control_content(original);
        let edited_content = modeled_control_content(edited);
        // Runs that only split the same text differently read as equal, as
        // they do in a paragraph, and keep the original source. Equal runs
        // keep the path below.
        let only_runs = |content: &[&SdtContent]| {
            !content.is_empty()
                && content
                    .iter()
                    .all(|child| matches!(child, SdtContent::Run(_)))
        };
        if !metadata.leaf_capture
            && only_runs(&original_content)
            && only_runs(&edited_content)
            && original_content != edited_content
        {
            let runs_of = |content: &[&SdtContent]| {
                let mut paragraph = CT_P::new();
                paragraph.runs = content
                    .iter()
                    .filter_map(|child| match child {
                        SdtContent::Run(run) => Some(run.clone()),
                        _ => None,
                    })
                    .collect();
                paragraph
            };
            if coalesced_paragraphs_equal(
                &runs_of(&original_content),
                &runs_of(&edited_content),
                metadata.options,
                metadata,
            ) {
                return Ok(original_xml.to_owned());
            }
        }
        let direct_run_or_raw =
            |content: &&SdtContent| matches!(content, SdtContent::Run(_) | SdtContent::RawXml(_));
        if uses_attributed_run_path(metadata.options)
            && original_content.iter().all(direct_run_or_raw)
            && edited_content.iter().all(direct_run_or_raw)
            && (!original_content.is_empty() || !edited_content.is_empty())
            && inline_control_raw_boundaries(&original_content, metadata.options)
                == inline_control_raw_boundaries(&edited_content, metadata.options)
        {
            let property_sources = [
                original_content
                    .iter()
                    .filter_map(|child| match child {
                        SdtContent::Run(run) => Some(metadata.run_source(run).cloned()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                edited_content
                    .iter()
                    .filter_map(|child| match child {
                        SdtContent::Run(run) => Some(metadata.run_source(run).cloned()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ];
            let original_runs = original_content
                .iter()
                .filter_map(|content| match content {
                    SdtContent::Run(run) => Some(run.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let edited_runs = edited_content
                .iter()
                .filter_map(|content| match content {
                    SdtContent::Run(run) => Some(run.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let content_spans = direct_word_element_spans(original_xml, "sdtContent")?;
            let content_span = content_spans.first().ok_or_else(|| {
                Error::Other(format!(
                    "comparison could not find inline-control content at {location}"
                ))
            })?;
            let scoped_content = source_child(original_xml, content_span.clone())?;
            let content_source = scoped_content.as_str();
            let inner = element_inner_range_any_prefix(content_source, "sdtContent")?;
            // Raw children remain physical shell boundaries after the runs are
            // extracted. Logical boundary equality above does not preserve them
            // in the run-only alignment by itself.
            let raw_runs = |content: &[&SdtContent]| {
                let mut runs = 0;
                content
                    .iter()
                    .filter_map(|child| match child {
                        SdtContent::Run(_) => {
                            runs += 1;
                            None
                        }
                        SdtContent::RawXml(_) => Some(runs),
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>()
            };
            let boundaries = raw_runs(&original_content)
                .into_iter()
                .zip(raw_runs(&edited_content))
                .collect::<Vec<_>>();
            let modeled_spans = modeled_control_source_spans(&original_content, content_source)?;
            let run_spans = original_content
                .iter()
                .zip(modeled_spans)
                .filter_map(|(child, span)| {
                    matches!(child, SdtContent::Run(_))
                        .then_some(span.start - inner.start..span.end - inner.start)
                })
                .collect::<Vec<_>>();
            let child_mark = metadata.leaf_publications.len();
            let mut field_bindings =
                metadata.bind_cloned_control_runs(&original_content, &original_runs)?;
            field_bindings
                .extend(metadata.bind_cloned_control_runs(&edited_content, &edited_runs)?);
            let compared_result = compare_control_runs_with_options(
                [&original_runs, &edited_runs],
                Some((
                    &source_fragment(
                        &format!(
                            "<w:inlineControl>{}</w:inlineControl>",
                            &content_source[inner]
                        ),
                        &root_namespace_bindings(content_source, "sdtContent", &[])?,
                    )?,
                    &run_spans,
                )),
                Some(&property_sources),
                &boundaries,
                location,
                &root_namespace_bindings(content_source, "sdtContent", &[])?,
                metadata,
                diagnostics,
            );
            metadata.unbind_cloned_control_runs(field_bindings)?;
            let compared = compared_result?;
            // The leaves sit where the written content starts, which moves
            // when a self-closing content gets its end tag.
            let (output, inner_start) =
                replace_element_inner(original_xml, "w:sdtContent", &compared)?;
            shift_leaf_publications(
                &mut metadata.leaf_publications[child_mark..],
                inner_start as isize,
            )?;
            return Ok(output);
        }
        // The last paragraph mark of a block control's content cannot be
        // taken out: a control holding nothing in one version cannot be
        // marked to hold the other's paragraphs. Runs of an inline one can.
        let blocks = |content: &[&SdtContent]| {
            content
                .iter()
                .any(|item| matches!(item, SdtContent::Paragraph(_) | SdtContent::Table(_)))
        };
        if original_content.is_empty() != edited_content.is_empty()
            && (blocks(&original_content) || blocks(&edited_content))
        {
            return Err(Error::Other(format!(
                "comparison cannot give an empty block content control content, nor take all of it, at {location} (limitation docx-compare-empty-container)"
            )));
        }
        let whitespace_slots = control_whitespace_slots(original)?;
        let original_signatures = original_content
            .iter()
            .map(|content| control_content_signature_with_options(content, metadata.options))
            .collect::<Vec<_>>();
        let edited_signatures = edited_content
            .iter()
            .map(|content| control_content_signature_with_options(content, metadata.options))
            .collect::<Vec<_>>();
        fn paragraphs<'a>(content: &[&'a SdtContent]) -> Vec<Option<&'a CT_P>> {
            content
                .iter()
                .map(|content| match content {
                    SdtContent::Paragraph(paragraph) => Some(paragraph),
                    _ => None,
                })
                .collect()
        }
        let original_paragraphs = paragraphs(&original_content);
        let edited_paragraphs = paragraphs(&edited_content);
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
            &metadata.shared_bookmarks,
        )?;
        let trailing_paragraph_insert_start = aligned
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, (left, right))| {
                left.is_none()
                    && right
                        .and_then(|index| edited_content.get(index))
                        .is_some_and(|content| matches!(content, SdtContent::Paragraph(_)))
            })
            .map(|(position, _)| position)
            .last();
        let trailing_paragraph_delete_start = aligned
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, (left, right))| {
                right.is_none()
                    && left
                        .and_then(|index| original_content.get(index))
                        .is_some_and(|content| matches!(content, SdtContent::Paragraph(_)))
            })
            .map(|(position, _)| position)
            .last();
        let content_spans = direct_word_element_spans(original_xml, "sdtContent")?;
        let content_span = content_spans.first().ok_or_else(|| {
            Error::Other(format!(
                "comparison could not find content-control content at {location}"
            ))
        })?;
        let scoped_content = source_child(original_xml, content_span.clone())?;
        let content_source = scoped_content.as_str();
        let content_range = element_inner_range_any_prefix(content_source, "sdtContent")?;
        let original_spans = modeled_control_source_spans(&original_content, content_source)?
            .into_iter()
            .map(|span| span.start - content_range.start..span.end - content_range.start)
            .collect::<Vec<_>>();
        let content_inner = &content_source[content_range];
        let bindings = root_namespace_bindings(original_xml, "sdt", &[])?;
        let bindings = root_namespace_bindings(content_source, "sdtContent", &bindings)?;
        if original_spans.len() != original_content.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate content-control owners at {location}"
            )));
        }
        let mut content: Vec<(EmittedOwner, String)> = Vec::new();
        let mut trailing_merged = PropertySide::default();
        let mut whitespace_emitted = vec![false; whitespace_slots.len()];
        let mut child_offsets = Vec::new();
        for (position, (left, right)) in aligned.iter().copied().enumerate() {
            let whitespace_boundary = left.or_else(|| {
                let right = right?;
                let (Some(next_left), None) = aligned.get(position + 1).copied()? else {
                    return None;
                };
                (matches!(original_content[next_left], SdtContent::Paragraph(_))
                    && matches!(edited_content[right], SdtContent::Table(_)))
                .then_some(next_left)
            });
            if let Some(index) = whitespace_boundary
                && !whitespace_emitted[index]
                && !whitespace_slots[index].is_empty()
            {
                content.push((EmittedOwner::Whitespace, whitespace_slots[index].clone()));
                whitespace_emitted[index] = true;
            }
            let item = |(left, right): (Option<usize>, Option<usize>)| {
                right
                    .and_then(|index| edited_content.get(index).copied())
                    .or_else(|| left.and_then(|index| original_content.get(index).copied()))
            };
            let next_takes_mark = next_kept_owner(&aligned, position, |pair| {
                matches!(
                    item(pair),
                    Some(SdtContent::Table(_) | SdtContent::ContentControl(_))
                )
            })
            .is_some_and(|pair| {
                matches!(
                    item(pair),
                    Some(
                        SdtContent::Paragraph(_)
                            | SdtContent::Table(_)
                            | SdtContent::ContentControl(_)
                    )
                )
            });
            let child_mark = metadata.leaf_publications.len();
            let child_offset = content.iter().map(|(_, xml)| xml.len()).sum::<usize>();
            let child_location = format!("{location}/content[{position}]");
            match (left, right) {
                (Some(i), Some(j)) => content.push((
                    EmittedOwner::of_control(
                        original_content[i],
                        edited_content[j],
                        None,
                        metadata,
                    )?,
                    compare_control_content(
                        original_content[i],
                        edited_content[j],
                        Some(&source_fragment(
                            &content_inner[original_spans[i].clone()],
                            &bindings,
                        )?),
                        &child_location,
                        &bindings,
                        metadata,
                        diagnostics,
                    )?,
                )),
                (Some(i), None) => {
                    let whole = carried.original.contains(&i);
                    if let SdtContent::Paragraph(paragraph) = original_content[i]
                        && trailing_paragraph_delete_start.is_some_and(|start| position >= start)
                    {
                        if trailing_paragraph_delete_start == Some(position) {
                            trailing_merged =
                                mark_previous_paragraph(&mut content, "del", &bindings, metadata)?;
                        }
                        if next_takes_mark {
                            content.push((
                                EmittedOwner::paragraph(paragraph, paragraph, metadata)?,
                                marked_control_content(
                                    original_content[i],
                                    "del",
                                    whole,
                                    &bindings,
                                    metadata,
                                )?,
                            ));
                        } else {
                            content.push(final_paragraph(
                                paragraph,
                                "del",
                                None,
                                whole,
                                trailing_merged.clone(),
                                metadata,
                            )?);
                        }
                    } else if let SdtContent::Paragraph(paragraph) = original_content[i]
                        && !next_takes_mark
                    {
                        let merged =
                            mark_previous_paragraph(&mut content, "del", &bindings, metadata)?;
                        content.push(final_paragraph(
                            paragraph, "del", None, whole, merged, metadata,
                        )?);
                    } else {
                        content.push((
                            EmittedOwner::of_control(
                                original_content[i],
                                original_content[i],
                                Some("del"),
                                metadata,
                            )?,
                            marked_control_content(
                                original_content[i],
                                "del",
                                whole,
                                &bindings,
                                metadata,
                            )?,
                        ));
                    }
                }
                (None, Some(j)) => {
                    let whole = carried.edited.contains(&j);
                    if let SdtContent::Paragraph(paragraph) = edited_content[j]
                        && trailing_paragraph_insert_start.is_some_and(|start| position >= start)
                    {
                        if trailing_paragraph_insert_start == Some(position) {
                            trailing_merged =
                                mark_previous_paragraph(&mut content, "ins", &bindings, metadata)?;
                        }
                        if next_takes_mark {
                            content.push((
                                EmittedOwner::paragraph(paragraph, paragraph, metadata)?,
                                marked_control_content(
                                    edited_content[j],
                                    "ins",
                                    whole,
                                    &bindings,
                                    metadata,
                                )?,
                            ));
                        } else {
                            content.push(final_paragraph(
                                paragraph,
                                "ins",
                                None,
                                whole,
                                trailing_merged.clone(),
                                metadata,
                            )?);
                        }
                    } else if let SdtContent::Paragraph(paragraph) = edited_content[j]
                        && !next_takes_mark
                    {
                        let merged =
                            mark_previous_paragraph(&mut content, "ins", &bindings, metadata)?;
                        content.push(final_paragraph(
                            paragraph, "ins", None, whole, merged, metadata,
                        )?);
                    } else {
                        content.push((
                            EmittedOwner::of_control(
                                edited_content[j],
                                edited_content[j],
                                Some("ins"),
                                metadata,
                            )?,
                            marked_control_content(
                                edited_content[j],
                                "ins",
                                whole,
                                &bindings,
                                metadata,
                            )?,
                        ));
                    }
                }
                (None, None) => unreachable!(),
            }
            child_offsets.push((child_mark, metadata.leaf_publications.len(), child_offset));
        }
        if let Some(trailing) = whitespace_slots.last()
            && !trailing.is_empty()
        {
            content.push((EmittedOwner::Whitespace, trailing.clone()));
        }
        let content = content.into_iter().map(|(_, xml)| xml).collect::<String>();
        let (output, inner_start) = replace_element_inner(original_xml, "w:sdtContent", &content)?;
        for (start, end, offset) in child_offsets {
            shift_leaf_publications(
                &mut metadata.leaf_publications[start..end],
                (inner_start + offset) as isize,
            )?;
        }
        Ok(output)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_Sdt as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

fn inline_control_raw_boundaries(
    content: &[&SdtContent],
    options: &ComparisonOptions,
) -> Vec<(usize, Vec<u8>)> {
    let mut runs = Vec::new();
    let mut raw = Vec::new();
    for child in content {
        match child {
            SdtContent::Run(run) => runs.push((*run).clone()),
            SdtContent::RawXml(bytes) => {
                let boundary = attributed_run_units(&runs, options)
                    .into_iter()
                    .filter(|unit| !unit_is_ignorable(unit) && !unit_is_empty(unit))
                    .count();
                raw.push((boundary, bytes.clone()));
            }
            _ => unreachable!(),
        }
    }
    raw
}

fn compare_control_content(
    original: &SdtContent,
    edited: &SdtContent,
    original_source: Option<&str>,
    location: &str,
    destination_parent: &NamespaceBindings,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    match (original, edited) {
        (SdtContent::Paragraph(left), SdtContent::Paragraph(right)) => compare_paragraph(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (SdtContent::Table(left), SdtContent::Table(right)) => compare_table(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (SdtContent::ContentControl(left), SdtContent::ContentControl(right)) => {
            if let Some(source) = original_source {
                compare_control_from_xml(left, right, location, metadata, diagnostics, source)
            } else {
                compare_control(left, right, location, metadata, diagnostics)
            }
        }
        (SdtContent::Row(left), SdtContent::Row(right)) => compare_row(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (SdtContent::Cell(left), SdtContent::Cell(right)) => compare_cell_from_xml(
            left,
            right,
            location,
            metadata,
            diagnostics,
            &original_source
                .map(str::to_owned)
                .map_or_else(|| cell_xml(left), Ok)?,
        ),
        (SdtContent::Run(left), SdtContent::Run(right)) => {
            if uses_attributed_run_path(metadata.options) {
                compare_control_runs_with_options(
                    [std::slice::from_ref(left), std::slice::from_ref(right)],
                    None,
                    None,
                    &[],
                    location,
                    destination_parent,
                    metadata,
                    diagnostics,
                )
            } else if run_is_field(left) || run_is_field(right) {
                compared_run_xml(
                    left,
                    right,
                    None,
                    None,
                    location,
                    destination_parent,
                    metadata,
                    diagnostics,
                )
            } else if run_signature(left) == run_signature(right) {
                if !metadata.options.ignore_formatting
                    && run_formatting(left) != run_formatting(right)
                {
                    formatting_diagnostic(diagnostics, location.to_owned());
                }
                if left == right
                    && let Some(source) = original_source
                {
                    Ok(source.to_owned())
                } else {
                    publish_run_source(
                        left,
                        left,
                        run_xml(left)?,
                        metadata.run_source(left),
                        None,
                        None,
                        destination_parent,
                        metadata,
                    )
                }
            } else {
                paired_run_replacement(
                    [left, right],
                    [None, None],
                    [None, None],
                    location,
                    destination_parent,
                    metadata,
                    diagnostics,
                )
            }
        }
        (SdtContent::RawXml(left), SdtContent::RawXml(right)) if left == right => {
            String::from_utf8(left.clone()).map_err(utf8_error)
        }
        _ => Err(Error::Other(format!(
            "comparison cannot revise incompatible content-control children at {location}"
        ))),
    }
}

fn compare_control_runs_with_options(
    runs: [&[CT_R]; 2],
    original_source: Option<(&str, &[Range<usize>])>,
    property_sources: Option<&[Vec<Option<RunSource>>; 2]>,
    raw_boundaries: &[(usize, usize)],
    location: &str,
    destination_parent: &NamespaceBindings,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let revision_parent = metadata.ids.revision_parent_scope(destination_parent);
    let [original, edited] = runs;
    let projection_pairs = metadata
        .leaf_capture
        .then(|| projection_leaf_pairs([original, edited], raw_boundaries));
    let projection_sources = [original, edited]
        .into_iter()
        .enumerate()
        .map(|(side, runs)| {
            runs.iter()
                .enumerate()
                .map(|(owner, run)| {
                    let _ = (side, owner);
                    property_sources
                        .and_then(|sources| sources[side].get(owner).cloned().flatten())
                        .or_else(|| metadata.run_source(run).cloned())
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let projection_sources = [projection_sources[0].clone(), projection_sources[1].clone()];
    let mut projection_ledgers = if let Some(pairs) = &projection_pairs {
        projection_action_ledgers(pairs, [original, edited], &projection_sources)?
    } else {
        Vec::new()
    };
    let original_units = attributed_run_units(original, metadata.options);
    let edited_units = attributed_run_units(edited, metadata.options);
    validate_raw_attribution(original, &original_units)?;
    validate_raw_attribution(edited, &edited_units)?;
    let original_signatures = original_units
        .iter()
        .map(attributed_unit_signature)
        .collect::<Vec<_>>();
    let edited_signatures = edited_units
        .iter()
        .map(attributed_unit_signature)
        .collect::<Vec<_>>();
    let cuts = if metadata.options.granularity == ComparisonGranularity::Run {
        Vec::new()
    } else {
        shell_unit_cuts(
            &original_units,
            &edited_units,
            raw_boundaries.iter().copied(),
        )
        .ok_or_else(|| {
            Error::Other(format!(
                "comparison cannot revise inline-control raw boundaries at {location}"
            ))
        })?
    };
    let (aligned, segments) = if let Some(pairs) = &projection_pairs {
        projection_unit_alignment(
            pairs,
            [&original_units, &edited_units],
            [&original_signatures, &edited_signatures],
        )?
    } else {
        align_between_shells(&original_signatures, &edited_signatures, &cuts)
    };
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
                    .unwrap_or(original.len())
            })
            .collect::<Vec<_>>()
    } else {
        std::iter::once(0)
            .chain(cuts.iter().map(|&(cut, _)| {
                original_units
                    .get(cut)
                    .map_or(original.len(), |unit| unit.owner)
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
        grouped_alignment.push(aligned[members.start]);
        grouped_segment_runs.push(segment_runs[segments[members.start]]);
        let left_indices = members
            .clone()
            .filter_map(|index| aligned[index].0)
            .collect::<Vec<_>>();
        let right_indices = members
            .filter_map(|index| aligned[index].1)
            .collect::<Vec<_>>();
        let mut left_raw = Vec::new();
        let mut left_typed = Vec::new();
        let mut right_raw = Vec::new();
        let mut right_typed = Vec::new();
        let left = merge_unit_runs(
            original,
            &original_units,
            &left_indices,
            &mut left_raw,
            &mut left_typed,
        )?;
        let right = merge_unit_runs(
            edited,
            &edited_units,
            &right_indices,
            &mut right_raw,
            &mut right_typed,
        )?;
        let left_source = left_indices.first().and_then(|index| {
            let owner = original_units[*index].owner;
            property_sources
                .and_then(|sources| sources[0].get(owner).cloned().flatten())
                .or_else(|| metadata.run_source(&original[owner]).cloned())
        });
        let right_source = right_indices.first().and_then(|index| {
            let owner = edited_units[*index].owner;
            property_sources
                .and_then(|sources| sources[1].get(owner).cloned().flatten())
                .or_else(|| metadata.run_source(&edited[owner]).cloned())
        });
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
                let run = &original[owner];
                let source = property_sources
                    .and_then(|sources| sources[0].get(owner).cloned().flatten())
                    .or_else(|| metadata.run_source(run).cloned())
                    .ok_or_else(|| {
                        Error::Other("paired controlled shell has no actual source".into())
                    })?;
                if source
                    .typed_slots
                    .get(ordinal)
                    .and_then(Option::as_ref)
                    .is_none()
                {
                    return Err(Error::Other(
                        "paired controlled leaf has no physical source slot".into(),
                    ));
                }
                Ok((run, source))
            })
            .transpose()?;
        let mut field_bindings = Vec::new();
        for (indices, units, originals, cloned, selected) in [
            (
                &left_indices,
                &original_units,
                original,
                left.as_ref(),
                &left_typed,
            ),
            (
                &right_indices,
                &edited_units,
                edited,
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
                    destination_parent,
                    metadata,
                )?,
                GranularAction::Equal => compared_run_xml(
                    left.as_ref().unwrap(),
                    right.as_ref().unwrap(),
                    left_source.as_ref(),
                    Some((&left_raw, &left_typed)),
                    location,
                    destination_parent,
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
                    destination_parent,
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
                    metadata
                        .ids
                        .revision("del", metadata.author, metadata.timestamp, &payload)?
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
                    metadata
                        .ids
                        .revision("ins", metadata.author, metadata.timestamp, &payload)?
                }
                GranularAction::Drop => String::new(),
            })
        })();
        metadata.unbind_cloned_control_runs(field_bindings)?;
        replacements.push(replacement?);
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
                    destination_parent,
                )?;
            }
        }
        fragment_publications.push(leaves);
    }
    if metadata.leaf_capture {
        attach_projection_actions(&mut fragment_publications, &projection_ledgers)?;
    }
    let Some((source, spans)) = original_source else {
        let mut output = String::new();
        for (replacement, mut leaves) in replacements.into_iter().zip(fragment_publications) {
            shift_leaf_publications(&mut leaves, output.len() as isize)?;
            metadata.leaf_publications.extend(leaves);
            output.push_str(&replacement);
        }
        return Ok(output);
    };
    let inner = element_inner_range_any_prefix(source, "inlineControl")?;
    let source = &source[inner];
    if spans.len() != original.len() {
        return Err(Error::Other(format!(
            "comparison could not correlate inline-control run owners at {location}"
        )));
    }
    let insertion_boundary = spans.first().map_or(source.len(), |span| span.start);
    let mut output = source[..insertion_boundary].to_owned();
    let mut cursor = insertion_boundary;
    let mut consumed_owner = None;
    for (replacement_index, (((left, _), replacement), &segment_run)) in grouped_alignment
        .iter()
        .zip(&replacements)
        .zip(&grouped_segment_runs)
        .enumerate()
    {
        // An insertion can be the first action after a raw child, with no
        // original unit to consume. Copy that shell before its new text.
        let segment_start = spans
            .get(segment_run)
            .map_or(source.len(), |span| span.start);
        if segment_start > cursor {
            output.push_str(&source[cursor..segment_start]);
            cursor = segment_start;
        }
        if let Some(unit) = left.map(|index| &original_units[index])
            && consumed_owner != Some(unit.owner)
        {
            let span = &spans[unit.owner];
            output.push_str(&source[cursor..span.start]);
            cursor = span.end;
            consumed_owner = Some(unit.owner);
        }
        let mut leaves = fragment_publications[replacement_index].clone();
        shift_leaf_publications(&mut leaves, output.len() as isize)?;
        metadata.leaf_publications.extend(leaves);
        output.push_str(replacement);
    }
    output.push_str(&source[cursor..]);
    Ok(output)
}

fn marked_control_content(
    content: &SdtContent,
    kind: &str,
    carried: bool,
    destination_parent: &NamespaceBindings,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    match content {
        SdtContent::Paragraph(paragraph) if kind == "del" => {
            deleted_paragraph(paragraph, carried, metadata)
        }
        SdtContent::Paragraph(paragraph) => inserted_paragraph(paragraph, carried, metadata),
        SdtContent::Table(table) => marked_table(table, kind, metadata),
        SdtContent::Row(row) => marked_row(row, kind, metadata),
        SdtContent::Run(run) => {
            let generated = if kind == "del" {
                deleted_run_xml(run)?
            } else {
                paragraph_owned_run_xml(run)?
            };
            let revision_parent = metadata.ids.revision_parent_scope(destination_parent);
            let payload = publish_run_source(
                run,
                run,
                generated,
                None,
                None,
                None,
                &revision_parent,
                metadata,
            )?;
            metadata
                .ids
                .revision(kind, metadata.author, metadata.timestamp, &payload)
        }
        SdtContent::RawXml(raw) => String::from_utf8(raw.clone()).map_err(utf8_error),
        SdtContent::ContentControl(nested) => {
            marked_control(nested, kind, None, carried, None, metadata)
        }
        SdtContent::Cell(_) => Err(Error::Other(
            "comparison cannot add or remove an unmatched cell in a content control".to_owned(),
        )),
    }
}

/// A block content control added, removed or moved whole, as Word marks
/// one: each paragraph and table inside marked as added, removed or moved
/// whole (`kind`, under `move_id` for a move), nested controls alike, and
/// its properties kept. The resolution that removes that content removes
/// the control with it. `carried` is passed to its paragraphs (see
/// [`deleted_paragraph`]).
///
/// With `last_merged`, the control ends the body and its last paragraph is
/// written as a [`final_paragraph`] that merges those properties: its mark,
/// the body's last, stays untracked, which Word needs to resolve it, and the
/// mark of the paragraph before the control carries the change.
pub(super) fn marked_control<'a>(
    control: &'a CT_Sdt,
    kind: &str,
    move_id: Option<i32>,
    carried: bool,
    last_merged: Option<PropertySide<'a>>,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let source = metadata
        .control_source(control)
        .map(str::to_owned)
        .map_or_else(|| control_xml(control), Ok)?;
    let mut content = String::new();
    let mut last_merged = last_merged;
    for (index, child) in control.content.iter().enumerate() {
        if index + 1 == control.content.len()
            && let SdtContent::Paragraph(paragraph) = child
            && let Some(merged) = last_merged.take()
        {
            content
                .push_str(&final_paragraph(paragraph, kind, move_id, carried, merged, metadata)?.1);
            continue;
        }
        content.push_str(&match (child, move_id) {
            (SdtContent::Paragraph(paragraph), Some(id)) => {
                moved_paragraph(paragraph, kind, id, metadata)?
            }
            (SdtContent::Paragraph(paragraph), None) if kind == "del" => {
                deleted_paragraph(paragraph, carried, metadata)?
            }
            (SdtContent::Paragraph(paragraph), None) => {
                inserted_paragraph(paragraph, carried, metadata)?
            }
            (SdtContent::Table(table), Some(id)) => moved_table(table, kind, id, metadata)?,
            (SdtContent::Table(table), None) => marked_table(table, kind, metadata)?,
            (SdtContent::ContentControl(nested), _) => {
                marked_control(nested, kind, move_id, carried, None, metadata)?
            }
            (SdtContent::RawXml(raw), _) if block_markup_between_owners(raw) => {
                String::from_utf8(raw.clone()).map_err(utf8_error)?
            }
            _ => {
                return Err(Error::Other(
                    "comparison can add or remove whole only a content control that holds paragraphs, tables or content controls"
                        .to_owned(),
                ));
            }
        });
    }
    if !control.content.iter().any(|child| {
        matches!(
            child,
            SdtContent::Paragraph(_) | SdtContent::Table(_) | SdtContent::ContentControl(_)
        )
    }) {
        return Err(Error::Other(
            "comparison cannot add or remove an empty content control".to_owned(),
        ));
    }
    metadata
        .ids
        .generated(replace_element_inner(&source, "w:sdtContent", &content)?.0)
}

/// Whether unmodelled XML between the owners of a block content control is
/// whitespace, a proofing mark or a bookmark, comment or permission range
/// marker: markup that resolving the control's content keeps or drops as
/// it stands, and that a control added or removed whole carries verbatim.
fn block_markup_between_owners(raw: &[u8]) -> bool {
    let text = String::from_utf8_lossy(raw);
    let text = text.trim();
    if text.is_empty() {
        return true;
    }
    let Some(tag) = text.strip_prefix('<') else {
        return false;
    };
    let name = tag
        .split(|character: char| character.is_whitespace() || matches!(character, '/' | '>'))
        .next()
        .unwrap_or_default();
    matches!(
        name.rsplit(':').next().unwrap_or_default(),
        "proofErr"
            | "bookmarkStart"
            | "bookmarkEnd"
            | "commentRangeStart"
            | "commentRangeEnd"
            | "permStart"
            | "permEnd"
    ) && text.ends_with("/>")
        && text.matches('<').count() == 1
}
