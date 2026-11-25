//! A walk of what the writer writes for a body block, without writing it.
//!
//! The walk follows the writer branch by branch: content the writer writes
//! from stored bytes (verbatim or with rebound prefixes) is handed to
//! [`WrittenVisitor::raw`] and its typed projection is not walked; content
//! the writer writes from the typed model is handed to the typed callbacks.
//! Records that only hold an owner's own attributes are skipped.
//!
//! Two owners are written from bytes the model does not store: an equation
//! (re-serialized alone, it is small), and a paragraph or an inline content
//! control holding a field the writer rewrites (a changed field, a field
//! detached from the run it shared with text): the walk then serializes that
//! owner alone and hands its bytes to `raw`. An unchanged field is written
//! from its source bytes, which are handed as they are.

use super::*;
use crate::borders::CT_BorderEdge;
use crate::content_control::WrittenSdtPart;
use crate::document::{BodyContent, CT_Document, CT_SectPr};
use crate::table::{CT_TblBorders, CT_TblPr};

/// What the writer writes for a body block, as stored bytes and typed owners.
///
/// The bytes handed to [`Self::raw`] and the owners handed to the typed
/// callbacks cover everything the writer writes for the block. A typed owner
/// is handed only when the writer writes it from its typed fields: its typed
/// ids are written. Bytes may use any prefix for the WordprocessingML
/// namespace, an inherited one included.
#[doc(hidden)]
pub trait WrittenVisitor {
    /// Bytes the writer writes verbatim (or with rebound prefixes).
    fn raw(&mut self, _xml: &[u8]) {}
    /// A paragraph written from its typed model: its `comment_ranges` are written.
    fn paragraph(&mut self, _paragraph: &CT_P) {}
    /// Paragraph properties written from their typed fields (`style_id`).
    fn paragraph_properties(&mut self, _properties: &CT_PPr) {}
    /// A run written from its typed model: its note and comment references are written.
    fn run(&mut self, _run: &CT_R) {}
    /// Run properties written from their typed fields (`style_id`), the
    /// paragraph mark's and a ruby run's among them.
    fn run_properties(&mut self, _properties: &CT_RPr) {}
    /// Table properties written from their typed fields (`style_id`).
    fn table_properties(&mut self, _properties: &CT_TblPr) {}
}

impl BodyContent {
    /// Hand `visitor` what the writer writes for this block.
    #[doc(hidden)]
    pub fn visit_written(&self, visitor: &mut dyn WrittenVisitor) {
        match self {
            BodyContent::Paragraph(paragraph) => visit_paragraph(paragraph, visitor),
            BodyContent::Table(table) => visit_table(table, visitor),
            BodyContent::ContentControl(control) => visit_control(control, visitor),
            BodyContent::RawXml(raw) => visitor.raw(raw),
        }
    }
}

impl CT_SectPr {
    /// Hand `visitor` what the writer writes for these section properties.
    #[doc(hidden)]
    pub fn visit_written(&self, visitor: &mut dyn WrittenVisitor) {
        for (raw, position) in self.extra_xml.iter().zip(&self.extra_xml_positions) {
            if !Self::raw_position_is_root_attributes(position, raw) {
                visitor.raw(raw);
            }
        }
        // Records past the positions are written by no branch of the writer.
        if let Some(change) = &self.change {
            visitor.raw(change.retained_xml());
        }
        if let Some(raw) = self
            .page_number
            .as_ref()
            .and_then(|page| page.raw_xml.as_deref())
        {
            visitor.raw(raw);
        }
        if let Some(borders) = &self.page_borders {
            for raw in &borders.extra_xml {
                visitor.raw(raw);
            }
            for edge in [&borders.top, &borders.left, &borders.bottom, &borders.right] {
                visit_border(edge.as_ref(), visitor);
            }
        }
        for notes in [&self.footnote_pr, &self.endnote_pr].into_iter().flatten() {
            for (_, raw) in &notes.extra_xml {
                visitor.raw(raw);
            }
        }
    }
}

impl CT_Document {
    /// Hand `visitor` what the writer writes outside the body's children:
    /// the background and the body's final section properties.
    #[doc(hidden)]
    pub fn visit_written_outside_body(&self, visitor: &mut dyn WrittenVisitor) {
        if let Some(background) = &self.background_xml {
            visitor.raw(background);
        }
        for raw in &self.background_extra_xml {
            visitor.raw(raw);
        }
        if let Some(section) = &self.body.sect_pr {
            section.visit_written(visitor);
        }
    }
}

