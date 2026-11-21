//! Comparing two paragraphs, plain or complex, and coalesced runs.

use super::*;

pub(super) fn compare_body_content(
    original: &BodyContent,
    edited: &BodyContent,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    match (original, edited) {
        (BodyContent::Paragraph(left), BodyContent::Paragraph(right)) => compare_paragraph(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (BodyContent::Table(left), BodyContent::Table(right)) => compare_table(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (BodyContent::ContentControl(left), BodyContent::ContentControl(right)) => {
            if let Some(source) = original_source {
                compare_control_from_xml(left, right, location, metadata, diagnostics, source)
            } else {
                compare_control(left, right, location, metadata, diagnostics)
            }
        }
        _ if body_signature(original) == body_signature(edited) => body_content_xml(original),
        _ => Err(Error::Other(format!(
            "comparison cannot replace unlike body structures at {location}"
        ))),
    }
}

pub(super) fn deleted_body_content(
    content: &BodyContent,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    match content {
        BodyContent::Paragraph(paragraph) => deleted_paragraph(paragraph, metadata),
        BodyContent::Table(table) => marked_table(table, "del", metadata),
        BodyContent::ContentControl(_) | BodyContent::RawXml(_) => Err(Error::Other(
            "comparison cannot delete an unmatched content-control or opaque body node".to_owned(),
        )),
    }
}

pub(super) fn inserted_body_content(
    content: &BodyContent,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    match content {
        BodyContent::Paragraph(paragraph) => inserted_paragraph(paragraph, metadata),
        BodyContent::Table(table) => marked_table(table, "ins", metadata),
        BodyContent::ContentControl(_) | BodyContent::RawXml(_) => Err(Error::Other(
            "comparison cannot insert an unmatched content-control or opaque body node".to_owned(),
        )),
    }
}

fn coalescible_plain_run(run: &CT_R) -> bool {
    !run.content.is_empty()
        && run.extra_xml.is_empty()
        && run.alt_drawings.is_empty()
        && run.content.iter().all(|content| {
            matches!(
                content,
                RunContent::Text(_) | RunContent::Tab | RunContent::Break(_)
            )
        })
}

fn coalesced_paragraph_signature(paragraph: &CT_P, options: &ComparisonOptions) -> String {
    let scalar = ComparisonOptions {
        granularity: ComparisonGranularity::Character,
        ..options.clone()
    };
    let mut properties = Vec::<(Option<&oxoffice_docx_oxml::properties::CT_RPr>, usize)>::new();
    if !options.ignore_formatting {
        for run in paragraph
            .runs
            .iter()
            .filter(|run| coalescible_plain_run(run))
        {
            let width = run
                .content
                .iter()
                .map(|content| match content {
                    RunContent::Text(text) => text
                        .text
                        .chars()
                        .filter(|character| {
                            !options.ignore_whitespace || !character.is_whitespace()
                        })
                        .count(),
                    _ => 1,
                })
                .sum::<usize>();
            // Empty text and ignored whitespace still carry a real format.
            let current = run_formatting(run);
            if let Some((previous, count)) = properties.last_mut()
                && *previous == current
            {
                *count += width;
            } else {
                properties.push((current, width));
            }
        }
    }
    // Character publication protects every new whitespace boundary. On the
    // whole-run path, retain the actual unprotected edge characters as
    // additional content facts before using that publication view as equality.
    // Internal whitespace and explicitly protected split boundaries stay equal.
    let mut unprotected_edges = Vec::new();
    if !uses_attributed_run_path(options) {
        let xml_space = |character: char| matches!(character, ' ' | '\t' | '\n' | '\r');
        let mut offset = 0usize;
        for run in &paragraph.runs {
            for content in &run.content {
                if let RunContent::Text(text) = content {
                    let leading = text.text.len() - text.text.trim_start_matches(xml_space).len();
                    let trailing = text.text.trim_end_matches(xml_space).len();
                    for (byte, character) in text.text.char_indices() {
                        if !text.preserve_space
                            && xml_space(character)
                            && (byte < leading || byte >= trailing)
                        {
                            unprotected_edges.push(offset);
                        }
                        offset += 1;
                    }
                } else {
                    offset += 1;
                }
            }
        }
    }
    format!(
        "{}:{properties:?}:{unprotected_edges:?}",
        paragraph_signature_with_options(paragraph, &scalar)
    )
}

/// Compare complete protected owners, coalescing only their actual paragraph
/// leaves. Shell clones are equality-only and never become source lookups or
/// publications, so private control state remains part of the proof.
pub(super) fn coalesced_held_owner_equal(
    left: &BodyContent,
    right: &BodyContent,
    options: &ComparisonOptions,
    metadata: &Metadata<'_>,
) -> bool {
    fn controls(
        left: &[(usize, usize, CT_Sdt)],
        right: &[(usize, usize, CT_Sdt)],
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|((a, before_a, left), (b, before_b, right))| {
                    a == b && before_a == before_b && control(left, right, options, metadata)
                })
    }
    fn table(
        left: &CT_Tbl,
        right: &CT_Tbl,
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        if left.rows.len() != right.rows.len()
            || left.content_controls.len() != right.content_controls.len()
        {
            return false;
        }
        let mut a = left.clone();
        let mut b = right.clone();
        a.rows.clear();
        b.rows.clear();
        a.content_controls.clear();
        b.content_controls.clear();
        a == b
            && left
                .rows
                .iter()
                .zip(&right.rows)
                .all(|(left, right)| row(left, right, options, metadata))
            && controls(
                &left.content_controls,
                &right.content_controls,
                options,
                metadata,
            )
    }
    fn row(
        left: &CT_Row,
        right: &CT_Row,
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        if left.cells.len() != right.cells.len()
            || left.content_controls.len() != right.content_controls.len()
        {
            return false;
        }
        let mut a = left.clone();
        let mut b = right.clone();
        a.cells.clear();
        b.cells.clear();
        a.content_controls.clear();
        b.content_controls.clear();
        a == b
            && left
                .cells
                .iter()
                .zip(&right.cells)
                .all(|(left, right)| cell(left, right, options, metadata))
            && controls(
                &left.content_controls,
                &right.content_controls,
                options,
                metadata,
            )
    }
    fn cell(
        left: &CT_Tc,
        right: &CT_Tc,
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        if left.content.len() != right.content.len() {
            return false;
        }
        let mut a = left.clone();
        let mut b = right.clone();
        a.content.clear();
        b.content.clear();
        a == b
            && left
                .content
                .iter()
                .zip(&right.content)
                .all(|(left, right)| match (left, right) {
                    (CellContent::Paragraph(left), CellContent::Paragraph(right)) => {
                        coalesced_paragraphs_equal(left, right, options, metadata)
                    }
                    (CellContent::Table(left), CellContent::Table(right)) => {
                        table(left, right, options, metadata)
                    }
                    (CellContent::ContentControl(left), CellContent::ContentControl(right)) => {
                        control(left, right, options, metadata)
                    }
                    _ => false,
                })
    }
    fn control(
        left: &CT_Sdt,
        right: &CT_Sdt,
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        if left.content.len() != right.content.len() {
            return false;
        }
        let mut a = left.clone();
        let mut b = right.clone();
        a.content.clear();
        b.content.clear();
        a == b
            && left
                .content
                .iter()
                .zip(&right.content)
                .all(|(left, right)| match (left, right) {
                    (SdtContent::Paragraph(left), SdtContent::Paragraph(right)) => {
                        coalesced_paragraphs_equal(left, right, options, metadata)
                    }
                    (SdtContent::Table(left), SdtContent::Table(right)) => {
                        table(left, right, options, metadata)
                    }
                    (SdtContent::Row(left), SdtContent::Row(right)) => {
                        row(left, right, options, metadata)
                    }
                    (SdtContent::Cell(left), SdtContent::Cell(right)) => {
                        cell(left, right, options, metadata)
                    }
                    (SdtContent::ContentControl(left), SdtContent::ContentControl(right)) => {
                        control(left, right, options, metadata)
                    }
                    _ => left == right,
                })
    }
    match (left, right) {
        (BodyContent::Paragraph(left), BodyContent::Paragraph(right)) => {
            coalesced_paragraphs_equal(left, right, options, metadata)
        }
        (BodyContent::Table(left), BodyContent::Table(right)) => {
            table(left, right, options, metadata)
        }
        (BodyContent::ContentControl(left), BodyContent::ContentControl(right)) => {
            control(left, right, options, metadata)
        }
        _ => left == right,
    }
}

pub(super) fn coalesced_paragraphs_equal(
    original: &CT_P,
    edited: &CT_P,
    options: &ComparisonOptions,
    metadata: &Metadata<'_>,
) -> bool {
    // A drawing signature deliberately omits its nested Word body. Atomic
    // barriers must remain equal in full, so their own publishers still run.
    if !original
        .runs
        .iter()
        .filter(|run| !coalescible_plain_run(run))
        .eq(edited.runs.iter().filter(|run| !coalescible_plain_run(run)))
    {
        return false;
    }
    let original_fields = original
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let RunContent::Field(field) = content {
                Some(field)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let edited_fields = edited
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let RunContent::Field(field) = content {
                Some(field)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if original_fields.len() != edited_fields.len()
        || original_fields
            .iter()
            .zip(&edited_fields)
            .any(|(left, right)| {
                match (
                    metadata.field_source_facts(left),
                    metadata.field_source_facts(right),
                ) {
                    (Some(left), Some(right)) => left != right,
                    _ => true,
                }
            })
    {
        return false;
    }
    if original.content_controls.len() != edited.content_controls.len()
        || original
            .content_controls
            .iter()
            .zip(&edited.content_controls)
            .any(|((_, _, _, left), (_, _, _, right))| {
                match (
                    metadata.control_source(left),
                    metadata.control_source(right),
                ) {
                    (Some(left), Some(right)) => left != right,
                    _ => true,
                }
            })
    {
        return false;
    }

    let equal_fields = original
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let RunContent::Field(field) = content {
                Some((field, field.is_complex()))
            } else {
                None
            }
        })
        .eq(edited
            .runs
            .iter()
            .flat_map(|run| &run.content)
            .filter_map(|content| {
                if let RunContent::Field(field) = content {
                    Some((field, field.is_complex()))
                } else {
                    None
                }
            }));
    (original.properties == edited.properties
        || original
            .properties
            .clone()
            .and_then(nonempty_paragraph_properties)
            == edited
                .properties
                .clone()
                .and_then(nonempty_paragraph_properties))
        && equal_fields
        && coalesced_paragraph_signature(original, options)
            == coalesced_paragraph_signature(edited, options)
}

pub(super) fn compare_paragraph(
    original: &CT_P,
    edited: &CT_P,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    if !metadata.leaf_capture
        && coalesced_paragraphs_equal(original, edited, metadata.options, metadata)
        && let Some(source) = original_source
    {
        return Ok(source.to_owned());
    }
    let output = (|| -> Result<String> {
        if original.bookmark_markers == edited.bookmark_markers
            && paragraph_boundaries_differ(original, edited, metadata.options)
        {
            return Ok(format!(
                "{}{}",
                deleted_paragraph(original, metadata)?,
                inserted_paragraph(edited, metadata)?
            ));
        }
        if metadata.options.ignore_formatting
            && metadata.options.ignore_fields
            && paragraph_signature_with_options(original, metadata.options)
                == paragraph_signature_with_options(edited, metadata.options)
            && let Some(source) = original_source
        {
            return Ok(source.to_owned());
        }
        if original == edited
            && let Some((left, right)) = metadata
                .paragraph_source(original)
                .zip(metadata.paragraph_source(edited))
            && left == right
        {
            return Ok(original_source.unwrap_or(left).to_owned());
        }
        let detached = detach_field_spans(original, original_source)?;
        let original_source = detached.as_deref().or(original_source);
        if uses_attributed_run_path(metadata.options) {
            return compare_granular_paragraph(
                original,
                edited,
                original_source,
                location,
                metadata,
                diagnostics,
            );
        }
        if !original.hyperlinks.is_empty()
            || !edited.hyperlinks.is_empty()
            || !original.comment_ranges.is_empty()
            || !edited.comment_ranges.is_empty()
            || !original.bookmark_markers.is_empty()
            || !edited.bookmark_markers.is_empty()
            || !original.content_controls.is_empty()
            || !edited.content_controls.is_empty()
            || original
                .extra_xml
                .iter()
                .any(|(position, raw)| !CT_P::raw_is_root_attributes(*position, raw))
            || edited
                .extra_xml
                .iter()
                .any(|(position, raw)| !CT_P::raw_is_root_attributes(*position, raw))
        {
            return compare_complex_paragraph(
                original,
                edited,
                original_source,
                location,
                metadata,
                diagnostics,
            );
        }

        let (mut output, paragraph_close) = paragraph_shell(original_source)?;
        let destination_parent = root_namespace_bindings(&output, "p", &[])?;
        let revision_parent = metadata.ids.revision_parent_scope(&destination_parent);
        output.push_str(&paragraph_properties_xml(
            original,
            edited,
            location,
            metadata,
            diagnostics,
        )?);
        let original_signatures = original
            .runs
            .iter()
            .map(|run| run_signature_with_options(run, metadata.options))
            .collect::<Vec<_>>();
        let edited_signatures = edited
            .runs
            .iter()
            .map(|run| run_signature_with_options(run, metadata.options))
            .collect::<Vec<_>>();
        let aligned = align(&original_signatures, &edited_signatures);
        if !metadata.options.ignore_fields {
            validate_field_alignment(
                &aligned,
                &original.runs,
                &edited.runs,
                &original_signatures,
                &edited_signatures,
                location,
            )?;
        }
        let original_run_spans = original_source
            .map(|source| modeled_paragraph_run_spans(original, source))
            .transpose()?
            .unwrap_or_default();
        if original_source.is_some() && original_run_spans.len() != original.runs.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate paragraph run owners at {location}"
            )));
        }
        for (left, right) in aligned {
            match (left, right) {
                (Some(i), Some(j)) => {
                    if original_signatures[i] == edited_signatures[j] {
                        if !run_is_field(&original.runs[i])
                            && original.runs[i] == edited.runs[j]
                            && let Some(source) = original_source
                        {
                            output.push_str(&source[original_run_spans[i].clone()]);
                        } else {
                            output.push_str(&compared_run_xml(
                                &original.runs[i],
                                &edited.runs[j],
                                None,
                                None,
                                &format!("{location}/run[{j}]"),
                                &destination_parent,
                                metadata,
                                diagnostics,
                            )?);
                        }
                    } else {
                        output.push_str(&paired_run_replacement(
                            [&original.runs[i], &edited.runs[j]],
                            [None, None],
                            [None, None],
                            &format!("{location}/run[{j}]"),
                            &destination_parent,
                            metadata,
                            diagnostics,
                        )?);
                    }
                }
                (Some(i), None) => {
                    if run_is_ignored(&original.runs[i], metadata.options) {
                        if let Some(source) = original_source {
                            output.push_str(&source[original_run_spans[i].clone()]);
                        } else {
                            output.push_str(&paragraph_owned_run_xml(&original.runs[i])?);
                        }
                        continue;
                    }
                    let run = publish_run_source(
                        &original.runs[i],
                        &original.runs[i],
                        deleted_run_xml(&original.runs[i])?,
                        None,
                        None,
                        None,
                        &revision_parent,
                        metadata,
                    )?;
                    output.push_str(&metadata.ids.revision(
                        "del",
                        metadata.author,
                        metadata.timestamp,
                        &run,
                    )?);
                }
                (None, Some(j)) => {
                    if run_is_ignored(&edited.runs[j], metadata.options) {
                        continue;
                    }
                    let run = publish_run_source(
                        &edited.runs[j],
                        &edited.runs[j],
                        paragraph_owned_run_xml(&edited.runs[j])?,
                        None,
                        None,
                        None,
                        &revision_parent,
                        metadata,
                    )?;
                    output.push_str(&metadata.ids.revision(
                        "ins",
                        metadata.author,
                        metadata.timestamp,
                        &run,
                    )?);
                }
                (None, None) => unreachable!(),
            }
        }
        output.push_str(&paragraph_close);
        Ok(output)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_P as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

