//! Text units of a granular comparison: words, merged runs, interleaving.

use super::*;

/// The content ordinal of the text element a plain unit is a fragment of.
pub(super) fn unit_text_element(unit: &AttributedRunUnit) -> Option<usize> {
    unit_plain_text(unit)?;
    match unit.typed_sources.as_slice() {
        [Some(selected)] => Some(selected.ordinal),
        _ => None,
    }
}

/// The text of a unit that holds nothing but text, which grouping may move
/// into a deletion or an insertion.
pub(super) fn unit_plain_text(unit: &AttributedRunUnit) -> Option<&str> {
    match unit.run.content.as_slice() {
        [RunContent::Text(text)]
            if !unit.ignored
                && semantic_run_raw(&unit.run).is_empty()
                && unit.run.alt_drawings.is_empty() =>
        {
            Some(&text.text)
        }
        _ => None,
    }
}

/// Count word starts while carrying state across adjacent attributed units.
/// Character fragments from one word must not count as separate words.
fn text_words(text: Option<&str>, in_word: &mut bool) -> usize {
    let mut words = 0;
    if let Some(text) = text {
        for character in text.chars() {
            let word = character.is_alphanumeric() || character == '_';
            words += usize::from(word && !*in_word);
            *in_word = word;
        }
    }
    words
}

