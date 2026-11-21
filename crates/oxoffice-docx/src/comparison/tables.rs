//! Comparing tables, rows and cells.

use super::*;

pub(super) fn compare_table(
    original: &CT_Tbl,
    edited: &CT_Tbl,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let output = (|| -> Result<String> {
        if original.grid != edited.grid {
            let deleted = marked_table(original, "del", metadata)?;
            let inserted = marked_table(edited, "ins", metadata)?;
            return Ok(format!("{deleted}{inserted}"));
        }
        if original.extra_xml != edited.extra_xml
            || table_control_boundaries(original) != table_control_boundaries(edited)
        {
            return Err(Error::Other(format!(
                "comparison cannot revise table boundary structures at {location}"
            )));
        }
        let original_signatures = original
            .rows
            .iter()
            .map(|row| row_signature_with_options(row, metadata.options))
            .collect::<Vec<_>>();
        let edited_signatures = edited
            .rows
            .iter()
            .map(|row| row_signature_with_options(row, metadata.options))
            .collect::<Vec<_>>();
        let mut source = original_source
            .map(str::to_owned)
            .map_or_else(|| table_xml(original), Ok)?;
        let properties = table_properties_xml(
            original.properties.as_ref(),
            edited.properties.as_ref(),
            location,
            metadata,
            diagnostics,
        )?;
        let template = direct_property(&source, "tblPr")?;
        let before_properties = original
            .properties
            .as_ref()
            .map(table_property_xml)
            .transpose()?
            .unwrap_or_default();
        let properties = patch_property_source(
            &before_properties,
            &properties,
            template.as_deref(),
            &metadata.ids,
        )?;
        let properties = if let Some(template) = template {
            preserve_prior_property_sources(&properties, &template)?
        } else {
            metadata.ids.generated(properties)?
        };
        let property_spans = direct_word_element_spans(&source, "tblPr")?;
        match (property_spans.first(), properties.is_empty()) {
            (Some(span), false) => source.replace_range(span.clone(), &properties),
            (Some(span), true) => source.replace_range(span.clone(), ""),
            (None, false) => {
                source = insert_first_child(&source, &properties)?;
            }
            (None, true) => {}
        }
        let row_spans = direct_word_element_spans(&source, "tr")?;
        if row_spans.len() != original.rows.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate direct table rows at {location}"
            )));
        }
        let aligned = align(&original_signatures, &edited_signatures);
        // One read of the table answers the source of every row.
        let mut row_sources = source_children(&source, &row_spans)
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        let mut output = String::new();
        let mut cursor = 0usize;
        for (alignment_index, &(left, right)) in aligned.iter().enumerate() {
            if let Some(index) = left {
                let span = &row_spans[index];
                output.push_str(&source[cursor..span.start]);
                cursor = span.end;
            } else {
                let next_row_start = aligned[alignment_index + 1..]
                    .iter()
                    .find_map(|(next_left, _)| next_left.map(|index| row_spans[index].start))
                    .map_or_else(|| paragraph_close_start(&source), Ok)?;
                output.push_str(&source[cursor..next_row_start]);
                cursor = next_row_start;
            }
            let child_mark = metadata.leaf_publications.len();
            let child_offset = output.len();
            match (left, right) {
                (Some(i), Some(j)) => {
                    output.push_str(&compare_row(
                        &original.rows[i],
                        &edited.rows[j],
                        Some(&row_sources[i].take().expect("a row is compared once")?),
                        &format!("{location}/row[{j}]"),
                        metadata,
                        diagnostics,
                    )?);
                }
                (Some(i), None) => {
                    output.push_str(&marked_row(&original.rows[i], "del", metadata)?)
                }
                (None, Some(j)) => output.push_str(&marked_row(&edited.rows[j], "ins", metadata)?),
                (None, None) => unreachable!(),
            }
            shift_leaf_publications(
                &mut metadata.leaf_publications[child_mark..],
                child_offset as isize,
            )?;
        }
        output.push_str(&source[cursor..]);
        let spans = direct_word_element_spans(&output, "sdt")?;
        if spans.len() != original.content_controls.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate table content controls at {location}"
            )));
        }
        let mut child_leaves = Vec::new();
        let mut replacements = Vec::with_capacity(spans.len());
        let mut sources = source_children(&output, &spans).into_iter();
        for (index, ((_, _, left), (_, _, right))) in original
            .content_controls
            .iter()
            .zip(&edited.content_controls)
            .enumerate()
        {
            let child_mark = metadata.leaf_publications.len();
            let source = sources.next().expect("one source per control span");
            replacements.push(compare_control_from_xml(
                left,
                right,
                &format!("{location}/content-control[{index}]"),
                metadata,
                diagnostics,
                &source?,
            )?);
            child_leaves.push(metadata.leaf_publications.split_off(child_mark));
        }
        splice_leaf_publications(metadata, leaf_mark, &spans, &replacements, child_leaves)?;
        replace_direct_word_elements(&output, "sdt", &replacements)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_Tbl as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

