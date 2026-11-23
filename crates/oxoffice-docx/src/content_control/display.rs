//! A control's display text in the typed model: reading it as lines, and replacing it with a value, prompt properties and line breaks included.

use super::*;

pub(super) fn collect_sdt_text(sdt: &CT_Sdt, output: &mut String) {
    let mut lines = Vec::new();
    collect_sdt_lines(sdt, &mut lines);
    output.push_str(&lines.join("\n"));
}

/// The text of a control as lines: one per paragraph of the control and of
/// its nested controls, an inline control's runs and tracked insertions on
/// the current line. Table content reads as one line of its cells' text,
/// each cell's paragraphs on lines of their own, as a cell's text reads.
fn collect_sdt_lines(sdt: &CT_Sdt, lines: &mut Vec<String>) {
    for boundary in 0..=sdt.content.len() {
        for (_, revision) in sdt.revisions().iter().filter(|(at, _)| *at == boundary) {
            if let Some(content) = revision.accepted_content() {
                push_inline_text(lines, content.accepted_text());
            }
        }
        let Some(content) = sdt.content.get(boundary) else {
            continue;
        };
        match content {
            SdtContent::Paragraph(paragraph) => lines.push(paragraph_text(paragraph)),
            SdtContent::Table(table) => lines.push(table_text(table)),
            SdtContent::Row(row) => lines.push(row_text(row)),
            SdtContent::Cell(cell) => lines.push(cell_text(cell)),
            SdtContent::Run(run) => push_inline_text(lines, run.accepted_text()),
            SdtContent::ContentControl(nested) => collect_sdt_lines(nested, lines),
            SdtContent::RawXml(_) => {}
        }
    }
}

/// Add inline text to the current line, or start the first one.
fn push_inline_text(lines: &mut Vec<String>, text: String) {
    match lines.last_mut() {
        Some(line) => line.push_str(&text),
        None => lines.push(text),
    }
}

fn paragraph_text(paragraph: &CT_P) -> String {
    ParagraphRef { inner: paragraph }.text()
}

fn table_text(table: &CT_Tbl) -> String {
    table.rows.iter().map(row_text).collect()
}

fn row_text(row: &CT_Row) -> String {
    row.cells.iter().map(cell_text).collect()
}

fn cell_text(cell: &CT_Tc) -> String {
    let lines: Vec<String> = cell
        .content
        .iter()
        .map(|content| match content {
            CellContent::Paragraph(paragraph) => paragraph_text(paragraph),
            CellContent::Table(table) => table_text(table),
            CellContent::ContentControl(nested) => {
                let mut lines = Vec::new();
                collect_sdt_lines(nested, &mut lines);
                lines.join("\n")
            }
        })
        .collect();
    lines.join("\n")
}

pub(super) fn replace_document_controls(
    body: &mut CT_Body,
    replacements: &ControlReplacements,
) -> Result<()> {
    let expected_count = body.content_controls().len();
    let mut index = 0usize;
    for content in &mut body.content {
        match content {
            BodyContent::Paragraph(paragraph) => {
                replace_paragraph_controls(paragraph, replacements, &mut index)?
            }
            BodyContent::Table(table) => replace_table_controls(table, replacements, &mut index)?,
            BodyContent::ContentControl(control) => {
                replace_one_control(control, replacements, &mut index)?
            }
            BodyContent::RawXml(_) => {}
        }
    }
    if index != expected_count
        || replacements
            .values
            .keys()
            .any(|selected| *selected >= index)
    {
        return Err(Error::Other(
            "content-control traversal changed during staged mutation".to_owned(),
        ));
    }
    Ok(())
}