fn visit_border(edge: Option<&CT_BorderEdge>, visitor: &mut dyn WrittenVisitor) {
    if let Some(raw) = edge.and_then(|edge| edge.source_xml.as_deref()) {
        visitor.raw(raw);
    }
}

fn visit_table_borders(borders: Option<&CT_TblBorders>, visitor: &mut dyn WrittenVisitor) {
    let Some(borders) = borders else {
        return;
    };
    for raw in &borders.extra_xml {
        visitor.raw(raw);
    }
    for edge in [
        &borders.top,
        &borders.bottom,
        &borders.left,
        &borders.right,
        &borders.inside_h,
        &borders.inside_v,
    ] {
        visit_border(edge.as_ref(), visitor);
    }
}

/// The source bytes the writer writes for `field` as they are, or `None`
/// when it rewrites them or builds the field anew.
///
/// Mirrors `write_field_with_result_properties`: a field read from a run it
/// shared with text writes a detached part of its source, an ordered result
/// that needs normalizing is rewritten first, and only an unchanged parsed
/// field then writes its source.
fn field_written_source(field: &Field) -> Option<&[u8]> {
    let FieldSource::Parsed { raw_xml, .. } = field.source.as_ref() else {
        return None;
    };
    (field.span.is_none() && field.is_unchanged() && field.ordered_result_is_settled())
        .then_some(raw_xml.as_slice())
}

/// Whether every field the paragraph writer reaches is written from its
/// source bytes as they are, by the decisions of `write_xml_content_mapped_in`.
fn paragraph_fields_written_from_source(paragraph: &CT_P) -> bool {
    let holds_field = |run: &CT_R| {
        run.content
            .iter()
            .any(|content| matches!(content, RunContent::Field(_)))
    };
    if !paragraph.runs.iter().any(holds_field) {
        return true;
    }
    let mut index = 0;
    let mut written_owner = None;
    while let Some(run) = paragraph.runs.get(index) {
        if let Some((end, _, true, _)) = paragraph.field_span_at(index) {
            index = end;
            written_owner = None;
            continue;
        }
        match run.content.as_slice() {
            [RunContent::Field(field)] => {
                if field.span.is_some() {
                    return false;
                }
                let owner = field.source_owner_id();
                if owner.is_none() || owner != written_owner {
                    written_owner = owner;
                    if field_written_source(field).is_none() {
                        return false;
                    }
                }
            }
            _ if holds_field(run) => return false,
            _ => written_owner = None,
        }
        index += 1;
    }
    true
}

/// Serialize `write` alone and hand its bytes to `visitor`; `false` when the
/// writer fails.
fn visit_serialized(
    visitor: &mut dyn WrittenVisitor,
    write: impl FnOnce(&mut Writer<Vec<u8>>) -> Result<()>,
) -> bool {
    let mut writer = Writer::new(Vec::new());
    if write(&mut writer).is_err() {
        return false;
    }
    visitor.raw(&writer.into_inner());
    true
}