fn table_properties_xml(
    original: Option<&CT_TblPr>,
    edited: Option<&CT_TblPr>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    if metadata.options.ignore_formatting {
        return original
            .map(table_property_xml)
            .transpose()
            .map(Option::unwrap_or_default)
            .and_then(|xml| metadata.ids.generated(xml));
    }
    let original_modeled = modeled_table_properties(original);
    let edited_modeled = modeled_table_properties(edited);
    let unmodeled = |properties: &CT_TblPr| {
        (
            properties.extra_xml.clone(),
            properties.revision_xml.clone(),
        )
    };
    if original.map(unmodeled).unwrap_or_default() != edited.map(unmodeled).unwrap_or_default() {
        formatting_diagnostic(diagnostics, location.to_owned());
    }
    if original_modeled == edited_modeled {
        return original
            .map(table_property_xml)
            .transpose()
            .map(Option::unwrap_or_default)
            .and_then(|xml| metadata.ids.generated(xml));
    }
    let mut current = edited.cloned().unwrap_or_default();
    current.change = None;
    let (original_extra_xml, original_revision_xml) = original.map(unmodeled).unwrap_or_default();
    current.extra_xml = original_extra_xml;
    current.revision_xml = original_revision_xml;
    let previous = original_modeled
        .as_ref()
        .map(table_property_xml)
        .transpose()?
        .unwrap_or_else(|| "<w:tblPr/>".to_owned());
    let change = metadata.ids.revision(
        "tblPrChange",
        metadata.author,
        metadata.timestamp,
        &previous,
    )?;
    let current = table_property_xml(&current)?;
    metadata
        .ids
        .generated(inject_before_close(&current, "</w:tblPr>", &change)?)
}

fn modeled_table_properties(properties: Option<&CT_TblPr>) -> Option<CT_TblPr> {
    properties.cloned().map(|mut properties| {
        properties.change = None;
        properties.revision_xml.clear();
        properties.extra_xml.clear();
        // An empty border container, as LibreOffice writes it, is no border.
        if properties
            .borders
            .as_ref()
            .is_some_and(oxoffice_docx_oxml::table::CT_TblBorders::is_empty)
        {
            properties.borders = None;
        }
        properties
    })
}

fn table_property_xml(properties: &CT_TblPr) -> Result<String> {
    let mut bytes = Vec::new();
    properties.to_xml(&mut Writer::new(&mut bytes))?;
    String::from_utf8(bytes).map_err(utf8_error)
}

fn table_control_boundaries(table: &CT_Tbl) -> Vec<(usize, usize)> {
    table
        .content_controls
        .iter()
        .map(|(at, raw_before, _)| (*at, *raw_before))
        .collect()
}

