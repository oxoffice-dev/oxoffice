//! Returned reads: the block and run inputs of a read, and template clone pairs.

use super::*;

pub(super) fn returned_body_owner(item: &BodyContent) -> Option<WordOwner<'_>> {
    match item {
        BodyContent::Paragraph(value) => Some(WordOwner::Paragraph(value)),
        BodyContent::Table(value) => Some(WordOwner::Table(value)),
        BodyContent::ContentControl(value) => Some(WordOwner::Control(value)),
        BodyContent::RawXml(_) => None,
    }
}

fn returned_path_owner<'b>(
    mut owner: WordOwner<'b>,
    path: &[crate::text::WordBlockComponent],
) -> Result<WordOwner<'b>> {
    use crate::text::WordBlockComponent as C;
    // The checked row reader returns the sole actual row of its existing
    // temporary table. Its companion still includes that physical root slot.
    let path = if matches!(owner, WordOwner::Row(_)) && matches!(path.first(), Some(C::TableRow(0)))
    {
        &path[1..]
    } else {
        path
    };
    let invalid = || OxmlError::InvalidValue("returned Word model path changed".into());
    for component in path {
        owner = match (owner, component) {
            (WordOwner::Paragraph(paragraph), C::ParagraphControl(index)) => WordOwner::Control(
                &paragraph
                    .content_controls
                    .get(*index)
                    .ok_or_else(invalid)?
                    .3,
            ),
            (WordOwner::Paragraph(paragraph), C::ParagraphRevision(index)) => {
                WordOwner::Revision(&paragraph.revisions.get(*index).ok_or_else(invalid)?.2)
            }
            (WordOwner::Revision(revision), C::RevisionParagraph) => {
                WordOwner::Paragraph(revision.content_paragraph().ok_or_else(invalid)?)
            }
            (WordOwner::Control(control), C::ControlRevision(index)) => {
                WordOwner::Revision(&control.revisions().get(*index).ok_or_else(invalid)?.1)
            }
            (WordOwner::Table(table), C::TableRow(index)) => {
                WordOwner::Row(table.rows.get(*index).ok_or_else(invalid)?)
            }
            (WordOwner::Table(table), C::TableControl(index)) => {
                WordOwner::Control(&table.content_controls.get(*index).ok_or_else(invalid)?.2)
            }
            (WordOwner::Row(row), C::RowCell(index)) => {
                WordOwner::Cell(row.cells.get(*index).ok_or_else(invalid)?)
            }
            (WordOwner::Row(row), C::RowControl(index)) => {
                WordOwner::Control(&row.content_controls.get(*index).ok_or_else(invalid)?.2)
            }
            (WordOwner::Cell(cell), C::CellContent(index)) => {
                match cell.content.get(*index).ok_or_else(invalid)? {
                    crate::table::CellContent::Paragraph(value) => WordOwner::Paragraph(value),
                    crate::table::CellContent::Table(value) => WordOwner::Table(value),
                    crate::table::CellContent::ContentControl(value) => WordOwner::Control(value),
                }
            }
            (WordOwner::Control(control), C::SdtContent(index)) => {
                match control.content.get(*index).ok_or_else(invalid)? {
                    crate::content_control::SdtContent::Paragraph(value) => {
                        WordOwner::Paragraph(value)
                    }
                    crate::content_control::SdtContent::Run(value) => WordOwner::Run(value),
                    crate::content_control::SdtContent::Table(value) => WordOwner::Table(value),
                    crate::content_control::SdtContent::Row(value) => WordOwner::Row(value),
                    crate::content_control::SdtContent::Cell(value) => WordOwner::Cell(value),
                    crate::content_control::SdtContent::ContentControl(value) => {
                        WordOwner::Control(value)
                    }
                    _ => return Err(invalid()),
                }
            }
            _ => return Err(invalid()),
        };
    }
    Ok(owner)
}

// Text, Field and borrowed XML are actual consumers of this clone-pair check.
fn dedup_template_clone_pairs<T: ?Sized>(pairs: &mut Vec<(&T, &T)>) -> Result<()> {
    let mut seen = std::collections::HashMap::new();
    let mut conflict = false;
    pairs.retain(|(current, frozen)| {
        let current = (
            std::ptr::from_ref(*current).cast::<()>() as usize,
            std::mem::size_of_val(*current),
        );
        let frozen = (
            std::ptr::from_ref(*frozen).cast::<()>() as usize,
            std::mem::size_of_val(*frozen),
        );
        if let Some(previous) = seen.insert(current, frozen) {
            conflict |= previous != frozen;
            false
        } else {
            true
        }
    });
    if conflict {
        return Err(OxmlError::InvalidValue(
            "conflicting actual paragraph clone pairs".into(),
        ));
    }
    Ok(())
}

