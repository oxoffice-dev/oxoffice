//! Placeholder replacement across paragraphs, tables, headers and footers,
//! text boxes, and footnotes and endnotes.
//!
//! Handles the cross-run splitting problem: a placeholder like `{{name}}`
//! may be split across multiple `<w:r>` elements in the OOXML source.
//!
//! A replacement reads the runs of a paragraph that `Paragraph::text` reads,
//! in document order: the direct runs, the runs of its inline content
//! controls, those of its tracked insertions and move destinations, but not
//! those of a deletion or a move source, and those inside its smart tags,
//! inline custom XML elements and simple fields. It searches their text left
//! to right as one. A match must lie within one stretch of those runs, see
//! [`replaceable_texts`]. A match that straddles the boundary of a content
//! control, a tracked insertion or one of those wrappers is not replaced,
//! and the search goes on after it, so no part of the text it covers is
//! replaced either.

use crate::content_control::{CT_Sdt, SdtContent};
use crate::footnotes::NoteType;
use crate::header_footer::CT_HdrFtr;
use crate::namespace::W_NS;
use crate::numbering::{local_namespace_overrides, namespace_bindings, word_prefixes_at};
use crate::properties::is_word_element;
use crate::revision::{CT_Revision, RevisionKind};
use crate::table::{CT_Row, CT_Tbl, CT_Tc, CellContent};
use crate::text::{
    AcceptedRunPath, AcceptedRunPathSegment, BoundaryOwner, CT_P, CT_R, CT_Text, FieldXmlEmission,
    RunContent, SpecialCharacter, WordExtraSourceMaps, WordOperationWork, boundary_owners,
    optional_word_attribute, raw_with_external_bindings, simple_field_source,
};

/// Replace all occurrences of `placeholder` with `replacement` in a paragraph.
///
/// Handles placeholders split across multiple runs and reaches the runs of
/// inline content controls, tracked insertions, smart tags, inline custom
/// XML elements and simple fields. A match that straddles one of their
/// boundaries is not replaced.
/// Preserves the formatting of the first matched run. Returns the number of
/// replacements made.
pub fn replace_in_paragraph(para: &mut CT_P, placeholder: &str, replacement: &str) -> usize {
    try_replace_in_paragraph(para, placeholder, replacement).unwrap_or(0)
}

/// Fallible paragraph replacement for checked document operations.
#[doc(hidden)]
pub fn try_replace_in_paragraph(
    para: &mut CT_P,
    placeholder: &str,
    replacement: &str,
) -> crate::Result<usize> {
    if placeholder.is_empty() {
        return Ok(0);
    }
    install_paragraph_replacement(para, |candidate| {
        replace_matches(candidate, &mut |text, from| {
            let start = from + text[from..].find(placeholder)?;
            Some((start, start + placeholder.len(), replacement.to_owned()))
        })
    })
}

/// Run replacement, nested removal and wrapper write-back on one uninstalled
/// candidate. Refusals propagate and leave `para` unchanged.
fn install_paragraph_replacement(
    para: &mut CT_P,
    edit: impl FnOnce(&mut CT_P) -> crate::Result<usize>,
) -> crate::Result<usize> {
    crate::document::admit_word_model(crate::document::WordOwner::Paragraph(para))?;
    let mut candidate = para.clone();
    match edit(&mut candidate) {
        Ok(count) if count > 0 => {
            let previous = std::mem::replace(para, candidate);
            crate::text::discard_checked_paragraph(previous);
            Ok(count)
        }
        result => {
            crate::text::discard_checked_paragraph(candidate);
            result
        }
    }
}

/// Apply prepared edits to original replaceable stretches atomically.
///
/// Each tuple supplies a stretch index, nonempty UTF8 byte range, literal text
/// and an optional run-property override for the replacement alone.
#[doc(hidden)]
#[allow(clippy::type_complexity)] // Existing types cross the facade/model boundary without a new carrier.
pub fn replace_prepared_occurrences(
    paragraph: &mut CT_P,
    occurrences: &[(
        usize,
        std::ops::Range<usize>,
        String,
        Option<crate::properties::CT_RPr>,
    )],
) -> crate::Result<()> {
    let borrowed = occurrences
        .iter()
        .map(|(stretch, range, replacement, properties)| {
            (
                *stretch,
                range.clone(),
                replacement.as_str(),
                properties.as_ref(),
            )
        })
        .collect::<Vec<_>>();
    replace_prepared_occurrences_borrowed(paragraph, &borrowed)
}

/// The substantive prepared replacement kernel. Replacement payloads stay
/// borrowed until the existing edit installs owned run text and properties.
#[allow(clippy::type_complexity)] // The borrowed form of the existing four-field coordination surface.
pub(crate) fn replace_prepared_occurrences_borrowed(
    paragraph: &mut CT_P,
    occurrences: &[(
        usize,
        std::ops::Range<usize>,
        &str,
        Option<&crate::properties::CT_RPr>,
    )],
) -> crate::Result<()> {
    replace_prepared_occurrences_borrowed_in(paragraph, occurrences, None, None, None)
}

pub(crate) struct PreparedTemplateInputs<'a, 'b> {
    pub(crate) datums: &'b std::collections::HashMap<usize, &'a serde_json::Value>,
    pub(crate) operands: &'b [(
        Option<&'b std::cell::Cell<bool>>,
        &'b [&'a serde_json::Value],
    )],
}

#[allow(clippy::type_complexity)] // Same borrowed payloads and actual optional operation companions.
pub(crate) fn replace_prepared_occurrences_borrowed_in(
    paragraph: &mut CT_P,
    occurrences: &[(
        usize,
        std::ops::Range<usize>,
        &str,
        Option<&crate::properties::CT_RPr>,
    )],
    extra_maps: Option<&mut WordExtraSourceMaps>,
    mut work: Option<&mut WordOperationWork>,
    inputs: Option<PreparedTemplateInputs<'_, '_>>,
) -> crate::Result<()> {
    let walk = text_runs_in(paragraph, extra_maps.as_deref(), work.as_deref_mut())?;
    let stretches = text_stretches(paragraph, &walk);
    let edits = prepared_walk_edits(occurrences, &stretches)?;
    replace_prepared_walk(
        paragraph,
        edits,
        occurrences.len(),
        extra_maps,
        work,
        inputs,
        walk,
    )
}

type PreparedEdit<'a> = (
    std::ops::Range<usize>,
    &'a str,
    Option<&'a crate::properties::CT_RPr>,
    usize,
);

fn prepared_walk_edits<'a>(
    occurrences: &[(
        usize,
        std::ops::Range<usize>,
        &'a str,
        Option<&'a crate::properties::CT_RPr>,
    )],
    stretches: &[String],
) -> crate::Result<Vec<PreparedEdit<'a>>> {
    let invalid = || crate::OxmlError::InvalidValue("invalid prepared template occurrence".into());
    let mut offsets = Vec::with_capacity(stretches.len());
    let mut total = 0;
    for stretch in stretches {
        offsets.push(total);
        total += stretch.len();
    }
    let mut edits = Vec::with_capacity(occurrences.len());
    for (index, (stretch, range, replacement, properties)) in occurrences.iter().enumerate() {
        let source = stretches.get(*stretch).ok_or_else(invalid)?;
        if range.start >= range.end
            || source.get(range.clone()).is_none()
            || replacement
                .chars()
                .any(|character| !oxoffice_core::xml::is_xml_1_0_character(character))
        {
            return Err(invalid());
        }
        validate_borrowed_prepared_properties(replacement, *properties)?;
        edits.push((
            offsets[*stretch] + range.start..offsets[*stretch] + range.end,
            *replacement,
            *properties,
            index,
        ));
    }
    edits.sort_by_key(|(range, _, _, _)| range.start);
    if edits.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
        return Err(invalid());
    }
    Ok(edits)
}

fn replace_prepared_walk(
    paragraph: &mut CT_P,
    edits: Vec<PreparedEdit<'_>>,
    occurrence_count: usize,
    extra_maps: Option<&mut WordExtraSourceMaps>,
    mut work: Option<&mut WordOperationWork>,
    inputs: Option<PreparedTemplateInputs<'_, '_>>,
    walk: TextWalk,
) -> crate::Result<()> {
    let invalid = || crate::OxmlError::InvalidValue("invalid prepared template occurrence".into());
    crate::document::admit_word_model(crate::document::WordOwner::Paragraph(paragraph))?;
    let mut operand_bytes = Vec::new();
    if let Some(inputs) = &inputs {
        if inputs.operands.len() != occurrence_count {
            return Err(invalid());
        }
        for (callback, dependencies) in inputs.operands {
            if callback.is_some() && !dependencies.is_empty() {
                return Err(invalid());
            }
            let mut selected = Vec::new();
            for value in *dependencies {
                let key = std::ptr::from_ref(*value) as usize;
                if !inputs
                    .datums
                    .get(&key)
                    .is_some_and(|input| std::ptr::eq(*input, *value))
                {
                    return Err(crate::OxmlError::InvalidValue(
                        "template datum is not a selected input".into(),
                    ));
                }
                selected.push((key, crate::document::template_scalar_bytes(value)?));
            }
            operand_bytes.push(selected);
        }
    }
    if let Some(work) = work.as_deref_mut() {
        let (text, map) = build_char_map(paragraph, &walk.wrappers, &walk.runs, template_text);
        let mut fields = Vec::new();
        let mut callbacks = Vec::new();
        let mut datums = Vec::new();
        for (range, replacement, _, index) in &edits {
            let (first, last, _, _) =
                accepted_match_runs(&text, &map, &walk.runs, range.start, range.end)
                    .ok_or_else(invalid)?;
            let selected_range = first..last;
            let selected = selected_field_ancestors(
                paragraph,
                &walk,
                &map,
                std::slice::from_ref(&selected_range),
            );
            if !selected.is_empty()
                && let Some(inputs) = &inputs
            {
                if let Some(callback) = inputs.operands[*index].0 {
                    callbacks.push((*replacement, callback));
                } else {
                    datums.extend_from_slice(&operand_bytes[*index]);
                }
            }
            fields.extend(selected);
        }
        work.register_original_fields_with_inputs(fields, &callbacks, &datums)?;
    }
    drop(walk);
    let mut candidate = paragraph.clone();
    let mut candidate_maps = extra_maps.as_deref().cloned();
    // Original offsets remain valid to the left of edits already applied.
    for (range, replacement, properties, _) in edits.into_iter().rev() {
        if properties.is_some() {
            split_prepared_boundary_in(
                &mut candidate,
                range.end,
                candidate_maps.as_mut(),
                work.as_deref_mut(),
            )?;
            split_prepared_boundary_in(
                &mut candidate,
                range.start,
                candidate_maps.as_mut(),
                work.as_deref_mut(),
            )?;
            let mut walk = text_runs_in(&candidate, candidate_maps.as_ref(), work.as_deref_mut())?;
            let (text, map) = build_char_map(&candidate, &walk.wrappers, &walk.runs, template_text);
            let first = map
                .get(text[..range.start].chars().count())
                .ok_or_else(invalid)?;
            edit_run(
                &mut candidate,
                &mut walk.wrappers,
                &walk.runs[first.run_index],
                work.as_deref_mut(),
                |run| {
                    run.properties = properties.cloned();
                },
            )?;
            remove_emptied_runs_in(
                &mut candidate,
                walk.wrappers,
                &walk.runs,
                Vec::new(),
                candidate_maps.as_mut(),
                work.as_deref_mut(),
            )?;
        }
        let mut applied = false;
        let count = replace_matches_in(
            &mut candidate,
            &mut |text, from| {
                if applied || from > range.start || text.get(range.clone()).is_none() {
                    return None;
                }
                applied = true;
                Some((range.start, range.end, replacement.to_owned()))
            },
            candidate_maps.as_mut(),
            work.as_deref_mut(),
        )?;
        if count != 1 {
            return Err(invalid());
        }
    }
    validate_prepared_paragraph_in(&candidate, work)?;
    *paragraph = candidate;
    if let (Some(output), Some(maps)) = (extra_maps, candidate_maps) {
        *output = maps;
    }
    Ok(())
}

fn validate_borrowed_prepared_properties(
    replacement: &str,
    properties: Option<&crate::properties::CT_RPr>,
) -> crate::Result<()> {
    use quick_xml::events::{BytesEnd, BytesStart, Event};

    let xml = {
        let mut writer = quick_xml::Writer::new(Vec::new());
        let (paragraph, paragraph_prefix) = crate::text::generated_word_start(
            BytesStart::new("w:p"),
            &[],
            crate::text::XmlWriteRole::Publication,
            None,
        )?;
        let _paragraph_scope = crate::text::emitted_binding_scope(&paragraph, false, false)?;
        writer.write_event(Event::Start(paragraph))?;
        let mut run = BytesStart::new("w:r");
        crate::text::restore_generated_word_binding(&mut run)?;
        let run_scope = crate::text::emitted_binding_scope(&run, false, false)?;
        writer.write_event(Event::Start(run))?;
        if let Some(properties) = properties {
            properties.write_xml_role(
                &mut writer,
                None,
                crate::text::XmlWriteRole::Publication,
                None,
                None,
            )?;
        }
        // Character validation precedes this probe. Keep the actual text
        // scaffold and space policy without owning its escaped payload.
        let mut text = BytesStart::new("w:t");
        if replacement.starts_with(' ') || replacement.ends_with(' ') {
            text.push_attribute(("xml:space", "preserve"));
        }
        writer.write_event(Event::Start(text))?;
        writer.write_event(Event::End(BytesEnd::new("w:t")))?;
        writer.write_event(Event::End(BytesEnd::new("w:r")))?;
        drop(run_scope);
        writer.write_event(Event::End(BytesEnd::new(format!("{paragraph_prefix}:p"))))?;
        writer.into_inner()
    };
    validate_prepared_xml(&xml)
}

fn validate_prepared_paragraph_in(
    paragraph: &CT_P,
    work: Option<&mut WordOperationWork>,
) -> crate::Result<()> {
    let mut writer = quick_xml::Writer::new(Vec::new());
    if let Some(work) = work {
        crate::text::write_prepared_validation_xml(paragraph, &mut writer, work)?;
    } else {
        paragraph.to_xml(&mut writer)?;
    }
    validate_prepared_xml(&writer.into_inner())
}

fn validate_prepared_xml(xml: &[u8]) -> crate::Result<()> {
    let text = std::str::from_utf8(xml)?;
    oxoffice_core::xml::reject_non_xml_characters("prepared template XML", text)?;
    // Opaque source may rely on inherited namespace bindings. Use the existing
    // preservation validator here, then the facade validates the final owning
    // part under its real namespace authority before publishing the document.
    crate::text::validate_word_owner_fragment(xml, b"p", false)
}

fn split_prepared_boundary_in(
    paragraph: &mut CT_P,
    byte: usize,
    extra_maps: Option<&mut WordExtraSourceMaps>,
    mut work: Option<&mut WordOperationWork>,
) -> crate::Result<()> {
    let mut walk = text_runs_in(paragraph, extra_maps.as_deref(), work.as_deref_mut())?;
    let (text, map) = build_char_map(paragraph, &walk.wrappers, &walk.runs, template_text);
    let index = text
        .get(..byte)
        .ok_or_else(|| crate::OxmlError::InvalidValue("invalid prepared template boundary".into()))?
        .chars()
        .count();
    let Some(mapping) = map.get(index) else {
        return Ok(());
    };
    let source = &walk.runs[mapping.run_index];
    let emission = work
        .as_deref()
        .filter(|_| {
            source
                .wrapper
                .is_some_and(|index| walk.wrappers[index].in_field)
        })
        .map(WordOperationWork::field_xml_emission);
    let owner = match source.wrapper {
        Some(index) => {
            walk.wrappers[index].changed = true;
            &mut walk.wrappers[index].content
        }
        None => &mut *paragraph,
    };
    let run = owner
        .accepted_run(&source.path)
        .ok_or_else(|| crate::OxmlError::InvalidValue("stale prepared run".into()))?;
    let offset = run.content[..mapping.content_index]
        .iter()
        .filter_map(|content| match content {
            RunContent::Text(text) => Some(text.text.chars().count()),
            _ => None,
        })
        .sum::<usize>()
        + match &run.content[mapping.content_index] {
            RunContent::Text(text) => text.text[..mapping.byte_offset].chars().count(),
            _ => 0,
        };
    let accepted = owner
        .accepted_run_paths()
        .iter()
        .position(|path| path == &source.path)
        .ok_or_else(|| crate::OxmlError::InvalidValue("stale prepared run boundary".into()))?;
    owner.split_accepted_run_in(
        &source.path,
        accepted,
        offset,
        work.as_deref_mut(),
        emission.as_ref(),
    )?;
    remove_emptied_runs_in(
        paragraph,
        walk.wrappers,
        &walk.runs,
        Vec::new(),
        extra_maps,
        work,
    )?;
    Ok(())
}

/// The texts a replacement in `para` matches against, one per stretch of
/// runs. The direct runs between two inline content controls, tracked
/// insertions or wrappers form one stretch, and so do the runs of one
/// control, insertion, move destination, smart tag, inline custom XML
/// element or simple field, between those nested in it. A match never spans
/// two stretches.
#[doc(hidden)]
pub fn replaceable_texts(para: &CT_P) -> Vec<String> {
    replaceable_texts_in(para, None, None).unwrap_or_default()
}

fn replaceable_texts_in(
    para: &CT_P,
    extra_maps: Option<&WordExtraSourceMaps>,
    work: Option<&mut WordOperationWork>,
) -> crate::Result<Vec<String>> {
    let walk = text_runs_in(para, extra_maps, work)?;
    Ok(text_stretches(para, &walk))
}

fn text_stretches(para: &CT_P, walk: &TextWalk) -> Vec<String> {
    let mut texts: Vec<String> = Vec::new();
    let mut previous = None;
    for run in &walk.runs {
        if previous != Some(run.stretch) {
            texts.push(String::new());
            previous = Some(run.stretch);
        }
        if let (Some(text), Some(run)) = (texts.last_mut(), text_run(para, &walk.wrappers, run)) {
            text.extend(run.content.iter().filter_map(|content| match content {
                RunContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            }));
        }
    }
    texts
}

pub(crate) fn template_replaceable_texts_in(
    paragraph: &mut CT_P,
    extras: &WordExtraSourceMaps,
    work: &mut WordOperationWork,
    word: &mut crate::document::TemplateWordRead<'_>,
) -> crate::Result<Vec<String>> {
    let mut walk = text_runs_in(paragraph, Some(extras), Some(work))?;
    select_walk_words(paragraph, &walk, word)?;
    owned_walk_stretches(paragraph, &mut walk, word, None)
}