pub(super) fn marked_table(
    table: &CT_Tbl,
    kind: &str,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let source = metadata
        .table_source(table)
        .map(str::to_owned)
        .map_or_else(|| table_xml(table), Ok)?;
    let replacements = table
        .rows
        .iter()
        .map(|row| marked_row(row, kind, metadata))
        .collect::<Result<Vec<_>>>()?;
    let output = replace_direct_word_elements(&source, "tr", &replacements)?;
    let control_spans = direct_word_element_spans(&output, "sdt")?;
    if control_spans.len() != table.content_controls.len() {
        return Err(Error::Other(
            "comparison could not correlate table content controls while marking rows".to_owned(),
        ));
    }
    let replacements = table
        .content_controls
        .iter()
        .zip(&control_spans)
        .map(|((_, _, control), span)| {
            mark_control_owned_rows(control, kind, metadata, &output[span.clone()])
        })
        .collect::<Result<Vec<_>>>()?;
    metadata
        .ids
        .generated(replace_direct_word_elements(&output, "sdt", &replacements)?)
}

fn mark_control_owned_rows(
    control: &CT_Sdt,
    kind: &str,
    metadata: &mut Metadata<'_>,
    original_xml: &str,
) -> Result<String> {
    let content_span = direct_word_element_spans(original_xml, "sdtContent")?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("marked control has no actual content owner".into()))?;
    let actual_content = source_child(original_xml, content_span)?;
    let destination_parent = root_namespace_bindings(&actual_content, "sdtContent", &[])?;
    let mut content = String::new();
    for child in &control.content {
        content.push_str(&match child {
            SdtContent::Paragraph(paragraph) => metadata
                .paragraph_source(paragraph)
                .map(str::to_owned)
                .map_or_else(
                    || paragraph_xml(paragraph).and_then(|xml| metadata.ids.generated(xml)),
                    Ok,
                )?,
            SdtContent::Table(table) => marked_table(table, kind, metadata)?,
            SdtContent::Row(row) => marked_row(row, kind, metadata)?,
            SdtContent::Cell(cell) => metadata.cell_source(cell).map(str::to_owned).map_or_else(
                || cell_xml(cell).and_then(|xml| metadata.ids.generated(xml)),
                Ok,
            )?,
            SdtContent::Run(run) => publish_run_source(
                run,
                run,
                paragraph_owned_run_xml(run)?,
                None,
                None,
                None,
                &destination_parent,
                metadata,
            )?,
            SdtContent::ContentControl(nested) => {
                let nested_xml = metadata
                    .control_source(nested)
                    .map(str::to_owned)
                    .map_or_else(|| control_xml(nested), Ok)?;
                mark_control_owned_rows(nested, kind, metadata, &nested_xml)?
            }
            SdtContent::RawXml(raw) => String::from_utf8(raw.clone()).map_err(utf8_error)?,
        });
    }
    metadata.ids.generated(replace_element_inner(
        original_xml,
        "w:sdtContent",
        &content,
    )?)
}

pub(super) fn marked_row(row: &CT_Row, kind: &str, metadata: &mut Metadata<'_>) -> Result<String> {
    let source = metadata
        .row_source(row)
        .map(str::to_owned)
        .map_or_else(|| row_xml(row), Ok)?;
    marked_row_xml(&source, kind, metadata)
}

/// Mark a serialized row as Word marks an inserted or deleted row: the row
/// marker in `w:trPr`, then every cell paragraph mark and every cell run,
/// nested tables included. Without the cell marks Word merges an adjacent
/// removed paragraph mark into the first cell and leaves the row unresolved.
fn marked_row_xml(row: &str, kind: &str, metadata: &mut Metadata<'_>) -> Result<String> {
    let marker = metadata
        .ids
        .marker(kind, metadata.author, metadata.timestamp)?;
    let mut xml = row.to_owned();
    let properties = direct_word_element_spans(&xml, "trPr")?;
    if let Some(properties_span) = properties.first() {
        let updated = append_word_child(&xml[properties_span.clone()], "trPr", &marker)?;
        xml.replace_range(properties_span.clone(), &updated);
    } else {
        // `w:trPr` follows an optional `w:tblPrEx`.
        let properties = metadata
            .ids
            .generated(format!("<w:trPr>{marker}</w:trPr>"))?;
        if let Some(exception) = direct_word_element_spans(&xml, "tblPrEx")?.first() {
            xml.insert_str(exception.end, &properties);
        } else {
            xml = insert_first_child(&xml, &properties)?;
        }
    }
    let cell_spans = direct_word_element_spans(&xml, "tc")?;
    let cells = source_children(&xml, &cell_spans)
        .into_iter()
        .map(|source| marked_cell_xml(&source?, kind, metadata))
        .collect::<Result<Vec<_>>>()?;
    metadata
        .ids
        .generated(replace_direct_word_elements(&xml, "tc", &cells)?)
}