/// The word and character alignment with each stretch of changes written as
/// all its deleted units, then all its inserted units, as Word shows a
/// rewritten passage, and the segment of each pair.
///
/// A stretch is consecutive changed text units. It grows over an unchanged
/// stretch between two changes when that stretch is only whitespace, then
/// over one that has fewer words than the deletion and the insertion on each
/// side of it, in the spirit of diff-match-patch `diff_cleanupSemantic`, so a
/// one-word anchor between one-word replacements stays. Anything but plain
/// text ends a stretch: a field, a drawing, a preserved child, an ignored
/// unit, an unchanged unit whose formatting changed, and a shell segment
/// boundary, where a hyperlink, an inline control, a bookmark or a comment
/// range sits. Run granularity keeps its alignment.
pub(super) fn grouped_text_alignment(
    aligned: &[(Option<usize>, Option<usize>)],
    segments: &[usize],
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    original_signatures: &[String],
    edited_signatures: &[String],
    options: &ComparisonOptions,
) -> (Alignment, Vec<usize>) {
    /// Consecutive pairs of one kind in one segment: `changed` is `None`
    /// for a pair grouping must not move.
    struct Block {
        pairs: Range<usize>,
        changed: Option<bool>,
        deleted_words: usize,
        inserted_words: usize,
        whitespace: bool,
        deleted_in_word: bool,
        inserted_in_word: bool,
        original_owner: Option<usize>,
        edited_owner: Option<usize>,
    }
    if options.granularity == ComparisonGranularity::Run {
        return (aligned.to_vec(), segments.to_vec());
    }
    let same_owner_formatting =
        |left: Option<usize>, right: Option<usize>, units: &[AttributedRunUnit]| {
            left.zip(right).is_none_or(|(left, right)| {
                units[left].owner == units[right].owner
                    || run_formatting(&units[left].run) == run_formatting(&units[right].run)
            })
        };
    let mut blocks = Vec::<Block>::new();
    for (index, &(left, right)) in aligned.iter().enumerate() {
        let left_text = left.map(|index| unit_plain_text(&original_units[index]));
        let right_text = right.map(|index| unit_plain_text(&edited_units[index]));
        let plain = left_text.is_none_or(|text| text.is_some())
            && right_text.is_none_or(|text| text.is_some());
        let (left_text, right_text) = (left_text.flatten(), right_text.flatten());
        let same_formatting = match (left, right) {
            (Some(i), Some(j)) => {
                run_formatting(&original_units[i].run) == run_formatting(&edited_units[j].run)
            }
            _ => true,
        };
        let changed = match granular_action(
            (left, right),
            original_units,
            edited_units,
            original_signatures,
            edited_signatures,
        ) {
            // A replacement that ignores formatting writes its insertion in
            // the formatting of its deletion, which an insertion of its own
            // would not.
            GranularAction::Replace if plain && (same_formatting || !options.ignore_formatting) => {
                Some(true)
            }
            GranularAction::Delete | GranularAction::Insert if plain => Some(true),
            GranularAction::Equal if plain && same_formatting => Some(false),
            _ => None,
        };
        let inserted_text = right_text.filter(|_| changed == Some(true));
        let whitespace = left_text.is_some_and(|text| text.trim().is_empty());
        match blocks.last_mut() {
            Some(block)
                if changed.is_some()
                    && block.changed == changed
                    && segments[block.pairs.start] == segments[index]
                    && same_owner_formatting(block.original_owner, left, original_units)
                    && same_owner_formatting(block.edited_owner, right, edited_units) =>
            {
                block.pairs.end = index + 1;
                block.deleted_words += text_words(left_text, &mut block.deleted_in_word);
                block.inserted_words += text_words(inserted_text, &mut block.inserted_in_word);
                block.whitespace &= whitespace;
                block.original_owner = left.or(block.original_owner);
                block.edited_owner = right.or(block.edited_owner);
            }
            _ => {
                let mut deleted_in_word = false;
                let mut inserted_in_word = false;
                blocks.push(Block {
                    pairs: index..index + 1,
                    changed,
                    deleted_words: text_words(left_text, &mut deleted_in_word),
                    inserted_words: text_words(inserted_text, &mut inserted_in_word),
                    whitespace,
                    deleted_in_word,
                    inserted_in_word,
                    original_owner: left,
                    edited_owner: right,
                });
            }
        }
    }
    let between_changes = |block: usize| {
        let segment = segments[blocks[block].pairs.start];
        block > 0
            && block + 1 < blocks.len()
            && blocks[block].changed == Some(false)
            && [&blocks[block - 1], &blocks[block + 1]].iter().all(|side| {
                side.changed == Some(true)
                    && segments[side.pairs.start] == segment
                    && same_owner_formatting(
                        side.original_owner,
                        blocks[block].original_owner,
                        original_units,
                    )
                    && same_owner_formatting(
                        side.edited_owner,
                        blocks[block].edited_owner,
                        edited_units,
                    )
            })
    };
    let mut folded = (0..blocks.len())
        .map(|block| between_changes(block) && blocks[block].whitespace)
        .collect::<Vec<_>>();
    // The words deleted and inserted by the change, grown over whitespace,
    // that holds each change block.
    let mut stretch = vec![0usize; blocks.len()];
    let mut stretch_words = vec![(0usize, 0usize); blocks.len()];
    for block in 0..blocks.len() {
        if blocks[block].changed == Some(true) {
            stretch[block] = if block >= 2 && folded[block - 1] {
                stretch[block - 2]
            } else {
                block
            };
            stretch_words[stretch[block]].0 += blocks[block].deleted_words;
            stretch_words[stretch[block]].1 += blocks[block].inserted_words;
        }
    }
    for block in 0..blocks.len() {
        if between_changes(block) && !folded[block] {
            let (before, after) = (
                stretch_words[stretch[block - 1]],
                stretch_words[stretch[block + 1]],
            );
            folded[block] =
                blocks[block].deleted_words < before.0.min(before.1).min(after.0).min(after.1);
        }
    }
    let mut grouped = Vec::with_capacity(aligned.len());
    let mut grouped_segments = Vec::with_capacity(aligned.len());
    let mut block = 0;
    while block < blocks.len() {
        let mut end = block + 1;
        if blocks[block].changed == Some(true) {
            while end + 1 < blocks.len() && folded[end] {
                end += 2;
            }
        }
        let pairs = blocks[block].pairs.start..blocks[end - 1].pairs.end;
        if blocks[block].changed == Some(true) && pairs.len() > 1 {
            let deleted = pairs.clone().filter_map(|index| aligned[index].0);
            let inserted = pairs.clone().filter_map(|index| aligned[index].1);
            grouped.extend(deleted.map(|index| (Some(index), None)));
            grouped.extend(inserted.map(|index| (None, Some(index))));
            grouped_segments.resize(grouped.len(), segments[pairs.start]);
        } else {
            grouped.extend_from_slice(&aligned[pairs.clone()]);
            grouped_segments.extend_from_slice(&segments[pairs]);
        }
        block = end;
    }
    (grouped, grouped_segments)
}