fn select_walk_words(
    paragraph: &CT_P,
    walk: &TextWalk,
    word: &mut crate::document::TemplateWordRead<'_>,
) -> crate::Result<()> {
    // Wrapper paragraphs are actual additional returned models. Bind their
    // nested Text objects to the same reader facts before consuming this walk.
    let mut nested_wrappers = std::collections::HashMap::new();
    for wrapper in &walk.wrappers {
        if let Some(facts) = &wrapper.word_facts {
            for (text, event) in crate::document::returned_block_inputs(
                crate::document::WordOwner::Paragraph(&wrapper.content),
                &facts.nested,
            )?
            .0
            {
                nested_wrappers.insert(std::ptr::from_ref(text) as usize, event);
            }
        }
    }
    // This is the same complete walk that aggregation consumes. Register its
    // actual visited inputs before any generated prefix needs an append.
    for item in &walk.runs {
        if let Some(run) = text_run(paragraph, &walk.wrappers, item) {
            for (content, value) in run.content.iter().enumerate() {
                if let RunContent::Text(text) = value {
                    if item.wrapper.is_some()
                        && let Some(event) =
                            nested_wrappers.get(&(std::ptr::from_ref(text) as usize))
                    {
                        word.work
                            .select_word(event.origin.clone(), event.decoded.clone())?;
                        continue;
                    }
                    word.select(
                        &item.path,
                        item.wrapper,
                        item.wrapper
                            .and_then(|index| walk.wrappers[index].word_facts.as_ref()),
                        content,
                        text,
                    )?;
                }
            }
        }
    }
    Ok(())
}

struct MovedWalkText {
    stretch: usize,
    run: usize,
    content: usize,
    length: usize,
}

fn owned_walk_stretches(
    paragraph: &mut CT_P,
    walk: &mut TextWalk,
    word: &mut crate::document::TemplateWordRead<'_>,
    mut moved: Option<&mut Vec<MovedWalkText>>,
) -> crate::Result<Vec<String>> {
    let mut visits = std::collections::HashMap::new();
    for item in &walk.runs {
        *visits.entry((item.wrapper, &item.path)).or_insert(0usize) += 1;
    }
    let mut texts: Vec<String> = Vec::new();
    let mut previous = None;
    for (run_index, item) in walk.runs.iter().enumerate() {
        if previous != Some(item.stretch) {
            texts.push(String::new());
            previous = Some(item.stretch);
        }
        let length = text_run(paragraph, &walk.wrappers, item).map_or(0, |run| run.content.len());
        let stretch = texts.len() - 1;
        for content in 0..length {
            let output = texts.last_mut().expect("selected stretch");
            if output.is_empty()
                && visits.get(&(item.wrapper, &item.path)) == Some(&1)
                && let Some(text) =
                    owned_walk_text_mut(paragraph, &mut walk.wrappers, item, content)
            {
                if !text.is_empty()
                    && let Some(moved) = moved.as_deref_mut()
                {
                    moved.push(MovedWalkText {
                        stretch,
                        run: run_index,
                        content,
                        length: text.len(),
                    });
                }
                *output = std::mem::take(text);
                continue;
            }
            if let Some(run) = text_run(paragraph, &walk.wrappers, item)
                && let Some(RunContent::Text(text)) = run.content.get(content)
                && let Err(error) = word.append(output, &text.text)
            {
                if let Some(moved) = moved.as_deref() {
                    restore_walk_texts(paragraph, walk, &mut texts, moved);
                }
                return Err(error);
            }
        }
        *visits
            .get_mut(&(item.wrapper, &item.path))
            .expect("counted selected run") -= 1;
    }
    Ok(texts)
}

// These are the two existing Native row visitors, expressed over the unchanged
// model returned by this one parser. Paths consume its producer companions.
fn row_grammar_visits(row: &CT_Row, direct: bool) -> Vec<Vec<crate::text::WordBlockComponent>> {
    use crate::text::WordBlockComponent as C;
    enum Node<'a> {
        Row(&'a CT_Row),
        Cell(&'a CT_Tc),
        Table(&'a CT_Tbl),
        Control(&'a CT_Sdt),
        Paragraph,
        Run,
    }
    let mut pending = vec![(Node::Row(row), vec![C::TableRow(0)])];
    let mut visits = Vec::new();
    while let Some((node, path)) = pending.pop() {
        let mut children = Vec::new();
        match node {
            Node::Paragraph | Node::Run => {
                visits.push(path);
                continue;
            }
            Node::Row(row) => {
                for (index, (_, _, control)) in row.content_controls.iter().enumerate() {
                    children.push((Node::Control(control), C::RowControl(index)));
                }
                for (index, cell) in row.cells.iter().enumerate() {
                    children.push((Node::Cell(cell), C::RowCell(index)));
                }
            }
            Node::Table(table) => {
                for (index, (_, _, control)) in table.content_controls.iter().enumerate() {
                    children.push((Node::Control(control), C::TableControl(index)));
                }
                for (index, row) in table.rows.iter().enumerate() {
                    children.push((Node::Row(row), C::TableRow(index)));
                }
            }
            Node::Cell(cell) => {
                for (index, content) in cell.content.iter().enumerate() {
                    let node = match content {
                        CellContent::Paragraph(_) => Node::Paragraph,
                        CellContent::Table(table) if !direct => Node::Table(table),
                        CellContent::Table(_) => continue,
                        CellContent::ContentControl(control) => Node::Control(control),
                    };
                    children.push((node, C::CellContent(index)));
                }
            }
            Node::Control(control) => {
                for (index, content) in control.content.iter().enumerate() {
                    let node = match content {
                        SdtContent::Paragraph(_) => Node::Paragraph,
                        SdtContent::Table(table) if !direct => Node::Table(table),
                        SdtContent::Table(_) => continue,
                        SdtContent::Row(row) => Node::Row(row),
                        SdtContent::Cell(cell) => Node::Cell(cell),
                        SdtContent::ContentControl(control) => Node::Control(control),
                        SdtContent::Run(_) if direct => Node::Run,
                        SdtContent::Run(_) | SdtContent::RawXml(_) => continue,
                    };
                    children.push((node, C::SdtContent(index)));
                }
            }
        }
        for (child, component) in children.into_iter().rev() {
            let mut child_path = path.clone();
            child_path.push(component);
            pending.push((child, child_path));
        }
    }
    visits
}

enum RowTextOwner<'a> {
    Paragraph(&'a mut CT_P),
    Run(&'a mut CT_R),
}

fn row_text_owner<'a>(
    row: &'a mut CT_Row,
    path: &[crate::text::WordBlockComponent],
) -> crate::Result<RowTextOwner<'a>> {
    use crate::text::WordBlockComponent as C;
    let invalid = || crate::OxmlError::InvalidValue("constructed row Text path changed".into());
    let Some((C::TableRow(0), path)) = path.split_first() else {
        return Err(invalid());
    };
    let mut owner = ParagraphEditOwner::Row(row);
    for (position, component) in path.iter().enumerate() {
        let last = position + 1 == path.len();
        owner = match (owner, component) {
            (ParagraphEditOwner::Row(row), C::RowCell(index)) => {
                ParagraphEditOwner::Cell(row.cells.get_mut(*index).ok_or_else(invalid)?)
            }
            (ParagraphEditOwner::Row(row), C::RowControl(index)) => ParagraphEditOwner::Control(
                &mut row.content_controls.get_mut(*index).ok_or_else(invalid)?.2,
            ),
            (ParagraphEditOwner::Table(table), C::TableRow(index)) => {
                ParagraphEditOwner::Row(table.rows.get_mut(*index).ok_or_else(invalid)?)
            }
            (ParagraphEditOwner::Table(table), C::TableControl(index)) => {
                ParagraphEditOwner::Control(
                    &mut table
                        .content_controls
                        .get_mut(*index)
                        .ok_or_else(invalid)?
                        .2,
                )
            }
            (ParagraphEditOwner::Cell(cell), C::CellContent(index)) => {
                match cell.content.get_mut(*index).ok_or_else(invalid)? {
                    CellContent::Paragraph(paragraph) if last => {
                        return Ok(RowTextOwner::Paragraph(paragraph));
                    }
                    CellContent::Table(table) => ParagraphEditOwner::Table(table),
                    CellContent::ContentControl(control) => ParagraphEditOwner::Control(control),
                    _ => return Err(invalid()),
                }
            }
            (ParagraphEditOwner::Control(control), C::SdtContent(index)) => {
                match control.content.get_mut(*index).ok_or_else(invalid)? {
                    SdtContent::Paragraph(paragraph) if last => {
                        return Ok(RowTextOwner::Paragraph(paragraph));
                    }
                    SdtContent::Run(run) if last => return Ok(RowTextOwner::Run(run)),
                    SdtContent::Table(table) => ParagraphEditOwner::Table(table),
                    SdtContent::Row(row) => ParagraphEditOwner::Row(row),
                    SdtContent::Cell(cell) => ParagraphEditOwner::Cell(cell),
                    SdtContent::ContentControl(control) => ParagraphEditOwner::Control(control),
                    _ => return Err(invalid()),
                }
            }
            _ => return Err(invalid()),
        };
    }
    Err(invalid())
}

struct RowTextPlan {
    path: Vec<crate::text::WordBlockComponent>,
    walk: Option<TextWalk>,
    paragraph: Option<crate::text::WordParagraphReadFacts>,
    run: Option<crate::text::ParsedWordRunTexts>,
}

pub(crate) fn template_row_texts_in(
    row: &mut CT_Row,
    maps: crate::text::WordBlockSourceMaps,
    input: &crate::document::WordSourceMap,
    work: &mut WordOperationWork,
    copies: &mut crate::document::TemplateCopyWork,
) -> crate::Result<(Vec<String>, Vec<usize>, Vec<usize>)> {
    let nested =
        crate::document::returned_block_inputs(crate::document::WordOwner::Row(row), &maps)?
            .0
            .into_iter()
            .map(|(text, event)| (std::ptr::from_ref(text) as usize, event))
            .collect::<std::collections::HashMap<_, _>>();
    let direct = row_grammar_visits(row, true);
    let all = row_grammar_visits(row, false);
    let mut extras = maps
        .paragraphs
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut paragraphs = maps
        .word_texts
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut runs = maps
        .word_runs
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut seen = std::collections::BTreeMap::new();
    let mut plans = Vec::new();
    // Exactly the old direct-before-all order, with no second wrapper read for
    // the same actual parsed paragraph occurrence.
    for path in direct.iter().chain(&all) {
        if seen.contains_key(path) {
            continue;
        }
        let owner = row_text_owner(row, path)?;
        let (walk, paragraph, run) = match owner {
            RowTextOwner::Paragraph(paragraph) => {
                let extras = extras.remove(path).ok_or_else(|| {
                    crate::OxmlError::InvalidValue("missing row paragraph source companion".into())
                })?;
                (
                    Some(text_runs_in(paragraph, Some(&extras), Some(work))?),
                    paragraphs.remove(path).flatten(),
                    None,
                )
            }
            RowTextOwner::Run(_) => (None, None, runs.remove(path).flatten()),
        };
        seen.insert(path.clone(), plans.len());
        plans.push(RowTextPlan {
            path: path.clone(),
            walk,
            paragraph,
            run,
        });
    }
    let empty = Vec::new();
    // All these are genuinely visited by the two already-built views. Select
    // inputs before aggregation so a generated prefix cannot hide later input.
    for plan in &plans {
        let mut read = crate::document::TemplateWordRead {
            map: plan
                .paragraph
                .as_ref()
                .map_or(input, |facts| facts.source_map.as_ref()),
            texts: plan.paragraph.as_ref().map_or(&empty, |facts| &facts.texts),
            work: copies,
            nested: Some(&nested),
        };
        match row_text_owner(row, &plan.path)? {
            RowTextOwner::Paragraph(paragraph) => select_walk_words(
                paragraph,
                plan.walk.as_ref().expect("paragraph walk"),
                &mut read,
            )?,
            RowTextOwner::Run(run) => {
                if let Some(facts) = &plan.run {
                    for (content, value) in run.content.iter().enumerate() {
                        if let RunContent::Text(text) = value {
                            read.select_run(facts, content, text)?;
                        }
                    }
                }
            }
        }
    }
    let mut buffers = Vec::new();
    let mut indexes = Vec::new();
    for mut plan in plans {
        let mut read = crate::document::TemplateWordRead {
            map: plan
                .paragraph
                .as_ref()
                .map_or(input, |facts| facts.source_map.as_ref()),
            texts: plan.paragraph.as_ref().map_or(&empty, |facts| &facts.texts),
            work: copies,
            nested: Some(&nested),
        };
        let values = match row_text_owner(row, &plan.path)? {
            RowTextOwner::Paragraph(paragraph) => owned_walk_stretches(
                paragraph,
                plan.walk.as_mut().expect("paragraph walk"),
                &mut read,
                None,
            )?,
            RowTextOwner::Run(run) => {
                let mut value = String::new();
                for content in &mut run.content {
                    if let RunContent::Text(text) = content {
                        if value.is_empty() {
                            value = std::mem::take(&mut text.text);
                        } else {
                            read.append(&mut value, &text.text)?;
                        }
                    }
                }
                vec![value]
            }
        };
        let start = buffers.len();
        buffers.extend(values);
        indexes.push(start..buffers.len());
    }
    let view = |paths: Vec<Vec<crate::text::WordBlockComponent>>| {
        paths
            .into_iter()
            .flat_map(|path| indexes[seen[&path]].clone())
            .collect::<Vec<_>>()
    };
    Ok((buffers, view(direct), view(all)))
}

// Only the first nonempty uniquely owned Text of a stretch can move. The
// editing consumer restores that exact buffer before model validation/cloning.
fn restore_walk_texts(
    paragraph: &mut CT_P,
    walk: &mut TextWalk,
    texts: &mut [String],
    moved: &[MovedWalkText],
) {
    for moved in moved {
        texts[moved.stretch].truncate(moved.length);
        let slot = owned_walk_text_mut(
            paragraph,
            &mut walk.wrappers,
            &walk.runs[moved.run],
            moved.content,
        )
        .expect("unchanged uniquely owned Text slot");
        *slot = std::mem::take(&mut texts[moved.stretch]);
    }
}

fn owned_walk_text_mut<'a>(
    paragraph: &'a mut CT_P,
    wrappers: &'a mut [Wrapper],
    item: &TextRun,
    content: usize,
) -> Option<&'a mut String> {
    let owner = match item.wrapper {
        Some(index) => &mut wrappers.get_mut(index)?.content,
        None => paragraph,
    };
    owned_paragraph_text_mut(owner, item.path.segments(), content)
}

pub(crate) fn owned_paragraph_text_mut<'a>(
    paragraph: &'a mut CT_P,
    path: &[AcceptedRunPathSegment],
    content: usize,
) -> Option<&'a mut String> {
    let (first, rest) = path.split_first()?;
    match *first {
        AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
            owned_run_text_mut(paragraph.runs.get_mut(index)?, content)
        }
        AcceptedRunPathSegment::ContentControl(index) => owned_control_text_mut(
            &mut paragraph.content_controls.get_mut(index)?.3,
            rest,
            content,
        ),
        AcceptedRunPathSegment::Revision(index) => paragraph
            .revisions
            .get_mut(index)?
            .2
            .template_text_mut(rest, content),
        AcceptedRunPathSegment::Run(_) => None,
    }
}

fn owned_control_text_mut<'a>(
    control: &'a mut CT_Sdt,
    path: &[AcceptedRunPathSegment],
    content: usize,
) -> Option<&'a mut String> {
    let (first, rest) = path.split_first()?;
    match *first {
        AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
            match control.content.get_mut(index)? {
                SdtContent::Run(run) => owned_run_text_mut(run, content),
                _ => None,
            }
        }
        AcceptedRunPathSegment::ContentControl(index) => match control.content.get_mut(index)? {
            SdtContent::ContentControl(child) => owned_control_text_mut(child, rest, content),
            _ => None,
        },
        AcceptedRunPathSegment::Revision(index) => control
            .revisions
            .get_mut(index)?
            .1
            .template_text_mut(rest, content),
        AcceptedRunPathSegment::Run(_) => None,
    }
}

fn owned_run_text_mut(run: &mut CT_R, content: usize) -> Option<&mut String> {
    match run.content.get_mut(content)? {
        RunContent::Text(text) => Some(&mut text.text),
        _ => None,
    }
}

/// Takes a text and the byte offset to search from, and returns the byte
/// range of the next match and the text that replaces it.
type MatchFinder<'a> = dyn FnMut(&str, usize) -> Option<(usize, usize, String)> + 'a;

/// Find each match that `next_match` reports in the text of `para` and
/// replace it.
fn replace_matches(para: &mut CT_P, next_match: &mut MatchFinder<'_>) -> crate::Result<usize> {
    replace_matches_in(para, next_match, None, None)
}

fn replace_matches_in(
    para: &mut CT_P,
    next_match: &mut MatchFinder<'_>,
    extra_maps: Option<&mut WordExtraSourceMaps>,
    mut work: Option<&mut WordOperationWork>,
) -> crate::Result<usize> {
    let TextWalk {
        runs, mut wrappers, ..
    } = text_runs_in(para, extra_maps.as_deref(), work.as_deref_mut())?;
    let mut emptied = Vec::new();
    let mut total = 0;

    // Byte offset in the concatenated paragraph text at which to look for the
    // next match. Resuming *after* the text we just inserted is what keeps this
    // terminating when the replacement itself contains the placeholder
    // (`replace("NAME", "NAME Smith")`); restarting from 0 would rematch the
    // replacement forever.
    let mut search_from = 0usize;

    loop {
        // 1. Concatenate all run text and build a char map.
        let (full_text, char_map) = build_char_map(para, &wrappers, &runs, searched_text);
        if search_from > full_text.len() {
            break;
        }

        // 2. Find the next match (byte offsets in full_text).
        let Some((byte_start, byte_end, replacement)) = next_match(&full_text, search_from) else {
            break;
        };
        let next_char = full_text[byte_start..]
            .chars()
            .next()
            .map_or(1, char::len_utf8);

        // A zero-width match would neither consume input nor advance the
        // cursor, so step past it and the loop always makes progress.
        if byte_start == byte_end {
            search_from = byte_start + next_char;
            continue;
        }

        let Some((match_start, match_end, first_run, last_run)) =
            accepted_match_runs(&full_text, &char_map, &runs, byte_start, byte_end)
        else {
            // Preserve the old whole-stream crossing skip, including a later
            // overlapping candidate that starts before this rejected end.
            search_from = byte_end;
            continue;
        };

        if first_run == last_run {
            // Single-run match: simple in-place replacement on that run's text content.
            edit_run(
                para,
                &mut wrappers,
                &runs[first_run],
                work.as_deref_mut(),
                |run| {
                    replace_in_single_run(run, &char_map, match_start, match_end, &replacement);
                },
            )?;
        } else {
            // Cross-run match: put replacement in first run, clear matched parts from others.
            replace_across_runs(
                para,
                &mut wrappers,
                &runs,
                &char_map,
                match_start..match_end,
                &replacement,
                work.as_deref_mut(),
            )?;
            emptied.extend(first_run + 1..=last_run);
        }

        // Text before `byte_start` is untouched and the replacement now
        // occupies `byte_start..byte_start + replacement.len()`, so this stays
        // on a char boundary of the rebuilt text.
        search_from = byte_start + replacement.len();

        total += 1;
    }

    remove_emptied_runs_in(para, wrappers, &runs, emptied, extra_maps, work)?;
    Ok(total)
}