fn replace_one_control(
    control: &mut CT_Sdt,
    replacements: &ControlReplacements,
    index: &mut usize,
) -> Result<()> {
    if let Some(value) = replacements.values.get(index) {
        let mut current = String::new();
        collect_sdt_text(control, &mut current);
        if current != value.display || value.checkbox.is_some() {
            replace_display_text(
                control,
                &value.display,
                value.checkbox.as_ref().map(|(_, font)| font.as_str()),
                &replacements.placeholder_styles,
            )?;
        }
        if let Some(properties) = &mut control.properties {
            properties.set_value_metadata(value.full_date.as_deref())?;
        }
        if let Some((checked, _)) = &value.checkbox {
            control
                .properties
                .as_mut()
                .unwrap()
                .set_checkbox_checked(*checked)?;
        }
    }
    *index = index
        .checked_add(1)
        .ok_or_else(|| Error::Other("content-control count overflow".to_owned()))?;
    replace_nested_controls(control, replacements, index)
}

fn replace_nested_controls(
    control: &mut CT_Sdt,
    replacements: &ControlReplacements,
    index: &mut usize,
) -> Result<()> {
    for content in &mut control.content {
        match content {
            SdtContent::Paragraph(paragraph) => {
                replace_paragraph_controls(paragraph, replacements, index)?
            }
            SdtContent::Table(table) => replace_table_controls(table, replacements, index)?,
            SdtContent::Row(row) => replace_row_controls(row, replacements, index)?,
            SdtContent::Cell(cell) => replace_cell_controls(cell, replacements, index)?,
            SdtContent::ContentControl(nested) => replace_one_control(nested, replacements, index)?,
            SdtContent::Run(_) | SdtContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn replace_paragraph_controls(
    paragraph: &mut CT_P,
    replacements: &ControlReplacements,
    index: &mut usize,
) -> Result<()> {
    for position in 0..=paragraph.runs.len() {
        for (_, _, _, control) in paragraph
            .content_controls
            .iter_mut()
            .filter(|(at, _, _, _)| *at == position)
        {
            replace_one_control(control, replacements, index)?;
        }
    }
    Ok(())
}

fn replace_table_controls(
    table: &mut CT_Tbl,
    replacements: &ControlReplacements,
    index: &mut usize,
) -> Result<()> {
    for position in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == position)
        {
            replace_one_control(control, replacements, index)?;
        }
        if let Some(row) = table.rows.get_mut(position) {
            replace_row_controls(row, replacements, index)?;
        }
    }
    Ok(())
}

fn replace_row_controls(
    row: &mut CT_Row,
    replacements: &ControlReplacements,
    index: &mut usize,
) -> Result<()> {
    for position in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == position)
        {
            replace_one_control(control, replacements, index)?;
        }
        if let Some(cell) = row.cells.get_mut(position) {
            replace_cell_controls(cell, replacements, index)?;
        }
    }
    Ok(())
}

fn replace_cell_controls(
    cell: &mut CT_Tc,
    replacements: &ControlReplacements,
    index: &mut usize,
) -> Result<()> {
    for content in &mut cell.content {
        match content {
            CellContent::Paragraph(paragraph) => {
                replace_paragraph_controls(paragraph, replacements, index)?
            }
            CellContent::Table(table) => replace_table_controls(table, replacements, index)?,
            CellContent::ContentControl(control) => {
                replace_one_control(control, replacements, index)?
            }
        }
    }
    Ok(())
}

fn replace_display_text(
    control: &mut CT_Sdt,
    value: &str,
    font: Option<&str>,
    placeholder_styles: &[String],
) -> Result<()> {
    let (showing_placeholder, prompt_properties) = match &control.properties {
        Some(properties) => (
            properties.showing_placeholder()?,
            properties.run_properties()?,
        ),
        None => (false, None),
    };
    let replacement = || DisplayReplacement {
        value,
        font,
        showing_placeholder,
        prompt_properties: prompt_properties.as_ref(),
        placeholder_styles,
        wrote_value: false,
        saw_run: false,
    };
    let mut first = replacement();
    replace_sdt_display(control, &mut first);
    if !first.wrote_value {
        if !add_text_to_first_paragraph(control, value) {
            return Err(Error::Other(
                "content control has no typed text display location".to_owned(),
            ));
        }
        // The new run takes the value as the first run of any control does:
        // its line breaks, the font, and the control's run properties.
        replace_sdt_display(control, &mut replacement());
    }
    drop_emptied_paragraphs(control);
    Ok(())
}

