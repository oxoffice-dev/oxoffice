//! Run units attributed to their raw children.

use super::*;

pub(super) fn attributed_run_units(
    runs: &[CT_R],
    options: &ComparisonOptions,
) -> Vec<AttributedRunUnit> {
    let mut units = Vec::new();
    for (owner, run) in runs.iter().enumerate() {
        let unit_start = units.len();
        let mut content_units = Vec::<Range<usize>>::with_capacity(run.content.len());
        for (content_index, content) in run.content.iter().enumerate() {
            let start = units.len() - unit_start;
            let fragments = match content {
                RunContent::Text(text) => granular_text(text, options)
                    .into_iter()
                    .map(RunContent::Text)
                    .collect::<Vec<_>>(),
                RunContent::DeletedText(text) => granular_text(text, options)
                    .into_iter()
                    .map(RunContent::DeletedText)
                    .collect::<Vec<_>>(),
                content => vec![content.clone()],
            };
            let whole = fragments.len() == 1;
            let mut scalar_cursor = 0;
            for fragment in fragments {
                let scalars = match &fragment {
                    RunContent::Text(text) | RunContent::DeletedText(text) if !whole => {
                        let end = scalar_cursor + text.text.chars().count();
                        let range = Some((scalar_cursor, end));
                        scalar_cursor = end;
                        range
                    }
                    _ => None,
                };
                let ignored = ignored_run_content(&fragment, options);
                units.push(AttributedRunUnit {
                    run: CT_R {
                        properties: run.properties.clone(),
                        content: vec![fragment],
                        extra_xml: Vec::new(),
                        extra_xml_positions: Vec::new(),
                        alt_drawings: Vec::new(),
                    },
                    ignored,
                    owner,
                    raw_sources: Vec::new(),
                    typed_sources: vec![Some(TypedSourceSelection {
                        ordinal: content_index,
                        scalars,
                    })],
                });
            }
            content_units.push(start..units.len() - unit_start);
        }
        if units.len() == unit_start {
            units.push(AttributedRunUnit {
                run: CT_R {
                    properties: run.properties.clone(),
                    content: Vec::new(),
                    extra_xml: Vec::new(),
                    extra_xml_positions: Vec::new(),
                    alt_drawings: Vec::new(),
                },
                ignored: false,
                owner,
                raw_sources: Vec::new(),
                typed_sources: Vec::new(),
            });
        }
        attribute_raw_children(run, &content_units, &mut units[unit_start..]);
        let property_boundary = usize::from(run.properties.is_some());
        let mut inserted = 0usize;
        for (ordinal, (raw, &encoded)) in run
            .extra_xml
            .iter()
            .zip(&run.extra_xml_positions)
            .enumerate()
        {
            if !is_text_box_host_marker(raw) {
                continue;
            }
            let boundary = CT_R::raw_child_position(encoded);
            let offset = if boundary <= property_boundary || content_units.is_empty() {
                0
            } else {
                let content_index = (boundary - property_boundary - 1).min(content_units.len() - 1);
                content_units[content_index].end
            };
            let mut position = encoded;
            CT_R::set_raw_child_position(&mut position, property_boundary);
            units.insert(
                unit_start + offset + inserted,
                AttributedRunUnit {
                    run: CT_R {
                        properties: run.properties.clone(),
                        content: Vec::new(),
                        extra_xml: vec![raw.clone()],
                        extra_xml_positions: vec![position],
                        alt_drawings: Vec::new(),
                    },
                    ignored: false,
                    owner,
                    raw_sources: vec![(0, ordinal, boundary)],
                    typed_sources: Vec::new(),
                },
            );
            inserted += 1;
        }
        preserve_marker_raw_order(run, &mut units[unit_start..]);
        // Each AlternateContent projection follows its actual flagged raw slot.
        let projected = run
            .extra_xml_positions
            .iter()
            .enumerate()
            .filter(|(_, encoded)| CT_R::raw_child_has_alternate_content_drawing(**encoded))
            .map(|(ordinal, _)| ordinal)
            .collect::<Vec<_>>();
        for unit in &mut units[unit_start..] {
            for &(_, ordinal, _) in &unit.raw_sources {
                if let Some(index) = projected.iter().position(|candidate| *candidate == ordinal)
                    && let Some(drawing) = run.alt_drawings.get(index)
                {
                    unit.run.alt_drawings.push(drawing.clone());
                }
            }
        }
        if projected.len() < run.alt_drawings.len() {
            units[unit_start]
                .run
                .alt_drawings
                .extend_from_slice(&run.alt_drawings[projected.len()..]);
        }
    }
    units
}