// Byte ranges come from the unchanged actual matcher. This is the same
// run/stretch decision for selection-before-growth and subsequent mutation.
fn accepted_match_runs(
    text: &str,
    map: &[CharMapping],
    runs: &[TextRun],
    start: usize,
    end: usize,
) -> Option<(usize, usize, usize, usize)> {
    let first = text[..start].chars().count();
    let last = first + text[start..end].chars().count();
    let first_run = map[first].run_index;
    let last_run = map[last - 1].run_index;
    (runs[first_run].stretch == runs[last_run].stretch)
        .then_some((first, last, first_run, last_run))
}

fn selected_field_ancestors<'a>(
    paragraph: &'a CT_P,
    walk: &'a TextWalk,
    map: &[CharMapping],
    ranges: &[std::ops::Range<usize>],
) -> Vec<&'a crate::text::Field> {
    let mut selected = std::collections::BTreeSet::new();
    for range in ranges {
        for item in &map[range.clone()] {
            let mut wrapper = walk.runs[item.run_index].wrapper;
            while let Some(index) = wrapper {
                if matches!(walk.wrappers[index].source, WrapperSource::Field(_)) {
                    selected.insert(index);
                }
                wrapper = walk.wrappers[index].parent;
            }
        }
    }
    selected
        .into_iter()
        .filter_map(|index| {
            let wrapper = &walk.wrappers[index];
            let WrapperSource::Field(run) = wrapper.source else {
                return None;
            };
            let parent = wrapper
                .parent
                .map_or(paragraph, |parent| &walk.wrappers[parent].content);
            let [RunContent::Field(field)] = parent.runs.get(run)?.content.as_slice() else {
                return None;
            };
            Some(field)
        })
        .collect()
}

/// The real external literal event is separate from generated XML provenance.
/// Internal prepared/template replay does not enter this adapter.
fn try_replace_in_paragraph_with_literal(
    paragraph: &mut CT_P,
    placeholder: &str,
    replacement: &str,
    extra_maps: Option<&mut WordExtraSourceMaps>,
    work: &mut WordOperationWork,
    delivered: &std::cell::Cell<bool>,
) -> crate::Result<usize> {
    if placeholder.is_empty() {
        return Ok(0);
    }
    crate::document::admit_word_model(crate::document::WordOwner::Paragraph(paragraph))?;
    let walk = text_runs_in(paragraph, extra_maps.as_deref(), Some(work))?;
    let (text, map) = build_char_map(paragraph, &walk.wrappers, &walk.runs, searched_text);
    let mut from = 0;
    let mut selected = Vec::new();
    while let Some(relative) = text[from..].find(placeholder) {
        let start = from + relative;
        let end = start + placeholder.len();
        if let Some((first, last, _, _)) = accepted_match_runs(&text, &map, &walk.runs, start, end)
        {
            selected.push(first..last);
        }
        from = end;
    }
    let fields = selected_field_ancestors(paragraph, &walk, &map, &selected);
    work.register_original_fields_with_inputs(fields, &[(replacement, delivered)], &[])?;
    drop(walk);
    let mut candidate = paragraph.clone();
    let mut candidate_maps = extra_maps.as_deref().cloned();
    let result = replace_matches_in(
        &mut candidate,
        &mut |text, from| {
            let start = from + text[from..].find(placeholder)?;
            Some((start, start + placeholder.len(), replacement.to_owned()))
        },
        candidate_maps.as_mut(),
        Some(work),
    );
    match result {
        Ok(count) if count > 0 => {
            let previous = std::mem::replace(paragraph, candidate);
            crate::text::discard_checked_paragraph(previous);
            if let (Some(output), Some(maps)) = (extra_maps, candidate_maps) {
                *output = maps;
            }
            Ok(count)
        }
        result => {
            crate::text::discard_checked_paragraph(candidate);
            result
        }
    }
}

/// A run of the text a replacement reads.
struct TextRun {
    /// The stretch of runs it belongs to. A match never spans two.
    stretch: usize,
    /// The wrapper whose content holds it, an index into
    /// [`TextWalk::wrappers`], or None for the paragraph being replaced.
    wrapper: Option<usize>,
    /// Its address in the paragraph that holds it.
    path: AcceptedRunPath,
}

/// A smart tag, an inline custom XML element or a simple field that a
/// replacement reads, in the paragraph being replaced or nested in another
/// such wrapper. Its content is parsed from its preserved source, see
/// [`run_wrapper_paragraph`], and written back into that source only when a
/// replacement changed it, so any other wrapper keeps its bytes.
struct Wrapper {
    content: CT_P,
    /// The wrapper whose content holds this one, or None for the paragraph.
    parent: Option<usize>,
    source: WrapperSource,
    changed: bool,
    source_map: Option<crate::document::WordSourceMap>,
    extra_maps: Option<WordExtraSourceMaps>,
    in_field: bool,
    word_facts: Option<crate::text::WordParagraphReadFacts>,
}

/// Where the source of a [`Wrapper`] is in the paragraph that holds it.
enum WrapperSource {
    /// An index into `extra_xml`.
    Raw(usize),
    /// The index of the run that holds a simple field.
    Field(usize),
}

/// The runs of a paragraph that a replacement reads, see [`text_runs`].
#[derive(Default)]
struct TextWalk {
    runs: Vec<TextRun>,
    wrappers: Vec<Wrapper>,
    /// The stretch of the next run.
    stretch: usize,
    /// The wrapper whose content is being walked.
    wrapper: Option<usize>,
}

impl TextWalk {
    fn push(&mut self, prefix: &[AcceptedRunPathSegment]) {
        self.runs.push(TextRun {
            stretch: self.stretch,
            wrapper: self.wrapper,
            path: AcceptedRunPath {
                segments: prefix.to_vec(),
            },
        });
    }
}

/// The runs of `para` that a replacement reads, in the order of the accepted
/// view that `Paragraph::text` reads, each with the stretch it belongs to
/// and its address, and the wrappers that hold some of them.
fn text_runs_in(
    para: &CT_P,
    extra_maps: Option<&WordExtraSourceMaps>,
    work: Option<&mut WordOperationWork>,
) -> crate::Result<TextWalk> {
    if extra_maps.is_some_and(|maps| maps.len() != para.extra_xml.len()) {
        return Err(crate::OxmlError::InvalidValue(
            "paragraph extra source map alignment changed".into(),
        ));
    }
    let mut walk = TextWalk::default();
    paragraph_text_runs(para, &mut Vec::new(), &mut walk, extra_maps, work)?;
    Ok(walk)
}

/// The run that `run` addresses.
fn text_run<'a>(para: &'a CT_P, wrappers: &'a [Wrapper], run: &TextRun) -> Option<&'a CT_R> {
    run.wrapper
        .map_or(para, |index| &wrappers[index].content)
        .accepted_run(&run.path)
}

fn paragraph_text_runs(
    para: &CT_P,
    prefix: &mut Vec<AcceptedRunPathSegment>,
    walk: &mut TextWalk,
    extra_maps: Option<&WordExtraSourceMaps>,
    mut work: Option<&mut WordOperationWork>,
) -> crate::Result<()> {
    let word = ["w".to_owned()];
    for index in 0..=para.runs.len() {
        for owner in boundary_owners(para, index) {
            match owner {
                BoundaryOwner::ContentControl(control) => {
                    prefix.push(AcceptedRunPathSegment::ContentControl(control));
                    walk.stretch += 1;
                    control_text_runs(
                        &para.content_controls[control].3,
                        prefix,
                        walk,
                        work.as_deref_mut(),
                    )?;
                    walk.stretch += 1;
                    prefix.pop();
                }
                BoundaryOwner::Revision(revision) => revision_text_runs(
                    &para.revisions[revision].2,
                    AcceptedRunPathSegment::Revision(revision),
                    prefix,
                    walk,
                    work.as_deref_mut(),
                )?,
                // `Paragraph::text` reads the wrappers of the paragraph and
                // those nested in them, not those inside a control or a
                // revision.
                BoundaryOwner::Wrapper(raw) if prefix.is_empty() => wrapper_text_runs(
                    &para.extra_xml[raw].1,
                    &word,
                    WrapperSource::Raw(raw),
                    walk,
                    extra_maps.and_then(|maps| maps[raw].as_deref()).cloned(),
                    work.as_deref_mut(),
                )?,
                BoundaryOwner::Wrapper(_) => {}
            }
        }
        let Some(run) = para.runs.get(index) else {
            continue;
        };
        match simple_field_source(run) {
            Some((raw, prefixes)) if prefix.is_empty() => {
                let map = work.as_deref().and_then(|work| {
                    let [RunContent::Field(field)] = run.content.as_slice() else {
                        return None;
                    };
                    work.field_origins
                        .lock()
                        .expect("field origin registry poisoned")
                        .source_map(field)
                });
                wrapper_text_runs(
                    raw,
                    prefixes,
                    WrapperSource::Field(index),
                    walk,
                    map,
                    work.as_deref_mut(),
                )?;
            }
            _ => {
                prefix.push(AcceptedRunPathSegment::Run(index));
                walk.push(prefix);
                prefix.pop();
            }
        }
    }
    Ok(())
}

/// The runs of a smart tag, an inline custom XML element or a simple field
/// form a stretch of their own, so a replacement inside them stays inside
/// the wrapper. A wrapper whose content cannot be read still ends the
/// stretch before it. For a simple field, the runs are its cached result,
/// which Word computes again when it updates the field, and its instruction
/// is never read.
fn wrapper_text_runs(
    raw: &[u8],
    prefixes: &[String],
    source: WrapperSource,
    walk: &mut TextWalk,
    source_map: Option<crate::document::WordSourceMap>,
    mut work: Option<&mut WordOperationWork>,
) -> crate::Result<()> {
    walk.stretch += 1;
    let mapped = work.as_deref().map(|work| {
        crate::document::WordFieldInput::produced(
            source_map
                .clone()
                .unwrap_or_else(|| crate::document::WordSourceMap::generated(raw.len())),
            std::sync::Arc::clone(&work.field_origins),
        )
    });
    let mut extra_maps = Vec::new();
    let mut word_facts = None;
    let word_input = mapped
        .as_ref()
        .is_some_and(|input| input.projection_parts().0.has_word_inputs());
    if let Some(content) = crate::text::run_wrapper_paragraph_in(
        raw,
        prefixes,
        mapped.as_ref(),
        work.as_deref_mut(),
        mapped.as_ref().map(|_| &mut extra_maps),
        word_input.then_some(&mut word_facts),
    )? {
        let in_field = matches!(source, WrapperSource::Field(_))
            || walk
                .wrapper
                .is_some_and(|index| walk.wrappers[index].in_field);
        let index = walk.wrappers.len();
        walk.wrappers.push(Wrapper {
            content: CT_P::new(),
            parent: walk.wrapper,
            source,
            changed: false,
            source_map: mapped
                .as_ref()
                .map(|input| input.projection_parts().0.clone()),
            extra_maps: None,
            in_field,
            word_facts,
        });
        let parent = walk.wrapper.replace(index);
        paragraph_text_runs(
            &content,
            &mut Vec::new(),
            walk,
            mapped.as_ref().map(|_| &extra_maps),
            work,
        )?;
        walk.wrapper = parent;
        walk.wrappers[index].content = content;
        walk.wrappers[index].extra_maps = mapped.map(|_| extra_maps);
    }
    walk.stretch += 1;
    Ok(())
}

fn control_text_runs(
    sdt: &CT_Sdt,
    prefix: &mut Vec<AcceptedRunPathSegment>,
    walk: &mut TextWalk,
    mut work: Option<&mut WordOperationWork>,
) -> crate::Result<()> {
    for index in 0..=sdt.content.len() {
        for (revision, (_, wrapper)) in sdt
            .revisions()
            .iter()
            .enumerate()
            .filter(|(_, (at, _))| *at == index)
        {
            revision_text_runs(
                wrapper,
                AcceptedRunPathSegment::Revision(revision),
                prefix,
                walk,
                work.as_deref_mut(),
            )?;
        }
        match sdt.content.get(index) {
            Some(SdtContent::Run(_)) => {
                prefix.push(AcceptedRunPathSegment::Run(index));
                walk.push(prefix);
                prefix.pop();
            }
            Some(SdtContent::ContentControl(nested)) => {
                prefix.push(AcceptedRunPathSegment::ContentControl(index));
                walk.stretch += 1;
                control_text_runs(nested, prefix, walk, work.as_deref_mut())?;
                walk.stretch += 1;
                prefix.pop();
            }
            _ => {}
        }
    }
    Ok(())
}

/// The runs of a tracked insertion or move destination form a stretch of
/// their own, so a replacement inside them stays inside the wrapper, with
/// its id, author and date. A wrapper whose runs cannot be written back
/// still ends the stretch before it. A deletion or a move source is not
/// read, as the reader does not see it, and does not split a stretch.
fn revision_text_runs(
    revision: &CT_Revision,
    segment: AcceptedRunPathSegment,
    prefix: &mut Vec<AcceptedRunPathSegment>,
    walk: &mut TextWalk,
    work: Option<&mut WordOperationWork>,
) -> crate::Result<()> {
    if !matches!(
        revision.kind(),
        RevisionKind::Insertion | RevisionKind::MoveTo
    ) || revision.content_paragraph().is_none()
    {
        return Ok(());
    }
    prefix.push(segment);
    walk.stretch += 1;
    if let Some(inserted) = revision.editable_accepted_paragraph() {
        paragraph_text_runs(inserted, prefix, walk, None, work)?;
    }
    walk.stretch += 1;
    prefix.pop();
    Ok(())
}

/// Apply `edit` to the run that `run` addresses.
fn edit_run(
    para: &mut CT_P,
    wrappers: &mut [Wrapper],
    run: &TextRun,
    work: Option<&mut WordOperationWork>,
    edit: impl FnOnce(&mut CT_R),
) -> crate::Result<()> {
    let checked = work.is_some();
    let emission = work
        .as_deref()
        .filter(|_| run.wrapper.is_some_and(|index| wrappers[index].in_field))
        .map(WordOperationWork::field_xml_emission);
    let paragraph = match run.wrapper {
        Some(index) => {
            wrappers[index].changed = true;
            &mut wrappers[index].content
        }
        None => para,
    };
    if let Some(mut edited) = paragraph.accepted_run(&run.path).cloned() {
        edit(&mut edited);
        let replaced = paragraph.replace_accepted_run_segments_in(
            run.path.segments(),
            edited,
            work,
            emission.as_ref(),
            false,
        );
        if checked {
            if !replaced? {
                return Err(crate::OxmlError::InvalidValue(
                    "text run path is stale".into(),
                ));
            }
        } else {
            debug_assert!(matches!(replaced, Ok(true)), "text run path is stale");
        }
    }
    Ok(())
}

/// A mapping from character position in the concatenated text to its source run and content item.
#[derive(Debug)]
struct CharMapping {
    /// Index of the run in the list of text runs.
    run_index: usize,
    /// Index of the RunContent item within the run.
    content_index: usize,
    /// Byte offset of the character within the text that item reads as.
    byte_offset: usize,
}

/// The text a search reads in one run item: the text of a `w:t`, and a tab,
/// a break or a carriage return as `Paragraph::text` shows them.
fn searched_text(content: &RunContent) -> Option<&str> {
    match content {
        RunContent::Text(t) => Some(&t.text),
        RunContent::Tab => Some("\t"),
        RunContent::Break(_) | RunContent::SpecialCharacter(SpecialCharacter::CarriageReturn) => {
            Some("\n")
        }
        _ => None,
    }
}

/// The text a prepared template edit reads in one run item: only `w:t`, as
/// the stretches its offsets come from.
fn template_text(content: &RunContent) -> Option<&str> {
    match content {
        RunContent::Text(t) => Some(&t.text),
        _ => None,
    }
}

fn build_char_map(
    para: &CT_P,
    wrappers: &[Wrapper],
    runs: &[TextRun],
    item_text: fn(&RunContent) -> Option<&str>,
) -> (String, Vec<CharMapping>) {
    let mut full_text = String::new();
    let mut char_map = Vec::new();

    for (run_idx, text) in runs.iter().enumerate() {
        let Some(run) = text_run(para, wrappers, text) else {
            continue;
        };
        for (content_idx, content) in run.content.iter().enumerate() {
            if let Some(text) = item_text(content) {
                for (byte_pos, _ch) in text.char_indices() {
                    char_map.push(CharMapping {
                        run_index: run_idx,
                        content_index: content_idx,
                        byte_offset: byte_pos,
                    });
                }
                full_text.push_str(text);
            }
        }
    }

    (full_text, char_map)
}

/// The position just past the character `mapping` points at, in its run.
fn position_after(run: &CT_R, mapping: &CharMapping) -> (usize, usize) {
    let width = searched_text(&run.content[mapping.content_index])
        .and_then(|text| text[mapping.byte_offset..].chars().next())
        .map_or(1, char::len_utf8);
    (mapping.content_index, mapping.byte_offset + width)
}