// These pairs come from one actual typed clone, before the checked Field-slot
// installer changes ordinary Text or retained raw slots. Field children retain
// their own preparation receipts instead of borrowing a new clone baseline.
#[allow(clippy::type_complexity)]
pub(crate) fn template_paragraph_clone_pairs<'b, 'c>(
    previous: &'b CT_P,
    current: &'c CT_P,
) -> Result<(
    Vec<(&'c crate::text::CT_Text, &'b crate::text::CT_Text)>,
    Vec<(&'c crate::text::Field, &'b crate::text::Field)>,
    Vec<(&'c [u8], &'b [u8])>,
)> {
    use crate::text::RunContent;
    let mut texts = Vec::new();
    let mut fields = Vec::new();
    let mut raws = Vec::new();
    let mut pending = vec![(
        WordOwner::Paragraph(previous),
        WordOwner::Paragraph(current),
    )];
    while let Some((old, new)) = pending.pop() {
        match (old, new) {
            (WordOwner::Run(old), WordOwner::Run(new)) => {
                for (old, new) in old.content.iter().zip(&new.content) {
                    if let (RunContent::Text(old), RunContent::Text(new)) = (old, new) {
                        texts.push((new, old));
                    }
                }
                raws.extend(
                    new.extra_xml
                        .iter()
                        .zip(&old.extra_xml)
                        .map(|(new, old)| (new.as_slice(), old.as_slice())),
                );
            }
            (WordOwner::Paragraph(old), WordOwner::Paragraph(new)) => {
                raws.extend(
                    new.extra_xml
                        .iter()
                        .zip(&old.extra_xml)
                        .map(|(new, old)| (new.1.as_slice(), old.1.as_slice())),
                );
                for (old, new) in old.rubies.iter().zip(&new.rubies) {
                    raws.extend(
                        new.raw_xml
                            .iter()
                            .zip(&old.raw_xml)
                            .map(|(new, old)| (new.as_slice(), old.as_slice())),
                    );
                }
            }
            (WordOwner::Control(old), WordOwner::Control(new)) => {
                for ((old_index, old), (new_index, new)) in old
                    .retained_resource_run_xml()
                    .zip(new.retained_resource_run_xml())
                {
                    if old_index != new_index {
                        return Err(OxmlError::InvalidValue(
                            "cloned control source slots changed".into(),
                        ));
                    }
                    raws.push((new, old));
                }
                for (old, new) in old.content.iter().zip(&new.content) {
                    if let (
                        crate::content_control::SdtContent::RawXml(old),
                        crate::content_control::SdtContent::RawXml(new),
                    ) = (old, new)
                    {
                        raws.push((new, old));
                    }
                }
            }
            (WordOwner::Revision(old), WordOwner::Revision(new)) => {
                raws.push((new.retained_xml(), old.retained_xml()))
            }
            (WordOwner::Field(old), WordOwner::Field(new)) => {
                fields.push((new, old));
                continue;
            }
            (WordOwner::Drawing(old), WordOwner::Drawing(new)) => {
                if let (Some(old), Some(new)) = (&old.inline, &new.inline)
                    && let (Some(old), Some(new)) = (&old.raw_xml, &new.raw_xml)
                {
                    raws.push((new, old));
                }
            }
            (WordOwner::Anchor(old), WordOwner::Anchor(new)) => {
                if let (Some(old), Some(new)) = (&old.raw_xml, &new.raw_xml) {
                    raws.push((new, old));
                }
            }
            _ => {}
        }
        let before = old.resource_children();
        let after = new.resource_children();
        if before.len() != after.len()
            || before
                .iter()
                .zip(&after)
                .any(|(old, new)| old.key().0 != new.key().0)
        {
            return Err(OxmlError::InvalidValue(
                "cloned paragraph source shape changed".into(),
            ));
        }
        pending.extend(before.into_iter().zip(after));
    }
    dedup_template_clone_pairs(&mut texts)?;
    dedup_template_clone_pairs(&mut fields)?;
    dedup_template_clone_pairs(&mut raws)?;
    Ok((texts, fields, raws))
}