fn visit_paragraph(paragraph: &CT_P, visitor: &mut dyn WrittenVisitor) {
    // A field the writer rewrites has no stored bytes: write the paragraph.
    // A writer failure falls through to the walk below, which then hands the
    // field's source bytes (the save fails on the same writer anyway).
    if !paragraph_fields_written_from_source(paragraph)
        && visit_serialized(visitor, |writer| paragraph.to_xml(writer))
    {
        return;
    }
    visitor.paragraph(paragraph);
    if let Some(properties) = &paragraph.properties {
        visit_paragraph_properties(properties, visitor);
    }

    let mut current_hyperlink: Option<usize> = None;
    let mut current_ruby: Option<usize> = None;
    let mut written_field_owner = None;
    let mut written_span_end = 0;
    for (run_index, run) in paragraph.runs.iter().enumerate() {
        if run_index < written_span_end {
            continue;
        }
        // The writer maps each run to the last hyperlink that covers it.
        let in_hyperlink = paragraph.hyperlinks.iter().rposition(|hyperlink| {
            hyperlink.run_start <= run_index && run_index < hyperlink.run_end
        });

        if let Some(ruby_index) = current_ruby
            && paragraph.rubies[ruby_index].base_end == run_index
        {
            visit_ruby_end(&paragraph.rubies[ruby_index], visitor);
            current_ruby = None;
        }
        if let Some(hyperlink_index) = current_hyperlink
            && current_hyperlink != in_hyperlink
        {
            visit_hyperlink_boundary(paragraph, hyperlink_index, run_index, visitor);
            current_hyperlink = None;
        }
        visit_paragraph_boundary(paragraph, run_index, visitor);
        visit_empty_hyperlinks(paragraph, run_index, visitor);
        if in_hyperlink.is_some() && current_hyperlink != in_hyperlink {
            current_hyperlink = in_hyperlink;
        }
        if let Some(hyperlink_index) = current_hyperlink {
            visit_hyperlink_boundary(paragraph, hyperlink_index, run_index, visitor);
        }

        if let Some((span_end, raw, true, _)) = paragraph.field_span_at(run_index) {
            visitor.raw(raw);
            written_field_owner = None;
            written_span_end = span_end;
            continue;
        }
        if let [RunContent::Field(field)] = run.content.as_slice() {
            if field.span.is_some() {
                visit_field_source(field, visitor);
                written_field_owner = None;
                continue;
            }
            let owner = field.source_owner_id();
            if owner.is_some() && owner == written_field_owner {
                continue;
            }
            written_field_owner = owner;
            visit_field_source(field, visitor);
            continue;
        }
        if run
            .content
            .iter()
            .any(|content| matches!(content, RunContent::Field(_)))
        {
            // Reached only when the paragraph's writer failed: hand what
            // the run keeps, its typed parts and its fields' sources.
            written_field_owner = None;
            visit_run(run, visitor);
            for content in &run.content {
                if let RunContent::Field(field) = content {
                    visit_field_source(field, visitor);
                }
            }
            continue;
        }
        written_field_owner = None;

        if current_ruby.is_none()
            && let Some(ruby_index) = paragraph
                .rubies
                .iter()
                .position(|ruby| ruby.base_start == run_index && ruby.base_end > run_index)
        {
            visit_ruby_start(&paragraph.rubies[ruby_index], visitor);
            current_ruby = Some(ruby_index);
        }
        visit_run(run, visitor);
    }

    if let Some(ruby_index) = current_ruby {
        visit_ruby_end(&paragraph.rubies[ruby_index], visitor);
    }
    if let Some(hyperlink_index) = current_hyperlink {
        visit_hyperlink_boundary(paragraph, hyperlink_index, paragraph.runs.len(), visitor);
    }
    visit_paragraph_boundary(paragraph, paragraph.runs.len(), visitor);
    visit_empty_hyperlinks(paragraph, paragraph.runs.len(), visitor);
}

/// A field's stored source, when it has one. Exact for an unchanged field;
/// otherwise reached only when the writer failed.
fn visit_field_source(field: &Field, visitor: &mut dyn WrittenVisitor) {
    if let FieldSource::Parsed { raw_xml, .. } = field.source.as_ref() {
        visitor.raw(raw_xml);
    }
}

/// Mirrors `write_paragraph_boundary`.
fn visit_paragraph_boundary(paragraph: &CT_P, run_index: usize, visitor: &mut dyn WrittenVisitor) {
    if paragraph.extra_xml.is_empty()
        && paragraph.content_controls.is_empty()
        && paragraph.comment_ranges.is_empty()
    {
        return;
    }
    let extra_count = paragraph
        .extra_xml
        .iter()
        .filter(|(position, _)| *position == run_index)
        .count();
    let mut extras = paragraph
        .extra_xml
        .iter()
        .filter(|(position, _)| *position == run_index)
        .map(|(_, raw)| raw);
    for raw_index in 0..=extra_count {
        let marker_count = paragraph
            .comment_ranges
            .iter()
            .filter(|marker| {
                marker.run_index() == run_index && marker.raw_before().min(extra_count) == raw_index
            })
            .count();
        for marker_index in 0..=marker_count {
            for (_, _, _, control) in
                paragraph
                    .content_controls
                    .iter()
                    .filter(|(at, raw_before, markers_before, _)| {
                        *at == run_index
                            && (*raw_before).min(extra_count) == raw_index
                            && (*markers_before).min(marker_count) == marker_index
                    })
            {
                visit_control(control, visitor);
            }
            // The comment range markers are written from the typed paragraph.
        }
        let Some(raw) = extras.next() else {
            continue;
        };
        if let Some((_, _, equation)) = paragraph
            .equations
            .iter()
            .find(|(at, slot, _)| *at == run_index && *slot == raw_index)
        {
            if let Ok(xml) = equation.to_xml() {
                visitor.raw(&xml);
            }
        } else if let Some((_, _, revision)) = paragraph.revisions.iter().find(|(at, slot, _)| {
            *at == run_index && hyperlink_revision_index(*slot).is_none() && *slot == raw_index
        }) {
            visitor.raw(revision.retained_xml());
        } else if let Some(hyperlink_index) = paragraph.hyperlinks.iter().position(|hyperlink| {
            hyperlink.run_start == run_index
                && hyperlink.run_end == run_index
                && hyperlink.preserved_raw_before == Some(raw_index)
        }) {
            visit_hyperlink_boundary(paragraph, hyperlink_index, run_index, visitor);
        } else {
            visitor.raw(raw);
        }
    }
}