/// The units at `indices`, all fragments of one run of `runs`, as one run.
///
/// Consecutive fragments of one text element are written back as one text
/// with one source selection, so a merged run holds one `w:t` or `w:delText`
/// per stretch of its source text, never one per compared unit.
pub(super) fn merge_unit_runs(
    runs: &[CT_R],
    units: &[AttributedRunUnit],
    indices: &[usize],
    raw_sources: &mut Vec<(usize, usize, usize)>,
    typed_sources: &mut Vec<Option<TypedSourceSelection>>,
) -> Result<Option<CT_R>> {
    let Some(first) = indices.first().and_then(|index| units.get(*index)) else {
        return Ok(None);
    };
    let mut merged = first.run.clone();
    raw_sources.extend_from_slice(&first.raw_sources);
    typed_sources.extend_from_slice(&first.typed_sources);
    let property_boundary = usize::from(merged.properties.is_some());
    let mut joined = false;
    for &index in &indices[1..] {
        let unit = &units[index];
        if unit.owner != first.owner {
            return Err(Error::Other(
                "comparison merged different physical run owners".into(),
            ));
        }
        let next = &unit.run;
        let continued = join_text_fragment(&mut merged, typed_sources, unit, property_boundary);
        joined |= continued;
        // The raw children of a joined fragment follow the text it extends.
        let content_offset = merged.content.len() - usize::from(continued);
        if !continued {
            merged.content.extend(next.content.iter().cloned());
            typed_sources.extend_from_slice(&unit.typed_sources);
        }
        merged
            .alt_drawings
            .extend(next.alt_drawings.iter().cloned());
        for (local, (raw, &encoded)) in next
            .extra_xml
            .iter()
            .zip(&next.extra_xml_positions)
            .enumerate()
        {
            if CT_R::raw_child_is_root_attributes(encoded) {
                continue;
            }
            if let Some(&(_, ordinal, original_boundary)) =
                unit.raw_sources.iter().find(|(at, _, _)| *at == local)
            {
                raw_sources.push((merged.extra_xml.len(), ordinal, original_boundary));
            }
            let boundary = CT_R::raw_child_position(encoded);
            let mut rebased = encoded;
            CT_R::set_raw_child_position(&mut rebased, boundary + content_offset);
            merged.extra_xml.push(raw.clone());
            merged.extra_xml_positions.push(rebased);
        }
        if property_boundary == 0 && next.properties.is_some() {
            merged.properties = next.properties.clone();
        }
    }
    if raw_sources.windows(2).any(|pair| pair[0].1 >= pair[1].1) {
        return Err(Error::Other(
            "fragment raw source order or multiplicity is invalid".into(),
        ));
    }
    if joined {
        // A joined text reads its space flag as a unit of its own would, and
        // one that is its whole source element again selects it whole.
        let source = runs
            .get(first.owner)
            .ok_or_else(|| Error::Other("merged run owner is absent".into()))?;
        for (content, selected) in merged.content.iter_mut().zip(typed_sources.iter_mut()) {
            let (
                RunContent::Text(text) | RunContent::DeletedText(text),
                Some(TypedSourceSelection {
                    ordinal,
                    scalars: scalars @ Some(_),
                }),
            ) = (content, selected)
            else {
                continue;
            };
            let Some(RunContent::Text(actual) | RunContent::DeletedText(actual)) =
                source.content.get(*ordinal)
            else {
                return Err(Error::Other(
                    "joined text has no actual source element".into(),
                ));
            };
            text.preserve_space = actual.preserve_space || has_edge_whitespace(&text.text);
            if *scalars == Some((0, actual.text.chars().count())) {
                *scalars = None;
            }
        }
    }
    Ok(Some(merged))
}

