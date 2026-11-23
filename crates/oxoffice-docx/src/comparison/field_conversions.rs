//! Simple and complex field conversions: the role of each converted source.

use super::*;

/// Check each guard role against freshly read input occurrences and the actual
/// allocated publication, then consume only the independently verified encoding
/// exception at its actual resolved occurrence.
#[allow(clippy::too_many_arguments)]
pub(super) fn validate_field_conversion_role(
    candidate: &Document,
    actual: &mut NormalizedPackage,
    wanted: &NormalizedPackage,
    stories: &[StoryPart],
    witnesses: &[FieldConversionWitness],
    whole: &[WholeFieldWitness],
    role: usize,
    new_ids: &HashSet<i32>,
    restorations: &HyperlinkRestorations,
) -> Result<()> {
    /// One input's side of a conversion: the field's occurrence there, and
    /// the id and payload of the one wrapper that publishes it.
    struct Side<'w> {
        source: &'w FieldSourceOccurrence,
        revision_id: i32,
        generated_span: &'w str,
        input_complex: bool,
    }
    fn observations<'a>(
        items: &'a [GuardItem],
        output: &mut Vec<(
            &'a GuardFieldObservation,
            &'a GuardField,
            &'a ContentMetadata,
        )>,
    ) {
        for item in items {
            for observation in &item.field_observations {
                let mut field = item.fields.get(observation.field_index);
                for slot in &observation.result_path {
                    field = field.and_then(|field| field.result_fields.get(*slot));
                }
                if let Some(field) = field {
                    output.push((observation, field, &item.content_metadata));
                }
            }
            for host in &item.hosts {
                for body in &host.bodies {
                    observations(body, output);
                }
            }
        }
    }
    let mut actual_inputs = Vec::new();
    observations(&wanted.0, &mut actual_inputs);
    for (_, _, items) in &wanted.1 {
        observations(items, &mut actual_inputs);
    }
    let mut sides = Vec::with_capacity(witnesses.len() + whole.len());
    for witness in witnesses {
        let forms = [
            actual_field_is_complex(&witness.sources[0].source, &witness.sources[0].scope)?,
            actual_field_is_complex(&witness.sources[1].source, &witness.sources[1].scope)?,
        ];
        if forms != witness.input_complex || witness.form_changed != (forms[0] != forms[1]) {
            return Err(Error::Other(
                "field conversion reason differs from actual qualified input forms".into(),
            ));
        }
        if !forms[0] && !forms[1] {
            let instructions = [
                conversion_source_inventory(&witness.sources[0].source, &witness.sources[0].scope)?
                    .0,
                conversion_source_inventory(&witness.sources[1].source, &witness.sources[1].scope)?
                    .0,
            ];
            let left = oxoffice_docx_oxml::text::Field::new(&instructions[0], "");
            let right = oxoffice_docx_oxml::text::Field::new(&instructions[1], "");
            if witness.instruction_changed
                == comparison_instruction_eq(&left.instruction, &right.instruction)
            {
                return Err(Error::Other("field conversion instruction reason differs from independently read source instructions".into()));
            }
        }
        if !(witness.instruction_changed || witness.form_changed)
            || witness.revision_ids[0] == witness.revision_ids[1]
            || witness.revision_ids.iter().any(|id| !new_ids.contains(id))
        {
            return Err(Error::Other(
                "invalid or multiply consumed field conversion witness".into(),
            ));
        }
        sides.push(Side {
            source: &witness.sources[role],
            revision_id: witness.revision_ids[role],
            generated_span: &witness.generated_spans[role],
            input_complex: witness.input_complex[role],
        });
    }
    // A field written with its whole paragraph is checked by the resolution
    // that keeps the paragraph; the other one removes it with the paragraph.
    for witness in whole {
        if witness.source.input_role != role {
            continue;
        }
        if !new_ids.contains(&witness.revision_id)
            || actual_field_is_complex(&witness.source.source, &witness.source.scope)?
        {
            return Err(Error::Other(
                "invalid whole-paragraph field conversion witness".into(),
            ));
        }
        sides.push(Side {
            source: &witness.source,
            revision_id: witness.revision_id,
            generated_span: &witness.generated_span,
            input_complex: false,
        });
    }
    let mut consumed = HashSet::new();
    let mut consumed_occurrences = HashSet::new();
    for side in &sides {
        let source = side.source;
        if source.input_role != role
            || source.bounds.start >= source.bounds.end
            || source.bounds.len() != source.source.len()
            || !consumed.insert(side.revision_id)
            || !consumed_occurrences.insert((
                source.story.clone(),
                source.owner_path.clone(),
                source.field_slot,
                source.bounds.clone(),
                source.result_ancestors.clone(),
            ))
        {
            return Err(Error::Other(
                "invalid or multiply consumed field conversion witness".into(),
            ));
        }
        let matches = actual_inputs
            .iter()
            .filter(|(observation, _, _)| {
                let actual = &observation.occurrence;
                actual.story == source.story
                    && actual.owner_path == source.owner_path
                    && actual.field_slot == source.field_slot
                    && actual.result_ancestors == source.result_ancestors
                    && actual.bounds == source.bounds
                    && actual.source == source.source
                    && actual.scope == source.scope
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 || matches[0].1.complex != side.input_complex {
            return Err(Error::Other(
                "field conversion witness has no unique actual input occurrence".into(),
            ));
        }
        let part = if source.story == "body" {
            candidate.doc_part_name.as_str()
        } else {
            stories
                .iter()
                .find(|story| {
                    let location = format!("{}:{}", story.kind.label(), story.part_name);
                    source.story == location || source.story.starts_with(&format!("{location}/"))
                })
                .map(|story| story.part_name.as_str())
                .ok_or_else(|| {
                    Error::Other("field conversion has no proven physical story part".into())
                })?
        };
        let (transported, ranges) = transported_publication(
            candidate,
            part,
            side.revision_id,
            side.generated_span,
            role,
            new_ids,
            restorations,
        )?;
        let extent = ranges
            .first()
            .zip(ranges.last())
            .map(|(first, last)| first.start..last.end)
            .ok_or_else(|| {
                Error::Other("selected publication has no resolved root extent".into())
            })?;
        let mut current_observations = Vec::new();
        observations(&actual.0, &mut current_observations);
        for (_, _, items) in &actual.1 {
            observations(items, &mut current_observations);
        }
        let current = current_observations
            .iter()
            .filter(|(observation, _, _)| {
                let actual = &observation.occurrence;
                actual.story == source.story
                    && actual.owner_path == source.owner_path
                    && actual.field_slot == source.field_slot
            })
            .collect::<Vec<_>>();
        if current.len() != 1 {
            return Err(Error::Other(
                "publication has no unique independently observed resolved logical Field target"
                    .into(),
            ));
        }
        let current = &current[0].0.occurrence;
        if current.part_span.as_ref() != Some(&extent)
            || current.part_hash != Some(Sha256::digest(&transported).into())
            || transported.get(extent.clone()) != Some(current.source.as_bytes())
        {
            return Err(Error::Other("publication transport does not reach the actual resolved Field's complete part extent".into()));
        }
        if !side.input_complex {
            let (observation, field, metadata) = *matches[0];
            let expected_record = observation
                .metadata_record
                .2
                .map(|index| {
                    metadata
                        .get("field")
                        .and_then(|records| records.get(index))
                        .cloned()
                        .ok_or_else(|| {
                            Error::Other("input field metadata record is missing".into())
                        })
                })
                .transpose()?;
            let mut count =
                map_field_conversion_role(&mut actual.0, source, field, expected_record.as_ref())?;
            for (_, _, items) in &mut actual.1 {
                count += map_field_conversion_role(items, source, field, expected_record.as_ref())?;
            }
            if count != 1 {
                return Err(Error::Other(
                    "resolved conversion lacks one actual current Field occurrence".into(),
                ));
            }
        }
    }
    let converted = sides
        .iter()
        .filter(|side| !side.input_complex)
        .map(|side| side.source)
        .collect::<Vec<_>>();
    map_nested_conversion_metadata(&mut actual.0, &wanted.0, &converted)?;
    for ((actual_kind, actual_part, actual_items), (wanted_kind, wanted_part, wanted_items)) in
        actual.1.iter_mut().zip(&wanted.1)
    {
        if actual_kind != wanted_kind || actual_part != wanted_part {
            return Err(Error::Other(
                "nested conversion changed physical story inventory".into(),
            ));
        }
        map_nested_conversion_metadata(actual_items, wanted_items, &converted)?;
    }
    Ok(())
}

/// Consume one proven encoding exception at an actual resolved occurrence.
/// Every other semantic and owning-source record keeps normal guard equality.
fn map_field_conversion_role(
    items: &mut [GuardItem],
    input: &FieldSourceOccurrence,
    expected: &GuardField,
    expected_record: Option<&(usize, usize, Vec<String>)>,
) -> Result<usize> {
    let mut count = 0usize;
    for item in items {
        for observation in item.field_observations.clone() {
            let current = &observation.occurrence;
            if current.story != input.story
                || current.owner_path != input.owner_path
                || current.field_slot != input.field_slot
            {
                continue;
            }
            let mut target = item.fields.get_mut(observation.field_index);
            for slot in &observation.result_path {
                target = target.and_then(|field| field.result_fields.get_mut(*slot));
            }
            let field = target.ok_or_else(|| {
                Error::Other("resolved conversion has no actual nested Field target".into())
            })?;
            if !field.complex || expected.complex {
                return Err(Error::Other(
                    "conversion exception does not describe actual simple-to-complex encoding"
                        .into(),
                ));
            }
            let mut semantic = field.clone();
            encoded_as(&mut semantic, expected);
            if semantic != *expected
                || conversion_source_inventory(&input.source, &input.scope)?
                    != conversion_source_inventory(&current.source, &current.scope)?
            {
                return Err(Error::Other(
                    "resolved field conversion changed independent semantic or source facts".into(),
                ));
            }
            encoded_as(field, expected);
            map_field_metadata_record(item, &observation, expected_record)?;
            count += 1;
        }
        for host in &mut item.hosts {
            for body in &mut host.bodies {
                count += map_field_conversion_role(body, input, expected, expected_record)?;
            }
        }
    }
    Ok(count)
}

/// The converter writes a simple field and every simple field nested in its
/// result as complex fields: give each the form the input field has.
fn encoded_as(actual: &mut GuardField, expected: &GuardField) {
    actual.complex &= expected.complex;
    for (actual, expected) in actual.result_fields.iter_mut().zip(&expected.result_fields) {
        encoded_as(actual, expected);
    }
}

fn map_field_metadata_record(
    item: &mut GuardItem,
    observation: &GuardFieldObservation,
    expected_record: Option<&(usize, usize, Vec<String>)>,
) -> Result<()> {
    let (start, width, index) = observation.metadata_record;
    if let Some((expected_start, expected_width, _)) = expected_record
        && (start != *expected_start || width != *expected_width)
    {
        return Err(Error::Other(
            "converted field owning metadata coverage changed".into(),
        ));
    }
    match (index, expected_record) {
        (Some(index), Some(record)) => {
            let actual = item
                .content_metadata
                .get_mut("field")
                .and_then(|records| records.get_mut(index))
                .ok_or_else(|| Error::Other("resolved field metadata record is missing".into()))?;
            if actual.0 != start || actual.1 != width {
                return Err(Error::Other(
                    "resolved field source record lost its actual owner".into(),
                ));
            }
            *actual = record.clone();
        }
        (Some(index), None) => {
            let records = item.content_metadata.get_mut("field").ok_or_else(|| {
                Error::Other("resolved field metadata inventory is missing".into())
            })?;
            if records
                .get(index)
                .is_none_or(|record| record.0 != start || record.1 != width)
            {
                return Err(Error::Other(
                    "resolved field metadata has a different owner".into(),
                ));
            }
            records.remove(index);
            if records.is_empty() {
                item.content_metadata.remove("field");
            }
            for other in &mut item.field_observations {
                if let Some(record) = &mut other.metadata_record.2 {
                    if *record == index {
                        other.metadata_record.2 = None;
                    } else if *record > index {
                        *record -= 1;
                    }
                }
            }
        }
        (None, Some(record)) => {
            let records = item.content_metadata.entry("field".into()).or_default();
            let index = records.partition_point(|actual| actual.0 < start);
            records.insert(index, record.clone());
            for other in &mut item.field_observations {
                if let Some(record) = &mut other.metadata_record.2
                    && *record >= index
                {
                    *record += 1;
                }
            }
        }
        (None, None) => {}
    }
    Ok(())
}

/// Verify every delegated source span before mapping a coarse ancestor record.
fn map_nested_conversion_metadata(
    actual: &mut [GuardItem],
    wanted: &[GuardItem],
    converted: &[&FieldSourceOccurrence],
) -> Result<()> {
    fn same_occurrence(left: &FieldSourceOccurrence, right: &FieldSourceOccurrence) -> bool {
        left.story == right.story
            && left.owner_path == right.owner_path
            && left.field_slot == right.field_slot
    }
    fn observation<'a>(
        item: &'a GuardItem,
        root: usize,
        path: &[usize],
    ) -> Result<&'a GuardFieldObservation> {
        let matches = item
            .field_observations
            .iter()
            .filter(|observation| {
                observation.field_index == root && observation.result_path == path
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(Error::Other(
                "nested source budget lacks one actual semantic target".into(),
            ));
        }
        Ok(matches[0])
    }
    fn tree(
        actual: &GuardItem,
        wanted: &GuardItem,
        roots: [usize; 2],
        path: &mut Vec<usize>,
        claims: &[Vec<usize>],
        consumed: &mut HashSet<Vec<usize>>,
    ) -> Result<()> {
        let input = observation(wanted, roots[0], path)?;
        let current = observation(actual, roots[1], path)?;
        if !same_occurrence(&input.occurrence, &current.occurrence) {
            return Err(Error::Other(
                "nested source budget changed physical owner or logical slot".into(),
            ));
        }
        if claims.contains(path) {
            if !consumed.insert(path.clone()) {
                return Err(Error::Other(
                    "nested source budget consumed a child twice".into(),
                ));
            }
            // The role-local child checker already consumed its complete carrier,
            // result, semantic facts and allocated publication association.
            return Ok(());
        }
        if !claims.iter().any(|claim| claim.starts_with(path)) {
            if field_source_sequence_budget(&input.occurrence.source, &input.occurrence.scope)?
                != field_source_sequence_budget(
                    &current.occurrence.source,
                    &current.occurrence.scope,
                )?
            {
                return Err(Error::Other(
                    "unclaimed nested source changed its strict opaque facts".into(),
                ));
            }
            return Ok(());
        }
        if input.occurrence.carrier_budget.is_none()
            || input.occurrence.carrier_budget != current.occurrence.carrier_budget
            || input.occurrence.result_budget.is_none()
            || input.occurrence.result_budget != current.occurrence.result_budget
        {
            return Err(Error::Other("nested conversion changed parent shell, typed hole map, context or outside source budget".into()));
        }
        let mut input_field = &wanted.fields[roots[0]];
        let mut current_field = &actual.fields[roots[1]];
        for slot in path.iter() {
            input_field = input_field
                .result_fields
                .get(*slot)
                .ok_or_else(|| Error::Other("input nested source target disappeared".into()))?;
            current_field = current_field
                .result_fields
                .get(*slot)
                .ok_or_else(|| Error::Other("actual nested source target disappeared".into()))?;
        }
        if input_field != current_field {
            return Err(Error::Other(
                "nested source exception changed independent parent semantic facts".into(),
            ));
        }
        for slot in 0..input_field.result_fields.len() {
            path.push(slot);
            tree(actual, wanted, roots, path, claims, consumed)?;
            path.pop();
        }
        Ok(())
    }
    for actual_item in actual {
        let roots = actual_item
            .field_observations
            .iter()
            .filter(|observation| observation.result_path.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        for current_root in roots {
            let mut candidates = Vec::new();
            for input_item in wanted {
                for input_root in input_item
                    .field_observations
                    .iter()
                    .filter(|observation| observation.result_path.is_empty())
                {
                    if same_occurrence(&input_root.occurrence, &current_root.occurrence) {
                        candidates.push((input_item, input_root));
                    }
                }
            }
            let relevant = actual_item
                .field_observations
                .iter()
                .filter(|observation| {
                    observation.field_index == current_root.field_index
                        && !observation.result_path.is_empty()
                })
                .filter(|observation| {
                    converted
                        .iter()
                        .any(|source| same_occurrence(source, &observation.occurrence))
                })
                .map(|observation| observation.result_path.clone())
                .collect::<Vec<_>>();
            if relevant.is_empty() {
                continue;
            }
            if candidates.len() != 1 {
                return Err(Error::Other(
                    "nested parent source budget has no unique actual input root".into(),
                ));
            }
            let (input_item, input_root) = candidates[0];
            let mut claims = relevant;
            claims.sort();
            if claims.windows(2).any(|pair| pair[1].starts_with(&pair[0])) {
                return Err(Error::Other(
                    "nested conversion child claims overlap or duplicate".into(),
                ));
            }
            let mut consumed = HashSet::new();
            tree(
                actual_item,
                input_item,
                [input_root.field_index, current_root.field_index],
                &mut Vec::new(),
                &claims,
                &mut consumed,
            )?;
            if consumed.len() != claims.len() {
                return Err(Error::Other(
                    "nested source budget left an unconsumed child claim".into(),
                ));
            }
            let expected = input_root
                .metadata_record
                .2
                .map(|index| {
                    input_item
                        .content_metadata
                        .get("field")
                        .and_then(|records| records.get(index))
                        .cloned()
                        .ok_or_else(|| {
                            Error::Other("nested input parent metadata record is missing".into())
                        })
                })
                .transpose()?;
            map_field_metadata_record(actual_item, &current_root, expected.as_ref())?;
        }
        // Host bodies retain their own independently captured physical inventory.
        for (host_index, host) in actual_item.hosts.iter_mut().enumerate() {
            let wanted_host = wanted.iter().flat_map(|item| &item.hosts).nth(host_index);
            if let Some(wanted_host) = wanted_host {
                for (body, input) in host.bodies.iter_mut().zip(&wanted_host.bodies) {
                    map_nested_conversion_metadata(body, input, converted)?;
                }
            }
        }
    }
    Ok(())
}

/// The part `part` of `candidate` once `role`'s resolution of `new_ids` is
/// applied, and the range there of each direct child of the one wrapper
/// `revision_id` that publishes `generated_span`, in order and with nothing but
/// white space between them. `restorations` are the edits made since in the
/// resolved part (see [`restore_whole_hyperlinks`]): they apply to the part
/// and move the ranges, which none of them may cross.
#[allow(clippy::too_many_arguments)]
fn transported_publication(
    candidate: &Document,
    part: &str,
    revision_id: i32,
    generated_span: &str,
    role: usize,
    new_ids: &HashSet<i32>,
    restorations: &HyperlinkRestorations,
) -> Result<(Vec<u8>, Vec<Range<usize>>)> {
    let roots = publication_roots(candidate, part, revision_id, generated_span, role)?;
    let resolution = if role == 0 {
        crate::revision::Resolution::Reject
    } else {
        crate::revision::Resolution::Accept
    };
    let (transported, mapped) = if is_note_part(candidate, part)? {
        let mut trace = crate::revision::NotePublication::new(part, revision_id, &roots);
        let mut carrier = candidate.clone_for_staging();
        carrier.resolve_revisions_with_note_publication(
            resolution,
            crate::revision::RevisionScope::Ids(new_ids),
            Some(&mut trace),
        )?;
        (
            trace
                .output
                .ok_or_else(|| Error::Other("selected publication note part was removed".into()))?,
            trace.roots,
        )
    } else {
        let (transported, _, mapped) = crate::revision::resolve_story_xml_with_publication_roots(
            publication_part(candidate, part)?,
            resolution,
            crate::revision::RevisionScope::Ids(new_ids),
            revision_id,
            &roots,
        )?;
        (transported, mapped)
    };
    let ranges = resolved_publication_roots(mapped, &transported)?;
    let Some(edits) = restorations.get(part) else {
        return Ok((transported, ranges));
    };
    let ranges = ranges
        .into_iter()
        .map(|range| restored_range(edits, range))
        .collect::<Result<Vec<_>>>()?;
    Ok((restored_part(&transported, edits), ranges))
}

/// Whether `part` is a footnote or endnote part of `candidate`, which only
/// the resolution of the whole document resolves.
fn is_note_part(candidate: &Document, part: &str) -> Result<bool> {
    let mut note_part = false;
    for kind in NoteKind::ALL {
        note_part |= NotePart::name_of(candidate, kind)?.is_some_and(|name| name == part);
    }
    Ok(note_part)
}

/// The part `part` of `candidate`.
fn publication_part<'d>(candidate: &'d Document, part: &str) -> Result<&'d [u8]> {
    candidate
        .package
        .get_part(part)
        .ok_or_else(|| Error::Other("field conversion part is missing".into()))
}