/// Mirrors `write_empty_hyperlinks`.
fn visit_empty_hyperlinks(paragraph: &CT_P, run_index: usize, visitor: &mut dyn WrittenVisitor) {
    for (hyperlink_index, _) in paragraph
        .hyperlinks
        .iter()
        .enumerate()
        .filter(|(_, hyperlink)| {
            hyperlink.run_start == run_index
                && hyperlink.run_end == run_index
                && hyperlink.preserved_raw_before.is_none()
        })
    {
        visit_hyperlink_boundary(paragraph, hyperlink_index, run_index, visitor);
    }
}

/// Mirrors `write_hyperlink_boundary`: the hyperlink's raw children and its
/// revisions at this run boundary.
fn visit_hyperlink_boundary(
    paragraph: &CT_P,
    hyperlink_index: usize,
    run_index: usize,
    visitor: &mut dyn WrittenVisitor,
) {
    let hyperlink = &paragraph.hyperlinks[hyperlink_index];
    let relative = run_index.saturating_sub(hyperlink.run_start);
    for (_, _, raw) in hyperlink
        .extra_xml
        .iter()
        .filter(|(boundary, _, _)| *boundary == relative)
    {
        visitor.raw(raw);
    }
    for (_, _, revision) in paragraph.revisions.iter().filter(|(at, slot, _)| {
        *at == run_index && hyperlink_revision_index(*slot) == Some(hyperlink_index)
    }) {
        visitor.raw(revision.retained_xml());
    }
}

/// Mirrors `CT_Ruby::write_start`: the holding run's properties, the ruby
/// properties and the phonetic runs.
fn visit_ruby_start(ruby: &crate::ruby::CT_Ruby, visitor: &mut dyn WrittenVisitor) {
    if let Some(properties) = ruby.run.as_ref().and_then(|run| run.properties.as_ref()) {
        visit_run_properties(properties, visitor);
    }
    if let Some(properties) = &ruby.properties {
        for raw in &properties.raw_xml {
            visitor.raw(raw);
        }
    }
    for run in &ruby.ruby_text {
        visit_run(run, visitor);
    }
}

/// Mirrors `CT_Ruby::write_end`: the ruby's unmodelled children.
fn visit_ruby_end(ruby: &crate::ruby::CT_Ruby, visitor: &mut dyn WrittenVisitor) {
    for raw in &ruby.raw_xml {
        visitor.raw(raw);
    }
}

/// Mirrors `CT_PPr::write_xml_role_in` and its positioned raw children.
fn visit_paragraph_properties(properties: &CT_PPr, visitor: &mut dyn WrittenVisitor) {
    visitor.paragraph_properties(properties);
    if let Some(borders) = &properties.borders {
        for edge in [
            &borders.top,
            &borders.bottom,
            &borders.left,
            &borders.right,
            &borders.between,
            &borders.bar,
        ] {
            visit_border(edge.as_ref(), visitor);
        }
    }
    for (_, raw, _) in [&properties.num_ilvl_raw, &properties.num_id_raw]
        .into_iter()
        .flatten()
    {
        visitor.raw(raw);
    }
    for (_, _, raw) in &properties.num_pr_extra_xml {
        visitor.raw(raw);
    }
    for raw in &properties.numbering_revision_xml {
        visitor.raw(raw);
    }
    if let Some(revision) = &properties.numbering_revision {
        visitor.raw(revision.retained_xml());
    }
    if let Some(run_properties) = &properties.rpr {
        visit_run_properties(run_properties, visitor);
    }
    if let Some(section) = &properties.sect_pr {
        section.visit_written(visitor);
    }
    for raw in &properties.revision_xml {
        visitor.raw(raw);
    }
    if let Some(change) = &properties.change {
        visitor.raw(change.retained_xml());
    }
}

