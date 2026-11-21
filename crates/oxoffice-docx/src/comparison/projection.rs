//! Projection of granular actions on source leaves, and their ledgers.

use super::*;

/// Only isolated paired-body projection selects these ordered source-slot pairs.
/// Scalar alignment is confined to each pair, including ignored whitespace.
pub(super) fn projection_leaf_pairs(
    runs: [&[CT_R]; 2],
    boundaries: &[(usize, usize)],
) -> Vec<[Option<(usize, usize)>; 2]> {
    let signatures = runs.map(|runs| {
        runs.iter()
            .map(attributed_run_signature)
            .collect::<Vec<_>>()
    });
    let mut output = Vec::new();
    for (left, right) in align_between_shells(&signatures[0], &signatures[1], boundaries).0 {
        let slots = [left, right]
            .into_iter()
            .enumerate()
            .map(|(side, owner)| {
                owner
                    .map(|owner| {
                        let run = &runs[side][owner];
                        if run.content.is_empty() {
                            vec!["raw-run".to_owned()]
                        } else {
                            run.content.iter().map(run_content_signature).collect()
                        }
                    })
                    .unwrap_or_default()
            })
            .collect::<Vec<Vec<String>>>();
        let raw_cuts = [left, right]
            .into_iter()
            .enumerate()
            .map(|(side, owner)| {
                owner
                    .map(|owner| {
                        let run = &runs[side][owner];
                        run.extra_xml_positions
                            .iter()
                            .filter(|encoded| !CT_R::raw_child_is_root_attributes(**encoded))
                            .map(|encoded| {
                                CT_R::raw_child_position(*encoded)
                                    .saturating_sub(usize::from(run.properties.is_some()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let cuts = if raw_cuts[0].len() == raw_cuts[1].len() {
            raw_cuts[0]
                .iter()
                .copied()
                .zip(raw_cuts[1].iter().copied())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for (a, b) in align_between_shells(&slots[0], &slots[1], &cuts).0 {
            output.push([
                left.zip(a).map(|(owner, ordinal)| {
                    (
                        owner,
                        if runs[0][owner].content.is_empty() {
                            usize::MAX
                        } else {
                            ordinal
                        },
                    )
                }),
                right.zip(b).map(|(owner, ordinal)| {
                    (
                        owner,
                        if runs[1][owner].content.is_empty() {
                            usize::MAX
                        } else {
                            ordinal
                        },
                    )
                }),
            ]);
        }
    }
    output
}

/// One logical occurrence has independent original and edited scalar domains.
/// Capture source membership before unit grouping, then record actual actions.
pub(super) struct ProjectionActionLedger {
    domains: [Option<(usize, usize, usize)>; 2],
    source: String,
    context: NamespaceBindings,
    actions: Vec<([Option<Range<usize>>; 2], GranularAction)>,
}

pub(super) fn projection_action_ledgers(
    pairs: &[[Option<(usize, usize)>; 2]],
    runs: [&[CT_R]; 2],
    sources: &[Vec<Option<RunSource>>; 2],
) -> Result<Vec<ProjectionActionLedger>> {
    let mut result = Vec::new();
    for pair in pairs {
        let mut domains = [None, None];
        let mut template = None;
        for side in 0..2 {
            let Some((owner, ordinal)) = pair[side] else {
                continue;
            };
            if !matches!(
                runs[side][owner].content.get(ordinal),
                Some(RunContent::Text(_) | RunContent::DeletedText(_))
            ) {
                continue;
            }
            let source = sources[side][owner]
                .as_ref()
                .ok_or_else(|| Error::Other("projection leaf ledger has no source owner".into()))?;
            let span = source
                .typed_slots
                .get(ordinal)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    Error::Other("projection leaf ledger has no actual lexical slot".into())
                })?;
            let context = root_namespace_bindings(&source.scoped, "r", &source.parent)?;
            let raw = source.scoped[span.clone()].to_owned();
            let Some(RunContent::Text(text) | RunContent::DeletedText(text)) =
                source_content_model(&raw, &context)?
            else {
                return Err(Error::Other(
                    "projection leaf ledger has incompatible source grammar".into(),
                ));
            };
            domains[side] = Some((source.owner, ordinal, text.text.chars().count()));
            if template.is_none() {
                template = Some((raw, context));
            }
        }
        if let Some((source, context)) = template {
            result.push(ProjectionActionLedger {
                domains,
                source,
                context,
                actions: Vec::new(),
            });
        }
    }
    Ok(result)
}

pub(super) fn record_projection_actions(
    ledgers: &mut [ProjectionActionLedger],
    action: GranularAction,
    members: Range<usize>,
    aligned: &Alignment,
    units: [&[AttributedRunUnit]; 2],
    sources: &[Vec<Option<RunSource>>; 2],
) -> Result<()> {
    for at in members {
        let pair = [aligned[at].0, aligned[at].1];
        let mut inputs = [None, None];
        let mut selected = None;
        for side in 0..2 {
            let Some(index) = pair[side] else {
                continue;
            };
            let unit = &units[side][index];
            let Some(source) = sources[side][unit.owner].as_ref() else {
                continue;
            };
            for slot in unit.typed_sources.iter().flatten() {
                if let Some((ledger_index, domain)) =
                    ledgers.iter().enumerate().find_map(|(i, ledger)| {
                        ledger.domains[side]
                            .filter(|(owner, ordinal, _)| {
                                *owner == source.owner && *ordinal == slot.ordinal
                            })
                            .map(|domain| (i, domain))
                    })
                {
                    if selected.is_some_and(|previous| previous != ledger_index)
                        || inputs[side].is_some()
                    {
                        return Err(Error::Other(
                            "projection action crosses selected logical occurrences".into(),
                        ));
                    }
                    selected = Some(ledger_index);
                    let (start, end) = slot.scalars.unwrap_or((0, domain.2));
                    inputs[side] = Some(start..end);
                }
            }
        }
        if let Some(index) = selected {
            ledgers[index].actions.push((inputs, action));
        }
    }
    Ok(())
}

pub(super) fn attach_projection_actions(
    leaves: &mut [Vec<LeafPublication>],
    ledgers: &[ProjectionActionLedger],
) -> Result<()> {
    for ledger in ledgers {
        validate_projection_actions(&ledger.domains, &ledger.actions)?;
    }
    for leaf in leaves.iter_mut().flatten() {
        let ledger = ledgers
            .iter()
            .find(|ledger| {
                ledger.domains.iter().flatten().any(|(owner, ordinal, _)| {
                    *owner == leaf.physical.0 && *ordinal == leaf.physical.1
                })
            })
            .ok_or_else(|| {
                Error::Other("emitted leaf lost its selected logical occurrence".into())
            })?;
        let (owner, ordinal, _) = ledger.domains.iter().flatten().next().copied().unwrap();
        leaf.owner = owner;
        leaf.ordinal = ordinal;
        leaf.source = ledger.source.clone();
        leaf.context = ledger.context.clone();
        leaf.domains = ledger.domains;
        leaf.actions = ledger.actions.clone();
    }
    Ok(())
}

pub(super) fn validate_projection_actions(
    domains: &[Option<(usize, usize, usize)>; 2],
    actions: &[([Option<Range<usize>>; 2], GranularAction)],
) -> Result<()> {
    for side in 0..2 {
        let ranges = actions
            .iter()
            .filter_map(|(inputs, _)| inputs[side].clone())
            .collect::<Vec<_>>();
        let Some((_, _, length)) = domains[side] else {
            if !ranges.is_empty() {
                return Err(Error::Other(
                    "projection actions invent an absent input domain".into(),
                ));
            }
            continue;
        };
        let mut covered = 0;
        for range in &ranges {
            if range.start != covered || range.end < range.start || range.end > length {
                return Err(Error::Other(format!(
                    "projection input {side} action coverage is incomplete or repeated"
                )));
            }
            covered = range.end;
        }
        if covered != length || (length == 0 && ranges.len() != 1) {
            return Err(Error::Other(format!(
                "projection input {side} action coverage omits source scalars"
            )));
        }
    }
    for (inputs, action) in actions {
        let valid = match action {
            GranularAction::Preserve => inputs[0].is_some(),
            GranularAction::Equal | GranularAction::Replace => inputs.iter().all(Option::is_some),
            GranularAction::Delete => inputs[0].is_some() && inputs[1].is_none(),
            GranularAction::Insert | GranularAction::Drop => {
                inputs[0].is_none() && inputs[1].is_some()
            }
        };
        if !valid {
            return Err(Error::Other(
                "projection action has inconsistent input roles".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn projection_unit_alignment(
    pairs: &[[Option<(usize, usize)>; 2]],
    units: [&[AttributedRunUnit]; 2],
    signatures: [&[String]; 2],
) -> Result<(Alignment, Vec<usize>)> {
    let mut output = Vec::new();
    let mut segments = Vec::new();
    let mut seen = [vec![false; units[0].len()], vec![false; units[1].len()]];
    for (segment, pair) in pairs.iter().enumerate() {
        let indices = [0, 1].map(|side| {
            units[side]
                .iter()
                .enumerate()
                .filter_map(|(index, unit)| {
                    pair[side]
                        .filter(|(owner, ordinal)| {
                            unit.owner == *owner
                                && (if *ordinal == usize::MAX {
                                    unit.typed_sources.is_empty()
                                } else {
                                    unit.typed_sources
                                        .iter()
                                        .flatten()
                                        .any(|slot| slot.ordinal == *ordinal)
                                })
                        })
                        .map(|_| index)
                })
                .collect::<Vec<_>>()
        });
        for side in 0..2 {
            for &index in &indices[side] {
                if std::mem::replace(&mut seen[side][index], true) {
                    return Err(Error::Other("projection source slot repeats a unit".into()));
                }
            }
        }
        let values = [0, 1].map(|side| {
            indices[side]
                .iter()
                .map(|index| signatures[side][*index].clone())
                .collect::<Vec<_>>()
        });
        for (left, right) in align(&values[0], &values[1]) {
            output.push((left.map(|i| indices[0][i]), right.map(|i| indices[1][i])));
            segments.push(segment);
        }
    }
    if seen.iter().flatten().any(|seen| !seen) {
        return Err(Error::Other("projection source slots omit a unit".into()));
    }
    Ok((output, segments))
}

/// Publish a source separator only when one-occurrence assembly would otherwise
/// merge two surviving Run/Word tokens. Ordinary comparison never calls this.
pub(super) fn retain_projection_separators(
    grouped: &mut [(GranularAction, Range<usize>)],
    aligned: &Alignment,
    segments: &[usize],
    pairs: &[[Option<(usize, usize)>; 2]],
    units: [&[AttributedRunUnit]; 2],
    sources: &[Vec<Option<RunSource>>; 2],
    options: &ComparisonOptions,
) -> Result<()> {
    if !options.ignore_whitespace || options.granularity == ComparisonGranularity::Character {
        return Ok(());
    }
    let mut actions = vec![GranularAction::Drop; aligned.len()];
    for (action, members) in grouped.iter() {
        actions[members.clone()].fill(*action);
    }
    // Source raw slots are ordered relative to the actual selected typed slot.
    // A right edge may retain a suffix, but that suffix cannot precede the edge.
    let edge = |at: usize, before: bool| -> Result<Option<char>> {
        let (side, index) = match actions[at] {
            GranularAction::Preserve | GranularAction::Equal => (0, aligned[at].0),
            GranularAction::Replace | GranularAction::Insert => (1, aligned[at].1),
            GranularAction::Delete | GranularAction::Drop => return Ok(None),
        };
        let unit = &units[side][index.unwrap()];
        let [RunContent::Text(text)] = unit.run.content.as_slice() else {
            return Ok(None);
        };
        let [Some(selected)] = unit.typed_sources.as_slice() else {
            return Ok(None);
        };
        if !unit.run.alt_drawings.is_empty() {
            return Ok(None);
        }
        let source = sources[side][unit.owner]
            .as_ref()
            .ok_or_else(|| Error::Other("separator neighbor has no actual source owner".into()))?;
        let span = source
            .typed_slots
            .get(selected.ordinal)
            .and_then(Option::as_ref)
            .ok_or_else(|| Error::Other("separator neighbor has no typed source slot".into()))?;
        for &(_, ordinal, _) in &unit.raw_sources {
            let raw = source
                .raw_slots
                .get(ordinal)
                .and_then(Option::as_ref)
                .ok_or_else(|| Error::Other("separator neighbor has no raw source slot".into()))?;
            if if before {
                raw.0.end > span.start
            } else {
                raw.0.start < span.end
            } {
                return Ok(None);
            }
        }
        Ok(if before {
            text.text.chars().next_back()
        } else {
            text.text.chars().next()
        })
    };
    for (action, members) in grouped {
        if *action != GranularAction::Drop {
            continue;
        }
        if members.len() != 1 {
            return Err(Error::Other(
                "separator Drop has ambiguous unit coverage".into(),
            ));
        }
        let at = members.start;
        let pair = pairs[segments[at]];
        let (Some(_), Some((right_owner, right_ordinal))) = (pair[0], pair[1]) else {
            continue;
        };
        let (None, Some(right_index)) = aligned[at] else {
            return Err(Error::Other(
                "separator Drop has inconsistent source roles".into(),
            ));
        };
        let right = &units[1][right_index];
        let [Some(selected)] = right.typed_sources.as_slice() else {
            continue;
        };
        if right.owner != right_owner || selected.ordinal != right_ordinal {
            return Err(Error::Other(
                "separator crosses its selected source leaf".into(),
            ));
        }
        let source = sources[1][right.owner]
            .as_ref()
            .ok_or_else(|| Error::Other("separator has no actual source owner".into()))?;
        let span = source
            .typed_slots
            .get(selected.ordinal)
            .and_then(Option::as_ref)
            .ok_or_else(|| Error::Other("separator has no typed source slot".into()))?;
        if source_content_metadata(source, span.clone())?.is_empty() {
            continue;
        }
        let [RunContent::Text(text)] = right.run.content.as_slice() else {
            continue;
        };
        if text.text.is_empty() || !text.text.chars().all(char::is_whitespace) {
            return Err(Error::Other(
                "separator is not actual ignored whitespace".into(),
            ));
        }
        let neighbor = |before| -> Result<Option<char>> {
            let mut index = at;
            loop {
                let next = if before {
                    index.checked_sub(1)
                } else {
                    index.checked_add(1).filter(|index| *index < aligned.len())
                };
                let Some(next) = next else { return Ok(None) };
                index = next;
                if segments[index] != segments[at] {
                    return Ok(None);
                }
                if matches!(
                    actions[index],
                    GranularAction::Delete | GranularAction::Drop
                ) {
                    continue;
                }
                return edge(index, before);
            }
        };
        let (Some(left), Some(right_edge)) = (neighbor(true)?, neighbor(false)?) else {
            continue;
        };
        if left.is_whitespace() || right_edge.is_whitespace() {
            continue;
        }
        let joined = CT_Text {
            text: format!("{left}{right_edge}"),
            preserve_space: false,
        };
        if granular_text(&joined, options).len() != 1 {
            continue;
        }
        *action = GranularAction::Insert;
    }
    Ok(())
}

/// The aligned pairs grouped into the pieces the redline writes as one run.
///
/// The granularity sets what the diff compares, never how many runs carry the
/// result. A change takes every consecutive pair of its kind that the same
/// physical runs own. Unchanged text takes every consecutive pair that
/// continues the same original and edited text element, so it stays one run
/// between two revisions whether the units are words or characters.
pub(super) fn coalesced_granular_alignment(
    aligned: &[(Option<usize>, Option<usize>)],
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    original_signatures: &[String],
    edited_signatures: &[String],
) -> Vec<(GranularAction, Range<usize>)> {
    let owners = |at: usize| {
        (
            aligned[at].0.map(|index| original_units[index].owner),
            aligned[at].1.map(|index| edited_units[index].owner),
        )
    };
    // The text element each side of an unchanged pair is a fragment of.
    let elements = |at: usize| {
        aligned[at]
            .0
            .and_then(|index| unit_text_element(&original_units[index]))
            .zip(
                aligned[at]
                    .1
                    .and_then(|index| unit_text_element(&edited_units[index])),
            )
    };
    let mut groups = Vec::new();
    let mut start = 0usize;
    while start < aligned.len() {
        let action = granular_action(
            aligned[start],
            original_units,
            edited_units,
            original_signatures,
            edited_signatures,
        );
        let continues = |at: usize| match action {
            GranularAction::Replace | GranularAction::Delete | GranularAction::Insert => true,
            GranularAction::Equal => elements(start).is_some() && elements(at) == elements(start),
            GranularAction::Preserve | GranularAction::Drop => false,
        };
        let mut end = start + 1;
        while end < aligned.len()
            && continues(end)
            && granular_action(
                aligned[end],
                original_units,
                edited_units,
                original_signatures,
                edited_signatures,
            ) == action
            && owners(end) == owners(start)
        {
            end += 1;
        }
        groups.push((action, start..end));
        start = end;
    }
    groups
}