/// Append `unit` to the text that ends `merged` when it holds the next scalars
/// of the same source text element with no raw child between the two, and
/// report whether it did. `typed` holds one selection per content of `merged`.
fn join_text_fragment(
    merged: &mut CT_R,
    typed: &mut [Option<TypedSourceSelection>],
    unit: &AttributedRunUnit,
    property_boundary: usize,
) -> bool {
    let content_end = property_boundary + merged.content.len();
    let raw_positions = |run: &CT_R| {
        run.extra_xml_positions
            .iter()
            .filter(|encoded| !CT_R::raw_child_is_root_attributes(**encoded))
            .map(|encoded| CT_R::raw_child_position(*encoded))
            .collect::<Vec<_>>()
    };
    if !unit.run.alt_drawings.is_empty()
        || raw_positions(merged)
            .iter()
            .any(|position| *position >= content_end)
        || raw_positions(&unit.run)
            .iter()
            .any(|position| *position <= property_boundary)
    {
        return false;
    }
    let (
        Some(TypedSourceSelection {
            ordinal: previous,
            scalars: Some((_, previous_end)),
        }),
        [
            Some(TypedSourceSelection {
                ordinal,
                scalars: Some((start, end)),
            }),
        ],
    ) = (
        typed.last_mut().and_then(Option::as_mut),
        unit.typed_sources.as_slice(),
    )
    else {
        return false;
    };
    if *previous != *ordinal || *previous_end != *start {
        return false;
    }
    match (merged.content.last_mut(), unit.run.content.as_slice()) {
        (Some(RunContent::Text(text)), [RunContent::Text(fragment)])
        | (Some(RunContent::DeletedText(text)), [RunContent::DeletedText(fragment)]) => {
            text.text.push_str(&fragment.text);
            *previous_end = *end;
            true
        }
        _ => false,
    }
}

/// The paragraph source with `properties` in place of its own `w:pPr`. A
/// self-closing paragraph, such as an empty one, is opened first, so the
/// properties and runs go inside it.
fn paragraph_with_properties(
    paragraph: &CT_P,
    original_source: Option<&str>,
    properties: &str,
) -> Result<String> {
    let mut source = original_source
        .map(str::to_owned)
        .map_or_else(|| paragraph_xml(paragraph), Ok)?;
    // The shell opens a self-closing start tag, which is then the whole source.
    let (opening, closing) = paragraph_shell(Some(&source))?;
    if !source.starts_with(&opening) {
        source = format!("{opening}{closing}");
    }
    let property_spans = direct_word_element_spans(&source, "pPr")?;
    match (property_spans.first(), properties.is_empty()) {
        (Some(span), false) => source.replace_range(span.clone(), properties),
        (Some(span), true) => source.replace_range(span.clone(), ""),
        (None, false) => source.insert_str(opening.len(), properties),
        (None, true) => {}
    }
    Ok(source)
}