pub(super) fn marked_cell_xml(
    cell: &str,
    kind: &str,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let paragraph_spans = direct_word_element_spans(cell, "p")?;
    let paragraphs = source_children(cell, &paragraph_spans)
        .into_iter()
        .map(|source| {
            let marker = metadata
                .ids
                .marker(kind, metadata.author, metadata.timestamp)?;
            let source = source?;
            let paragraph = marked_paragraph_xml(&source, &marker, &metadata.ids)?;
            let paragraph = marked_direct_runs_xml(&paragraph, kind, metadata)?;
            let hyperlink_spans = direct_word_element_spans(&paragraph, "hyperlink")?;
            let hyperlinks = hyperlink_spans
                .iter()
                .cloned()
                .zip(source_children(&paragraph, &hyperlink_spans))
                .map(|(span, scoped)| {
                    let scoped = scoped?;
                    let marked = marked_direct_runs_xml(&scoped, kind, metadata)?;
                    let destination = root_namespace_bindings(&paragraph, "p", &[])?;
                    retain_source_root_namespaces(&marked, &paragraph[span], &destination)
                })
                .collect::<Result<Vec<_>>>()?;
            replace_direct_word_elements(&paragraph, "hyperlink", &hyperlinks)
        })
        .collect::<Result<Vec<_>>>()?;
    let cell = replace_direct_word_elements(cell, "p", &paragraphs)?;
    let table_spans = direct_word_element_spans(&cell, "tbl")?;
    let tables = source_children(&cell, &table_spans)
        .into_iter()
        .map(|scoped_table| {
            let scoped_table = scoped_table?;
            let table = scoped_table.as_str();
            let row_spans = direct_word_element_spans(table, "tr")?;
            let rows = source_children(table, &row_spans)
                .into_iter()
                .map(|source| marked_row_xml(&source?, kind, metadata))
                .collect::<Result<Vec<_>>>()?;
            replace_direct_word_elements(table, "tr", &rows)
        })
        .collect::<Result<Vec<_>>>()?;
    metadata
        .ids
        .generated(replace_direct_word_elements(&cell, "tbl", &tables)?)
}

/// Wrap every direct `w:r` of `owner` in a revision, with deleted text as
/// `w:delText`.
fn marked_direct_runs_xml(owner: &str, kind: &str, metadata: &mut Metadata<'_>) -> Result<String> {
    let spans = direct_word_element_spans(owner, "r")?
        .into_iter()
        .map(|span| (None, span))
        .collect::<Vec<_>>();
    let runs = RunSource::capture_all(&spans, owner)
        .into_iter()
        .map(|source| {
            let source = source?;
            let mut run = source.scoped.clone();
            if kind == "del" {
                for (local, deleted_local) in [("t", "delText"), ("instrText", "delInstrText")] {
                    for text in direct_word_element_spans(&run, local)?.into_iter().rev() {
                        let raw = &run[text.clone()];
                        let (_, empty, name) = element_opening(raw)?;
                        let prefix = name
                            .rsplit_once(':')
                            .map(|(prefix, _)| format!("{prefix}:"))
                            .unwrap_or_default();
                        let mut deleted = raw.to_owned();
                        if !empty {
                            let close = deleted
                                .rfind(&format!("</{name}>"))
                                .ok_or_else(|| Error::Other("marked text has no end".into()))?;
                            deleted.replace_range(
                                close + 2..close + 2 + name.len(),
                                &format!("{prefix}{deleted_local}"),
                            );
                        }
                        deleted
                            .replace_range(1..1 + name.len(), &format!("{prefix}{deleted_local}"));
                        run.replace_range(text, &deleted);
                    }
                }
            }
            let revision_parent = metadata.ids.revision_parent_scope(&source.parent);
            run = retain_source_root_namespaces(&run, &source.raw, &revision_parent)?;
            metadata
                .ids
                .revision(kind, metadata.author, metadata.timestamp, &run)
        })
        .collect::<Result<Vec<_>>>()?;
    replace_direct_word_elements(owner, "r", &runs)
}