/// Remove the paragraphs of a block control that the value left empty, so
/// that Word shows no blank line under it. A paragraph that holds anything
/// but empty runs stays, and so does the first one if all are empty.
fn drop_emptied_paragraphs(control: &mut CT_Sdt) {
    let emptied = |content: &SdtContent| match content {
        SdtContent::Paragraph(paragraph) => is_empty_paragraph(paragraph),
        _ => false,
    };
    let paragraphs = control
        .content
        .iter()
        .filter(|content| matches!(content, SdtContent::Paragraph(_)))
        .count();
    let empty = control
        .content
        .iter()
        .filter(|content| emptied(content))
        .count();
    if paragraphs < 2 || empty == 0 {
        return;
    }
    let mut keep_first = empty == paragraphs;
    control.content.retain(|content| {
        if !emptied(content) {
            return true;
        }
        std::mem::take(&mut keep_first)
    });
}

/// Whether a paragraph holds only runs without content. The attributes of
/// the paragraph and of its runs, such as Word's `w14:paraId` and `w:rsidR`,
/// are no content, nor are the marks Word leaves for its proofing
/// (`w:proofErr`) and layout (`w:lastRenderedPageBreak`). A comment
/// reference shows nothing either, but it stays in the run's content and
/// counts there. A section break and a tracked paragraph mark or property
/// change are content: they would go with the paragraph.
fn is_empty_paragraph(paragraph: &CT_P) -> bool {
    let keeps_state = paragraph.properties.as_ref().is_some_and(|properties| {
        properties.sect_pr.is_some()
            || properties.change.is_some()
            || !properties.revision_xml.is_empty()
            || properties.numbering_revision.is_some()
            || !properties.numbering_revision_xml.is_empty()
            || properties
                .rpr
                .as_ref()
                .is_some_and(|mark| !mark.revision_markers.is_empty() || mark.change.is_some())
    });
    !keeps_state
        && paragraph.runs.iter().all(|run| {
            run.extra_xml.iter().enumerate().all(|(index, raw)| {
                CT_R::raw_child_shows_nothing(raw, run.extra_xml_positions.get(index).copied())
            }) && run
                .content
                .iter()
                .all(|content| matches!(content, RunContent::Text(text) if text.text.is_empty()))
        })
        && paragraph.hyperlinks.is_empty()
        && paragraph.comment_ranges.is_empty()
        && paragraph.bookmark_markers.is_empty()
        && paragraph.extra_xml.iter().all(|(position, raw)| {
            CT_P::raw_is_root_attributes(*position, raw) || is_proofing_mark(raw)
        })
        && paragraph.content_controls.is_empty()
        && paragraph.revisions.is_empty()
        && paragraph.equations.is_empty()
        && paragraph.rubies.is_empty()
}

/// Whether a raw paragraph child is a `w:proofErr`, the mark Word writes at
/// each end of a word its proofing flagged.
fn is_proofing_mark(raw: &[u8]) -> bool {
    let mut reader = Reader::from_reader(raw);
    matches!(
        reader.read_event(),
        Ok(Event::Empty(element) | Event::Start(element))
            if element.local_name().as_ref() == b"proofErr"
    )
}

struct DisplayReplacement<'a> {
    value: &'a str,
    font: Option<&'a str>,
    /// The control shows its placeholder: its text is a prompt.
    showing_placeholder: bool,
    /// The control's own run properties, for a value over a prompt.
    prompt_properties: Option<&'a CT_RPr>,
    /// The ids of the Placeholder Text style, which marks a prompt's run.
    placeholder_styles: &'a [String],
    wrote_value: bool,
    saw_run: bool,
}