fn attribute_raw_children(
    run: &CT_R,
    content_units: &[Range<usize>],
    units: &mut [AttributedRunUnit],
) {
    let property_boundary = usize::from(run.properties.is_some());
    for (ordinal, (raw, &encoded)) in run
        .extra_xml
        .iter()
        .zip(&run.extra_xml_positions)
        .enumerate()
    {
        if is_text_box_host_marker(raw) {
            continue;
        }
        let boundary = CT_R::raw_child_position(encoded);
        let comment = raw_comment_content_index(run, ordinal);
        let (unit_index, position) = if let Some(index) = comment {
            (content_units[index].start, property_boundary)
        } else if boundary <= property_boundary || content_units.is_empty() {
            (0, boundary.min(property_boundary))
        } else {
            let content_index = (boundary - property_boundary - 1).min(content_units.len() - 1);
            let range = &content_units[content_index];
            (range.end.saturating_sub(1), property_boundary + 1)
        };
        let local = units[unit_index].run.extra_xml.len();
        if !CT_R::raw_child_is_root_attributes(encoded) {
            units[unit_index]
                .raw_sources
                .push((local, ordinal, boundary));
        }
        units[unit_index].run.extra_xml.push(raw.clone());
        let mut attributed = encoded;
        CT_R::set_raw_child_position(&mut attributed, position);
        units[unit_index].run.extra_xml_positions.push(attributed);
    }
}

fn preserve_marker_raw_order(run: &CT_R, units: &mut [AttributedRunUnit]) {
    let hosts = units
        .iter()
        .enumerate()
        .flat_map(|(index, unit)| {
            unit.raw_sources
                .iter()
                .filter_map(move |&(local, ordinal, boundary)| {
                    is_text_box_host_marker(&unit.run.extra_xml[local])
                        .then_some((index, ordinal, boundary))
                })
        })
        .collect::<Vec<_>>();
    for (host, host_ordinal, host_boundary) in hosts {
        let mut moved = Vec::new();
        for (index, unit) in units.iter_mut().enumerate() {
            if index == host {
                continue;
            }
            let provenance = std::mem::take(&mut unit.raw_sources);
            let raw = std::mem::take(&mut unit.run.extra_xml);
            let positions = std::mem::take(&mut unit.run.extra_xml_positions);
            for (local, (raw, position)) in raw.into_iter().zip(positions).enumerate() {
                let source = provenance.iter().find(|(at, _, _)| *at == local).copied();
                if let Some((_, ordinal, boundary)) = source
                    && boundary == host_boundary
                    && ordinal > host_ordinal
                    && !is_text_box_host_marker(&raw)
                {
                    moved.push((ordinal, boundary, raw, position));
                } else {
                    if let Some((_, ordinal, boundary)) = source {
                        unit.raw_sources
                            .push((unit.run.extra_xml.len(), ordinal, boundary));
                    }
                    unit.run.extra_xml.push(raw);
                    unit.run.extra_xml_positions.push(position);
                }
            }
        }
        moved.sort_by_key(|(ordinal, _, _, _)| *ordinal);
        for (ordinal, boundary, raw, mut position) in moved {
            let unit = &mut units[host];
            CT_R::set_raw_child_position(&mut position, usize::from(run.properties.is_some()));
            unit.raw_sources
                .push((unit.run.extra_xml.len(), ordinal, boundary));
            unit.run.extra_xml.push(raw);
            unit.run.extra_xml_positions.push(position);
        }
    }
}

fn raw_comment_content_index(run: &CT_R, ordinal: usize) -> Option<usize> {
    let raw = run.extra_xml.get(ordinal)?;
    let mut reader = NsReader::from_reader(raw.as_slice());
    let (namespace, event) = reader.read_resolved_event().ok()?;
    if !matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == W_NS.as_bytes()) {
        return None;
    }
    let (Event::Start(root) | Event::Empty(root)) = event else {
        return None;
    };
    if root.local_name().as_ref() != b"commentReference" {
        return None;
    }
    run.content.iter().position(|content| matches!(content, RunContent::CommentReference {raw_before, ..} if *raw_before == ordinal + 1))
}

