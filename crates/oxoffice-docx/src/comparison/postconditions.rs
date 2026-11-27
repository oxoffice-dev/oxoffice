//! The checks a comparison result must pass: accepted and rejected forms, held revisions.

use super::*;

pub(crate) fn reopen_staged(candidate: Document) -> Result<Document> {
    candidate.prepare_and_reopen_staged()
}

pub(super) type NormalizedPackage = (
    Vec<GuardItem>,
    Vec<(ComparisonStoryKind, String, Vec<GuardItem>)>,
);

pub(super) fn comparison_postcondition_error(
    outcome: &str,
    expected: &str,
    actual: &NormalizedPackage,
    wanted: &NormalizedPackage,
) -> Error {
    Error::Other(format!(
        "comparison {outcome} does not reproduce the {expected} stories at {}",
        first_normalized_mismatch(actual, wanted)
    ))
}

fn first_normalized_mismatch(left: &NormalizedPackage, right: &NormalizedPackage) -> String {
    let main_count = left.0.len().min(right.0.len());
    for index in 0..main_count {
        if left.0[index] != right.0[index] {
            return format!("body story item[{index}]");
        }
    }
    if left.0.len() != right.0.len() {
        return format!("body story item[{main_count}]");
    }
    let story_count = left.1.len().min(right.1.len());
    for story_index in 0..story_count {
        let (left_kind, left_part, left_items) = &left.1[story_index];
        let (right_kind, right_part, right_items) = &right.1[story_index];
        if left_kind != right_kind || left_part != right_part {
            return format!("story inventory[{story_index}]");
        }
        let item_count = left_items.len().min(right_items.len());
        for item_index in 0..item_count {
            if left_items[item_index] != right_items[item_index] {
                return format!(
                    "{} story {} item[{item_index}]",
                    left_kind.label(),
                    left_part
                );
            }
        }
        if left_items.len() != right_items.len() {
            return format!(
                "{} story {} item[{item_count}]",
                left_kind.label(),
                left_part
            );
        }
    }
    format!("story inventory[{story_count}]")
}

/// Refuse a redline that does not keep each existing revision of the
/// original byte for byte, with its author, date and id, in every compared
/// story. The revisions this comparison wrote carry `new_ids`, which no id of
/// a compared story uses. Ignored stories and text boxes are not seeded and
/// keep their original bytes, so they are left out here.
pub(super) fn refuse_changed_existing_revisions(
    original: &Document,
    candidate: &Document,
    stories: &[StoryPart],
    options: &ComparisonOptions,
    new_ids: &HashSet<i32>,
) -> Result<()> {
    let mut parts = Vec::new();
    if !story_ignored(options, ComparisonStoryKind::Main) {
        parts.push(original.doc_part_name.clone());
    }
    parts.extend(stories.iter().map(|story| story.part_name.clone()));
    for part in parts {
        let revisions = |document: &Document| -> Result<Vec<Vec<u8>>> {
            let xml = document
                .package
                .get_part(&part)
                .ok_or_else(|| Error::Other(format!("missing comparison story part {part}")))?;
            let xml = if story_ignored(options, ComparisonStoryKind::TextBox) {
                without_text_box_subtrees(xml)?
            } else {
                xml.to_vec()
            };
            Ok(crate::revision::modeled_revision_spans(&xml)?
                .into_iter()
                .filter(|(_, id)| !new_ids.contains(id))
                .map(|(range, _)| xml[range].to_vec())
                .collect())
        };
        if revisions(original)? != revisions(candidate)? {
            return Err(Error::Other(format!(
                "comparison would change an existing tracked revision in {part}"
            )));
        }
    }
    Ok(())
}

/// The candidate with its revisions resolved one way and the transient note
/// pairs of fully resolved units projected out. Every postcondition of that
/// resolution reads this one document, so the package is resolved once.
pub(super) fn resolved_candidate(
    candidate: &Document,
    resolve: impl FnOnce(&mut Document) -> Result<usize>,
    custody_ids: Option<&HashSet<i32>>,
) -> Result<Document> {
    let mut resolved = candidate.clone_for_staging();
    let mut units = Vec::new();
    if let Some(ids) = custody_ids {
        // The kinds the resolution itself settles.
        for kind in resolved_note_kinds(candidate) {
            let scope = crate::revision::RevisionScope::Ids(ids);
            units.push((kind, validate_note_custody(candidate, kind, &scope)?));
        }
    }
    resolve(&mut resolved)?;
    for (kind, units) in units {
        project_resolved_note_tombstones(candidate, &mut resolved, kind, &units)?;
    }
    Ok(resolved)
}