pub(super) fn compare_row(
    original: &CT_Row,
    edited: &CT_Row,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let output = (|| -> Result<String> {
        if original.cells.len() != edited.cells.len()
            || row_raw_children(original) != row_raw_children(edited)
            || row_control_boundaries(original) != row_control_boundaries(edited)
        {
            return Err(Error::Other(format!(
                "comparison cannot revise row boundary structures at {location}"
            )));
        }
        if !metadata.options.ignore_formatting
            && row_formatting(original)? != row_formatting(edited)?
        {
            formatting_diagnostic(diagnostics, location.to_owned());
        }
        let mut output = original_source
            .map(str::to_owned)
            .map_or_else(|| row_xml(original), Ok)?;
        let cell_spans = direct_word_element_spans(&output, "tc")?;
        if cell_spans.len() != original.cells.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate row cells at {location}"
            )));
        }
        let mut child_leaves = Vec::new();
        let mut cell_replacements = Vec::with_capacity(cell_spans.len());
        let mut sources = source_children(&output, &cell_spans).into_iter();
        for (index, (left, right)) in original.cells.iter().zip(&edited.cells).enumerate() {
            let child_mark = metadata.leaf_publications.len();
            let source = sources.next().expect("one source per cell span");
            cell_replacements.push(compare_cell_from_xml(
                left,
                right,
                &format!("{location}/cell[{index}]"),
                metadata,
                diagnostics,
                &source?,
            )?);
            child_leaves.push(metadata.leaf_publications.split_off(child_mark));
        }
        splice_leaf_publications(
            metadata,
            leaf_mark,
            &cell_spans,
            &cell_replacements,
            child_leaves,
        )?;
        output = replace_direct_word_elements(&output, "tc", &cell_replacements)?;

        let control_spans = direct_word_element_spans(&output, "sdt")?;
        if control_spans.len() != original.content_controls.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate row content controls at {location}"
            )));
        }
        let mut child_leaves = Vec::new();
        let mut control_replacements = Vec::with_capacity(control_spans.len());
        let mut sources = source_children(&output, &control_spans).into_iter();
        for (index, ((_, _, left), (_, _, right))) in original
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
        replace_direct_word_elements(&output, "sdt", &control_replacements)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_Row as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

/// Return the raw children of a row without the record of its start-tag
/// attributes, which carry producer identities and no content.
pub(super) fn row_raw_children(row: &CT_Row) -> Vec<&(usize, Vec<u8>)> {
    row.extra_xml
        .iter()
        .filter(|(position, raw)| !CT_Row::raw_is_root_attributes(*position, raw))
        .collect()
}

fn row_control_boundaries(row: &CT_Row) -> Vec<(usize, usize)> {
    row.content_controls
        .iter()
        .map(|(at, raw_before, _)| (*at, *raw_before))
        .collect()
}