fn returned_run_inputs<'b>(
    run: &'b crate::text::CT_R,
    facts: &crate::text::ParsedWordRunTexts,
    outer: &WordSourceMap,
    output: &mut Vec<(&'b crate::text::CT_Text, crate::text::TemplateWordEvent)>,
    raws: &mut Vec<(&'b [u8], WordSourceMap)>,
) -> Result<()> {
    let map = facts.source_map.as_deref().unwrap_or(outer);
    for parsed in &facts.texts {
        if let Some(maps) = &parsed.drawing
            && let Some(crate::text::RunContent::Drawing(drawing)) = run.content.get(parsed.content)
        {
            for (raw, map) in [
                (
                    drawing
                        .inline
                        .as_ref()
                        .and_then(|owner| owner.raw_xml.as_ref()),
                    maps.inline.as_deref(),
                ),
                (
                    drawing
                        .anchor
                        .as_ref()
                        .and_then(|owner| owner.raw_xml.as_ref()),
                    maps.anchor.as_deref(),
                ),
            ] {
                if let (Some(raw), Some(map)) = (raw, map)
                    && raw.len() == map.len()
                {
                    raws.push((raw.as_slice(), map.clone()));
                }
            }
            continue;
        }
        let Some(crate::text::RunContent::Text(text)) = run.content.get(parsed.content) else {
            continue;
        };
        let encoded =
            facts.source.start + parsed.encoded.start..facts.source.start + parsed.encoded.end;
        let event = map
            .word_events
            .binary_search_by_key(&encoded.start, |event| event.encoded.start)
            .ok()
            .and_then(|index| map.word_events.get(index))
            .filter(|event| event.encoded == encoded && event.decoded.len() == text.text.len())
            .cloned()
            .or_else(|| {
                map.word_range(encoded.clone())
                    .map(|origin| crate::text::TemplateWordEvent {
                        encoded,
                        origin: crate::text::TemplateWordOrigin::Raw {
                            start: origin.start,
                            end: origin.end,
                        },
                        decoded: 0..text.text.len(),
                    })
            });
        if let Some(event) = event {
            output.push((text, event));
        }
    }
    Ok(())
}

#[allow(clippy::type_complexity)]
pub(crate) fn returned_block_inputs<'b>(
    owner: WordOwner<'b>,
    maps: &crate::text::WordBlockSourceMaps,
) -> Result<(
    Vec<(&'b crate::text::CT_Text, crate::text::TemplateWordEvent)>,
    Vec<(&'b [u8], WordSourceMap)>,
)> {
    let mut texts = Vec::new();
    let mut raws = Vec::new();
    for (path, facts) in &maps.word_texts {
        let Some(facts) = facts else {
            continue;
        };
        let WordOwner::Paragraph(paragraph) = returned_path_owner(owner, path)? else {
            return Err(OxmlError::InvalidValue(
                "returned paragraph path changed".into(),
            ));
        };
        let (nested_texts, nested_raws) =
            returned_block_inputs(WordOwner::Paragraph(paragraph), &facts.nested)?;
        texts.extend(nested_texts);
        raws.extend(nested_raws);
        for (index, run) in facts.texts.iter().enumerate() {
            if let (Some(run_facts), Some(run)) = (run, paragraph.runs.get(index)) {
                returned_run_inputs(run, run_facts, &facts.source_map, &mut texts, &mut raws)?;
            }
        }
    }
    for (path, map) in &maps.word_raws {
        if let WordOwner::Revision(revision) = returned_path_owner(owner, path)? {
            let raw = revision.retained_xml();
            if map.len() == raw.len() {
                raws.push((raw, map.as_ref().clone()));
            }
        }
    }
    for (path, facts) in &maps.word_runs {
        let Some(facts) = facts else {
            continue;
        };
        if let WordOwner::Run(run) = returned_path_owner(owner, path)?
            && let Some(map) = facts.source_map.as_deref()
        {
            returned_run_inputs(run, facts, map, &mut texts, &mut raws)?;
            if let Some(crate::text::WordBlockComponent::SdtContent(index)) = path.last()
                && let WordOwner::Control(control) =
                    returned_path_owner(owner, &path[..path.len() - 1])?
            {
                if let Some((_, raw)) = control
                    .retained_resource_run_xml()
                    .find(|(actual, _)| actual == index)
                    && map.len() == raw.len()
                {
                    raws.push((raw, map.clone()));
                }
                if let Some((_, original)) = control
                    .retained_resource_run_xml()
                    .zip(control.original_resource_runs())
                    .find(|((actual, _), _)| actual == index)
                {
                    returned_run_inputs(original, facts, map, &mut texts, &mut raws)?;
                }
            }
        }
    }
    for (path, extra_maps) in &maps.paragraphs {
        let WordOwner::Paragraph(paragraph) = returned_path_owner(owner, path)? else {
            continue;
        };
        for (raw, map) in paragraph.extra_xml.iter().zip(extra_maps) {
            if let Some(map) = map
                && map.len() == raw.1.len()
            {
                raws.push((raw.1.as_slice(), map.as_ref().clone()));
            }
        }
    }
    Ok((texts, raws))
}