pub(super) fn replace_paragraph_properties_and_runs(
    paragraph: &CT_P,
    original_source: Option<&str>,
    properties: &str,
    runs: &[String],
) -> Result<String> {
    let source = paragraph_with_properties(paragraph, original_source, properties)?;
    replace_paragraph_run_elements(paragraph, &source, runs)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn interleave_granular_paragraph(
    original: &CT_P,
    edited: &CT_P,
    original_source: Option<&str>,
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    aligned: &[(Option<usize>, Option<usize>)],
    segment_runs: &[usize],
    replacements: &[String],
    fragment_publications: &[Vec<LeafPublication>],
    properties: &str,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let source = paragraph_with_properties(original, original_source, properties)?;
    let spans = modeled_paragraph_run_spans(original, &source)?;
    if spans.len() != original.runs.len()
        || aligned.len() != replacements.len()
        || aligned.len() != segment_runs.len()
    {
        return Err(Error::Other(format!(
            "comparison could not correlate granular run owners at {location}"
        )));
    }
    let run_start = |run: usize| {
        spans
            .get(run)
            .map_or_else(|| paragraph_close_start(&source), |span| Ok(span.start))
    };
    let insertion_boundary = run_start(0)?;
    let mut output = source[..insertion_boundary].to_owned();
    let mut cursor = insertion_boundary;
    let mut consumed_owner = None;
    let mut original_owner_units = vec![0usize; original.runs.len()];
    let mut edited_owner_units = vec![0usize; edited.runs.len()];
    for unit in original_units {
        original_owner_units[unit.owner] += 1;
    }
    for unit in edited_units {
        edited_owner_units[unit.owner] += 1;
    }
    for (replacement_index, (((left, right), replacement), &segment_run)) in aligned
        .iter()
        .zip(replacements)
        .zip(segment_runs)
        .enumerate()
    {
        // Text inserted at the start of a shell segment comes after the
        // shell tags that open it.
        let segment_start = run_start(segment_run)?;
        if segment_start > cursor {
            output.push_str(&source[cursor..segment_start]);
            cursor = segment_start;
        }
        let mut exact_run = None;
        if let Some(unit) = left.map(|index| &original_units[index])
            && consumed_owner != Some(unit.owner)
        {
            let span = &spans[unit.owner];
            output.push_str(&source[cursor..span.start]);
            cursor = span.end;
            consumed_owner = Some(unit.owner);
            if let Some(right_owner) = right.map(|index| edited_units[index].owner)
                && !run_is_field(&original.runs[unit.owner])
                && original.runs[unit.owner] == edited.runs[right_owner]
                && original_owner_units[unit.owner] == 1
                && edited_owner_units[right_owner] == 1
            {
                exact_run = Some(&source[span.clone()]);
            }
        }
        if exact_run.is_none() {
            let mut leaves = fragment_publications[replacement_index].clone();
            shift_leaf_publications(&mut leaves, output.len() as isize)?;
            metadata.leaf_publications.extend(leaves);
        }
        output.push_str(exact_run.unwrap_or(replacement));
    }
    output.push_str(&source[cursor..]);
    compare_granular_controls(
        original,
        edited,
        &output,
        location,
        metadata,
        diagnostics,
        leaf_mark,
    )
}

/// Compare the inline controls of a paragraph whose revised runs are in
/// `output`, which still holds the original controls.
pub(super) fn compare_granular_controls(
    original: &CT_P,
    edited: &CT_P,
    output: &str,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
    leaf_mark: usize,
) -> Result<String> {
    let control_spans = direct_word_element_spans(output, "sdt")?;
    if control_spans.len() != original.content_controls.len() {
        return Err(Error::Other(format!(
            "comparison could not correlate granular content controls at {location}"
        )));
    }
    let mut child_leaves = Vec::new();
    let mut control_replacements = Vec::with_capacity(control_spans.len());
    let mut sources = source_children(output, &control_spans).into_iter();
    for (index, ((_, _, _, left), (_, _, _, right))) in original
        .content_controls
        .iter()
        .zip(&edited.content_controls)
        .enumerate()
    {
        let child_mark = metadata.leaf_publications.len();
        let source = sources.next().expect("one source per control span");
        control_replacements.push(compare_control_from_xml(
            left,
            right,
            &format!("{location}/content-control[{index}]"),
            metadata,
            diagnostics,
            &source?,
        )?);
        child_leaves.push(metadata.leaf_publications.split_off(child_mark));
    }
    splice_leaf_publications(
        metadata,
        leaf_mark,
        &control_spans,
        &control_replacements,
        child_leaves,
    )?;
    replace_direct_word_elements(output, "sdt", &control_replacements)
}