/// Prove the attribution partition once per physical input role. This validates
/// identities supplied by the parser, never a lookup by generated XML spelling.
pub(super) fn validate_raw_attribution(runs: &[CT_R], units: &[AttributedRunUnit]) -> Result<()> {
    let mut seen = runs
        .iter()
        .map(|run| vec![false; run.extra_xml.len()])
        .collect::<Vec<_>>();
    let mut previous = vec![None; runs.len()];
    let mut typed_coverage = runs
        .iter()
        .map(|run| vec![(false, 0usize); run.content.len()])
        .collect::<Vec<_>>();
    for unit in units {
        let run = runs
            .get(unit.owner)
            .ok_or_else(|| Error::Other("attributed run owner is absent".into()))?;
        if unit.typed_sources.len() != unit.run.content.len() {
            return Err(Error::Other(
                "typed attribution coverage is incomplete".into(),
            ));
        }
        for (content, selected) in unit.run.content.iter().zip(&unit.typed_sources) {
            let selected = selected
                .ok_or_else(|| Error::Other("typed attribution has no source role".into()))?;
            let original = run
                .content
                .get(selected.ordinal)
                .ok_or_else(|| Error::Other("typed source ordinal is absent".into()))?;
            let (whole, cursor) = &mut typed_coverage[unit.owner][selected.ordinal];
            if *whole {
                return Err(Error::Other(
                    "typed attribution repeats a whole source role".into(),
                ));
            }
            if let Some((start, end)) = selected.scalars {
                let (
                    RunContent::Text(actual) | RunContent::DeletedText(actual),
                    RunContent::Text(fragment) | RunContent::DeletedText(fragment),
                ) = (original, content)
                else {
                    return Err(Error::Other(
                        "typed scalar selection has no text source".into(),
                    ));
                };
                if start != *cursor
                    || end < start
                    || end > actual.text.chars().count()
                    || actual
                        .text
                        .chars()
                        .skip(start)
                        .take(end - start)
                        .collect::<String>()
                        != fragment.text
                {
                    return Err(Error::Other(
                        "typed source scalar partition is incomplete or reordered".into(),
                    ));
                }
                *cursor = end;
            } else {
                if *cursor != 0
                    || match (original, content) {
                        (RunContent::Text(a), RunContent::Text(b))
                        | (RunContent::DeletedText(a), RunContent::DeletedText(b)) => {
                            a.text != b.text
                        }
                        _ => original != content,
                    }
                {
                    return Err(Error::Other("typed whole source authority changed".into()));
                }
                *whole = true;
            }
        }
        for &(local, ordinal, boundary) in &unit.raw_sources {
            if ordinal >= run.extra_xml.len()
                || local >= unit.run.extra_xml.len()
                || seen[unit.owner][ordinal]
                || previous[unit.owner].is_some_and(|previous| ordinal <= previous)
                || CT_R::raw_child_position(run.extra_xml_positions[ordinal]) != boundary
                || run.extra_xml[ordinal] != unit.run.extra_xml[local]
            {
                return Err(Error::Other(
                    "raw attribution has an invalid or repeated source occurrence".into(),
                ));
            }
            seen[unit.owner][ordinal] = true;
            previous[unit.owner] = Some(ordinal);
        }
    }
    for (run, roles) in runs.iter().zip(typed_coverage) {
        for (content, (whole, covered)) in run.content.iter().zip(roles) {
            let count = match content {
                RunContent::Text(text) | RunContent::DeletedText(text) => text.text.chars().count(),
                _ => 0,
            };
            if !whole && (count == 0 || covered != count) {
                return Err(Error::Other(
                    "typed attribution lost an input occurrence".into(),
                ));
            }
        }
    }
    for (run, seen) in runs.iter().zip(seen) {
        if seen
            .iter()
            .zip(&run.extra_xml_positions)
            .any(|(seen, encoded)| !*seen && !CT_R::raw_child_is_root_attributes(*encoded))
        {
            return Err(Error::Other(
                "raw attribution lost an input occurrence".into(),
            ));
        }
    }
    Ok(())
}