/// Replace the searched text of `run` from `start` to `end`, each a content
/// item and a byte offset in the text it reads as, with `replacement`.
/// `None` stands for the start or the end of the run, and no replacement
/// only removes the text. Matched text, tabs and breaks go; any other item in
/// the range, such as a drawing or a field, stays, after the replacement.
fn replace_in_run(
    run: &mut CT_R,
    start: Option<(usize, usize)>,
    end: Option<(usize, usize)>,
    replacement: Option<&str>,
) {
    let mut replacement = replacement;
    let content = std::mem::take(&mut run.content);
    for (index, item) in content.into_iter().enumerate() {
        let in_range = start.is_none_or(|(first, _)| index >= first)
            && end.is_none_or(|(last, _)| index <= last);
        if !in_range || searched_text(&item).is_none() {
            run.content.push(item);
            continue;
        }
        match item {
            RunContent::Text(mut t) => {
                let from = start
                    .filter(|(first, _)| *first == index)
                    .map_or(0, |(_, byte)| byte);
                let to = end
                    .filter(|(last, _)| *last == index)
                    .map_or(t.text.len(), |(_, byte)| byte);
                let host = replacement.take();
                if host.is_none() && from == 0 && to == t.text.len() {
                    continue;
                }
                let mut text =
                    String::with_capacity(t.text.len() - (to - from) + host.map_or(0, str::len));
                text.push_str(&t.text[..from]);
                text.push_str(host.unwrap_or(""));
                text.push_str(&t.text[to..]);
                t.text = text;
                // Keep the flag the producer wrote. Dropping it rewrites an
                // unchanged run, which a later comparison against the source
                // then reports.
                t.preserve_space =
                    t.preserve_space || t.text.starts_with(' ') || t.text.ends_with(' ');
                run.content.push(RunContent::Text(t));
            }
            // A tab, a break or a carriage return in the range is matched
            // whole: it goes, and the first one hosts the replacement.
            _ => {
                if let Some(host) = replacement.take() {
                    run.content.push(RunContent::Text(CT_Text::new(host)));
                }
            }
        }
    }
}

fn replace_in_single_run(
    run: &mut CT_R,
    char_map: &[CharMapping],
    match_start: usize,
    match_end: usize,
    replacement: &str,
) {
    let first = &char_map[match_start];
    let end = position_after(run, &char_map[match_end - 1]);
    replace_in_run(
        run,
        Some((first.content_index, first.byte_offset)),
        Some(end),
        Some(replacement),
    );
}

fn replace_across_runs(
    para: &mut CT_P,
    wrappers: &mut [Wrapper],
    runs: &[TextRun],
    char_map: &[CharMapping],
    range: std::ops::Range<usize>,
    replacement: &str,
    mut work: Option<&mut WordOperationWork>,
) -> crate::Result<()> {
    let (match_start, match_end) = (range.start, range.end);
    // The first run takes the replacement in place of its matched text.
    let first_mapping = &char_map[match_start];
    edit_run(
        para,
        wrappers,
        &runs[first_mapping.run_index],
        work.as_deref_mut(),
        |run| {
            replace_in_run(
                run,
                Some((first_mapping.content_index, first_mapping.byte_offset)),
                None,
                Some(replacement),
            );
        },
    )?;

    // The last run loses its text up to the match end.
    let last_mapping = &char_map[match_end - 1];
    edit_run(
        para,
        wrappers,
        &runs[last_mapping.run_index],
        work.as_deref_mut(),
        |run| {
            let end = position_after(run, last_mapping);
            replace_in_run(run, None, Some(end), None);
        },
    )?;

    // The runs strictly between lose all their searched text.
    for text in &runs[first_mapping.run_index + 1..last_mapping.run_index] {
        edit_run(para, wrappers, text, work.as_deref_mut(), |run| {
            replace_in_run(run, None, None, None);
        })?;
    }
    Ok(())
}

/// Remove the runs at `candidates` (indices into `runs`) that the replacement
/// left without any content, keeping every anchor of the paragraph and of
/// its content controls on the boundary that remains, and write each changed
/// wrapper back into the paragraph that holds it.
fn remove_emptied_runs_in(
    para: &mut CT_P,
    mut wrappers: Vec<Wrapper>,
    runs: &[TextRun],
    mut candidates: Vec<usize>,
    mut extra_maps: Option<&mut WordExtraSourceMaps>,
    mut work: Option<&mut WordOperationWork>,
) -> crate::Result<()> {
    candidates.sort_unstable();
    candidates.dedup();
    // Capture each actual immutable wrapper consumer before reverse writeback
    // changes its parent's content. Raw wrapper parsing uses the existing
    // canonical-w entry convention; fields expose their own original scope.
    let contexts = wrappers
        .iter()
        .map(|wrapper| {
            let parent = wrapper
                .parent
                .map_or(&*para, |index| &wrappers[index].content);
            let (raw, inherited) = match wrapper.source {
                WrapperSource::Raw(index) => {
                    let raw = parent.extra_xml.get(index)?.1.as_slice();
                    (raw, vec!["w".to_owned()])
                }
                WrapperSource::Field(index) => {
                    let (raw, prefixes) = simple_field_source(parent.runs.get(index)?)?;
                    (raw, prefixes.to_vec())
                }
            };
            let mut reader = quick_xml::Reader::from_reader(raw);
            let quick_xml::events::Event::Start(root) = reader.read_event().ok()? else {
                return None;
            };
            word_prefixes_at(&root, &inherited)
                .ok()
                .map(std::borrow::Cow::into_owned)
        })
        .collect::<Vec<_>>();
    // A wrapper holds only wrappers read after it, so writing them back in
    // reverse order writes each one into its parent before the parent loses
    // its emptied runs, which would move the run of a simple field.
    while let Some(wrapper) = wrappers.pop() {
        let index = wrappers.len();
        let mut content = wrapper.content;
        let mut content_maps = wrapper.extra_maps;
        let emission = work
            .as_deref()
            .filter(|_| wrapper.in_field)
            .map(WordOperationWork::field_xml_emission);
        // Recovered before this removal, not only at write-back.
        let context = contexts[index]
            .as_deref()
            .map(crate::text::field_scope_bindings);
        remove_paragraph_emptied_runs(
            &mut content,
            candidates
                .iter()
                .filter(|candidate| runs[**candidate].wrapper == Some(index))
                .map(|candidate| &runs[*candidate].path),
            context.as_deref(),
            content_maps.as_mut(),
            work.as_deref_mut(),
            emission.as_ref(),
        )?;
        if !wrapper.changed {
            continue;
        }
        let (parent, parent_maps) = match wrapper.parent {
            Some(parent) => {
                wrappers[parent].changed = true;
                let parent = &mut wrappers[parent];
                (&mut parent.content, parent.extra_maps.as_mut())
            }
            None => (&mut *para, extra_maps.as_deref_mut()),
        };
        let written = match context.as_deref() {
            Some(context) => write_wrapper_back(
                parent,
                &wrapper.source,
                &content,
                content_maps.as_ref(),
                Some(context),
                wrapper.source_map.as_ref(),
                parent_maps,
                work.as_deref_mut(),
                emission.as_ref(),
            )?,
            None => false,
        };
        if !written {
            return Err(crate::OxmlError::InvalidValue(
                "wrapper source write-back was refused".into(),
            ));
        }
    }
    remove_paragraph_emptied_runs(
        para,
        candidates
            .iter()
            .filter(|candidate| runs[**candidate].wrapper.is_none())
            .map(|candidate| &runs[*candidate].path),
        None,
        extra_maps,
        work,
        None,
    )?;
    Ok(())
}

/// Write `content` back as the content of the wrapper at `source` in
/// `paragraph`. Returns false when the source cannot take it.
#[allow(clippy::too_many_arguments)] // Actual source/map companions and inherited Field role are independent inputs.
fn write_wrapper_back(
    paragraph: &mut CT_P,
    source: &WrapperSource,
    content: &CT_P,
    content_maps: Option<&WordExtraSourceMaps>,
    source_context: Option<&[(&str, &str)]>,
    source_map: Option<&crate::document::WordSourceMap>,
    extra_maps: Option<&mut WordExtraSourceMaps>,
    mut work: Option<&mut WordOperationWork>,
    emission: Option<&FieldXmlEmission>,
) -> crate::Result<bool> {
    if work.is_none() {
        return Ok(match *source {
            WrapperSource::Raw(index) => {
                let Some((_, raw)) = paragraph.extra_xml.get_mut(index) else {
                    return Ok(false);
                };
                let Ok(updated) =
                    crate::text::with_run_wrapper_content(raw, content, source_context)
                else {
                    return Ok(false);
                };
                *raw = updated;
                true
            }
            WrapperSource::Field(index) => {
                let Some(run) = paragraph.runs.get_mut(index) else {
                    return Ok(false);
                };
                let Some(Ok(updated)) = simple_field_source(run).map(|(raw, _)| {
                    crate::text::with_run_wrapper_content(raw, content, source_context)
                }) else {
                    return Ok(false);
                };
                crate::text::set_simple_field_source(run, &updated).unwrap_or(false)
            }
        });
    }
    let raw = match *source {
        WrapperSource::Raw(index) => paragraph
            .extra_xml
            .get(index)
            .map(|(_, raw)| raw.as_slice()),
        WrapperSource::Field(index) => paragraph
            .runs
            .get(index)
            .and_then(simple_field_source)
            .map(|(raw, _)| raw),
    };
    let Some(raw) = raw else {
        return Ok(false);
    };
    let mapped = work.as_deref().map(|work| {
        crate::document::WordFieldInput::produced(
            source_map
                .cloned()
                .unwrap_or_else(|| crate::document::WordSourceMap::generated(raw.len())),
            std::sync::Arc::clone(&work.field_origins),
        )
    });
    let result = crate::text::with_run_wrapper_content_in(
        raw,
        content,
        source_context,
        mapped.as_ref(),
        work.as_deref_mut(),
        emission,
        content_maps,
    );
    let (updated, updated_map) = match result {
        Ok(value) => value,
        Err(error) => {
            if mapped
                .as_ref()
                .and_then(crate::document::WordFieldInput::resource_failure)
                .is_some()
                || work
                    .as_deref()
                    .is_some_and(WordOperationWork::resource_refused)
            {
                return Err(error);
            }
            return Ok(false);
        }
    };
    match *source {
        WrapperSource::Raw(index) => {
            if extra_maps
                .as_ref()
                .is_some_and(|maps| maps.len() != paragraph.extra_xml.len())
            {
                return Err(crate::OxmlError::InvalidValue(
                    "paragraph extra source map alignment changed".into(),
                ));
            }
            paragraph.extra_xml[index].1 = updated;
            if let Some(maps) = extra_maps {
                maps[index] = Some(std::sync::Arc::new(updated_map));
            }
            Ok(true)
        }
        WrapperSource::Field(index) => {
            let next = work.as_deref().map(|work| {
                crate::document::WordFieldInput::produced(
                    updated_map,
                    std::sync::Arc::clone(&work.field_origins),
                )
            });
            match crate::text::set_simple_field_source_in(
                &mut paragraph.runs[index],
                &updated,
                next.as_ref(),
                work.as_deref_mut(),
            ) {
                Ok(written) => Ok(written),
                Err(error) => {
                    if next
                        .as_ref()
                        .and_then(crate::document::WordFieldInput::resource_failure)
                        .is_some()
                        || work
                            .as_deref()
                            .is_some_and(WordOperationWork::resource_refused)
                    {
                        Err(error)
                    } else {
                        Ok(false)
                    }
                }
            }
        }
    }
}

/// Remove the runs of `para` at `paths` that the replacement left without
/// any content.
fn remove_paragraph_emptied_runs<'a>(
    para: &mut CT_P,
    paths: impl DoubleEndedIterator<Item = &'a AcceptedRunPath>,
    context: Option<&[(&str, &str)]>,
    extra_maps: Option<&mut WordExtraSourceMaps>,
    mut work: Option<&mut WordOperationWork>,
    emission: Option<&FieldXmlEmission>,
) -> crate::Result<()> {
    let mut direct = vec![false; para.runs.len()];
    // Later runs first, so that removing one inside a control leaves the
    // addresses of the others valid.
    for path in paths.rev() {
        // A run keeps the attributes of its start tag, such as `w:rsidR`, as
        // a raw record, which does not make it worth keeping once empty.
        let emptied = para.accepted_run(path).is_some_and(|run| {
            run.content.is_empty()
                && run
                    .extra_xml_positions
                    .iter()
                    .filter(|position| CT_R::raw_child_is_root_attributes(**position))
                    .count()
                    == run.extra_xml.len()
        });
        if !emptied {
            continue;
        }
        match path.segments() {
            [AcceptedRunPathSegment::Run(run)] => direct[*run] = true,
            nested => remove_paragraph_run(para, nested, context, work.as_deref_mut(), emission)?,
        }
    }
    if direct.contains(&true) {
        let mut candidate = para.clone();
        candidate.remove_runs_core_in(&direct, context, extra_maps, work, emission)?;
        *para = candidate;
    }
    Ok(())
}