fn replace_sdt_display(control: &mut CT_Sdt, replacement: &mut DisplayReplacement<'_>) {
    for content in &mut control.content {
        match content {
            SdtContent::Paragraph(paragraph) => replace_paragraph_display(paragraph, replacement),
            SdtContent::Table(table) => replace_table_display(table, replacement),
            SdtContent::Row(row) => replace_row_display(row, replacement),
            SdtContent::Cell(cell) => replace_cell_display(cell, replacement),
            SdtContent::Run(run) => replace_run_display(run, replacement),
            SdtContent::ContentControl(_) | SdtContent::RawXml(_) => {}
        }
    }
}

fn replace_run_display(run: &mut CT_R, replacement: &mut DisplayReplacement<'_>) {
    let is_first_run = !replacement.saw_run;
    replacement.saw_run = true;
    if is_first_run {
        take_prompt_properties(run, replacement);
    }
    if is_first_run && let Some(font) = replacement.font {
        let properties = run.properties.get_or_insert_with(Default::default);
        properties.font_ascii = Some(font.to_owned());
        properties.font_hansi = Some(font.to_owned());
        properties.font_east_asia = Some(font.to_owned());
        properties.font_cs = Some(font.to_owned());
        properties.font_ascii_theme = None;
        properties.font_hansi_theme = None;
        properties.font_east_asia_theme = None;
        properties.font_cs_theme = None;
    }
    let insert_replacement = is_first_run
        && !replacement.wrote_value
        && !run
            .content
            .iter()
            .any(|content| matches!(content, RunContent::Text(_)));
    let original_raw_positions = insert_replacement.then(|| run.extra_xml_positions.clone());
    let property_boundary = usize::from(run.properties.is_some());
    let removed = run
        .content
        .iter()
        .map(|content| {
            !matches!(
                content,
                RunContent::Text(_)
                    | RunContent::DeletedText(_)
                    | RunContent::CommentReference { .. }
            )
        })
        .collect::<Vec<_>>();
    run.remap_removed_content(&removed);
    run.content.retain(|content| {
        matches!(
            content,
            RunContent::Text(_) | RunContent::DeletedText(_) | RunContent::CommentReference { .. }
        )
    });
    let mut written_at = None;
    for (at, content) in run.content.iter_mut().enumerate() {
        if let RunContent::Text(text) = content {
            if is_first_run && !replacement.wrote_value {
                *text = CT_Text::new(replacement.value);
                replacement.wrote_value = true;
                written_at = Some(at);
            } else {
                text.text.clear();
                text.preserve_space = false;
            }
        }
    }
    if is_first_run && !replacement.wrote_value {
        if let Some(original_raw_positions) = original_raw_positions {
            for (position, original) in run
                .extra_xml_positions
                .iter_mut()
                .zip(original_raw_positions)
            {
                if CT_R::raw_child_position(original) > property_boundary {
                    let shifted = CT_R::raw_child_position(*position) + 1;
                    CT_R::set_raw_child_position(position, shifted);
                }
            }
        }
        run.content
            .insert(0, RunContent::Text(CT_Text::new(replacement.value)));
        replacement.wrote_value = true;
        written_at = Some(0);
    }
    if let Some(at) = written_at {
        break_value_lines(run, at, replacement.value);
    }
}

/// Give the run that receives a value written over a prompt the control's
/// own run properties (ECMA-376 17.5.2.27), or, when the control has none,
/// drop the prompt's Placeholder Text style. The text is a prompt when the
/// control shows its placeholder or the run has that style. A value written
/// over an earlier value keeps its run's properties.
fn take_prompt_properties(run: &mut CT_R, replacement: &DisplayReplacement<'_>) {
    let styled = run
        .properties
        .as_ref()
        .and_then(|properties| properties.style_id.as_deref())
        .is_some_and(|style| replacement.placeholder_styles.iter().any(|id| id == style));
    if !styled && !replacement.showing_placeholder {
        return;
    }
    if let Some(properties) = replacement.prompt_properties {
        *run.ensure_properties() = properties.clone();
    } else if styled && let Some(properties) = &mut run.properties {
        properties.style_id = None;
    }
}