/// Comparison guards may omit only the current fully resolved units' transient
/// pairs. Their persisted result still retains them until the part settles.
fn project_resolved_note_tombstones(
    before: &Document,
    after: &mut Document,
    kind: NoteKind,
    units: &[FootnoteCustody],
) -> Result<()> {
    let current = validate_note_custody(after, kind, &crate::revision::RevisionScope::All)?;
    if !current.iter().any(|unit| !unit.revisions.is_empty()) {
        return Ok(());
    }
    let retained = units
        .iter()
        .filter(|unit| !unit.selected)
        .filter_map(|unit| unit.token)
        .collect::<HashSet<_>>();
    // Only marked units carry pairs to project. A pending tracked note alone
    // has none.
    if retained.is_empty()
        && !units
            .iter()
            .any(|unit| unit.selected && unit.token.is_some())
    {
        return Ok(());
    }
    let prior = note_markers(before, kind)?
        .into_iter()
        .filter(|marker| retained.contains(&marker.token))
        .collect::<Vec<_>>();
    let actual = note_markers(after, kind)?;
    for marker in prior {
        let found = actual
            .iter()
            .filter(|other| {
                other.token == marker.token
                    && other.family == marker.family
                    && other.part == marker.part
            })
            .collect::<Vec<_>>();
        if found.len() != 1
            || before
                .package
                .get_part(&marker.part)
                .expect("prior marker part")[marker.span.clone()]
                != after
                    .package
                    .get_part(&found[0].part)
                    .expect("retained marker part")[found[0].span.clone()]
        {
            return Err(Error::Other(
                "comparison changed a prior custody pair".into(),
            ));
        }
    }
    let projected = units
        .iter()
        .filter(|unit| unit.selected && unit.token.is_some())
        .cloned()
        .collect::<Vec<_>>();
    for unit in &projected {
        let Some(actual) = current.iter().find(|other| other.token == unit.token) else {
            return Err(Error::Other(
                "resolved current custody tombstone disappeared while another unit is pending"
                    .into(),
            ));
        };
        if !actual.revisions.is_empty()
            || actual.original != unit.original
            || actual.edited != unit.edited
        {
            return Err(Error::Other(
                "resolved current custody tombstone changed state or bits".into(),
            ));
        }
    }
    retire_note_markers(after, kind, &projected, None)
}

pub(super) fn normalized_package(
    document: &Document,
    stories: &[StoryPart],
    options: &ComparisonOptions,
    text_box_markers: &TextBoxMarkers,
) -> Result<NormalizedPackage> {
    let mut related = Vec::with_capacity(stories.len());
    for story in stories {
        related.push((
            story.kind,
            story.part_name.clone(),
            guard_story_part(
                document,
                &story.part_name,
                story_xml(document, story)?,
                story.kind,
                options,
                text_box_markers
                    .related
                    .get(&story.part_name)
                    .map(String::as_str),
            )?,
        ));
    }
    for kind in NoteKind::ALL {
        let story = kind.story_kind();
        if story_ignored(options, story) || stories.iter().any(|other| other.kind == story) {
            continue;
        }
        let Some(part) = NotePart::of(document, kind)? else {
            continue;
        };
        let groups = guard_story_owners(
            document,
            &part.name,
            part.xml.as_bytes(),
            story,
            options,
            text_box_markers.related.get(&part.name).map(String::as_str),
            true,
        )?;
        if groups.len() != part.normal().len() {
            return Err(Error::Other(
                "note guard did not cover every actual normal owner".into(),
            ));
        }
        related.extend(
            groups
                .into_iter()
                .map(|items| (story, part.name.clone(), items)),
        );
    }
    let source = document
        .package
        .get_part(&document.doc_part_name)
        .ok_or_else(|| Error::Other(format!("missing main story {}", document.doc_part_name)))?;
    let source = std::str::from_utf8(source).map_err(utf8_error)?;
    let main_source;
    let original_source = source;
    let mut coordinates = Vec::new();
    let source = if story_ignored(options, ComparisonStoryKind::TextBox) {
        let prefix = root_prefix(source, "document")?;
        let (masked, _) = mask_text_box_subtrees(source, &prefix, &text_box_markers.main)?;
        main_source = masked;
        coordinates = guard_mask_coordinates(
            original_source,
            &main_source,
            &prefix,
            &text_box_markers.main,
        )?;
        main_source.as_str()
    } else {
        source
    };
    let (body_range, bindings) = main_body_source(source.as_bytes())?;
    let main_document = story_document(extract_body_inner(source.as_bytes())?, &bindings)?;
    let main = guard_source_body_at(
        &main_document,
        extract_body_inner(source.as_bytes())?,
        &bindings,
        document,
        &document.doc_part_name,
        options,
        "body",
        Some((body_range.start, coordinates)),
    )?;
    Ok((main, related))
}

/// Whether `xml` holds a `w:t` or `w:instrText` inside a `w:del`: deleted
/// text written as text, which Word cannot read (moved text keeps `w:t`).
/// A run that holds a text box, deleted whole, writes it so.
pub(super) fn text_in_deletions(xml: &[u8]) -> bool {
    let mut reader = Reader::from_reader(xml);
    let mut deletions = Vec::<Vec<u8>>::new();
    let prefix = |name: &[u8]| {
        name.split(|byte| *byte == b':')
            .next()
            .unwrap_or_default()
            .to_vec()
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) if element.local_name().as_ref() == b"del" => {
                deletions.push(prefix(element.name().as_ref()));
            }
            Ok(Event::End(element)) if element.local_name().as_ref() == b"del" => {
                deletions.pop();
            }
            Ok(Event::Start(element) | Event::Empty(element))
                if matches!(element.local_name().as_ref(), b"t" | b"instrText")
                    && deletions.last() == Some(&prefix(element.name().as_ref())) =>
            {
                return true;
            }
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}