fn is_text_box_host_marker(raw: &[u8]) -> bool {
    raw.windows(b"urn:rdocx:comparison:private".len())
        .any(|window| window == b"urn:rdocx:comparison:private")
        && raw
            .windows(b"textBoxHost".len())
            .any(|window| window == b"textBoxHost")
}

pub(super) fn granular_text(text: &CT_Text, options: &ComparisonOptions) -> Vec<CT_Text> {
    let fragments = match options.granularity {
        ComparisonGranularity::Run if options.ignore_whitespace => whitespace_fragments(&text.text),
        ComparisonGranularity::Run => vec![text.text.clone()],
        ComparisonGranularity::Character => text.text.chars().map(String::from).collect(),
        ComparisonGranularity::Word => word_fragments(&text.text),
    };
    if fragments.is_empty() {
        return vec![text.clone()];
    }
    fragments
        .into_iter()
        .map(|value| CT_Text {
            // A unit is written back as its own `w:t`, where edge whitespace
            // needs the flag to survive. Whitespace in a unit then reads as
            // whitespace whatever the source flag was.
            preserve_space: text.preserve_space || has_edge_whitespace(&value),
            text: value,
        })
        .collect()
}

/// The whole-run signature that agrees with the attributed units.
///
/// It reads the space flag the way `granular_text` writes it on every unit,
/// so a run that differs only by the flag stays whole instead of being
/// rewritten one unit per run, which would move run-indexed bookmark ends.
pub(super) fn attributed_run_signature(run: &CT_R) -> String {
    let mut run = run.clone();
    for content in &mut run.content {
        if let RunContent::Text(text) | RunContent::DeletedText(text) = content {
            text.preserve_space |= has_edge_whitespace(&text.text);
        }
    }
    run_signature(&run)
}

fn whitespace_fragments(text: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut whitespace = None;
    for character in text.chars() {
        let next = character.is_whitespace();
        if whitespace.is_some_and(|value| value != next) {
            output.push(std::mem::take(&mut current));
        }
        current.push(character);
        whitespace = Some(next);
    }
    if !current.is_empty() {
        output.push(current);
    }
    output
}

fn word_fragments(text: &str) -> Vec<String> {
    fn class(character: char) -> u8 {
        if character.is_alphanumeric() || character == '_' {
            0
        } else if character.is_whitespace() {
            1
        } else {
            2
        }
    }
    let mut output = Vec::new();
    let mut current = String::new();
    let mut current_class = None;
    for character in text.chars() {
        let next_class = class(character);
        if current_class.is_some_and(|value| value != next_class) {
            output.push(std::mem::take(&mut current));
        }
        current.push(character);
        current_class = Some(next_class);
    }
    if !current.is_empty() {
        output.push(current);
    }
    output
}

pub(super) fn ignored_run_content(content: &RunContent, options: &ComparisonOptions) -> bool {
    (options.ignore_fields && matches!(content, RunContent::Field(_)))
        || (options.ignore_comments && matches!(content, RunContent::CommentReference { .. }))
        || (options.ignore_whitespace
            && matches!(
                content,
                RunContent::Text(text) | RunContent::DeletedText(text)
                    if text.text.chars().all(char::is_whitespace)
            ))
}

pub(super) fn run_is_ignored(run: &CT_R, options: &ComparisonOptions) -> bool {
    !run.content.is_empty()
        && run
            .content
            .iter()
            .all(|content| ignored_run_content(content, options))
        && semantic_run_raw(run).is_empty()
}

pub(super) fn attributed_unit_signature(unit: &AttributedRunUnit) -> String {
    if unit.ignored {
        let kind = match unit.run.content.first() {
            Some(RunContent::Field(_)) => "ignored:field".to_owned(),
            Some(RunContent::CommentReference { .. }) => "ignored:comment".to_owned(),
            _ => "ignored:whitespace".to_owned(),
        };
        let raw = semantic_run_raw(&unit.run);
        format!("{kind}:{raw:?}:{:?}", unit.run.alt_drawings)
    } else {
        match unit.run.content.first() {
            Some(RunContent::CommentReference { id, .. }) if unit.run.content.len() == 1 => {
                let raw = semantic_run_raw(&unit.run);
                format!(
                    "comment:{id}:{:?}:{raw:?}:{:?}",
                    unit.run.properties, unit.run.alt_drawings
                )
            }
            _ => run_signature(&unit.run),
        }
    }
}