pub(super) fn paragraph_shell(source: Option<&str>) -> Result<(String, String)> {
    let Some(source) = source else {
        return Ok(("<w:p>".to_owned(), "</w:p>".to_owned()));
    };
    let (end, empty, name) = element_opening(source)?;
    let mut opening = source[..end].to_owned();
    if empty {
        opening.truncate(opening.len() - 2);
        opening.push('>');
    }
    Ok((opening, format!("</{name}>")))
}

pub(super) fn uses_attributed_run_path(options: &ComparisonOptions) -> bool {
    options.granularity != ComparisonGranularity::Run
        || options.ignore_formatting
        || options.ignore_whitespace
        || options.ignore_fields
        || options.ignore_comments
        || story_ignored(options, ComparisonStoryKind::TextBox)
}

fn compare_complex_paragraph(
    original: &CT_P,
    edited: &CT_P,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    if paragraph_signature_with_options(original, metadata.options)
        == paragraph_signature_with_options(edited, metadata.options)
        && !SourceMetadata::paragraph_contains_fields(original, metadata.options)?
    {
        if !metadata.options.ignore_formatting
            && paragraph_formatting(original) != paragraph_formatting(edited)
        {
            formatting_diagnostic(diagnostics, location.to_owned());
        }
        for (index, (left, right)) in original.runs.iter().zip(&edited.runs).enumerate() {
            if !metadata.options.ignore_formatting && run_formatting(left) != run_formatting(right)
            {
                formatting_diagnostic(diagnostics, format!("{location}/run[{index}]"));
            }
        }
        for (index, ((_, _, _, left), (_, _, _, right))) in original
            .content_controls
            .iter()
            .zip(&edited.content_controls)
            .enumerate()
        {
            compare_control(
                left,
                right,
                &format!("{location}/content-control[{index}]"),
                metadata,
                diagnostics,
            )?;
        }
        return original_source
            .map(str::to_owned)
            .map_or_else(|| paragraph_xml(original), Ok);
    }
    if paragraph_boundaries_differ(original, edited, metadata.options)
        || (!metadata.options.ignore_formatting
            && paragraph_properties_differ(
                original.properties.as_ref(),
                edited.properties.as_ref(),
            ))
    {
        return Err(Error::Other(format!(
            "comparison cannot revise paragraph boundary structures at {location}"
        )));
    }

    let source = original_source
        .map(str::to_owned)
        .map_or_else(|| paragraph_xml(original), Ok)?;
    let run_spans = modeled_paragraph_run_spans(original, &source)?;
    if run_spans.len() != original.runs.len() {
        return Err(Error::Other(format!(
            "comparison could not correlate complex paragraph runs at {location}"
        )));
    }
    let original_signatures = original
        .runs
        .iter()
        .map(|run| run_signature_with_options(run, metadata.options))
        .collect::<Vec<_>>();
    let edited_signatures = edited
        .runs
        .iter()
        .map(|run| run_signature_with_options(run, metadata.options))
        .collect::<Vec<_>>();
    let aligned = align(&original_signatures, &edited_signatures);
    if !metadata.options.ignore_fields {
        validate_field_alignment(
            &aligned,
            &original.runs,
            &edited.runs,
            &original_signatures,
            &edited_signatures,
            location,
        )?;
    }
    // One read of the paragraph answers the scope at every run edge.
    let parents = ParentScopes::new(
        &source,
        std::iter::once(0)
            .chain(run_spans.iter().flat_map(|span| [span.start, span.end]))
            .collect(),
    );
    let mut output = String::new();
    let mut cursor = 0usize;
    for (left, right) in aligned {
        let at = left.map_or(cursor, |index| run_spans[index].start);
        let destination_parent = parents.at(at)?;
        let revision_parent = metadata.ids.revision_parent_scope(&destination_parent);
        if let Some(index) = left {
            let span = &run_spans[index];
            output.push_str(&source[cursor..span.start]);
            cursor = span.end;
        }
        match (left, right) {
            (Some(i), Some(j)) if original_signatures[i] == edited_signatures[j] => {
                if !run_is_field(&original.runs[i]) && original.runs[i] == edited.runs[j] {
                    output.push_str(&source[run_spans[i].clone()]);
                } else {
                    output.push_str(&compared_run_xml(
                        &original.runs[i],
                        &edited.runs[j],
                        None,
                        None,
                        &format!("{location}/run[{j}]"),
                        &destination_parent,
                        metadata,
                        diagnostics,
                    )?);
                }
            }
            (Some(i), Some(j)) => {
                output.push_str(&paired_run_replacement(
                    [&original.runs[i], &edited.runs[j]],
                    [None, None],
                    [None, None],
                    &format!("{location}/run[{j}]"),
                    &destination_parent,
                    metadata,
                    diagnostics,
                )?);
            }
            (Some(i), None) => {
                if run_is_ignored(&original.runs[i], metadata.options) {
                    output.push_str(&source[run_spans[i].clone()]);
                    continue;
                }
                let deleted = publish_run_source(
                    &original.runs[i],
                    &original.runs[i],
                    deleted_run_xml(&original.runs[i])?,
                    None,
                    None,
                    None,
                    &revision_parent,
                    metadata,
                )?;
                output.push_str(&metadata.ids.revision(
                    "del",
                    metadata.author,
                    metadata.timestamp,
                    &deleted,
                )?);
            }
            (None, Some(j)) => {
                if run_is_ignored(&edited.runs[j], metadata.options) {
                    continue;
                }
                let inserted = publish_run_source(
                    &edited.runs[j],
                    &edited.runs[j],
                    paragraph_owned_run_xml(&edited.runs[j])?,
                    None,
                    None,
                    None,
                    &revision_parent,
                    metadata,
                )?;
                output.push_str(&metadata.ids.revision(
                    "ins",
                    metadata.author,
                    metadata.timestamp,
                    &inserted,
                )?);
            }
            (None, None) => unreachable!(),
        }
    }
    output.push_str(&source[cursor..]);
    let spans = direct_word_element_spans(&output, "sdt")?;
    if spans.len() != original.content_controls.len() {
        return Err(Error::Other(format!(
            "comparison could not correlate paragraph content controls at {location}"
        )));
    }
    let mut replacements = Vec::with_capacity(spans.len());
    let mut sources = source_children(&output, &spans).into_iter();
    for (index, ((_, _, _, left), (_, _, _, right))) in original
        .content_controls
        .iter()
        .zip(&edited.content_controls)
        .enumerate()
    {
        let source = sources.next().expect("one source per control span");
        replacements.push(compare_control_from_xml(
            left,
            right,
            &format!("{location}/content-control[{index}]"),
            metadata,
            diagnostics,
            &source?,
        )?);
    }
    replace_direct_word_elements(&output, "sdt", &replacements)
}

