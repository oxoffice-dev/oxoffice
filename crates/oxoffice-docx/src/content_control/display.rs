//! A control's display text in the typed model: reading it as lines, and replacing it with a value, prompt properties and line breaks included.

use super::*;

use oxoffice_docx_oxml::revision::raw_changes_content;

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
                replace_one_control(control, replacements, &mut index, false)?
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

/// Give the control its value, if it takes one, then the controls nested in
/// it theirs. `inline` says whether the control sits among runs.
fn replace_one_control(
    control: &mut CT_Sdt,
    replacements: &ControlReplacements,
    index: &mut usize,
    inline: bool,
) -> Result<()> {
    if let Some(value) = replacements.values.get(index) {
        let mut current = String::new();
        collect_sdt_text(control, &mut current);
        // An untracked edit: the value replaces the content with its tracked
        // changes, even when it shows the same text.
        let dropped = drop_content_changes(control)?;
        if current != value.display || value.checkbox.is_some() || dropped > 0 {
            replace_display_text(
                control,
                &value.display,
                value.checkbox.as_ref().map(|(_, font)| font.as_str()),
                &replacements.placeholder_styles,
                inline,
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
    replace_nested_controls(control, replacements, index, inline)
}

/// A control nested directly in `control` sits among runs when `control`
/// does, and among blocks, rows or cells as its siblings do otherwise.
fn replace_nested_controls(
    control: &mut CT_Sdt,
    replacements: &ControlReplacements,
    index: &mut usize,
    inline: bool,
) -> Result<()> {
    for content in &mut control.content {
        match content {
            SdtContent::Paragraph(paragraph) => {
                replace_paragraph_controls(paragraph, replacements, index)?
            }
            SdtContent::Table(table) => replace_table_controls(table, replacements, index)?,
            SdtContent::Row(row) => replace_row_controls(row, replacements, index)?,
            SdtContent::Cell(cell) => replace_cell_controls(cell, replacements, index)?,
            SdtContent::ContentControl(nested) => {
                replace_one_control(nested, replacements, index, inline)?
            }
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
            replace_one_control(control, replacements, index, true)?;
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
            replace_one_control(control, replacements, index, false)?;
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
            replace_one_control(control, replacements, index, false)?;
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
                replace_one_control(control, replacements, index, false)?
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
    inline: bool,
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
        if inline {
            // An inline control without a run, as one that held only tracked
            // changes is now, takes one at the end of its content, as the
            // form fill gives it one.
            control.content.push(SdtContent::Run(CT_R::new(value)));
        } else if !add_text_to_first_paragraph(control, value) {
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

/// Drop the tracked insertions, deletions and moves of the content a value
/// replaces, with everything they hold: those among the control's own
/// children and those of its paragraphs, in its tables, rows and cells too,
/// as `replace_sdt_display` walks them, with the insertion, deletion or move
/// of a paragraph's mark. A value is an untracked edit: the control then
/// holds it and no pending change. A proofing mark paired with one in a
/// dropped change goes too. The changes of a nested control and of
/// properties, the control's own included, stay. Returns how many went;
/// when one is refused (see [`check_dropped_change`]), the call fails.
pub(super) fn drop_content_changes(control: &mut CT_Sdt) -> Result<usize> {
    let mut marks = Vec::new();
    let mut gone = Vec::new();
    for (index, child) in control.content.iter().enumerate() {
        if let SdtContent::RawXml(raw) = child {
            sort_raw_child(raw, index, &mut marks, &mut gone)?;
        }
    }
    gone.extend(orphaned_partners(&marks)?);
    let mut dropped = 0;
    let mut paragraphs = Vec::new();
    content_paragraphs(&mut control.content, &mut paragraphs);
    for paragraph in paragraphs {
        dropped += drop_paragraph_changes(paragraph)?;
    }
    Ok(dropped + control.remove_raw_children(&gone))
}

/// Drop the tracked insertions, deletions and moves of one paragraph a value
/// rewrites, with the proofing marks paired with theirs, then those inside
/// its hyperlinks and those of its mark, as [`drop_content_changes`] does.
fn drop_paragraph_changes(paragraph: &mut CT_P) -> Result<usize> {
    // The preserved children in document order: by run boundary, then as read.
    let mut order = (0..paragraph.extra_xml.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| paragraph.extra_xml[*index].0);
    let mut marks = Vec::new();
    let mut gone = Vec::new();
    for index in order {
        sort_raw_child(&paragraph.extra_xml[index].1, index, &mut marks, &mut gone)?;
    }
    gone.extend(orphaned_partners(&marks)?);
    let dropped = paragraph.remove_raw_children(&gone);
    // Those left are inside hyperlinks: their proofing marks pair among
    // themselves.
    for (_, _, revision) in &paragraph.revisions {
        if revision.kind().changes_content() {
            let marks = check_dropped_change(revision.retained_xml())?;
            orphaned_partners(
                &marks
                    .into_iter()
                    .map(|mark| (mark, None))
                    .collect::<Vec<_>>(),
            )?;
        }
    }
    Ok(dropped + paragraph.remove_content_revisions())
}

/// Sort the preserved child `raw` at `index`: a change goes, after its check,
/// with the proofing marks in it; a proofing mark stays, unless its partner
/// is in a change.
fn sort_raw_child(
    raw: &[u8],
    index: usize,
    marks: &mut Vec<(ProofingMark, Option<usize>)>,
    gone: &mut Vec<usize>,
) -> Result<()> {
    if raw_changes_content(raw) {
        marks.extend(
            check_dropped_change(raw)?
                .into_iter()
                .map(|mark| (mark, None)),
        );
        gone.push(index);
    } else if let Some(mark) = proofing_mark(raw) {
        marks.push((mark, Some(index)));
    }
    Ok(())
}

/// The paragraphs among `content` whose runs a value rewrites, at any depth
/// of its tables, rows and cells, without those of nested controls.
fn content_paragraphs<'a>(content: &'a mut [SdtContent], paragraphs: &mut Vec<&'a mut CT_P>) {
    for child in content {
        match child {
            SdtContent::Paragraph(paragraph) => paragraphs.push(paragraph),
            SdtContent::Table(table) => table_paragraphs(table, paragraphs),
            SdtContent::Row(row) => row_paragraphs(row, paragraphs),
            SdtContent::Cell(cell) => cell_paragraphs(cell, paragraphs),
            SdtContent::Run(_) | SdtContent::ContentControl(_) | SdtContent::RawXml(_) => {}
        }
    }
}

fn table_paragraphs<'a>(table: &'a mut CT_Tbl, paragraphs: &mut Vec<&'a mut CT_P>) {
    for row in &mut table.rows {
        row_paragraphs(row, paragraphs);
    }
}

fn row_paragraphs<'a>(row: &'a mut CT_Row, paragraphs: &mut Vec<&'a mut CT_P>) {
    for cell in &mut row.cells {
        cell_paragraphs(cell, paragraphs);
    }
}

fn cell_paragraphs<'a>(cell: &'a mut CT_Tc, paragraphs: &mut Vec<&'a mut CT_P>) {
    for content in &mut cell.content {
        match content {
            CellContent::Paragraph(paragraph) => paragraphs.push(paragraph),
            CellContent::Table(table) => table_paragraphs(table, paragraphs),
            CellContent::ContentControl(_) => {}
        }
    }
}

/// A `w:proofErr`, the mark Word writes at each end of the text its
/// spelling or grammar check flagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProofingMark {
    grammar: bool,
    start: bool,
}

/// The proofing mark `raw` is, by its root element.
pub(super) fn proofing_mark(raw: &[u8]) -> Option<ProofingMark> {
    let mut reader = Reader::from_reader(raw);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                return (element.local_name().as_ref() == b"proofErr")
                    .then(|| proofing_type(&element))
                    .flatten();
            }
            Ok(Event::Eof) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

fn proofing_type(element: &BytesStart<'_>) -> Option<ProofingMark> {
    let kind = local_attribute(element, b"type");
    let (grammar, start) = match kind.as_slice() {
        b"spellStart" => (false, true),
        b"spellEnd" => (false, false),
        b"gramStart" => (true, true),
        b"gramEnd" => (true, false),
        _ => return None,
    };
    Some(ProofingMark { grammar, start })
}

/// The value of the attribute named `local` under any prefix, or nothing,
/// as a preserved subtree does not always declare its prefixes.
fn local_attribute(element: &BytesStart<'_>, local: &[u8]) -> Vec<u8> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == local)
        .and_then(|attribute| {
            oxoffice_core::xml::attribute_text(&attribute)
                .ok()
                .map(|value| value.into_owned().into_bytes())
        })
        .unwrap_or_default()
}

/// Pair the proofing marks of one parent's children in document order, each
/// with the index of the child that holds it when it stays, or none when it
/// is in a dropped change, and return the indices of those that stay paired
/// with one that goes: a proofing mark means nothing once its text is
/// replaced. One in a dropped change whose partner is not among them
/// refuses the value.
pub(super) fn orphaned_partners(marks: &[(ProofingMark, Option<usize>)]) -> Result<Vec<usize>> {
    let mut open: [Vec<Option<usize>>; 2] = Default::default();
    let mut partners = Vec::new();
    for (mark, kept) in marks {
        let starts = &mut open[usize::from(mark.grammar)];
        if mark.start {
            starts.push(*kept);
            continue;
        }
        match (starts.pop(), kept) {
            (Some(Some(start)), None) => partners.push(start),
            (Some(None), Some(end)) => partners.push(*end),
            (None, None) => return refused_proofing(*mark),
            _ => {}
        }
    }
    for (grammar, starts) in open.iter().enumerate() {
        if starts.contains(&None) {
            return refused_proofing(ProofingMark {
                grammar: grammar == 1,
                start: true,
            });
        }
    }
    Ok(partners)
}

fn refused_proofing<T>(mark: ProofingMark) -> Result<T> {
    let kind = if mark.grammar { "grammar" } else { "spelling" };
    refused(format!("one end of a {kind} mark"))
}

/// A refused value: the dropped change held `what`.
fn refused<T>(what: String) -> Result<T> {
    Err(Error::Other(format!(
        "the content control holds a tracked change with {what}, which a value would drop; \
         accept or reject it first"
    )))
}

/// Check the tracked change `raw` that a value drops with the content it
/// replaces, and return the proofing marks Word reads in it, those of its
/// deleted and moved-away text left out. It is refused when it holds
/// something a partner outside it needs: a comment or note mark, whose
/// comment or note would be left without it; a content control; one end of
/// a bookmark, a permission or a tracked range whose other end is outside
/// it; part of a complex field whose other parts are. The value is then
/// refused and the document left as it was, as `Paragraph::set_text`
/// refuses part of a range.
pub(super) fn check_dropped_change(raw: &[u8]) -> Result<Vec<ProofingMark>> {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    // Open complex fields, open deleted or moved-away text, the range
    // markers by family, start or end and id, and the proofing marks.
    let mut fields = 0usize;
    let mut hidden = 0usize;
    let mut ranges: Vec<(String, bool, Vec<u8>)> = Vec::new();
    let mut proofing = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("tracked change XML is invalid: {error}")))?;
        let element = match &event {
            Event::Start(element) | Event::Empty(element) => element,
            Event::End(element) => {
                if matches!(element.local_name().as_ref(), b"del" | b"moveFrom") {
                    hidden = hidden.saturating_sub(1);
                }
                buffer.clear();
                continue;
            }
            Event::Eof => break,
            _ => {
                buffer.clear();
                continue;
            }
        };
        let id = || String::from_utf8_lossy(&local_attribute(element, b"id")).into_owned();
        let local = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
        match local.as_str() {
            "commentRangeStart" | "commentRangeEnd" | "commentReference" => {
                return refused(format!("a mark of comment {}", id()));
            }
            "footnoteReference" | "endnoteReference" => {
                let kind = local.trim_end_matches("Reference");
                return refused(format!("the mark of {kind} {}", id()));
            }
            "sdt" => return refused("a content control".to_owned()),
            "del" | "moveFrom" if matches!(event, Event::Start(_)) => hidden += 1,
            "proofErr" if hidden == 0 => proofing.extend(proofing_type(element)),
            "fldChar" => match local_attribute(element, b"fldCharType").as_slice() {
                b"begin" => fields += 1,
                b"separate" if fields == 0 => {
                    return refused("part of a complex field".to_owned());
                }
                b"end" => {
                    if fields == 0 {
                        return refused("part of a complex field".to_owned());
                    }
                    fields -= 1;
                }
                _ => {}
            },
            "instrText" | "delInstrText" if fields == 0 => {
                return refused("part of a complex field".to_owned());
            }
            _ => {
                if let Some((family, start)) = range_marker(&local) {
                    ranges.push((family.to_owned(), start, local_attribute(element, b"id")));
                }
            }
        }
        buffer.clear();
    }
    if fields > 0 {
        return refused("part of a complex field".to_owned());
    }
    for (family, start, id) in &ranges {
        let paired = ranges.iter().any(|(other, other_start, other_id)| {
            other == family && other_start != start && other_id == id
        });
        if !paired {
            let id = String::from_utf8_lossy(id);
            return refused(format!("one end of {family} {id}"));
        }
    }
    Ok(proofing)
}

/// The family of a range marker element and whether it starts the range:
/// bookmarks, permissions and the tracked move and custom XML ranges.
fn range_marker(local: &str) -> Option<(&str, bool)> {
    const FAMILIES: [&str; 8] = [
        "bookmark",
        "perm",
        "moveFromRange",
        "moveToRange",
        "customXmlInsRange",
        "customXmlDelRange",
        "customXmlMoveFromRange",
        "customXmlMoveToRange",
    ];
    FAMILIES
        .into_iter()
        .find_map(|family| match local.strip_prefix(family)? {
            "Start" => Some((family, true)),
            "End" => Some((family, false)),
            _ => None,
        })
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