pub(super) fn compare_cell_from_xml(
    original: &CT_Tc,
    edited: &CT_Tc,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
    original_xml: &str,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let output = (|| -> Result<String> {
        // Children compared index by index unless a pair there would refuse:
        // the counts differ, the kinds differ, or a paragraph pair needs a
        // whole replacement that the pair alone cannot hold.
        let last = original.content.len().saturating_sub(1);
        let aligned_children = original.extra_xml.is_empty()
            && edited.extra_xml.is_empty()
            && (original.content.len() != edited.content.len()
                || original
                    .content
                    .iter()
                    .zip(&edited.content)
                    .enumerate()
                    .any(|(index, pair)| match pair {
                        (CellContent::Paragraph(left), CellContent::Paragraph(right)) => {
                            paragraph_boundaries_differ(left, right, metadata.options)
                                && (left.bookmark_markers != right.bookmark_markers
                                    || index == last)
                        }
                        (CellContent::Table(_), CellContent::Table(_))
                        | (CellContent::ContentControl(_), CellContent::ContentControl(_)) => false,
                        _ => true,
                    }));
        if !aligned_children
            && (original.content.len() != edited.content.len()
                || original.extra_xml != edited.extra_xml)
        {
            return Err(Error::Other(format!(
                "comparison cannot revise cell boundary structures at {location}"
            )));
        }
        if !metadata.options.ignore_formatting
            && cell_formatting(original)? != cell_formatting(edited)?
        {
            formatting_diagnostic(diagnostics, location.to_owned());
        }
        if aligned_children {
            return compare_aligned_cell_children(
                original,
                edited,
                location,
                metadata,
                diagnostics,
                original_xml,
            );
        }
        let children = cell_child_spans(original, original_xml, location)?;
        let mut child_leaves = Vec::new();
        let mut replacements = Vec::with_capacity(children.len());
        let child_spans = children
            .iter()
            .map(|(_, span)| span.clone())
            .collect::<Vec<_>>();
        let mut sources = source_children(original_xml, &child_spans).into_iter();
        for (index, ((left, right), _)) in original
            .content
            .iter()
            .zip(&edited.content)
            .zip(&children)
            .enumerate()
        {
            let child_location = format!("{location}/content[{index}]");
            let source = sources.next().expect("one source per cell child span")?;
            let source = source.as_str();
            let child_mark = metadata.leaf_publications.len();
            replacements.push(compare_cell_child(
                left,
                right,
                source,
                &child_location,
                metadata,
                diagnostics,
            )?);
            child_leaves.push(metadata.leaf_publications.split_off(child_mark));
        }
        splice_leaf_publications(
            metadata,
            leaf_mark,
            &children
                .iter()
                .map(|(_, span)| span.clone())
                .collect::<Vec<_>>(),
            &replacements,
            child_leaves,
        )?;
        replace_ranges(original_xml, &children, &replacements)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_Tc as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

fn compare_cell_child(
    original: &CellContent,
    edited: &CellContent,
    source: &str,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    match (original, edited) {
        (CellContent::Paragraph(left), CellContent::Paragraph(right)) => {
            compare_paragraph(left, right, Some(source), location, metadata, diagnostics)
        }
        (CellContent::Table(left), CellContent::Table(right)) => {
            compare_table(left, right, Some(source), location, metadata, diagnostics)
        }
        (CellContent::ContentControl(left), CellContent::ContentControl(right)) => {
            compare_control_from_xml(left, right, location, metadata, diagnostics, source)
        }
        _ => Err(Error::Other(format!(
            "comparison cannot revise incompatible cell children at {location}"
        ))),
    }
}

/// Compare a cell whose number of children changed, aligning them as a block
/// content control aligns its own. A paragraph that one side inserts,
/// deletes, merges into another or splits off becomes a whole tracked
/// paragraph with its mark, and a pair whose inline boundaries differ is
/// replaced by the run of changed paragraphs around it. The last paragraph
/// mark stays the cell terminator: a final inserted or deleted paragraph
/// marks the paragraph mark before it instead, as [`final_paragraph`] does
/// in a story.
fn compare_aligned_cell_children(
    original: &CT_Tc,
    edited: &CT_Tc,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
    original_xml: &str,
) -> Result<String> {
    let options = metadata.options;
    let signatures = |content: &[CellContent]| {
        content
            .iter()
            .map(|child| match child {
                CellContent::Paragraph(paragraph) => {
                    format!("p:{}", paragraph_signature_with_options(paragraph, options))
                }
                CellContent::Table(table) => {
                    format!("t:{}", table_signature_with_options(table, options))
                }
                CellContent::ContentControl(control) => {
                    format!("s:{}", control_signature_with_options(control, options))
                }
            })
            .collect::<Vec<_>>()
    };
    fn paragraphs(content: &[CellContent]) -> Vec<Option<&CT_P>> {
        content
            .iter()
            .map(|child| match child {
                CellContent::Paragraph(paragraph) => Some(paragraph),
                _ => None,
            })
            .collect()
    }
    let original_signatures = signatures(&original.content);
    let edited_signatures = signatures(&edited.content);
    let original_paragraphs = paragraphs(&original.content);
    let edited_paragraphs = paragraphs(&edited.content);
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
        options,
    )?;
    let is_paragraph =
        |child: Option<&CellContent>| matches!(child, Some(CellContent::Paragraph(_)));
    let trailing_paragraph_insert_start = aligned
        .iter()
        .enumerate()
        .rev()
        .take_while(|(_, (left, right))| {
            left.is_none() && is_paragraph(right.and_then(|index| edited.content.get(index)))
        })
        .map(|(position, _)| position)
        .last();
    let trailing_paragraph_delete_start = aligned
        .iter()
        .enumerate()
        .rev()
        .take_while(|(_, (left, right))| {
            right.is_none() && is_paragraph(left.and_then(|index| original.content.get(index)))
        })
        .map(|(position, _)| position)
        .last();
    let children = cell_child_spans(original, original_xml, location)?;
    let (Some((_, first)), Some((_, last))) = (children.first(), children.last()) else {
        return Err(Error::Other(format!(
            "comparison cannot revise cell boundary structures at {location}"
        )));
    };
    let (first, last) = (first.clone(), last.clone());
    // Source text between two children, which only whitespace can be when the
    // cell holds no unmodeled child. It stays before the original child it
    // preceded.
    let mut whitespace_slots = vec![String::new(); children.len()];
    for (index, pair) in children.windows(2).enumerate() {
        let between = &original_xml[pair[0].1.end..pair[1].1.start];
        if !between.bytes().all(|byte| byte.is_ascii_whitespace()) {
            return Err(Error::Other(format!(
                "comparison cannot revise cell boundary structures at {location}"
            )));
        }
        whitespace_slots[index + 1] = between.to_owned();
    }
    let child_spans = children
        .iter()
        .map(|(_, span)| span.clone())
        .collect::<Vec<_>>();
    let sources = source_children(original_xml, &child_spans)
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let bindings = root_namespace_bindings(original_xml, "tc", &[])?;
    let mut content: Vec<(EmittedOwner, String)> = Vec::new();
    let mut content_leaves: Vec<Vec<LeafPublication>> = Vec::new();
    let mut trailing_merged = PropertySide::default();
    let mut whitespace_emitted = vec![false; whitespace_slots.len()];
    for (position, (left, right)) in aligned.iter().copied().enumerate() {
        let whitespace_boundary = left.or_else(|| {
            let right = right?;
            let (Some(next_left), None) = aligned.get(position + 1).copied()? else {
                return None;
            };
            (matches!(original.content[next_left], CellContent::Paragraph(_))
                && matches!(edited.content[right], CellContent::Table(_)))
            .then_some(next_left)
        });
        if let Some(index) = whitespace_boundary
            && !whitespace_emitted[index]
            && !whitespace_slots[index].is_empty()
        {
            content.push((EmittedOwner::Whitespace, whitespace_slots[index].clone()));
            content_leaves.push(Vec::new());
            whitespace_emitted[index] = true;
        }
        let item = |(left, right): (Option<usize>, Option<usize>)| {
            right
                .and_then(|index| edited.content.get(index))
                .or_else(|| left.and_then(|index| original.content.get(index)))
        };
        let next_takes_mark = next_kept_owner(&aligned, position, |pair| {
            matches!(item(pair), Some(CellContent::Table(_)))
        })
        .is_some_and(|pair| {
            matches!(
                item(pair),
                Some(CellContent::Paragraph(_) | CellContent::Table(_))
            )
        });
        let child_mark = metadata.leaf_publications.len();
        let child_location = format!("{location}/content[{position}]");
        match (left, right) {
            (Some(i), Some(j)) => content.push((
                EmittedOwner::of_cell(&original.content[i], &edited.content[j], None, metadata)?,
                compare_cell_child(
                    &original.content[i],
                    &edited.content[j],
                    &sources[i],
                    &child_location,
                    metadata,
                    diagnostics,
                )?,
            )),
            (Some(i), None) => {
                let child = &original.content[i];
                let whole = carried.original.contains(&i);
                if let CellContent::Paragraph(paragraph) = child
                    && trailing_paragraph_delete_start.is_some_and(|start| position >= start)
                {
                    if trailing_paragraph_delete_start == Some(position) {
                        trailing_merged =
                            mark_previous_paragraph(&mut content, "del", &bindings, metadata)?;
                    }
                    if next_takes_mark {
                        content.push((
                            EmittedOwner::paragraph(paragraph, paragraph, metadata)?,
                            marked_cell_child(child, "del", whole, metadata)?,
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
                } else if let CellContent::Paragraph(paragraph) = child
                    && !next_takes_mark
                {
                    let merged = mark_previous_paragraph(&mut content, "del", &bindings, metadata)?;
                    content.push(final_paragraph(
                        paragraph, "del", None, whole, merged, metadata,
                    )?);
                } else {
                    content.push((
                        EmittedOwner::of_cell(child, child, Some("del"), metadata)?,
                        marked_cell_child(child, "del", whole, metadata)?,
                    ));
                }
            }
            (None, Some(j)) => {
                let child = &edited.content[j];
                let whole = carried.edited.contains(&j);
                if let CellContent::Paragraph(paragraph) = child
                    && trailing_paragraph_insert_start.is_some_and(|start| position >= start)
                {
                    if trailing_paragraph_insert_start == Some(position) {
                        trailing_merged =
                            mark_previous_paragraph(&mut content, "ins", &bindings, metadata)?;
                    }
                    if next_takes_mark {
                        content.push((
                            EmittedOwner::paragraph(paragraph, paragraph, metadata)?,
                            marked_cell_child(child, "ins", whole, metadata)?,
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
                } else if let CellContent::Paragraph(paragraph) = child
                    && !next_takes_mark
                {
                    let merged = mark_previous_paragraph(&mut content, "ins", &bindings, metadata)?;
                    content.push(final_paragraph(
                        paragraph, "ins", None, whole, merged, metadata,
                    )?);
                } else {
                    content.push((
                        EmittedOwner::of_cell(child, child, Some("ins"), metadata)?,
                        marked_cell_child(child, "ins", whole, metadata)?,
                    ));
                }
            }
            (None, None) => unreachable!(),
        }
        content_leaves.push(metadata.leaf_publications.split_off(child_mark));
    }
    let mut output = original_xml[..first.start].to_owned();
    for ((_, xml), mut leaves) in content.into_iter().zip(content_leaves) {
        shift_leaf_publications(&mut leaves, output.len() as isize)?;
        metadata.leaf_publications.extend(leaves);
        output.push_str(&xml);
    }
    output.push_str(&original_xml[last.end..]);
    Ok(output)
}

fn marked_cell_child(
    child: &CellContent,
    kind: &str,
    carried: bool,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    match child {
        CellContent::Paragraph(paragraph) if kind == "del" => {
            deleted_paragraph(paragraph, carried, metadata)
        }
        CellContent::Paragraph(paragraph) => inserted_paragraph(paragraph, carried, metadata),
        CellContent::Table(table) => marked_table(table, kind, metadata),
        CellContent::ContentControl(_) => Err(Error::Other(
            "comparison cannot add or remove an unmatched content control in a cell".to_owned(),
        )),
    }
}