/// Mirrors `CT_RPr::write_xml_role_core` and its positioned raw children.
fn visit_run_properties(properties: &CT_RPr, visitor: &mut dyn WrittenVisitor) {
    visitor.run_properties(properties);
    visit_border(properties.border.as_deref(), visitor);
    for revision in &properties.revision_markers {
        visitor.raw(revision.retained_xml());
    }
    for raw in &properties.revision_xml {
        visitor.raw(raw);
    }
    if let Some(change) = &properties.change {
        visitor.raw(change.retained_xml());
    }
}

/// Mirrors `CT_R::write_xml_role`.
fn visit_run(run: &CT_R, visitor: &mut dyn WrittenVisitor) {
    visitor.run(run);
    if let Some(properties) = &run.properties {
        visit_run_properties(properties, visitor);
    }
    for content in &run.content {
        // A field in a run the paragraph does not write as a field is
        // written by no branch; `alt_drawings` are never written.
        if let RunContent::Drawing(drawing) = content {
            // A typed drawing writes a picture or a chart, never shape text.
            if let Some(raw) = drawing.inline.as_ref().and_then(|d| d.raw_xml.as_deref()) {
                visitor.raw(raw);
            }
            if let Some(raw) = drawing.anchor.as_ref().and_then(|d| d.raw_xml.as_deref()) {
                visitor.raw(raw);
            }
        }
    }
    for (index, raw) in run.extra_xml.iter().enumerate() {
        let position = run.extra_xml_positions.get(index).copied();
        let root_attributes = match position {
            Some(position) => CT_R::raw_child_is_root_attributes(position),
            None => is_root_attribute_record(raw),
        };
        if root_attributes {
            continue;
        }
        if position.is_some_and(|position| position & RAW_COMMENT_REFERENCE_FLAG != 0) {
            // A retained comment reference (one with extra attributes) is
            // written in place of the typed reference it matches, and only
            // then: the run must hold a typed reference with its id.
            let id = retained_comment_reference_id(raw);
            if id.is_some()
                && run.content.iter().any(|content| {
                    matches!(content, RunContent::CommentReference { id: typed, .. } if Some(*typed) == id)
                })
            {
                visitor.raw(raw);
            }
            continue;
        }
        visitor.raw(raw);
    }
}

/// Mirrors `CT_Tbl::write_xml_role_in`.
fn visit_table(table: &CT_Tbl, visitor: &mut dyn WrittenVisitor) {
    if let Some(properties) = &table.properties {
        visit_table_properties(properties, visitor);
    }
    if let Some(grid) = &table.grid {
        for raw in &grid.extra_xml {
            visitor.raw(raw);
        }
        if let Some(raw) = &grid.grid_change_xml {
            visitor.raw(raw);
        }
    }
    for (index, row) in table.rows.iter().enumerate() {
        visit_table_boundary(&table.extra_xml, &table.content_controls, index, visitor);
        visit_row(row, visitor);
    }
    visit_table_boundary(
        &table.extra_xml,
        &table.content_controls,
        table.rows.len(),
        visitor,
    );
}

/// Mirrors `write_boundary` in the table writer: controls placed exactly
/// before each raw child, then that child.
fn visit_table_boundary(
    extra_xml: &[(usize, Vec<u8>)],
    controls: &[(usize, usize, CT_Sdt)],
    position: usize,
    visitor: &mut dyn WrittenVisitor,
) {
    let mut extras = extra_xml
        .iter()
        .filter(|(at, _)| *at == position)
        .map(|(_, raw)| raw);
    let extra_count = extras.clone().count();
    for raw_before in 0..=extra_count {
        for (_, _, control) in controls
            .iter()
            .filter(|(at, before, _)| *at == position && *before == raw_before)
        {
            visit_control(control, visitor);
        }
        if let Some(raw) = extras.next() {
            visitor.raw(raw);
        }
    }
}

/// Mirrors `CT_TblPr::write_xml_role_in`.
fn visit_table_properties(properties: &CT_TblPr, visitor: &mut dyn WrittenVisitor) {
    visitor.table_properties(properties);
    for (_, raw) in &properties.extra_xml {
        visitor.raw(raw);
    }
    visit_table_borders(properties.borders.as_ref(), visitor);
    for raw in &properties.revision_xml {
        visitor.raw(raw);
    }
    if let Some(change) = &properties.change {
        visitor.raw(change.retained_xml());
    }
}