fn remove_paragraph_run(
    para: &mut CT_P,
    path: &[AcceptedRunPathSegment],
    context: Option<&[(&str, &str)]>,
    work: Option<&mut WordOperationWork>,
    emission: Option<&FieldXmlEmission>,
) -> crate::Result<()> {
    match path {
        [AcceptedRunPathSegment::Run(index)] if *index < para.runs.len() => {
            let mut removed = vec![false; para.runs.len()];
            removed[*index] = true;
            let mut candidate = para.clone();
            candidate.remove_runs_core_in(&removed, context, None, work, emission)?;
            *para = candidate;
        }
        [AcceptedRunPathSegment::ContentControl(control), rest @ ..] => {
            if let Some((_, _, _, sdt)) = para.content_controls.get_mut(*control) {
                remove_control_run(sdt, rest, context, work, emission)?;
            }
        }
        [AcceptedRunPathSegment::Revision(revision), rest @ ..] => {
            if let Some((_, _, revision)) = para.revisions.get_mut(*revision) {
                remove_revision_run(revision, rest, context, work, emission)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn remove_control_run(
    sdt: &mut CT_Sdt,
    path: &[AcceptedRunPathSegment],
    context: Option<&[(&str, &str)]>,
    work: Option<&mut WordOperationWork>,
    emission: Option<&FieldXmlEmission>,
) -> crate::Result<()> {
    match path {
        [AcceptedRunPathSegment::Run(index)] => sdt.remove_content(*index),
        [AcceptedRunPathSegment::ContentControl(index), rest @ ..] => {
            if let Some(SdtContent::ContentControl(nested)) = sdt.content.get_mut(*index) {
                remove_control_run(nested, rest, context, work, emission)?;
            }
        }
        [AcceptedRunPathSegment::Revision(index), rest @ ..] => {
            if let Some((_, revision)) = sdt.revisions.get_mut(*index) {
                remove_revision_run(revision, rest, context, work, emission)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Remove a run of a tracked insertion or move destination, which keeps
/// the wrapper and the other runs in it.
fn remove_revision_run(
    revision: &mut CT_Revision,
    path: &[AcceptedRunPathSegment],
    context: Option<&[(&str, &str)]>,
    work: Option<&mut WordOperationWork>,
    emission: Option<&FieldXmlEmission>,
) -> crate::Result<()> {
    revision.edit_accepted_paragraph_in(work, emission, |inserted, work| {
        remove_paragraph_run(inserted, path, context, work, emission)?;
        Ok(true)
    })?;
    Ok(())
}

/// Replace all occurrences of `placeholder` in all paragraphs of a slice.
pub fn replace_in_paragraphs(paras: &mut [CT_P], placeholder: &str, replacement: &str) -> usize {
    paras
        .iter_mut()
        .map(|p| replace_in_paragraph(p, placeholder, replacement))
        .sum()
}

/// Replace all occurrences of `placeholder` in a table (recursively handles
/// nested tables and content controls).
pub fn replace_in_table(table: &mut CT_Tbl, placeholder: &str, replacement: &str) -> usize {
    edit_paragraph_owners(ParagraphEditOwner::Table(table), &mut |paragraph| {
        Ok(replace_in_paragraph(paragraph, placeholder, replacement))
    })
    .unwrap_or(0)
}

enum ParagraphEditOwner<'a> {
    Paragraph(&'a mut CT_P),
    Table(&'a mut CT_Tbl),
    Row(&'a mut CT_Row),
    Cell(&'a mut CT_Tc),
    Control(&'a mut CT_Sdt),
}

/// Preserve the existing physical edit order while forwarding each refusal.
fn edit_paragraph_owners(
    root: ParagraphEditOwner<'_>,
    edit: &mut dyn FnMut(&mut CT_P) -> crate::Result<usize>,
) -> crate::Result<usize> {
    edit_paragraph_owners_with_maps(root, None, &mut |paragraph, _, _| edit(paragraph))
}

#[allow(clippy::type_complexity)] // Existing callback with its two actual parser-return companions.
fn edit_paragraph_owners_with_maps(
    root: ParagraphEditOwner<'_>,
    mut source_maps: Option<&mut crate::text::WordBlockSourceMaps>,
    edit: &mut dyn FnMut(
        &mut CT_P,
        Option<&mut WordExtraSourceMaps>,
        Option<&crate::text::WordParagraphReadFacts>,
    ) -> crate::Result<usize>,
) -> crate::Result<usize> {
    use crate::text::WordBlockComponent as C;
    if let Some(maps) = source_maps.as_deref_mut() {
        maps.paragraphs.sort_by(|left, right| left.0.cmp(&right.0));
        maps.word_texts.sort_by(|left, right| left.0.cmp(&right.0));
    }
    // These component coordinates consume the companion emitted by this
    // immediately preceding constructor. They never discover source identity.
    let mapped = source_maps.is_some();
    let mut pending = vec![(root, Vec::new())];
    let mut count = 0;
    while let Some((owner, path)) = pending.pop() {
        match owner {
            ParagraphEditOwner::Paragraph(paragraph) => {
                let companions = source_maps
                    .as_deref_mut()
                    .map(|maps| {
                        let index = maps
                            .paragraphs
                            .binary_search_by(|(known, _)| known.cmp(&path))
                            .map_err(|_| {
                                crate::OxmlError::InvalidValue(
                                    "missing constructed paragraph source companion".into(),
                                )
                            })?;
                        let facts = maps
                            .word_texts
                            .binary_search_by(|(known, _)| known.cmp(&path))
                            .ok()
                            .and_then(|index| maps.word_texts[index].1.as_ref());
                        Ok::<_, crate::OxmlError>((&mut maps.paragraphs[index].1, facts))
                    })
                    .transpose()?;
                let (maps, facts) =
                    companions.map_or((None, None), |(maps, facts)| (Some(maps), facts));
                count += edit(paragraph, maps, facts)?;
            }
            ParagraphEditOwner::Table(table) => {
                pending.extend(table.rows.iter_mut().enumerate().rev().map(|(index, row)| {
                    let mut path = path.clone();
                    if mapped {
                        path.push(C::TableRow(index));
                    }
                    (ParagraphEditOwner::Row(row), path)
                }));
                pending.extend(table.content_controls.iter_mut().enumerate().rev().map(
                    |(index, (_, _, control))| {
                        let mut path = path.clone();
                        if mapped {
                            path.push(C::TableControl(index));
                        }
                        (ParagraphEditOwner::Control(control), path)
                    },
                ));
            }
            ParagraphEditOwner::Row(row) => {
                pending.extend(row.cells.iter_mut().enumerate().rev().map(|(index, cell)| {
                    let mut path = path.clone();
                    if mapped {
                        path.push(C::RowCell(index));
                    }
                    (ParagraphEditOwner::Cell(cell), path)
                }));
                pending.extend(row.content_controls.iter_mut().enumerate().rev().map(
                    |(index, (_, _, control))| {
                        let mut path = path.clone();
                        if mapped {
                            path.push(C::RowControl(index));
                        }
                        (ParagraphEditOwner::Control(control), path)
                    },
                ));
            }
            ParagraphEditOwner::Cell(cell) => {
                pending.extend(cell.content.iter_mut().enumerate().rev().map(
                    |(index, content)| {
                        let mut path = path.clone();
                        if mapped {
                            path.push(C::CellContent(index));
                        }
                        (
                            match content {
                                CellContent::Paragraph(p) => ParagraphEditOwner::Paragraph(p),
                                CellContent::Table(table) => ParagraphEditOwner::Table(table),
                                CellContent::ContentControl(sdt) => {
                                    ParagraphEditOwner::Control(sdt)
                                }
                            },
                            path,
                        )
                    },
                ));
            }
            ParagraphEditOwner::Control(sdt) => {
                pending.extend(sdt.content.iter_mut().enumerate().rev().filter_map(
                    |(index, content)| {
                        let owner = match content {
                            SdtContent::Paragraph(p) => ParagraphEditOwner::Paragraph(p),
                            SdtContent::Table(table) => ParagraphEditOwner::Table(table),
                            SdtContent::Row(row) => ParagraphEditOwner::Row(row),
                            SdtContent::Cell(cell) => ParagraphEditOwner::Cell(cell),
                            SdtContent::ContentControl(sdt) => ParagraphEditOwner::Control(sdt),
                            SdtContent::Run(_) | SdtContent::RawXml(_) => return None,
                        };
                        let mut path = path.clone();
                        if mapped {
                            path.push(C::SdtContent(index));
                        }
                        Some((owner, path))
                    },
                ));
            }
        }
    }
    Ok(count)
}

/// Hand a paragraph kept as raw XML, or the paragraphs of a table or a block
/// content control kept so, to `edit`, and re-serialise the element in place
/// when the edit counts a change. Any other element, one the typed parsers
/// refuse, or one whose namespaces the rewrite cannot keep, see
/// [`with_source_namespaces`], keeps its bytes and counts nothing.
fn edit_raw_block(
    raw: &mut Vec<u8>,
    word_prefixes: &[String],
    edit: &mut dyn FnMut(&mut CT_P) -> crate::Result<usize>,
) -> crate::Result<usize> {
    Ok(edit_text_box_block(raw, word_prefixes, &mut TextBoxEditor::Ordinary(edit), None)?.0)
}

fn edit_text_box_block(
    raw: &mut Vec<u8>,
    word_prefixes: &[String],
    edit: &mut TextBoxEditor<'_, '_>,
    input: Option<&crate::document::WordFieldInput>,
) -> crate::Result<(usize, Option<crate::document::WordSourceMap>)> {
    use quick_xml::events::Event;
    use quick_xml::{Reader, Writer};
    let (record_input, _record_publication) = match input {
        Some(input) => {
            let (input, publication) = input.record_publication(raw)?;
            (Some(input), Some(publication))
        }
        None => (None, None),
    };
    let input = record_input.as_ref();
    let unchanged = || {
        let map = input.map(|input| {
            let mut map = input.projection_parts().0.clone();
            map.freeze_publication();
            map
        });
        (0, map)
    };
    let mut reader = Reader::from_reader(raw.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let Ok(Event::Start(start)) = reader.read_event_into(&mut buffer) else {
        return Ok(unchanged());
    };
    let Ok(prefixes) = word_prefixes_at(&start, word_prefixes) else {
        return Ok(unchanged());
    };
    let Ok(bindings) = local_namespace_overrides(&start, word_prefixes) else {
        return Ok(unchanged());
    };
    let paragraph_root = is_word_element(start.name().as_ref(), b"p", &prefixes);
    let table_root = is_word_element(start.name().as_ref(), b"tbl", &prefixes);
    let control_root = is_word_element(start.name().as_ref(), b"sdt", &prefixes);
    if !paragraph_root && !table_root && !control_root {
        return Ok(unchanged());
    }
    let admitted = crate::document::admit_raw_word_source(
        raw,
        word_prefixes,
        control_root.then_some(crate::content_control::SdtOwner::Body),
    )?;
    let mut controls = admitted
        .as_ref()
        .map(crate::document::AdmittedWordSource::controls);
    controls = match (controls, input) {
        (Some(control), Some(input)) => {
            let (map, registry, initial) = input.projection_parts();
            Some(
                control
                    .with_field_origins(registry, initial)
                    .bind_capture(raw, std::sync::Arc::new(map.clone())),
            )
        }
        (controls, _) => controls,
    };
    let collect = matches!(edit, TextBoxEditor::Template(_));
    let mut writer = Writer::new(Vec::new());
    let mut produced_map = None;
    let count = if paragraph_root {
        let mut extras = Vec::new();
        let mut texts = Vec::new();
        let mut nested = crate::text::WordBlockSourceMaps::default();
        let result = CT_P::from_xml_with_read_output(
            &mut reader,
            &prefixes,
            Some(&start),
            controls.as_mut(),
            crate::text::WordParagraphReadOutput {
                extra_maps: collect.then_some(&mut extras),
                text_facts: collect.then_some(&mut texts),
                nested_maps: collect.then_some(&mut nested),
                ..Default::default()
            },
        );
        if let TextBoxEditor::Template(context) = edit {
            check_source_failure(Some(context.work))?;
        }
        let Ok(mut paragraph) = result else {
            return Ok(unchanged());
        };
        if let (Some(input), TextBoxEditor::Template(context)) = (input, &mut *edit) {
            context
                .work
                .field_origins
                .lock()
                .expect("field origin registry poisoned")
                .commit_publication_owner(
                    crate::document::WordOwner::Paragraph(&paragraph),
                    input.projection_parts().0,
                );
        }
        let facts = controls
            .as_ref()
            .and_then(|cursor| cursor.paragraph_word_facts(&reader, texts))
            .map(|mut facts| {
                facts.nested = nested;
                facts
            });
        let count = edit.paragraph(
            &mut paragraph,
            collect.then_some(&mut extras),
            facts.as_ref(),
        )?;
        if count == 0 {
            return Ok(unchanged());
        }
        match edit {
            TextBoxEditor::Ordinary(_) => paragraph.to_xml(&mut writer)?,
            TextBoxEditor::Template(context) => {
                let (bytes, mut map, events) = crate::text::final_word_source(
                    crate::text::FinalWordBlock::Paragraph(&paragraph),
                    context.work,
                )?;
                for event in events {
                    map.push_word_event(event)?;
                }
                writer.get_mut().extend_from_slice(&bytes);
                produced_map = Some(map);
            }
        }
        count
    } else if table_root {
        let mut maps = crate::text::WordBlockSourceMaps {
            collect_word_texts: collect,
            ..Default::default()
        };
        let result = CT_Tbl::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
            &mut reader,
            &prefixes,
            &bindings,
            0,
            None,
            controls.as_mut(),
            collect.then_some(&mut maps),
        );
        if let TextBoxEditor::Template(context) = edit {
            check_source_failure(Some(context.work))?;
        }
        let Ok(mut table) = result else {
            return Ok(unchanged());
        };
        if let (Some(input), TextBoxEditor::Template(context)) = (input, &mut *edit) {
            context
                .work
                .field_origins
                .lock()
                .expect("field origin registry poisoned")
                .commit_publication_owner(
                    crate::document::WordOwner::Table(&table),
                    input.projection_parts().0,
                );
        }
        let count = edit_paragraph_owners_with_maps(
            ParagraphEditOwner::Table(&mut table),
            collect.then_some(&mut maps),
            &mut |paragraph, maps, facts| edit.paragraph(paragraph, maps, facts),
        )?;
        if count == 0 {
            return Ok(unchanged());
        }
        match edit {
            TextBoxEditor::Ordinary(_) => table.to_xml(&mut writer)?,
            TextBoxEditor::Template(context) => {
                let (bytes, mut map, events) = crate::text::final_word_source(
                    crate::text::FinalWordBlock::Table(&table),
                    context.work,
                )?;
                for event in events {
                    map.push_word_event(event)?;
                }
                writer.get_mut().extend_from_slice(&bytes);
                produced_map = Some(map);
            }
        }
        count
    } else {
        let mut maps = crate::text::WordBlockSourceMaps {
            collect_word_texts: collect,
            ..Default::default()
        };
        let result = CT_Sdt::from_raw_with_mapped_context_and_maps(
            raw,
            word_prefixes,
            crate::content_control::SdtOwner::Body,
            controls.as_mut(),
            input.map(|input| std::sync::Arc::new(input.projection_parts().0.clone())),
            collect.then_some(&mut maps),
        );
        if let TextBoxEditor::Template(context) = edit {
            check_source_failure(Some(context.work))?;
        }
        let Some(mut control) = result else {
            return Ok(unchanged());
        };
        if let (Some(input), TextBoxEditor::Template(context)) = (input, &mut *edit) {
            context
                .work
                .field_origins
                .lock()
                .expect("field origin registry poisoned")
                .commit_publication_owner(
                    crate::document::WordOwner::Control(&control),
                    input.projection_parts().0,
                );
        }
        let count = edit_paragraph_owners_with_maps(
            ParagraphEditOwner::Control(&mut control),
            collect.then_some(&mut maps),
            &mut |paragraph, maps, facts| edit.paragraph(paragraph, maps, facts),
        )?;
        if count == 0 {
            return Ok(unchanged());
        }
        match edit {
            TextBoxEditor::Ordinary(_) => control.to_xml(&mut writer)?,
            TextBoxEditor::Template(context) => {
                let (bytes, mut map, events) = crate::text::final_word_source(
                    crate::text::FinalWordBlock::Control(&control),
                    context.work,
                )?;
                for event in events {
                    map.push_word_event(event)?;
                }
                writer.get_mut().extend_from_slice(&bytes);
                produced_map = Some(map);
            }
        }
        count
    };
    let bytes = writer.into_inner();
    let (rewritten, mut map) = if let Some(map) = produced_map {
        let Ok((bytes, map)) = crate::text::raw_with_external_bindings_and_map(
            &bytes,
            &bindings,
            None,
            Some(std::sync::Arc::new(map)),
        ) else {
            if let TextBoxEditor::Template(context) = edit {
                check_source_failure(Some(context.work))?;
            }
            return Ok(unchanged());
        };
        if source_namespaces_compatible(raw, &bytes, word_prefixes) != Some(true) {
            return Ok(unchanged());
        }
        (bytes, map.map(|map| (*map).clone()))
    } else {
        let Some(bytes) = with_source_namespaces(raw, &bytes, &bindings, word_prefixes) else {
            return Ok(unchanged());
        };
        (bytes, None)
    };
    if let Some(map) = &mut map {
        map.freeze_publication();
    }
    *raw = rewritten;
    Ok((count, map))
}

/// Run the literal matcher on one namespace-closed paragraph, table, control or cell.
/// Unlike the legacy raw walker, parsing and namespace refusal are errors.
#[doc(hidden)]
pub fn try_replace_in_block(
    raw: &mut Vec<u8>,
    paragraph: Option<usize>,
    placeholder: &str,
    replacement: &str,
) -> crate::Result<usize> {
    try_replace_in_block_with_constructors(raw, paragraph, placeholder, replacement, None)
}

// Standalone callers admit their own source. Scoped operations supply outcomes
// from the same original source without granting a new expanded-byte allowance.
pub(crate) fn try_replace_in_block_with_constructors(
    raw: &mut Vec<u8>,
    paragraph: Option<usize>,
    placeholder: &str,
    replacement: &str,
    constructors: Option<(
        crate::document::ConstructorFailures,
        crate::document::WordSourceMap,
    )>,
) -> crate::Result<usize> {
    try_replace_in_block_with_constructors_in(
        raw,
        paragraph,
        placeholder,
        replacement,
        constructors,
        None,
    )
}

pub(crate) struct ScopedLiteralContext<'a> {
    pub(crate) work: &'a mut WordOperationWork,
    pub(crate) delivered: &'a std::cell::Cell<bool>,
    pub(crate) initial_delivery: bool,
}

fn replace_literal_block_owners(
    root: ParagraphEditOwner<'_>,
    maps: Option<&mut crate::text::WordBlockSourceMaps>,
    selected: Option<*const CT_P>,
    placeholder: &str,
    replacement: &str,
    mut context: Option<&mut ScopedLiteralContext<'_>>,
) -> crate::Result<usize> {
    edit_paragraph_owners_with_maps(root, maps, &mut |paragraph, maps, _| {
        if selected.is_some_and(|target| !std::ptr::eq(target, paragraph)) {
            return Ok(0);
        }
        if let Some(context) = context.as_deref_mut() {
            try_replace_in_paragraph_with_literal(
                paragraph,
                placeholder,
                replacement,
                maps,
                context.work,
                context.delivered,
            )
        } else {
            try_replace_in_paragraph(paragraph, placeholder, replacement)
        }
    })
}

fn check_source_failure(work: Option<&WordOperationWork>) -> crate::Result<()> {
    if let Some(work) = work {
        if let Some(error) = work
            .field_origins
            .lock()
            .expect("field origin registry poisoned")
            .source_resource_failure()
        {
            return Err(error);
        }
        if work.resource_refused() {
            return Err(work.writer_error(crate::OxmlError::InvalidValue(
                "mapped source resource admission refused".into(),
            )));
        }
    }
    Ok(())
}

pub(crate) fn try_replace_in_block_with_constructors_in(
    raw: &mut Vec<u8>,
    paragraph: Option<usize>,
    placeholder: &str,
    replacement: &str,
    constructors: Option<(
        crate::document::ConstructorFailures,
        crate::document::WordSourceMap,
    )>,
    mut context: Option<ScopedLiteralContext<'_>>,
) -> crate::Result<usize> {
    use crate::OxmlError;
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;
    use quick_xml::{NsReader, Reader, Writer};

    // Validate the whole closed source, including attributes and opaque children.
    let mut checked = NsReader::from_reader(raw.as_slice());
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut roots = 0usize;
    loop {
        let (namespace, event) = checked.read_resolved_event_into(&mut buffer)?;
        if matches!(namespace, ResolveResult::Unknown(_)) {
            return Err(OxmlError::InvalidValue(
                "unbound scoped replacement element".into(),
            ));
        }
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                if depth == 0 {
                    roots += 1;
                }
                for attribute in start.attributes() {
                    let attribute = attribute?;
                    if matches!(
                        checked.resolver().resolve_attribute(attribute.key).0,
                        ResolveResult::Unknown(_)
                    ) {
                        return Err(OxmlError::InvalidValue(
                            "unbound scoped replacement attribute".into(),
                        ));
                    }
                }
            }
            Event::End(_) if depth == 0 => {
                return Err(OxmlError::InvalidValue("unmatched scoped block end".into()));
            }
            Event::Eof => {
                if depth != 0 || roots != 1 {
                    return Err(OxmlError::InvalidValue(
                        "scoped replacement needs one complete block".into(),
                    ));
                }
                break;
            }
            Event::Text(ref text)
                if depth == 0 && text.as_ref().iter().any(|b| !b.is_ascii_whitespace()) =>
            {
                return Err(OxmlError::InvalidValue("text outside scoped block".into()));
            }
            Event::DocType(_) => {
                return Err(OxmlError::InvalidValue(
                    "scoped block forbids a document type".into(),
                ));
            }
            _ => {}
        }
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        buffer.clear();
    }
    let mut reader = Reader::from_reader(raw.as_slice());
    let start = match reader.read_event_into(&mut buffer)? {
        Event::Start(start) | Event::Empty(start) => start.into_owned(),
        _ => {
            return Err(OxmlError::InvalidValue(
                "scoped replacement needs one block".into(),
            ));
        }
    };
    let prefixes = word_prefixes_at(&start, &[])?;
    let bindings = local_namespace_overrides(&start, &[])?;
    let root_kind = [
        b"p".as_slice(),
        b"tbl".as_slice(),
        b"sdt".as_slice(),
        b"tc".as_slice(),
    ]
    .into_iter()
    .find(|local| {
        is_word_element(start.name().as_ref(), local, &prefixes)
            && (paragraph.is_none() || *local == b"tc")
    })
    .ok_or_else(|| OxmlError::InvalidValue("unsupported scoped replacement block".into()))?;
    let root_owner = (root_kind == b"sdt").then_some(crate::content_control::SdtOwner::Body);
    let captured_block_map = context.as_ref().and_then(|_| {
        constructors
            .as_ref()
            .map(|(_, map)| std::sync::Arc::new(map.clone()))
    });
    let admitted = if let Some((constructors, source_map)) = constructors {
        crate::document::admit_word_input_with_constructors(
            crate::document::WordXmlInput::Contiguous(raw),
            &[],
            None,
            root_owner,
            constructors,
        )?
        .with_source_map(source_map)?
    } else {
        crate::document::admit_word_source(raw, &[], None, root_owner)?
    };
    let mut control_facts = admitted.controls_for(raw);
    if let Some(context) = context.as_ref() {
        control_facts = control_facts.with_field_origins(
            std::sync::Arc::clone(&context.work.field_origins),
            context.initial_delivery,
        );
    }
    let mut writer = Writer::new(Vec::new());
    let mut maps = context
        .as_ref()
        .map(|_| crate::text::WordBlockSourceMaps::default());
    let count = if root_kind == b"p" {
        let mut paragraph_maps = maps.as_ref().map(|_| Vec::new());
        let mut value = CT_P::from_xml_with_revision_facts(
            &mut reader,
            &prefixes,
            Some(&start),
            None,
            Some(&mut control_facts),
            paragraph_maps.as_mut(),
            None,
        )?;
        if let (Some(maps), Some(paragraph_maps)) = (maps.as_mut(), paragraph_maps) {
            maps.paragraph(paragraph_maps);
        }
        check_source_failure(context.as_ref().map(|context| &*context.work))?;
        let count = replace_literal_block_owners(
            ParagraphEditOwner::Paragraph(&mut value),
            maps.as_mut(),
            None,
            placeholder,
            replacement,
            context.as_mut(),
        )?;
        if count > 0 {
            let _publication_scope = crate::text::raw_root_binding_scope(raw, true)?;
            if let Some(context) = context.as_mut() {
                crate::text::write_final_word_block(
                    crate::text::FinalWordBlock::Paragraph(&value),
                    &mut writer,
                    context.work,
                )?;
            } else {
                value.to_xml(&mut writer)?;
            }
        }
        count
    } else if root_kind == b"tbl" {
        let mut value = CT_Tbl::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
            &mut reader,
            &prefixes,
            &bindings,
            0,
            Some(&start),
            Some(&mut control_facts),
            maps.as_mut(),
        )?;
        check_source_failure(context.as_ref().map(|context| &*context.work))?;
        let count = replace_literal_block_owners(
            ParagraphEditOwner::Table(&mut value),
            maps.as_mut(),
            None,
            placeholder,
            replacement,
            context.as_mut(),
        )?;
        if count > 0 {
            let _publication_scope = crate::text::raw_root_binding_scope(raw, true)?;
            if let Some(context) = context.as_mut() {
                crate::text::write_final_word_block(
                    crate::text::FinalWordBlock::Table(&value),
                    &mut writer,
                    context.work,
                )?;
            } else {
                value.to_xml(&mut writer)?;
            }
        }
        count
    } else if root_kind == b"sdt" {
        let value = CT_Sdt::from_raw_with_mapped_context_and_maps(
            raw,
            &prefixes,
            crate::content_control::SdtOwner::Body,
            Some(&mut control_facts),
            captured_block_map,
            maps.as_mut(),
        );
        check_source_failure(context.as_ref().map(|context| &*context.work))?;
        let mut value = value
            .ok_or_else(|| OxmlError::InvalidValue("unsupported scoped content control".into()))?;
        let count = replace_literal_block_owners(
            ParagraphEditOwner::Control(&mut value),
            maps.as_mut(),
            None,
            placeholder,
            replacement,
            context.as_mut(),
        )?;
        if count > 0 {
            let _publication_scope = crate::text::raw_root_binding_scope(raw, true)?;
            if let Some(context) = context.as_mut() {
                crate::text::write_final_word_block(
                    crate::text::FinalWordBlock::Control(&value),
                    &mut writer,
                    context.work,
                )?;
            } else {
                value.to_xml(&mut writer)?;
            }
        }
        count
    } else if root_kind == b"tc" {
        let mut value = CT_Tc::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
            &mut reader,
            &prefixes,
            &bindings,
            0,
            Some(&start),
            Some(&mut control_facts),
            maps.as_mut(),
        )?;
        check_source_failure(context.as_ref().map(|context| &*context.work))?;
        let selected = paragraph
            .map(|index| {
                value
                    .paragraphs()
                    .get(index)
                    .map(|p| *p as *const CT_P)
                    .ok_or_else(|| {
                        OxmlError::InvalidValue(format!("cell paragraph {index} is out of range"))
                    })
            })
            .transpose()?;
        let count = replace_literal_block_owners(
            ParagraphEditOwner::Cell(&mut value),
            maps.as_mut(),
            selected,
            placeholder,
            replacement,
            context.as_mut(),
        )?;
        if count > 0 {
            let _publication_scope = crate::text::raw_root_binding_scope(raw, true)?;
            if let Some(context) = context.as_mut() {
                crate::text::write_final_word_block(
                    crate::text::FinalWordBlock::Cell(&value),
                    &mut writer,
                    context.work,
                )?;
            } else {
                value.to_xml(&mut writer)?;
            }
        }
        count
    } else {
        unreachable!("root ownership validated above")
    };
    if count > 0 {
        // These concrete typed publishers generate canonical Word names when
        // the source has no w binding. The snapshot's conventional fallback
        // is not a physical declaration in a standalone fragment. Close that
        // generated binding explicitly, while an actual foreign w still wins.
        let mut publication_bindings = bindings;
        if !publication_bindings.iter().any(|(prefix, _)| prefix == "w") {
            publication_bindings.push(("w".into(), W_NS.into()));
        }
        let rewritten =
            with_source_namespaces(raw, &writer.into_inner(), &publication_bindings, &[])
                .ok_or_else(|| {
                    OxmlError::InvalidValue(
                        "scoped replacement cannot preserve namespace bindings".into(),
                    )
                })?;
        *raw = restore_scoped_container_attributes(raw, &rewritten)?;
    }
    Ok(count)
}

// Table and cell models do not retain their root attributes. Keep their source
// carriers only after proving that replacement preserved qualified container
// order and ancestry. This correction belongs to the scoped transaction only.
struct ScopedContainerStart {
    range: std::ops::Range<usize>,
    local: Vec<u8>,
    parent: Option<usize>,
    start: quick_xml::events::BytesStart<'static>,
    empty: bool,
    attributes: Option<Vec<u8>>,
    scope: Vec<String>,
}

fn scoped_container_starts(raw: &[u8]) -> crate::Result<Vec<ScopedContainerStart>> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let mut stack: Vec<(Vec<String>, Option<usize>)> = Vec::new();
    let mut result = Vec::new();
    loop {
        let from = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer)?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let inherited = stack.last().map_or(&[][..], |entry| entry.0.as_slice());
                let scope = word_prefixes_at(start, inherited)?;
                let parent = stack.last().and_then(|entry| entry.1);
                let local = [b"tbl".as_slice(), b"tc".as_slice()]
                    .into_iter()
                    .find(|local| is_word_element(start.name().as_ref(), local, &scope));
                let owner = if let Some(local) = local {
                    let index = result.len();
                    result.push(ScopedContainerStart {
                        range: from..reader.buffer_position() as usize,
                        local: local.to_vec(),
                        parent,
                        start: start.clone().into_owned(),
                        empty: matches!(event, Event::Empty(_)),
                        attributes: crate::text::capture_root_attribute_record(start, &scope)?,
                        scope: scope.to_vec(),
                    });
                    Some(index)
                } else {
                    parent
                };
                if matches!(event, Event::Start(_)) {
                    stack.push((scope.into_owned(), owner));
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(result)
}

fn restore_scoped_container_attributes(source: &[u8], rewritten: &[u8]) -> crate::Result<Vec<u8>> {
    use crate::OxmlError;
    use quick_xml::events::Event;
    let original = scoped_container_starts(source)?;
    let retained = scoped_container_starts(rewritten)?;
    let refuse =
        || OxmlError::InvalidValue("scoped container attribute correspondence changed".into());
    if original.len() != retained.len() {
        return Err(refuse());
    }
    let mut output = rewritten.to_vec();
    for (original, mut retained) in original.into_iter().zip(retained).rev() {
        if original.local != retained.local || original.parent != retained.parent {
            return Err(refuse());
        }
        if original.attributes == retained.attributes {
            continue;
        }
        let Some(attributes) = original.attributes else {
            return Err(refuse());
        };
        if retained.attributes.is_some() {
            return Err(refuse());
        }
        // A declaration restored for an attribute must not redirect names in
        // the rewritten subtree. Refuse conflicting inherited bindings too.
        let mut reader = quick_xml::Reader::from_reader(attributes.as_slice());
        if let Event::Empty(record) = reader.read_event()? {
            let bindings = namespace_bindings(&retained.scope);
            for attribute in record.attributes() {
                let attribute = attribute?;
                if let Some(prefix) = attribute.key.as_ref().strip_prefix(b"xmlns:")
                    && bindings.iter().any(|(bound, uri)| {
                        bound.as_bytes() == prefix && uri.as_bytes() != attribute.value.as_ref()
                    })
                {
                    return Err(refuse());
                }
            }
        } else {
            return Err(refuse());
        }
        crate::text::push_root_attribute_record(
            &mut retained.start,
            &attributes,
            None,
            crate::text::XmlWriteRole::Publication,
        )?;
        let mut writer = quick_xml::Writer::new(Vec::new());
        writer.write_event(if retained.empty {
            Event::Empty(retained.start)
        } else {
            Event::Start(retained.start)
        })?;
        output.splice(retained.range, writer.into_inner());
    }
    Ok(output)
}

/// Declare the namespaces that the start tag of `source` declares other
/// than the part does, `start_bindings`, again on `rewritten`, the element
/// the typed writers produced from it, since they leave them out. They leave
/// out a declaration below the start tag too, so a prefix of `rewritten`
/// left to the part must not be one that `source` declares with a namespace
/// the part does not bind it to. Otherwise the prefix would be unbound, or
/// name another namespace, and this returns `None`.
fn with_source_namespaces(
    source: &[u8],
    rewritten: &[u8],
    start_bindings: &[(String, String)],
    word_prefixes: &[String],
) -> Option<Vec<u8>> {
    let rewritten = raw_with_external_bindings(rewritten, start_bindings).ok()?;
    source_namespaces_compatible(source, &rewritten, word_prefixes)?.then_some(rewritten)
}

fn source_namespaces_compatible(
    source: &[u8],
    rewritten: &[u8],
    word_prefixes: &[String],
) -> Option<bool> {
    let (_, declared) = namespace_use(source)?;
    let (left_to_part, _) = namespace_use(rewritten)?;
    let part = namespace_bindings(word_prefixes);
    let part_namespace = |prefix: &str| {
        part.iter()
            .find(|(candidate, _)| candidate == prefix)
            .map(|(_, namespace)| namespace.as_str())
            .or_else(|| word_prefixes.iter().any(|p| p == prefix).then_some(W_NS))
    };
    Some(left_to_part.iter().all(|prefix| {
        declared
            .iter()
            .filter(|(candidate, _)| candidate == prefix)
            .all(|(_, namespace)| part_namespace(prefix) == Some(namespace.as_str()))
    }))
}

/// The prefixes an element uses outside every declaration it makes, and
/// every declaration it makes, as prefix and namespace.
type NamespaceUse = (Vec<String>, Vec<(String, String)>);

/// The [`NamespaceUse`] of `xml`, the empty prefix standing for the default
/// namespace of an unprefixed element.
fn namespace_use(xml: &[u8]) -> Option<NamespaceUse> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let prefix_of = |name: &[u8]| {
        let name = std::str::from_utf8(name).ok()?;
        Some(name.split_once(':').map(|(prefix, _)| prefix.to_owned()))
    };
    let mut reader = Reader::from_reader(xml);
    let mut scopes: Vec<Vec<(String, String)>> = Vec::new();
    let mut unbound: Vec<String> = Vec::new();
    let mut declared = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let (element, opens) = match reader.read_event_into(&mut buffer).ok()? {
            Event::Start(element) => (element.into_owned(), true),
            Event::Empty(element) => (element.into_owned(), false),
            Event::End(_) => {
                scopes.pop();
                buffer.clear();
                continue;
            }
            Event::Eof => break,
            _ => {
                buffer.clear();
                continue;
            }
        };
        buffer.clear();
        let declarations = local_namespace_overrides(&element, &[]).ok()?;
        let mut prefixes = vec![prefix_of(element.name().as_ref())?.unwrap_or_default()];
        for attribute in element.attributes() {
            let key = attribute.ok()?.key;
            let key = key.as_ref();
            if key != b"xmlns" && !key.starts_with(b"xmlns:") {
                prefixes.extend(prefix_of(key)?);
            }
        }
        for prefix in prefixes {
            let bound = prefix == "xml"
                || declarations
                    .iter()
                    .chain(scopes.iter().flatten())
                    .any(|(candidate, _)| *candidate == prefix);
            if !bound && !unbound.contains(&prefix) {
                unbound.push(prefix);
            }
        }
        declared.extend(declarations.iter().cloned());
        if opens {
            scopes.push(declarations);
        }
    }
    Some((unbound, declared))
}

/// Replace all occurrences of `placeholder` in a header or footer.
///
/// Reaches its paragraphs and those of the tables and block content controls
/// it keeps as raw XML. A raw element the replacement changed is written back
/// in its place, and the others keep their bytes.
pub fn replace_in_header_footer(hf: &mut CT_HdrFtr, placeholder: &str, replacement: &str) -> usize {
    edit_header_footer(hf, &mut |paragraph| {
        replace_in_paragraph(paragraph, placeholder, replacement)
    })
}

/// Fallible header/footer replacement for checked document operations.
#[doc(hidden)]
pub fn try_replace_in_header_footer(
    hf: &mut CT_HdrFtr,
    placeholder: &str,
    replacement: &str,
) -> crate::Result<usize> {
    for paragraph in &hf.paragraphs {
        crate::document::admit_word_model(crate::document::WordOwner::Paragraph(paragraph))?;
    }
    let mut candidate = hf.clone();
    let count = try_edit_header_footer(&mut candidate, &mut |p| {
        try_replace_in_paragraph(p, placeholder, replacement)
    })?;
    if count > 0 {
        *hf = candidate;
    }
    Ok(count)
}

/// The texts a replacement in `hf` matches against, see [`replaceable_texts`].
#[doc(hidden)]
pub fn header_footer_replaceable_texts(hf: &CT_HdrFtr) -> Vec<String> {
    let mut texts = Vec::new();
    edit_header_footer(&mut hf.clone(), &mut |paragraph| {
        texts.extend(replaceable_texts(paragraph));
        0
    });
    texts
}

fn edit_header_footer(hf: &mut CT_HdrFtr, edit: &mut dyn FnMut(&mut CT_P) -> usize) -> usize {
    let mut count = 0;
    for paragraph in &mut hf.paragraphs {
        count += edit(paragraph);
    }
    for raw in &mut hf.extra_xml {
        count += edit_raw_block(raw, &hf.word_prefixes, &mut |p| Ok(edit(p))).unwrap_or(0);
    }
    count
}

fn try_edit_header_footer(
    hf: &mut CT_HdrFtr,
    edit: &mut dyn FnMut(&mut CT_P) -> crate::Result<usize>,
) -> crate::Result<usize> {
    let mut count = 0;
    for paragraph in &mut hf.paragraphs {
        count += edit(paragraph)?;
    }
    for raw in &mut hf.extra_xml {
        count += edit_raw_block(raw, &hf.word_prefixes, edit)?;
    }
    Ok(count)
}

/// Replace placeholders in text boxes and shapes within a raw XML part.
///
/// Walks the XML, finds `w:txbxContent` elements at any depth outside another
/// text box, parses their child `w:p` elements using `CT_P::from_xml`, and
/// their child tables and block content controls with the typed parsers,
/// performs replacement, and re-serializes the children it changed. Every
/// other child of the text box, such as a bookmark or a paragraph without a
/// match, is copied through verbatim in its place. A text box nested inside
/// another one is kept as it is, not edited. Word writes a text box twice in
/// `mc:AlternateContent`, as DrawingML in `mc:Choice` and as VML in
/// `mc:Fallback`. Every copy is edited, and a match counts once, from the
/// first Choice that holds a text box. Returns the modified XML and
/// replacement count.
pub fn replace_in_xml_part(
    xml: &[u8],
    placeholder: &str,
    replacement: &str,
) -> crate::error::Result<(Vec<u8>, usize)> {
    replace_many_in_xml_part(xml, &[(placeholder, replacement)])
}

/// Apply several placeholder replacements to a raw XML part in one pass.
///
/// Equivalent to calling [`replace_in_xml_part`] once per pair, but parses and
/// re-serialises the part a single time — which matters when filling a template
/// with many fields.
pub fn replace_many_in_xml_part(
    xml: &[u8],
    replacements: &[(&str, &str)],
) -> crate::error::Result<(Vec<u8>, usize)> {
    rewrite_text_boxes(xml, true, &mut |paragraph| {
        replacements
            .iter()
            .try_fold(0, |count, (placeholder, replacement)| {
                Ok(count + try_replace_in_paragraph(paragraph, placeholder, replacement)?)
            })
    })
}

/// Apply a regex replacement to text boxes and shapes within a raw XML part.
///
/// The regex counterpart to [`replace_in_xml_part`]; `replacement` supports
/// `$1`-style capture group references.
pub fn replace_regex_in_xml_part(
    xml: &[u8],
    re: &regex::Regex,
    replacement: &str,
) -> crate::error::Result<(Vec<u8>, usize)> {
    rewrite_text_boxes(xml, true, &mut |paragraph| {
        try_replace_regex_in_paragraph(paragraph, re, replacement)
    })
}

/// The texts a replacement in the text boxes of a raw XML part matches
/// against and counts, see [`replaceable_texts`]. The copies of a text box
/// that Word writes twice, see [`replace_in_xml_part`], are left out.
#[doc(hidden)]
pub fn xml_part_replaceable_texts(xml: &[u8]) -> crate::error::Result<Vec<String>> {
    let mut texts = Vec::new();
    rewrite_text_boxes(xml, false, &mut |paragraph| {
        texts.extend(replaceable_texts(paragraph));
        Ok(0)
    })?;
    Ok(texts)
}

struct TextBoxOutput {
    bytes: Vec<u8>,
    count: usize,
    map: Option<crate::document::WordSourceMap>,
}

fn textbox_transport(
    map: &mut Option<crate::document::WordSourceMap>,
    input: Option<&crate::document::WordFieldInput>,
    source: &[u8],
    range: std::ops::Range<usize>,
    output: &[u8],
    produced: Option<crate::document::WordSourceMap>,
) -> crate::Result<()> {
    let Some(map) = map else {
        return Ok(());
    };
    if let Some(produced) = produced {
        if produced.len() != output.len() {
            return Err(crate::OxmlError::InvalidValue(
                "textbox producer map length differs".into(),
            ));
        }
        map.extend(&produced)?;
    } else if let Some(input) = input
        && source.get(range.clone()) == Some(output)
    {
        map.extend(&input.projection_parts().0.slice(range)?)?;
    } else {
        map.extend(&crate::document::WordSourceMap::generated(output.len()))?;
    }
    Ok(())
}

// The ordinary callback and checked template callback share the actual
// textbox walker and its capture/branch decisions.
enum TextBoxEditor<'e, 'v> {
    Ordinary(&'e mut dyn FnMut(&mut CT_P) -> crate::Result<usize>),
    Template(TemplateTextboxEdit<'e, 'v>),
}

struct TemplateTextboxEdit<'e, 'v> {
    work: &'e mut WordOperationWork,
    copies: &'e mut crate::document::TemplateCopyWork,
    datums: &'e std::collections::HashMap<usize, &'v serde_json::Value>,
    operands: std::collections::BTreeMap<&'e str, (&'e str, &'e [&'v serde_json::Value])>,
    matcher: Option<regex::Regex>,
    sources: Vec<String>,
}

impl TextBoxEditor<'_, '_> {
    fn paragraph(
        &mut self,
        paragraph: &mut CT_P,
        mut extra_maps: Option<&mut WordExtraSourceMaps>,
        facts: Option<&crate::text::WordParagraphReadFacts>,
    ) -> crate::Result<usize> {
        let context = match self {
            Self::Ordinary(edit) => return edit(paragraph),
            Self::Template(context) => context,
        };
        let nested = facts
            .map(|facts| {
                crate::document::returned_block_inputs(
                    crate::document::WordOwner::Paragraph(paragraph),
                    &facts.nested,
                )
            })
            .transpose()?
            .map(|(texts, _)| {
                texts
                    .into_iter()
                    .map(|(text, event)| (std::ptr::from_ref(text) as usize, event))
                    .collect::<std::collections::HashMap<_, _>>()
            });
        let empty = crate::document::WordSourceMap::generated(0);
        let texts = Vec::new();
        let mut read = crate::document::TemplateWordRead {
            map: facts.map_or(&empty, |facts| facts.source_map.as_ref()),
            texts: facts.map_or(&texts, |facts| &facts.texts),
            work: context.copies,
            nested: nested.as_ref(),
        };
        if context.matcher.is_none() {
            let empty_extras = Vec::new();
            context.sources.extend(template_replaceable_texts_in(
                paragraph,
                extra_maps.as_deref().unwrap_or(&empty_extras),
                context.work,
                &mut read,
            )?);
            return Ok(0);
        }
        let mut walk = text_runs_in(paragraph, extra_maps.as_deref(), Some(context.work))?;
        select_walk_words(paragraph, &walk, &mut read)?;
        let mut moved = Vec::new();
        let mut stretches =
            owned_walk_stretches(paragraph, &mut walk, &mut read, Some(&mut moved))?;
        let matcher = context
            .matcher
            .as_ref()
            .expect("selected nonempty operands");
        let mut occurrences = Vec::new();
        let mut operands = Vec::new();
        for (stretch, text) in stretches.iter().enumerate() {
            for matched in matcher.find_iter(text) {
                let (replacement, dependencies) = context.operands[matched.as_str()];
                occurrences.push((stretch, matched.range(), replacement, None));
                operands.push((None, dependencies));
            }
        }
        let count = occurrences.len();
        let edits = prepared_walk_edits(&occurrences, &stretches);
        restore_walk_texts(paragraph, &mut walk, &mut stretches, &moved);
        replace_prepared_walk(
            paragraph,
            edits?,
            count,
            extra_maps.take(),
            Some(context.work),
            Some(PreparedTemplateInputs {
                datums: context.datums,
                operands: &operands,
            }),
            walk,
        )?;
        Ok(count)
    }
}

fn validate_original_textbox_exclusions(
    xml: &[u8],
    excluded: &[std::ops::Range<usize>],
) -> crate::Result<()> {
    if excluded
        .iter()
        .any(|range| range.start >= range.end || range.end > xml.len())
        || excluded.windows(2).any(|pair| pair[0].end > pair[1].start)
    {
        return Err(crate::OxmlError::InvalidValue(
            "invalid original textbox exclusions".into(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Actual existing operation inputs and exact carrier return.
pub(crate) fn replace_original_template_literals_in<'e, 'v>(
    xml: &[u8],
    excluded: &[std::ops::Range<usize>],
    replacements: &'e [(&'e str, &'e str, &'e [&'v serde_json::Value])],
    input: &crate::document::WordFieldInput,
    work: &'e mut WordOperationWork,
    copies: &'e mut crate::document::TemplateCopyWork,
    datums: &'e std::collections::HashMap<usize, &'v serde_json::Value>,
) -> crate::Result<(Vec<u8>, usize, Vec<String>, crate::document::WordSourceMap)> {
    validate_original_textbox_exclusions(xml, excluded)?;
    check_source_failure(Some(work))?;
    let mut operands = std::collections::BTreeMap::new();
    for &(literal, value, dependencies) in replacements {
        if literal.is_empty()
            || operands
                .insert(literal, (value, dependencies))
                .is_some_and(|old: (&str, &[&serde_json::Value])| old.0 != value)
        {
            return Err(crate::OxmlError::InvalidValue(
                "ambiguous original textbox literal operands".into(),
            ));
        }
    }
    let matcher = if replacements.is_empty() {
        None
    } else {
        let mut alternatives = operands.keys().copied().collect::<Vec<_>>();
        alternatives.sort_by_key(|literal| std::cmp::Reverse(literal.len()));
        Some(
            regex::Regex::new(
                &alternatives
                    .into_iter()
                    .map(regex::escape)
                    .collect::<Vec<_>>()
                    .join("|"),
            )
            .map_err(|error| crate::OxmlError::InvalidValue(error.to_string()))?,
        )
    };
    let mut editor = TextBoxEditor::Template(TemplateTextboxEdit {
        work,
        copies,
        datums,
        operands,
        matcher,
        sources: Vec::new(),
    });
    let prefixes = [
        "w".to_owned(),
        format!("\0r\0{}", crate::namespace::R_NS),
        format!("\0mc\0{}", crate::namespace::MC_NS),
    ];
    let output = rewrite_text_boxes_within(
        xml,
        &prefixes,
        !replacements.is_empty(),
        &mut editor,
        excluded,
        0,
        &mut false,
        Some(input),
    )?;
    let sources = match editor {
        TextBoxEditor::Template(context) => context.sources,
        TextBoxEditor::Ordinary(_) => Vec::new(),
    };
    if replacements.is_empty() {
        Ok((xml.to_vec(), 0, sources, input.projection_parts().0.clone()))
    } else {
        Ok((
            output.bytes,
            output.count,
            sources,
            output.map.expect("mapped textbox walk"),
        ))
    }
}

/// Stage original literal matches in the established raw textbox boundary.
/// Physical exclusions identify separately checked textbox owners in this input.
/// Empty operands discover original logical source texts without editing.
#[doc(hidden)]
pub fn replace_original_literals_in_xml_part(
    xml: &[u8],
    excluded: &[std::ops::Range<usize>],
    replacements: &[(&str, &str)],
) -> crate::Result<(Vec<u8>, usize, Vec<String>)> {
    use crate::OxmlError;
    validate_original_textbox_exclusions(xml, excluded)?;
    let prefixes = [
        "w".to_owned(),
        format!("\0r\0{}", crate::namespace::R_NS),
        format!("\0mc\0{}", crate::namespace::MC_NS),
    ];
    let mut texts = Vec::new();
    rewrite_text_boxes_within(
        xml,
        &prefixes,
        false,
        &mut TextBoxEditor::Ordinary(&mut |paragraph| {
            texts.extend(replaceable_texts(paragraph));
            Ok(0)
        }),
        excluded,
        0,
        &mut false,
        None,
    )?;
    if replacements.is_empty() {
        return Ok((xml.to_vec(), 0, texts));
    }
    let mut operands = std::collections::BTreeMap::new();
    for &(literal, value) in replacements {
        if literal.is_empty()
            || operands
                .insert(literal, value)
                .is_some_and(|old| old != value)
        {
            return Err(OxmlError::InvalidValue(
                "ambiguous original textbox literal operands".into(),
            ));
        }
    }
    let mut alternatives = operands.keys().copied().collect::<Vec<_>>();
    alternatives.sort_by_key(|literal| std::cmp::Reverse(literal.len()));
    let matcher = regex::Regex::new(
        &alternatives
            .into_iter()
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join("|"),
    )
    .map_err(|error| OxmlError::InvalidValue(error.to_string()))?;
    let rewritten = rewrite_text_boxes_within(
        xml,
        &prefixes,
        true,
        &mut TextBoxEditor::Ordinary(&mut |paragraph| {
            let occurrences = replaceable_texts(paragraph)
                .iter()
                .enumerate()
                .flat_map(|(stretch, text)| {
                    matcher
                        .find_iter(text)
                        .map(|matched| {
                            (
                                stretch,
                                matched.range(),
                                operands[matched.as_str()].to_owned(),
                                None,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            replace_prepared_occurrences(paragraph, &occurrences)?;
            Ok(occurrences.len())
        }),
        excluded,
        0,
        &mut false,
        None,
    )?;
    Ok((rewritten.bytes, rewritten.count, texts))
}

/// Walk `xml`, handing each paragraph of a `w:txbxContent` element to `edit`,
/// those of its tables and block content controls included, and
/// re-serialising a child in place when `edit` counts a change in it. Every
/// other child of the text box is copied through verbatim. A table or a
/// control is parsed with the namespaces the part declares around it, such as
/// the `w14` of `w14:paraId` that Word declares on the root. A text box that
/// Word writes twice is counted once, and its copies are handed to `edit`
/// only with `edit_copies`, see [`rewrite_alternate_content`]. Returns the
/// rewritten XML and the summed count.
fn rewrite_text_boxes(
    xml: &[u8],
    edit_copies: bool,
    edit: &mut dyn FnMut(&mut CT_P) -> crate::Result<usize>,
) -> crate::error::Result<(Vec<u8>, usize)> {
    use crate::namespace::{MC_NS, R_NS};

    // The bindings `CT_P::from_xml` assumes for a paragraph cut out of the part.
    let part_prefixes = [
        "w".to_owned(),
        format!("\0r\0{R_NS}"),
        format!("\0mc\0{MC_NS}"),
    ];
    let output = rewrite_text_boxes_within(
        xml,
        &part_prefixes,
        edit_copies,
        &mut TextBoxEditor::Ordinary(edit),
        &[],
        0,
        &mut false,
        None,
    )?;
    Ok((output.bytes, output.count))
}

/// [`rewrite_text_boxes`] over `xml`, an element cut out of a part where the
/// bindings `inherited` are in scope.
#[allow(clippy::too_many_arguments)] // Existing physical walker plus optional same-input transport.
fn rewrite_text_boxes_within(
    xml: &[u8],
    inherited: &[String],
    edit_copies: bool,
    edit: &mut TextBoxEditor<'_, '_>,
    excluded: &[std::ops::Range<usize>],
    source_offset: usize,
    found_text_box: &mut bool,
    input: Option<&crate::document::WordFieldInput>,
) -> crate::error::Result<TextBoxOutput> {
    use crate::error::OxmlError;
    use crate::namespace::matches_local_name;
    use crate::raw_xml::capture_element;
    use quick_xml::events::Event;
    use quick_xml::{Reader, Writer};

    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut total_count = 0;
    let cursor = input.map(|input| input.cursor(xml)).transpose()?;
    let mut output_map = input.map(|_| crate::document::WordSourceMap::generated(0));
    // The bindings the elements open around the current position declare.
    let mut scopes: Vec<Vec<String>> = Vec::new();

    loop {
        let before = reader.buffer_position() as usize;
        let physical_before = xml.len() - reader.get_ref().len();
        let output_before = writer.get_ref().len();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) if matches_local_name(e.name().as_ref(), b"txbxContent") => {
                *found_text_box = true;
                if let Ok(index) =
                    excluded.binary_search_by_key(&(source_offset + before), |range| range.start)
                {
                    let (raw, captured_input) = if let Some(cursor) = &cursor {
                        cursor.capture_element(&mut reader, e, before as u64)?
                    } else {
                        (capture_element(&mut reader, e)?, None)
                    };
                    if source_offset + reader.buffer_position() as usize != excluded[index].end {
                        return Err(OxmlError::InvalidValue(
                            "stale original textbox exclusion".into(),
                        ));
                    }
                    writer.get_mut().extend_from_slice(&raw);
                    textbox_transport(
                        &mut output_map,
                        input,
                        xml,
                        physical_before..xml.len() - reader.get_ref().len(),
                        &writer.get_ref()[output_before..],
                        captured_input.map(|input| input.projection_parts().0.clone()),
                    )?;
                    buf.clear();
                    continue;
                }
                let word_prefixes =
                    word_prefixes_at(e, scopes.last().map_or(inherited, Vec::as_slice))?;
                // We found a txbxContent element. Parse and edit each paragraph
                // and copy every other child through verbatim, in document order.
                writer.write_event(Event::Start(e.clone()))?;

                let mut inner_buf = Vec::new();

                // The start is emitted before child capture/parse, exactly as
                // in the ordinary walker. Track this actual write separately.
                textbox_transport(
                    &mut output_map,
                    input,
                    xml,
                    physical_before..xml.len() - reader.get_ref().len(),
                    &writer.get_ref()[output_before..],
                    None,
                )?;
                loop {
                    let inner_before = reader.buffer_position() as usize;
                    let inner_physical = xml.len() - reader.get_ref().len();
                    let inner_output = writer.get_ref().len();
                    let mut produced_map = None;
                    match reader.read_event_into(&mut inner_buf) {
                        Ok(Event::Start(ref ie)) => {
                            if matches_local_name(ie.name().as_ref(), b"p") {
                                // Parse this paragraph: collect its XML, then parse via CT_P
                                let (para_xml, paragraph_input) = if let Some(cursor) = &cursor {
                                    cursor.capture_element(&mut reader, ie, inner_before as u64)?
                                } else {
                                    (capture_element(&mut reader, ie)?, None)
                                };
                                let (paragraph_input, _record_publication) = match paragraph_input {
                                    Some(input) => {
                                        let (input, publication) =
                                            input.record_publication(&para_xml)?;
                                        (Some(input), Some(publication))
                                    }
                                    None => (None, None),
                                };
                                let mut para_reader = Reader::from_reader(para_xml.as_slice());
                                para_reader.config_mut().trim_text(true);
                                let mut prbuf = Vec::new();
                                // Advance past the <w:p> start tag
                                loop {
                                    match para_reader.read_event_into(&mut prbuf) {
                                        Ok(Event::Start(ref pe))
                                            if matches_local_name(pe.name().as_ref(), b"p") =>
                                        {
                                            break;
                                        }
                                        Ok(Event::Eof) => break,
                                        _ => {}
                                    }
                                    prbuf.clear();
                                }
                                let collect = matches!(edit, TextBoxEditor::Template(_));
                                let mut extras = Vec::new();
                                let mut texts = Vec::new();
                                let mut nested = crate::text::WordBlockSourceMaps::default();
                                let mut para = CT_P::from_xml_with_word_input(
                                    &mut para_reader,
                                    paragraph_input
                                        .as_ref()
                                        .map(|input| (input, para_xml.as_slice())),
                                    collect.then_some(&mut extras),
                                    collect.then_some(&mut texts),
                                    collect.then_some(&mut nested),
                                )?;
                                let facts = paragraph_input
                                    .as_ref()
                                    .map(|input| input.cursor(&para_xml))
                                    .transpose()?
                                    .and_then(|cursor| {
                                        cursor.paragraph_word_facts(&para_reader, texts)
                                    })
                                    .map(|mut facts| {
                                        facts.nested = nested;
                                        facts
                                    });
                                if let (Some(input), TextBoxEditor::Template(context)) =
                                    (paragraph_input.as_ref(), &mut *edit)
                                {
                                    context
                                        .work
                                        .field_origins
                                        .lock()
                                        .expect("field origin registry poisoned")
                                        .commit_publication_owner(
                                            crate::document::WordOwner::Paragraph(&para),
                                            input.projection_parts().0,
                                        );
                                }
                                let count = edit.paragraph(
                                    &mut para,
                                    collect.then_some(&mut extras),
                                    facts.as_ref(),
                                )?;
                                if count == 0 {
                                    // Nothing changed, so the paragraph keeps its
                                    // bytes, start-tag attributes included.
                                    writer.get_mut().extend_from_slice(&para_xml);
                                    produced_map = paragraph_input
                                        .as_ref()
                                        .map(|input| input.projection_parts().0.clone());
                                } else {
                                    match edit {
                                        TextBoxEditor::Ordinary(_) => {
                                            write_text_box_paragraph(&mut writer, &para, ie)?
                                        }
                                        TextBoxEditor::Template(context) => {
                                            let (bytes, map) = write_template_text_box_paragraph(
                                                &para,
                                                ie,
                                                context.work,
                                            )?;
                                            writer.get_mut().extend_from_slice(&bytes);
                                            produced_map = Some(map);
                                        }
                                    }
                                }
                                if let Some(map) = &mut produced_map {
                                    map.freeze_publication();
                                }
                                total_count += count;
                            } else {
                                // A table or a content control is edited through
                                // the typed model, and any other element the edit
                                // does not reach stays as it was.
                                let (mut raw, child_input) = if let Some(cursor) = &cursor {
                                    cursor.capture_element(&mut reader, ie, inner_before as u64)?
                                } else {
                                    (capture_element(&mut reader, ie)?, None)
                                };
                                let (count, map) = edit_text_box_block(
                                    &mut raw,
                                    &word_prefixes,
                                    edit,
                                    child_input.as_ref(),
                                )?;
                                total_count += count;
                                writer.get_mut().extend_from_slice(&raw);
                                produced_map = map;
                            }
                        }
                        // Every child element is consumed whole, so the first end
                        // tag at this level closes the txbxContent element.
                        Ok(Event::End(ie)) => {
                            writer.write_event(Event::End(ie))?;
                            textbox_transport(
                                &mut output_map,
                                input,
                                xml,
                                inner_physical..xml.len() - reader.get_ref().len(),
                                &writer.get_ref()[inner_output..],
                                None,
                            )?;
                            break;
                        }
                        Ok(Event::Eof) => {
                            return Err(OxmlError::MissingElement("w:txbxContent end".to_owned()));
                        }
                        // Empty elements such as `<w:p/>` or a bookmark, whitespace,
                        // comments and processing instructions.
                        Ok(ev) => writer.write_event(ev)?,
                        Err(e) => return Err(e.into()),
                    }
                    textbox_transport(
                        &mut output_map,
                        input,
                        xml,
                        inner_physical..xml.len() - reader.get_ref().len(),
                        &writer.get_ref()[inner_output..],
                        produced_map,
                    )?;
                    inner_buf.clear();
                }
            }
            Ok(Event::Start(ref e))
                if matches_local_name(e.name().as_ref(), b"AlternateContent") =>
            {
                let scope = scopes.last().map_or(inherited, Vec::as_slice);
                let (raw, captured_input) = if let Some(cursor) = &cursor {
                    cursor.capture_element(&mut reader, e, before as u64)?
                } else {
                    (capture_element(&mut reader, e)?, None)
                };
                let rewritten = rewrite_alternate_content(
                    &raw,
                    scope,
                    edit_copies,
                    edit,
                    excluded,
                    source_offset + before,
                    found_text_box,
                    captured_input.as_ref(),
                )?;
                writer.get_mut().extend_from_slice(&rewritten.bytes);
                total_count += rewritten.count;
                textbox_transport(
                    &mut output_map,
                    input,
                    xml,
                    physical_before..xml.len() - reader.get_ref().len(),
                    &writer.get_ref()[output_before..],
                    rewritten.map,
                )?;
            }
            Ok(Event::Start(e)) => {
                scopes.push(
                    word_prefixes_at(&e, scopes.last().map_or(inherited, Vec::as_slice))?
                        .into_owned(),
                );
                writer.write_event(Event::Start(e))?;
            }
            Ok(Event::End(e)) => {
                scopes.pop();
                writer.write_event(Event::End(e))?;
            }
            Ok(ev) => {
                writer.write_event(ev)?;
            }
            Err(e) => return Err(e.into()),
        }
        if output_map
            .as_ref()
            .is_some_and(|map| map.len() == output_before)
        {
            textbox_transport(
                &mut output_map,
                input,
                xml,
                physical_before..xml.len() - reader.get_ref().len(),
                &writer.get_ref()[output_before..],
                None,
            )?;
        }
        buf.clear();
    }

    Ok(TextBoxOutput {
        bytes: writer.into_inner(),
        count: total_count,
        map: output_map,
    })
}

/// Apply several placeholder replacements to the notes of a footnotes or
/// endnotes part in one pass.
///
/// Reaches the paragraphs of every footnote and endnote a reader writes,
/// those of their tables and block content controls included. A separator,
/// a continuation separator, a continuation notice and an untyped entry at
/// id 0 or below are never edited, as [`crate::footnotes::CT_Footnotes`]
/// reads them. A child the replacement changed is re-serialised in place and
/// every other byte of the part is kept. A part that does not bind the `w`
/// prefix to WordprocessingML keeps its bytes, since the rewritten children
/// use it. Returns the modified XML and the replacement count.
pub fn replace_many_in_notes_part(
    xml: &[u8],
    replacements: &[(&str, &str)],
) -> crate::error::Result<(Vec<u8>, usize)> {
    rewrite_notes(xml, &mut |paragraph| {
        replacements
            .iter()
            .try_fold(0, |count, (placeholder, replacement)| {
                Ok(count + try_replace_in_paragraph(paragraph, placeholder, replacement)?)
            })
    })
}

/// Apply a regex replacement to the notes of a footnotes or endnotes part,
/// see [`replace_many_in_notes_part`].
pub fn replace_regex_in_notes_part(
    xml: &[u8],
    re: &regex::Regex,
    replacement: &str,
) -> crate::error::Result<(Vec<u8>, usize)> {
    rewrite_notes(xml, &mut |paragraph| {
        try_replace_regex_in_paragraph(paragraph, re, replacement)
    })
}

/// The texts a replacement in the notes of a footnotes or endnotes part
/// matches against, see [`replaceable_texts`].
#[doc(hidden)]
pub fn notes_part_replaceable_texts(xml: &[u8]) -> crate::error::Result<Vec<String>> {
    let mut texts = Vec::new();
    rewrite_notes(xml, &mut |paragraph| {
        texts.extend(replaceable_texts(paragraph));
        Ok(0)
    })?;
    Ok(texts)
}

/// Walk a footnotes or endnotes part, handing each paragraph of a note to
/// `edit`, those of its tables and block content controls included, see
/// [`edit_raw_block`]. A child is parsed with the namespaces the part
/// declares around it. Every other child of a note, the start tag of the
/// note with its id, and every entry that is not a note a reader writes are
/// copied through verbatim. Returns the rewritten XML and the summed count.
fn rewrite_notes(
    xml: &[u8],
    edit: &mut dyn FnMut(&mut CT_P) -> crate::Result<usize>,
) -> crate::error::Result<(Vec<u8>, usize)> {
    use crate::error::OxmlError;
    use crate::raw_xml::capture_element;
    use quick_xml::events::Event;
    use quick_xml::{Reader, Writer};

    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut buffer = Vec::new();
    let mut total_count = 0;
    // The bindings the elements open around the current position declare.
    let mut scopes: Vec<Vec<String>> = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Eof => break,
            Event::Start(start) => {
                let prefixes = word_prefixes_at(&start, scopes.last().map_or(&[], Vec::as_slice))?;
                // A note is a child of the root. Its children are rewritten
                // with the `w` prefix, so the part must bind it.
                if scopes.len() == 1
                    && prefixes.iter().any(|prefix| prefix == "w")
                    && is_note_a_reader_writes(&start, &prefixes)
                {
                    writer.write_event(Event::Start(start))?;
                    let mut inner_buffer = Vec::new();
                    loop {
                        match reader.read_event_into(&mut inner_buffer)? {
                            Event::Start(child) => {
                                let mut raw = capture_element(&mut reader, &child)?;
                                total_count += edit_raw_block(&mut raw, &prefixes, edit)?;
                                writer.get_mut().extend_from_slice(&raw);
                            }
                            // Every child element is consumed whole, so the
                            // first end tag at this level closes the note.
                            Event::End(end) => {
                                writer.write_event(Event::End(end))?;
                                break;
                            }
                            Event::Eof => {
                                return Err(OxmlError::MissingElement("note end".to_owned()));
                            }
                            // Empty elements, whitespace, comments and
                            // processing instructions.
                            event => writer.write_event(event)?,
                        }
                        inner_buffer.clear();
                    }
                } else {
                    scopes.push(prefixes.into_owned());
                    writer.write_event(Event::Start(start))?;
                }
            }
            Event::End(end) => {
                scopes.pop();
                writer.write_event(Event::End(end))?;
            }
            event => writer.write_event(event)?,
        }
        buffer.clear();
    }

    Ok((writer.into_inner(), total_count))
}

/// Whether `start` opens a footnote or an endnote that a reader writes. A
/// separator, a continuation separator and a continuation notice are drawn
/// by the application, and so is an untyped entry at id 0 or below, as
/// `CT_Footnotes::from_xml` and the story walkers read them.
fn is_note_a_reader_writes(start: &quick_xml::events::BytesStart<'_>, prefixes: &[String]) -> bool {
    let name = start.name();
    (is_word_element(name.as_ref(), b"footnote", prefixes)
        || is_word_element(name.as_ref(), b"endnote", prefixes))
        && optional_word_attribute(start, b"type", prefixes)
            .is_none_or(|kind| NoteType::from_str(&kind) == NoteType::Normal)
        && optional_word_attribute(start, b"id", prefixes)
            .and_then(|id| id.parse::<i32>().ok())
            .is_some_and(|id| id > 0)
}

/// [`rewrite_text_boxes`] over `raw`, an `mc:AlternateContent` element, with
/// a text box that Word writes twice counted once.
///
/// Word writes a text box as a DrawingML shape in `mc:Choice` and a VML copy
/// of it in `mc:Fallback`. The first Choice that holds a text box, the one
/// layout draws, is edited and counted. Every branch after it, the Fallback
/// or another Choice, is a copy. With `edit_copies`, a copy is edited too,
/// whatever it holds, so that a reader of the VML sees the same change, and
/// it is never counted. Without it, a copy keeps its bytes. When no Choice
/// holds a text box, a picture for example, every branch is walked and
/// counted as any other markup.
#[allow(clippy::too_many_arguments)] // Existing branch walker plus same-capture input.
fn rewrite_alternate_content(
    raw: &[u8],
    inherited: &[String],
    edit_copies: bool,
    edit: &mut TextBoxEditor<'_, '_>,
    excluded: &[std::ops::Range<usize>],
    source_offset: usize,
    found_text_box: &mut bool,
    input: Option<&crate::document::WordFieldInput>,
) -> crate::Result<TextBoxOutput> {
    use crate::namespace::matches_local_name;
    use crate::raw_xml::capture_element;
    use quick_xml::events::Event;
    use quick_xml::{Reader, Writer};
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut map = input.map(|_| crate::document::WordSourceMap::generated(0));
    let cursor = input.map(|input| input.cursor(raw)).transpose()?;
    let Event::Start(start) = reader.read_event_into(&mut buf)? else {
        return Ok(TextBoxOutput {
            bytes: raw.to_vec(),
            count: 0,
            map: input.map(|input| input.projection_parts().0.clone()),
        });
    };
    let prefixes = word_prefixes_at(&start, inherited)?;
    writer.write_event(Event::Start(start.into_owned()))?;
    textbox_transport(
        &mut map,
        input,
        raw,
        0..raw.len() - reader.get_ref().len(),
        writer.get_ref(),
        None,
    )?;
    buf.clear();
    let mut counted_choice = false;
    let mut total_count = 0;
    loop {
        let before = reader.buffer_position() as usize;
        let physical = raw.len() - reader.get_ref().len();
        let produced = writer.get_ref().len();
        let mut branch_map = None;
        match reader.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(ref branch_start) => {
                let (branch, branch_input) = if let Some(cursor) = &cursor {
                    cursor.capture_element(&mut reader, branch_start, before as u64)?
                } else {
                    (capture_element(&mut reader, branch_start)?, None)
                };
                if counted_choice {
                    if edit_copies {
                        let output = rewrite_text_boxes_within(
                            &branch,
                            &prefixes,
                            edit_copies,
                            edit,
                            excluded,
                            source_offset + before,
                            found_text_box,
                            branch_input.as_ref(),
                        )?;
                        writer.get_mut().extend_from_slice(&output.bytes);
                        branch_map = output.map;
                    } else {
                        writer.get_mut().extend_from_slice(&branch);
                        branch_map = branch_input.map(|input| input.projection_parts().0.clone());
                    }
                } else {
                    let mut holds_text_box = false;
                    let output = rewrite_text_boxes_within(
                        &branch,
                        &prefixes,
                        edit_copies,
                        edit,
                        excluded,
                        source_offset + before,
                        &mut holds_text_box,
                        branch_input.as_ref(),
                    )?;
                    *found_text_box |= holds_text_box;
                    counted_choice = holds_text_box
                        && matches_local_name(branch_start.name().as_ref(), b"Choice");
                    writer.get_mut().extend_from_slice(&output.bytes);
                    total_count += output.count;
                    branch_map = output.map;
                }
            }
            event => writer.write_event(event)?,
        }
        textbox_transport(
            &mut map,
            input,
            raw,
            physical..raw.len() - reader.get_ref().len(),
            &writer.get_ref()[produced..],
            branch_map,
        )?;
        buf.clear();
    }
    Ok(TextBoxOutput {
        bytes: writer.into_inner(),
        count: total_count,
        map,
    })
}

/// Write a text-box paragraph that was parsed without its start tag.
///
/// The start tag keeps the attributes it was read with, such as `w:rsidR`,
/// `w14:paraId` or a local namespace declaration. The paragraph goes back to
/// the place it was read from, so every prefix they use resolves as before.
fn write_text_box_paragraph<W: std::io::Write>(
    writer: &mut quick_xml::Writer<W>,
    paragraph: &CT_P,
    source: &quick_xml::events::BytesStart<'_>,
) -> crate::error::Result<()> {
    use quick_xml::events::{BytesStart, Event};

    let mut xml = Vec::new();
    paragraph.to_xml(&mut quick_xml::Writer::new(&mut xml))?;
    let mut start = BytesStart::new("w:p");
    for attribute in source.attributes() {
        start.push_attribute(attribute?);
    }
    // Parsed without its start tag, the paragraph records no attribute of its
    // own, so it serializes with a bare start tag. Anything else is written as
    // it serialized.
    if xml == b"<w:p/>" {
        writer.write_event(Event::Empty(start))?;
    } else if let Some(content) = xml.strip_prefix(b"<w:p>") {
        writer.write_event(Event::Start(start))?;
        writer.get_mut().write_all(content)?;
    } else {
        writer.get_mut().write_all(&xml)?;
    }
    Ok(())
}

fn write_template_text_box_paragraph(
    paragraph: &CT_P,
    source: &quick_xml::events::BytesStart<'_>,
    work: &mut WordOperationWork,
) -> crate::Result<(Vec<u8>, crate::document::WordSourceMap)> {
    use quick_xml::events::{BytesStart, Event};
    let (xml, mut map, events) =
        crate::text::final_word_source(crate::text::FinalWordBlock::Paragraph(paragraph), work)?;
    for event in events {
        map.push_word_event(event)?;
    }
    let mut start = BytesStart::new("w:p");
    for attribute in source.attributes() {
        start.push_attribute(attribute?);
    }
    let mut writer = quick_xml::Writer::new(Vec::new());
    if xml == b"<w:p/>" {
        writer.write_event(Event::Empty(start))?;
        let bytes = writer.into_inner();
        let map = crate::document::WordSourceMap::generated(bytes.len());
        Ok((bytes, map))
    } else if let Some(content) = xml.strip_prefix(b"<w:p>") {
        writer.write_event(Event::Start(start))?;
        let mut selected = crate::document::WordSourceMap::generated(writer.get_ref().len());
        selected.extend(&map.slice(5..xml.len())?)?;
        writer.get_mut().extend_from_slice(content);
        Ok((writer.into_inner(), selected))
    } else {
        Ok((xml, map))
    }
}

/// Replace placeholders in chart XML parts.
///
/// Chart text uses DrawingML runs: `a:r` → `a:t` (not `w:r`/`w:t`).
/// Also replaces in string cache values (`c:v`).
/// Returns modified XML and replacement count.
pub fn replace_in_chart_xml(
    xml: &[u8],
    placeholder: &str,
    replacement: &str,
) -> crate::error::Result<(Vec<u8>, usize)> {
    replace_many_in_chart_xml(xml, &[(placeholder, replacement)])
}

/// Apply several placeholder replacements to chart XML in one pass.
pub fn replace_many_in_chart_xml(
    xml: &[u8],
    replacements: &[(&str, &str)],
) -> crate::error::Result<(Vec<u8>, usize)> {
    use crate::namespace::matches_local_name;
    use quick_xml::events::{BytesEnd, BytesText, Event};
    use quick_xml::{Reader, Writer};

    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut total_count = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            // <a:t> holds DrawingML run text, <c:v> a chart value cache entry.
            Ok(Event::Start(ref e))
                if matches_local_name(e.name().as_ref(), b"t")
                    || matches_local_name(e.name().as_ref(), b"v") =>
            {
                let name = e.name();
                writer.write_event(Event::Start(e.borrow()))?;

                // Consume the element as a whole. Reading it event by event
                // would split the value at every entity reference and could
                // miss a placeholder straddling one.
                let mut text = crate::xml_text::read_element_text(&mut reader, name);
                for (placeholder, replacement) in replacements {
                    let occurrences = text.matches(placeholder).count();
                    if occurrences > 0 {
                        total_count += occurrences;
                        text = text.replace(placeholder, replacement);
                    }
                }
                writer.write_event(Event::Text(BytesText::new(&text)))?;

                writer.write_event(Event::End(BytesEnd::new(
                    std::str::from_utf8(name.as_ref()).unwrap_or_default(),
                )))?;
            }
            Ok(ev) => {
                writer.write_event(ev)?;
            }
            Err(e) => return Err(e.into()),
        }
        buf.clear();
    }

    Ok((writer.into_inner(), total_count))
}

/// Replace all regex matches in a paragraph with the replacement string.
///
/// The `replacement` string supports capture group references: `$1`, `$2`, etc.
/// Uses the same cross-run char map algorithm as literal replacement, so it
/// reaches the runs of inline content controls, tracked insertions, smart
/// tags, inline custom XML elements and simple fields, and leaves a match
/// that straddles one of their boundaries as it is.
/// Returns the number of replacements made.
pub fn replace_regex_in_paragraph(para: &mut CT_P, re: &regex::Regex, replacement: &str) -> usize {
    try_replace_regex_in_paragraph(para, re, replacement).unwrap_or(0)
}

/// Fallible regex paragraph replacement for checked document operations.
#[doc(hidden)]
pub fn try_replace_regex_in_paragraph(
    para: &mut CT_P,
    re: &regex::Regex,
    replacement: &str,
) -> crate::Result<usize> {
    // `captures_at` (rather than slicing) keeps anchors and look-around
    // evaluating against the full paragraph text.
    install_paragraph_replacement(para, |candidate| {
        replace_matches(candidate, &mut |text, from| {
            let captures = re.captures_at(text, from)?;
            let matched = captures.get(0)?;
            // Expand capture groups in replacement
            let mut expanded = String::new();
            captures.expand(replacement, &mut expanded);
            Some((matched.start(), matched.end(), expanded))
        })
    })
}

/// Replace regex matches in all paragraphs.
pub fn replace_regex_in_paragraphs(
    paras: &mut [CT_P],
    re: &regex::Regex,
    replacement: &str,
) -> usize {
    paras
        .iter_mut()
        .map(|p| replace_regex_in_paragraph(p, re, replacement))
        .sum()
}

/// Replace regex matches in a table (recursively handles nested tables and
/// content controls).
pub fn replace_regex_in_table(table: &mut CT_Tbl, re: &regex::Regex, replacement: &str) -> usize {
    edit_paragraph_owners(ParagraphEditOwner::Table(table), &mut |paragraph| {
        Ok(replace_regex_in_paragraph(paragraph, re, replacement))
    })
    .unwrap_or(0)
}

/// Replace regex matches in a header or footer, see
/// [`replace_in_header_footer`].
pub fn replace_regex_in_header_footer(
    hf: &mut CT_HdrFtr,
    re: &regex::Regex,
    replacement: &str,
) -> usize {
    edit_header_footer(hf, &mut |paragraph| {
        replace_regex_in_paragraph(paragraph, re, replacement)
    })
}

/// Fallible regex header/footer replacement for checked document operations.
#[doc(hidden)]
pub fn try_replace_regex_in_header_footer(
    hf: &mut CT_HdrFtr,
    re: &regex::Regex,
    replacement: &str,
) -> crate::Result<usize> {
    for paragraph in &hf.paragraphs {
        crate::document::admit_word_model(crate::document::WordOwner::Paragraph(paragraph))?;
    }
    let mut candidate = hf.clone();
    let count = try_edit_header_footer(&mut candidate, &mut |p| {
        try_replace_regex_in_paragraph(p, re, replacement)
    })?;
    if count > 0 {
        *hf = candidate;
    }
    Ok(count)
}