/// The run content of a value: its text, with a tab as `w:tab` and a line
/// feed, a carriage return or the two together as one `w:br`, as the other
/// text setters write them.
pub(super) fn value_content(value: &str) -> Vec<RunContent> {
    let mut items = Vec::new();
    let mut text = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        let separator = match character {
            '\t' => RunContent::Tab,
            '\n' | '\r' => {
                if character == '\r' {
                    characters.next_if_eq(&'\n');
                }
                RunContent::Break(BreakType::Line, BreakClear::None)
            }
            _ => {
                text.push(character);
                continue;
            }
        };
        if !text.is_empty() {
            items.push(RunContent::Text(CT_Text::new(&std::mem::take(&mut text))));
        }
        items.push(separator);
    }
    if !text.is_empty() || items.is_empty() {
        items.push(RunContent::Text(CT_Text::new(&text)));
    }
    items
}

/// Write the tabs and line breaks of a value at content index `at` as
/// `w:tab` and `w:br`, instead of raw inside one `w:t`.
fn break_value_lines(run: &mut CT_R, at: usize, value: &str) {
    if !value.contains(['\t', '\n', '\r']) {
        return;
    }
    let items = value_content(value);
    let added = items.len() - 1;
    let property_boundary = usize::from(run.properties.is_some());
    for position in &mut run.extra_xml_positions {
        let decoded = CT_R::raw_child_position(*position);
        if decoded > property_boundary + at {
            CT_R::set_raw_child_position(position, decoded + added);
        }
    }
    run.content.splice(at..=at, items);
}

fn replace_paragraph_display(paragraph: &mut CT_P, replacement: &mut DisplayReplacement<'_>) {
    for run in &mut paragraph.runs {
        replace_run_display(run, replacement);
    }
}

fn replace_table_display(table: &mut CT_Tbl, replacement: &mut DisplayReplacement<'_>) {
    for row in &mut table.rows {
        replace_row_display(row, replacement);
    }
}

fn replace_row_display(row: &mut CT_Row, replacement: &mut DisplayReplacement<'_>) {
    for cell in &mut row.cells {
        replace_cell_display(cell, replacement);
    }
}

fn replace_cell_display(cell: &mut CT_Tc, replacement: &mut DisplayReplacement<'_>) {
    for content in &mut cell.content {
        match content {
            CellContent::Paragraph(paragraph) => replace_paragraph_display(paragraph, replacement),
            CellContent::Table(table) => replace_table_display(table, replacement),
            CellContent::ContentControl(_) => {}
        }
    }
}

fn add_text_to_first_paragraph(control: &mut CT_Sdt, value: &str) -> bool {
    for content in &mut control.content {
        match content {
            SdtContent::Paragraph(paragraph) => {
                paragraph.runs.insert(0, CT_R::new(value));
                return true;
            }
            SdtContent::Table(table) => {
                if add_text_to_table(table, value) {
                    return true;
                }
            }
            SdtContent::Row(row) => {
                if add_text_to_row(row, value) {
                    return true;
                }
            }
            SdtContent::Cell(cell) => {
                if add_text_to_cell(cell, value) {
                    return true;
                }
            }
            SdtContent::ContentControl(_) => {}
            _ => {}
        }
    }
    false
}

fn add_text_to_table(table: &mut CT_Tbl, value: &str) -> bool {
    for row in &mut table.rows {
        if add_text_to_row(row, value) {
            return true;
        }
    }
    false
}

fn add_text_to_row(row: &mut CT_Row, value: &str) -> bool {
    for cell in &mut row.cells {
        if add_text_to_cell(cell, value) {
            return true;
        }
    }
    false
}

fn add_text_to_cell(cell: &mut CT_Tc, value: &str) -> bool {
    for content in &mut cell.content {
        match content {
            CellContent::Paragraph(paragraph) => {
                paragraph.runs.insert(0, CT_R::new(value));
                return true;
            }
            CellContent::Table(table) => {
                if add_text_to_table(table, value) {
                    return true;
                }
            }
            CellContent::ContentControl(_) => {}
        }
    }
    false
}