fn paragraph_control_boundaries(paragraph: &CT_P) -> Vec<(usize, usize, usize)> {
    paragraph
        .content_controls
        .iter()
        .map(|(at, raw_before, markers_before, _)| (*at, *raw_before, *markers_before))
        .collect()
}

/// Whether two paragraphs differ in an inline structure that revising the
/// paragraph in place cannot express: a hyperlink, comment range, bookmark,
/// preserved raw child or inline content control.
///
/// The word and character paths follow a hyperlink or control through the
/// text alignment, so only the shells and their slots count there.
/// Boundary coordinates in the comparison-only plain-run view. Source owners
/// remain untouched, and every structural shell still terminates a group.
fn coalesced_run_boundaries(paragraph: &CT_P, options: &ComparisonOptions) -> Vec<usize> {
    let cuts = shell_run_boundaries(paragraph, options)
        .into_iter()
        .collect::<HashSet<_>>();
    let mut boundaries = vec![0];
    for (index, run) in paragraph.runs.iter().enumerate() {
        let joins = index > 0
            && !cuts.contains(&index)
            && coalescible_plain_run(run)
            && coalescible_plain_run(&paragraph.runs[index - 1])
            && run_formatting(run) == run_formatting(&paragraph.runs[index - 1]);
        boundaries.push(boundaries[index] + usize::from(!joins));
    }
    boundaries
}