/// The ranges in `part` of the direct children of the one wrapper
/// `revision_id` that publishes `generated_span` for `role`.
fn publication_roots(
    candidate: &Document,
    part: &str,
    revision_id: i32,
    generated_span: &str,
    role: usize,
) -> Result<Vec<Range<usize>>> {
    let bytes = publication_part(candidate, part)?;
    let spans = crate::revision::modeled_revision_spans(bytes)?
        .into_iter()
        .filter(|(_, id)| *id == revision_id)
        .collect::<Vec<_>>();
    if spans.len() != 1 {
        return Err(Error::Other(
            "field conversion publication id has no unique physical span".into(),
        ));
    }
    let raw = std::str::from_utf8(&bytes[spans[0].0.clone()]).map_err(utf8_error)?;
    let kind = if role == 0 { "del" } else { "ins" };
    let inner = element_inner_range_any_prefix(raw, kind)?;
    if raw[inner] != *generated_span {
        return Err(Error::Other(
            "field conversion publication payload differs from its checked local span".into(),
        ));
    }
    direct_element_spans(raw)?
        .into_iter()
        .map(|range| {
            range
                .start
                .checked_add(spans[0].0.start)
                .zip(range.end.checked_add(spans[0].0.start))
                .map(|(start, end)| start..end)
                .ok_or_else(|| Error::Other("publication root part offset overflow".into()))
        })
        .collect()
}