/// Mirrors `CT_Row::write_xml_role_in`.
fn visit_row(row: &CT_Row, visitor: &mut dyn WrittenVisitor) {
    if let Some(raw) = &row.table_property_exception {
        visitor.raw(raw);
    }
    if let Some(properties) = &row.properties {
        if let Some((_, raw)) = &properties.cnf_style_xml {
            visitor.raw(raw);
        }
        for (_, raw) in &properties.extra_xml {
            visitor.raw(raw);
        }
        for raw in &properties.revision_xml {
            visitor.raw(raw);
        }
        for revision in &properties.revision_markers {
            visitor.raw(revision.retained_xml());
        }
    }
    for (index, cell) in row.cells.iter().enumerate() {
        visit_table_boundary(&row.extra_xml, &row.content_controls, index, visitor);
        visit_cell(cell, visitor);
    }
    visit_table_boundary(
        &row.extra_xml,
        &row.content_controls,
        row.cells.len(),
        visitor,
    );
}

/// Mirrors `CT_Tc::write_xml_role_in`.
fn visit_cell(cell: &CT_Tc, visitor: &mut dyn WrittenVisitor) {
    if let Some(properties) = &cell.properties {
        if let Some((_, raw)) = &properties.cnf_style_xml {
            visitor.raw(raw);
        }
        for (_, raw) in &properties.extra_xml {
            visitor.raw(raw);
        }
        visit_table_borders(properties.borders.as_ref(), visitor);
    }
    for index in 0..=cell.content.len() {
        for (_, raw) in cell.extra_xml.iter().filter(|(at, _)| *at == index) {
            visitor.raw(raw);
        }
        match cell.content.get(index) {
            Some(CellContent::Paragraph(paragraph)) => visit_paragraph(paragraph, visitor),
            Some(CellContent::Table(table)) => visit_table(table, visitor),
            Some(CellContent::ContentControl(control)) => visit_control(control, visitor),
            None => {}
        }
    }
}

/// Mirrors `CT_Sdt::write_xml_role_in` and `write_content`.
fn visit_control(control: &CT_Sdt, visitor: &mut dyn WrittenVisitor) {
    // A direct run holding a field is matched to its stored bytes by the
    // field's writer: write the control alone.
    let field_run = control.content.iter().any(|child| {
        matches!(child, SdtContent::Run(run) if run.content.iter().any(|content| matches!(content, RunContent::Field(_))))
    });
    if field_run && visit_serialized(visitor, |writer| control.to_xml(writer)) {
        return;
    }
    control.for_each_written_part(|part| match part {
        WrittenSdtPart::Properties(properties) => {
            for raw in properties.written_raw_slots() {
                visitor.raw(raw);
            }
        }
        WrittenSdtPart::Raw(raw) => visitor.raw(raw),
        WrittenSdtPart::Content => visit_control_content(control, visitor),
    });
}

fn visit_control_content(control: &CT_Sdt, visitor: &mut dyn WrittenVisitor) {
    for (index, child) in control.content.iter().enumerate() {
        match child {
            SdtContent::Paragraph(paragraph) => visit_paragraph(paragraph, visitor),
            SdtContent::Table(table) => visit_table(table, visitor),
            SdtContent::Row(row) => visit_row(row, visitor),
            SdtContent::Cell(cell) => visit_cell(cell, visitor),
            SdtContent::Run(run) => {
                if let Some(raw) = control.written_inline_run_source(index, run) {
                    visitor.raw(raw);
                } else if !run.content.is_empty() || !run.extra_xml.is_empty() {
                    // `write_mixed_field_run` writes a run without a field as
                    // one segment, and nothing for an empty one.
                    visit_run(run, visitor);
                    for content in &run.content {
                        if let RunContent::Field(field) = content {
                            visit_field_source(field, visitor);
                        }
                    }
                }
            }
            SdtContent::ContentControl(nested) => visit_control(nested, visitor),
            SdtContent::RawXml(raw) => {
                match control.revisions.iter().find(|(at, _)| *at == index) {
                    Some((_, revision)) => visitor.raw(revision.retained_xml()),
                    None => visitor.raw(raw),
                }
            }
        }
    }
}