pub(super) fn paragraph_boundaries_differ(
    original: &CT_P,
    edited: &CT_P,
    options: &ComparisonOptions,
) -> bool {
    let original_boundaries = coalesced_run_boundaries(original, options);
    let edited_boundaries = coalesced_run_boundaries(edited, options);
    let comments = |paragraph: &CT_P, boundaries: &[usize]| {
        paragraph
            .comment_ranges
            .iter()
            .cloned()
            .map(|mut marker| {
                let (CommentRangeMarker::Start { run_index, .. }
                | CommentRangeMarker::End { run_index, .. }) = &mut marker;
                *run_index = boundaries.get(*run_index).copied().unwrap_or(*run_index);
                marker
            })
            .collect::<Vec<_>>()
    };
    let bookmarks = |paragraph: &CT_P, boundaries: &[usize]| {
        paragraph
            .current_bookmark_projection()
            .iter()
            .map(|marker| {
                (
                    marker.is_start(),
                    marker.id(),
                    marker.name().map(str::to_owned),
                    marker.raw_before(),
                    marker.has_child_content(),
                    boundaries
                        .get(marker.run_index())
                        .copied()
                        .unwrap_or(marker.run_index()),
                    boundaries
                        .get(marker.projected_run_index())
                        .copied()
                        .unwrap_or(marker.projected_run_index()),
                    boundaries
                        .get(marker.tracked_run_index())
                        .copied()
                        .unwrap_or(marker.tracked_run_index()),
                )
            })
            .collect::<Vec<_>>()
    };
    if (!options.ignore_comments
        && comments(original, &original_boundaries) != comments(edited, &edited_boundaries))
        || bookmarks(original, &original_boundaries) != bookmarks(edited, &edited_boundaries)
    {
        return true;
    }
    if uses_attributed_run_path(options) {
        let control_slots = |paragraph: &CT_P| {
            paragraph
                .content_controls
                .iter()
                .map(|(_, raw_before, markers_before, _)| (*raw_before, *markers_before))
                .collect::<Vec<_>>()
        };
        return hyperlink_shells(original, options) != hyperlink_shells(edited, options)
            || control_slots(original) != control_slots(edited);
    }
    original.hyperlinks != edited.hyperlinks
        || original
            .extra_xml
            .iter()
            .filter(|(position, raw)| !CT_P::raw_is_root_attributes(*position, raw))
            .ne(edited
                .extra_xml
                .iter()
                .filter(|(position, raw)| !CT_P::raw_is_root_attributes(*position, raw)))
        || paragraph_control_boundaries(original) != paragraph_control_boundaries(edited)
}