/// The resolved ranges of a publication's roots, each kept, in order and with
/// nothing but white space between them.
fn resolved_publication_roots(
    mapped: Vec<Option<Range<usize>>>,
    transported: &[u8],
) -> Result<Vec<Range<usize>>> {
    let mut ranges = Vec::with_capacity(mapped.len());
    let mut previous = None;
    for range in mapped {
        let range = range.ok_or_else(|| {
            Error::Other("a selected publication root was discarded or unproved".into())
        })?;
        if range.start >= range.end
            || range.end > transported.len()
            || previous.is_some_and(|end| {
                end > range.start
                    || !transported[end..range.start]
                        .iter()
                        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
            })
        {
            return Err(Error::Other(
                "resolved publication root order or intervening source coverage changed".into(),
            ));
        }
        previous = Some(range.end);
        ranges.push(range);
    }
    Ok(ranges)
}

/// The edits, by part and in order, that [`restore_whole_hyperlinks`] made in
/// a resolved document.
pub(super) type HyperlinkRestorations = HashMap<String, Vec<(Range<usize>, String)>>;

/// Write back as a `w:hyperlink` each hyperlink field that `role`'s resolution
/// keeps of a paragraph inserted or deleted whole (see
/// `whole_paragraph_hyperlinks`), as Word does when it saves the resolved
/// document, so the guard compares the link with the input's.
///
/// The link is read from the resolved field alone: its address, anchor,
/// tooltip and target frame from the instruction, its content from the
/// result, which stays in place. Only the order of the input's attributes
/// and its `w:history` value, which no switch carries, come from the witness.
/// An address gets a new relationship in the resolved document, which the
/// guard reads by its target.
pub(super) fn restore_whole_hyperlinks(
    candidate: &Document,
    resolved: &mut Document,
    witnesses: &[WholeHyperlinkWitness],
    role: usize,
    new_ids: &HashSet<i32>,
) -> Result<HyperlinkRestorations> {
    let witnesses = witnesses
        .iter()
        .filter(|witness| witness.input_role == role)
        .collect::<Vec<_>>();
    if witnesses
        .iter()
        .any(|witness| !new_ids.contains(&witness.revision_id))
    {
        return Err(Error::Other(
            "invalid whole-paragraph hyperlink witness".into(),
        ));
    }
    // A contents rebuilt whole holds a link per entry: each story part is
    // resolved once for all of its links. Only the resolution of the whole
    // document resolves a note part, once per link.
    let mut transports = HashMap::new();
    let mut by_part = BTreeMap::<&str, Vec<&WholeHyperlinkWitness>>::new();
    for witness in &witnesses {
        by_part.entry(&witness.owner).or_default().push(witness);
    }
    for (part, witnesses) in by_part {
        if is_note_part(candidate, part)? {
            for witness in witnesses {
                let (transported, roots) = transported_publication(
                    candidate,
                    part,
                    witness.revision_id,
                    &witness.generated_span,
                    role,
                    new_ids,
                    &HyperlinkRestorations::new(),
                )?;
                transports.insert(witness.revision_id, (std::rc::Rc::new(transported), roots));
            }
            continue;
        }
        let publications = witnesses
            .iter()
            .map(|witness| {
                Ok((
                    witness.revision_id,
                    publication_roots(
                        candidate,
                        part,
                        witness.revision_id,
                        &witness.generated_span,
                        role,
                    )?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let (transported, mapped) = crate::revision::resolve_story_xml_with_publications(
            publication_part(candidate, part)?,
            if role == 0 {
                crate::revision::Resolution::Reject
            } else {
                crate::revision::Resolution::Accept
            },
            crate::revision::RevisionScope::Ids(new_ids),
            &publications,
        )?;
        let transported = std::rc::Rc::new(transported);
        for ((revision_id, _), mapped) in publications.into_iter().zip(mapped) {
            let roots = resolved_publication_roots(mapped, &transported)?;
            transports.insert(revision_id, (transported.clone(), roots));
        }
    }
    let mut restorations = HyperlinkRestorations::new();
    let mut resolved_edits = HyperlinkRestorations::new();
    let mut relationships = 0usize;
    let mut begins = HashMap::new();
    for witness in witnesses {
        let (transported, roots) = &transports[&witness.revision_id];
        let transported = transported.as_slice();
        let resolved_part = resolved
            .package
            .get_part(&witness.owner)
            .ok_or_else(|| Error::Other("whole-paragraph hyperlink part is missing".into()))?;
        let [begin, instruction, separate, .., end] = roots.as_slice() else {
            return Err(Error::Other(
                "a whole-paragraph hyperlink publication is not one field".into(),
            ));
        };
        // The resolved document also retires the comparison's note markers,
        // which the resolution of this part alone keeps: the field is the
        // same occurrence of its bytes in both.
        // Every link field begins with the same generated run: its places
        // are found once per part.
        let extent = &transported[begin.start..end.end];
        let opening_run = &transported[begin.clone()];
        let places = begins
            .entry((witness.owner.as_str(), opening_run.to_vec()))
            .or_insert_with(|| {
                (
                    occurrences(transported, opening_run),
                    occurrences(resolved_part, opening_run),
                )
            });
        let matching = |part: &[u8], starts: &[usize]| {
            starts
                .iter()
                .copied()
                .filter(|start| part[*start..].starts_with(extent))
                .collect::<Vec<_>>()
        };
        let before = matching(transported, &places.0);
        let after = matching(resolved_part, &places.1);
        let rank = before.iter().position(|start| *start == begin.start);
        let Some(start) = rank
            .filter(|_| before.len() == after.len())
            .map(|rank| after[rank])
        else {
            return Err(Error::Other(
                "a whole-paragraph hyperlink publication does not reach the resolved part".into(),
            ));
        };
        let text = |range: &Range<usize>| {
            std::str::from_utf8(&transported[range.clone()]).map_err(utf8_error)
        };
        for (range, kind) in [(begin, "begin"), (separate, "separate"), (end, "end")] {
            if field_char_run(text(range)?)? != Some(kind.to_owned()) {
                return Err(Error::Other(format!(
                    "a whole-paragraph hyperlink publication has no field {kind} run where one field needs it"
                )));
            }
        }
        let field =
            oxoffice_docx_oxml::text::Field::new(&instruction_run_text(text(instruction)?)?, "");
        let instruction = &field.instruction;
        let mut values = HashMap::new();
        for switch in &instruction.switches {
            let Some(oxoffice_docx_oxml::text::FieldArgument::Text(value)) = &switch.argument
            else {
                return Err(Error::Other(
                    "a whole-paragraph hyperlink switch has no value".into(),
                ));
            };
            let name = match switch.name.as_str() {
                "l" => "w:anchor",
                "o" => "w:tooltip",
                "t" => "w:tgtFrame",
                _ => {
                    return Err(Error::Other(format!(
                        "a whole-paragraph hyperlink field has switch \\{}, which no link attribute carries",
                        switch.name
                    )));
                }
            };
            if values.insert(name, value.clone()).is_some() {
                return Err(Error::Other(
                    "a whole-paragraph hyperlink switch is repeated".into(),
                ));
            }
        }
        match instruction.arguments.as_slice() {
            [] => {}
            [oxoffice_docx_oxml::text::FieldArgument::Text(address)] => {
                let rels = resolved.package.get_or_create_part_rels(&witness.owner);
                let id = loop {
                    relationships += 1;
                    let id = format!("rIdOxofficeGuardLink{relationships}");
                    if rels.get_by_id(&id).is_none() {
                        break id;
                    }
                };
                rels.items.push(Relationship {
                    id: id.clone(),
                    rel_type: rel_types::HYPERLINK.to_owned(),
                    target: address.clone(),
                    target_mode: Some("External".to_owned()),
                });
                values.insert("r:id", id);
            }
            _ => {
                return Err(Error::Other(
                    "a whole-paragraph hyperlink field has more than one address".into(),
                ));
            }
        }
        if instruction.name != "HYPERLINK" {
            return Err(Error::Other(
                "a whole-paragraph hyperlink publication is not a HYPERLINK field".into(),
            ));
        }
        let mut opening = String::from("<w:hyperlink");
        for (name, carried) in &witness.opening {
            let value = carried
                .clone()
                .or_else(|| values.remove(name.as_str()))
                .ok_or_else(|| {
                    Error::Other(format!(
                        "a whole-paragraph hyperlink field does not carry the link's {name}"
                    ))
                })?;
            opening.push_str(&format!(
                r#" {name}="{}""#,
                oxoffice_core::xml::escape_attribute_value(&value)
            ));
        }
        if !values.is_empty() {
            return Err(Error::Other(
                "a whole-paragraph hyperlink field carries what the link did not".into(),
            ));
        }
        opening.push('>');
        let moved = |range: &Range<usize>| {
            range.start - begin.start + start..range.end - begin.start + start
        };
        let edits = restorations.entry(witness.owner.clone()).or_default();
        edits.push((begin.start..separate.end, opening.clone()));
        edits.push((end.clone(), "</w:hyperlink>".to_owned()));
        let edits = resolved_edits.entry(witness.owner.clone()).or_default();
        edits.push((moved(&(begin.start..separate.end)), opening));
        edits.push((moved(end), "</w:hyperlink>".to_owned()));
    }
    for edits in restorations.values_mut() {
        edits.sort_by_key(|(range, _)| range.start);
    }
    for (part, edits) in &mut resolved_edits {
        edits.sort_by_key(|(range, _)| range.start);
        if edits.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
            return Err(Error::Other(
                "whole-paragraph hyperlink fields overlap".into(),
            ));
        }
        let bytes = resolved
            .package
            .get_part(part)
            .ok_or_else(|| Error::Other("whole-paragraph hyperlink part is missing".into()))?;
        let restored = restored_part(bytes, edits);
        crate::document::set_story_source_xml(resolved, part, restored)?;
    }
    Ok(restorations)
}

/// The only child of the run `xml`, as its Word local name, its
/// `w:fldCharType` value and its text; `None` when `xml` is not a Word run
/// with one child.
fn single_run_child(xml: &str) -> Result<Option<(String, Option<String>, String)>> {
    let mut reader = NsReader::from_str(xml);
    let mut depth = 0usize;
    let mut child = None;
    let mut children = 0usize;
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let (element, empty) = match event {
            Event::Start(element) => (element, false),
            Event::Empty(element) => (element, true),
            Event::Text(text) if depth == 2 => {
                if let Some((_, _, value)) = &mut child {
                    let text = text
                        .xml_content(XmlVersion::Implicit1_0)
                        .map_err(|error| Error::Other(error.to_string()))?;
                    String::push_str(value, &text);
                }
                continue;
            }
            Event::GeneralRef(reference) if depth == 2 => {
                if let Some((_, _, value)) = &mut child {
                    if let Some(character) = reference
                        .resolve_char_ref()
                        .map_err(|error| Error::Other(error.to_string()))?
                    {
                        value.push(character);
                    } else {
                        let name = reference
                            .decode()
                            .map_err(|error| Error::Other(error.to_string()))?;
                        value.push_str(
                            quick_xml::escape::resolve_predefined_entity(&name)
                                .ok_or_else(|| Error::Other(format!("unknown entity &{name};")))?,
                        );
                    }
                }
                continue;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        match depth {
            0 if !(word && element.local_name().as_ref() == b"r") || empty => return Ok(None),
            1 => {
                children += 1;
                let mut kind = None;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                        && local.as_ref() == b"fldCharType"
                    {
                        kind = Some(
                            attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    element.decoder(),
                                )
                                .map_err(|error| Error::Other(error.to_string()))?
                                .into_owned(),
                        );
                    }
                }
                child = word.then(|| {
                    (
                        String::from_utf8_lossy(element.local_name().as_ref()).into_owned(),
                        kind,
                        String::new(),
                    )
                });
            }
            _ => {}
        }
        if !empty {
            depth += 1;
        }
    }
    Ok(child.filter(|_| children == 1))
}

/// The `w:fldCharType` of the run `xml` when its only child is a `w:fldChar`.
fn field_char_run(xml: &str) -> Result<Option<String>> {
    Ok(single_run_child(xml)?
        .filter(|(local, _, _)| local == "fldChar")
        .and_then(|(_, kind, _)| kind))
}

/// The instruction of the run `xml` when its only child is a `w:instrText`.
fn instruction_run_text(xml: &str) -> Result<String> {
    single_run_child(xml)?
        .filter(|(local, _, _)| local == "instrText")
        .map(|(_, _, text)| text)
        .ok_or_else(|| {
            Error::Other("a whole-paragraph hyperlink field has no instruction run".into())
        })
}

/// The start of each occurrence of `needle` in `haystack`, none overlapping.
fn occurrences(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut at = 0;
    while let Some(found) = haystack.get(at..).and_then(|rest| {
        rest.windows(needle.len())
            .position(|window| window == needle)
    }) {
        starts.push(at + found);
        at += found + needle.len().max(1);
    }
    starts
}

/// `part` with `edits`, in order and apart, applied.
fn restored_part(part: &[u8], edits: &[(Range<usize>, String)]) -> Vec<u8> {
    let mut output = Vec::with_capacity(part.len());
    let mut cursor = 0;
    for (range, replacement) in edits {
        output.extend_from_slice(&part[cursor..range.start]);
        output.extend_from_slice(replacement.as_bytes());
        cursor = range.end;
    }
    output.extend_from_slice(&part[cursor..]);
    output
}

/// Where `range` of a part sits once `edits` are applied; no edit may cross it.
fn restored_range(edits: &[(Range<usize>, String)], range: Range<usize>) -> Result<Range<usize>> {
    let mut shift = 0isize;
    for (edit, replacement) in edits {
        if edit.end <= range.start {
            shift += replacement.len() as isize - edit.len() as isize;
        } else if edit.start < range.end {
            return Err(Error::Other(
                "a field publication crosses a restored whole-paragraph hyperlink".into(),
            ));
        }
    }
    Ok(range.start.saturating_add_signed(shift)..range.end.saturating_add_signed(shift))
}
