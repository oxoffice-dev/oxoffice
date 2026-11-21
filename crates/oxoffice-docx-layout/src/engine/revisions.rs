//! Revision projection: runs seen through a revision view, tracked marks, change bars and the semantic projection of the layout input.

use super::*;

pub(super) fn project_paragraph_runs(para: &CT_P, view: RevisionView) -> Vec<ProjectedRun<'_>> {
    let mut projected = Vec::new();
    for boundary in 0..=para.runs.len() {
        let mut owners = para
            .content_controls
            .iter()
            .filter(|(at, _, _, _)| *at == boundary)
            .map(|(_, raw_before, _, control)| {
                (
                    RawOrder::Raw(*raw_before),
                    0u8,
                    ProjectedParagraphOwner::Control(control),
                )
            })
            .chain(
                para.revisions
                    .iter()
                    .filter(|(at, _, _)| *at == boundary)
                    .map(|(_, slot, revision)| {
                        (
                            paragraph_revision_raw_order(para, boundary, *slot),
                            1u8,
                            ProjectedParagraphOwner::Revision {
                                revision,
                                hyperlink_index: hyperlink_revision_index(*slot),
                            },
                        )
                    }),
            )
            .collect::<Vec<_>>();
        owners.sort_by_key(|(raw_order, kind, _)| (*raw_order, *kind));
        for (raw_order, _, owner) in owners {
            match owner {
                ProjectedParagraphOwner::Control(control) => {
                    project_control_runs(control, view, boundary, raw_order, &mut projected)
                }
                ProjectedParagraphOwner::Revision {
                    revision,
                    hyperlink_index,
                } => project_revision_runs(
                    revision,
                    view,
                    boundary,
                    raw_order,
                    hyperlink_index,
                    false,
                    false,
                    &mut projected,
                ),
            }
        }
        if let Some(run) = para.runs.get(boundary) {
            projected.push(ProjectedRun {
                control: None,
                run,
                boundary,
                raw_order: RawOrder::AfterRaw,
                ordinary_run_index: Some(boundary),
                hyperlink_index: None,
                force_underline: false,
                force_strike: false,
                revision: None,
            });
        }
    }
    projected
}

/// Whether the accepted view of `para` has nothing to lay out: no text, tab,
/// break, picture, field, symbol, special character or note reference, no
/// equation and no bookmark start.
///
/// A bookmark start keeps the paragraph, because a PAGEREF or REF field
/// reads the page, number and text of the paragraph that holds it, and
/// moving it onto the next paragraph, as accepting does, is not modeled.
/// The runs a paragraph draws in the accepted view, in order, each with the
/// innermost inline content control around it.
pub(crate) fn accepted_runs(para: &CT_P) -> Vec<(&CT_R, Option<&CT_Sdt>)> {
    project_paragraph_runs(para, RevisionView::Accepted)
        .into_iter()
        .map(|projected| (projected.run, projected.control))
        .collect()
}

pub(super) fn accepted_paragraph_is_blank(para: &CT_P) -> bool {
    para.accepted_text().is_empty()
        && para.equations.is_empty()
        && !para.bookmark_markers.iter().any(|marker| marker.is_start())
        && project_paragraph_runs(para, RevisionView::Accepted)
            .iter()
            .all(|projected| {
                projected.run.alt_drawings.is_empty()
                    && projected.run.content.iter().all(|content| match content {
                        RunContent::Text(text) => text.text.is_empty(),
                        RunContent::DeletedText(_) | RunContent::CommentReference { .. } => true,
                        RunContent::Tab
                        | RunContent::Break(_)
                        | RunContent::Drawing(_)
                        | RunContent::Field(_)
                        | RunContent::FootnoteRef { .. }
                        | RunContent::EndnoteRef { .. }
                        | RunContent::Symbol { .. }
                        | RunContent::SpecialCharacter(_) => false,
                    })
            })
}

enum ProjectedParagraphOwner<'a> {
    Control(&'a CT_Sdt),
    Revision {
        revision: &'a CT_Revision,
        hyperlink_index: Option<usize>,
    },
}

fn paragraph_revision_raw_order(para: &CT_P, boundary: usize, slot: usize) -> RawOrder {
    let Some(index) = hyperlink_revision_index(slot) else {
        return RawOrder::Raw(slot);
    };
    if let Some(raw_before) = para
        .hyperlinks
        .get(index)
        .and_then(|hyperlink| hyperlink.preserved_raw_before)
    {
        RawOrder::Raw(raw_before)
    } else if para
        .hyperlinks
        .get(index)
        .is_some_and(|hyperlink| boundary == hyperlink.run_end)
    {
        RawOrder::BeforeRaw
    } else {
        RawOrder::AfterRaw
    }
}

fn project_control_runs<'a>(
    control: &'a CT_Sdt,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    let first = projected.len();
    for content_boundary in 0..=control.content.len() {
        for (_, revision) in control
            .revisions()
            .iter()
            .filter(|(at, _)| *at == content_boundary)
        {
            project_revision_runs(
                revision, view, boundary, raw_order, None, false, false, projected,
            );
        }
        if let Some(content) = control.content.get(content_boundary) {
            match content {
                SdtContent::Run(run) => projected.push(ProjectedRun {
                    control: None,
                    run,
                    boundary,
                    raw_order,
                    ordinary_run_index: None,
                    hyperlink_index: None,
                    force_underline: false,
                    force_strike: false,
                    revision: None,
                }),
                SdtContent::ContentControl(control) => {
                    project_control_runs(control, view, boundary, raw_order, projected);
                }
                SdtContent::Paragraph(paragraph) => {
                    project_control_paragraph_runs(paragraph, view, boundary, raw_order, projected)
                }
                SdtContent::Table(table) => {
                    project_control_table_runs(table, view, boundary, raw_order, projected);
                }
                SdtContent::Row(row) => {
                    project_control_row_runs(row, view, boundary, raw_order, projected);
                }
                SdtContent::Cell(cell) => {
                    project_control_cell_runs(cell, view, boundary, raw_order, projected);
                }
                SdtContent::RawXml(_) => {}
            }
        }
    }
    // Runs of a nested control already name that innermost control.
    for run in &mut projected[first..] {
        run.control.get_or_insert(control);
    }
}

fn project_control_paragraph_runs<'a>(
    paragraph: &'a CT_P,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    for nested in project_paragraph_runs(paragraph, view) {
        projected.push(ProjectedRun {
            boundary,
            raw_order,
            ordinary_run_index: None,
            hyperlink_index: None,
            ..nested
        });
    }
}

fn project_control_table_runs<'a>(
    table: &'a CT_Tbl,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    for row_boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == row_boundary)
        {
            project_control_runs(control, view, boundary, raw_order, projected);
        }
        if let Some(row) = table.rows.get(row_boundary) {
            project_control_row_runs(row, view, boundary, raw_order, projected);
        }
    }
}

fn project_control_row_runs<'a>(
    row: &'a CT_Row,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    for cell_boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == cell_boundary)
        {
            project_control_runs(control, view, boundary, raw_order, projected);
        }
        if let Some(cell) = row.cells.get(cell_boundary) {
            project_control_cell_runs(cell, view, boundary, raw_order, projected);
        }
    }
}

fn project_control_cell_runs<'a>(
    cell: &'a CT_Tc,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => {
                project_control_paragraph_runs(paragraph, view, boundary, raw_order, projected)
            }
            CellContent::Table(table) => {
                project_control_table_runs(table, view, boundary, raw_order, projected);
            }
            CellContent::ContentControl(control) => {
                project_control_runs(control, view, boundary, raw_order, projected);
            }
        }
    }
}

fn project_revision_runs<'a>(
    revision: &'a CT_Revision,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    hyperlink_index: Option<usize>,
    inherited_underline: bool,
    inherited_strike: bool,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    let included = match view {
        RevisionView::Tracked => true,
        RevisionView::Accepted => matches!(
            revision.kind(),
            RevisionKind::Insertion | RevisionKind::MoveTo
        ),
    };
    if !included {
        return;
    }

    let force_underline = inherited_underline
        || (view == RevisionView::Tracked
            && matches!(
                revision.kind(),
                RevisionKind::Insertion | RevisionKind::MoveTo
            ));
    let force_strike = inherited_strike
        || (view == RevisionView::Tracked
            && matches!(
                revision.kind(),
                RevisionKind::Deletion | RevisionKind::MoveFrom
            ));
    let marked = (view == RevisionView::Tracked).then(|| revision.kind());
    if let Some(paragraph) = revision.content_paragraph() {
        for nested in project_paragraph_runs(paragraph, view) {
            projected.push(ProjectedRun {
                boundary,
                raw_order,
                ordinary_run_index: None,
                hyperlink_index,
                force_underline: force_underline || nested.force_underline,
                force_strike: force_strike || nested.force_strike,
                revision: nested.revision.or(marked),
                ..nested
            });
        }
        return;
    }
    let runs = match revision.content() {
        RevisionContent::Runs(runs) => runs.as_slice(),
        RevisionContent::Marker => &[],
        RevisionContent::PriorRunProperties(_)
        | RevisionContent::PriorParagraphProperties(_)
        | RevisionContent::PriorTableProperties(_)
        | RevisionContent::PriorSectionProperties(_) => return,
    };

    for run_boundary in 0..=runs.len() {
        for (_, nested) in revision
            .nested_revisions()
            .iter()
            .filter(|(at, _)| *at == run_boundary)
        {
            project_revision_runs(
                nested,
                view,
                boundary,
                raw_order,
                hyperlink_index,
                force_underline,
                force_strike,
                projected,
            );
        }
        if let Some(run) = runs.get(run_boundary) {
            projected.push(ProjectedRun {
                control: None,
                run,
                boundary,
                raw_order,
                ordinary_run_index: None,
                hyperlink_index,
                force_underline,
                force_strike,
                revision: marked,
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_equations_before_order(
    paragraph: &CT_P,
    boundary: usize,
    order: RawOrder,
    cursor: &mut usize,
    inline_items: &mut Vec<InlineItem>,
    fm: &mut FontManager,
    font_size: f64,
    color: Color,
    available_width: f64,
    math_properties: Option<&oxoffice_docx_oxml::math::MathProperties>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    while let Some((equation_boundary, raw_before, equation)) = paragraph.equations.get(*cursor) {
        if *equation_boundary > boundary
            || (*equation_boundary == boundary
                && !match order {
                    RawOrder::BeforeRaw => false,
                    RawOrder::Raw(limit) => *raw_before < limit,
                    RawOrder::AfterRaw => true,
                })
        {
            break;
        }
        let source_path =
            format!("paragraph/run-boundary/{equation_boundary}/raw-child/{raw_before}");
        let (measured, display) = crate::math::layout_officemath(
            equation,
            fm,
            font_size,
            color,
            available_width,
            math_properties,
            &source_path,
            diagnostics,
        )?;
        if display
            && !inline_items.is_empty()
            && !matches!(inline_items.last(), Some(InlineItem::LineBreak))
        {
            inline_items.push(InlineItem::LineBreak);
        }
        inline_items.push(InlineItem::Group {
            width: measured.width,
            height: measured.height(),
            baseline: Some(measured.ascent),
            group: measured.group,
        });
        if display {
            inline_items.push(InlineItem::LineBreak);
        }
        *cursor += 1;
    }
    Ok(())
}

pub(super) fn projected_paragraph_text(para: &CT_P, view: RevisionView) -> Result<String> {
    let mut text = String::new();
    for projected in project_paragraph_runs(para, view) {
        for content in &projected.run.content {
            if let RunContent::Field(field) = content {
                for stored in stored_note_cache_runs(field, None, view, None, false, None, false)? {
                    text.push_str(&stored.run.text());
                }
            } else {
                let run = CT_R {
                    content: vec![content.clone()],
                    ..CT_R::new("")
                };
                text.push_str(&run.text());
            }
        }
    }
    Ok(text)
}

pub(super) fn projected_content_char_starts(run: &CT_R) -> Vec<usize> {
    let mut starts = Vec::with_capacity(run.content.len());
    let mut char_offset = 0usize;
    for content in &run.content {
        starts.push(char_offset);
        char_offset += match content {
            RunContent::Text(text) | RunContent::DeletedText(text) => text.text.chars().count(),
            RunContent::Tab | RunContent::Break(_) => 1,
            RunContent::Field(field) => field
                .effective_projected_text()
                .map_or(0, |text| text.chars().count()),
            RunContent::SpecialCharacter(SpecialCharacter::CarriageReturn) => 1,
            RunContent::FootnoteRef { custom_mark, .. }
            | RunContent::EndnoteRef { custom_mark, .. } => {
                custom_mark.as_ref().map_or(0, |mark| mark.chars().count())
            }
            RunContent::Drawing(_)
            | RunContent::CommentReference { .. }
            | RunContent::Symbol { .. }
            | RunContent::SpecialCharacter(_) => 0,
        };
    }
    debug_assert_eq!(char_offset, run.text().chars().count());
    starts
}

fn paragraph_has_visible_revision(para: &CT_P) -> bool {
    let property_revision = para.properties.as_ref().is_some_and(|properties| {
        properties.numbering_revision.is_some()
            || properties.change.is_some()
            || properties
                .sect_pr
                .as_ref()
                .is_some_and(|section| section.change.is_some())
            || properties
                .rpr
                .as_ref()
                .is_some_and(run_properties_have_revision)
    });
    property_revision
        || para
            .runs
            .iter()
            .filter_map(|run| run.properties.as_ref())
            .any(run_properties_have_revision)
        || para
            .content_controls
            .iter()
            .any(|(_, _, _, control)| control_has_visible_revision(control))
        || para
            .revisions
            .iter()
            .any(|(_, _, revision)| revision_is_visible(revision))
}

/// The colour of an insertion, deletion or move in the legal revision style.
fn legal_revision_color(kind: RevisionKind) -> Option<Color> {
    let (r, g, b) = match kind {
        RevisionKind::Insertion => (0, 0, 0xFF),
        RevisionKind::Deletion => (0xFF, 0, 0),
        RevisionKind::MoveFrom | RevisionKind::MoveTo => (0, 0xA0, 0),
        _ => return None,
    };
    Some(Color {
        r: f64::from(r) / 255.0,
        g: f64::from(g) / 255.0,
        b: f64::from(b) / 255.0,
        a: 1.0,
    })
}

/// The text colour and underline of `projected`, given those of its run:
/// the tracked view underlines an insertion or a move destination, double
/// in the legal style, which also gives the run the colour of its revision.
pub(super) fn tracked_marks(
    projected: &ProjectedRun<'_>,
    style: RevisionStyle,
    color: Color,
    underline: Option<Underline>,
) -> (Color, Option<Underline>) {
    let legal = style == RevisionStyle::Legal;
    let underline = match (projected.force_underline, legal) {
        (true, true) => Some(Underline::Double),
        (true, false) => Some(Underline::Single),
        (false, _) => underline,
    };
    let color = projected
        .revision
        .filter(|_| legal)
        .and_then(legal_revision_color)
        .unwrap_or(color);
    (color, underline)
}

/// The colour of the change bar beside `para`, or `None` when the tracked
/// view shows no revision in it. In the legal style the bar takes the colour
/// of the first insertion, deletion or move, its runs' or its mark's.
pub(super) fn change_bar_color(para: &CT_P, input: &LayoutInput) -> Option<Color> {
    if input.revision_view != RevisionView::Tracked || !paragraph_has_visible_revision(para) {
        return None;
    }
    if input.revision_style == RevisionStyle::Plain {
        return Some(Color::BLACK);
    }
    let mark = para
        .properties
        .as_ref()
        .and_then(|properties| properties.rpr.as_ref())
        .into_iter()
        .flat_map(|properties| properties.revision_markers.iter().map(CT_Revision::kind));
    project_paragraph_runs(para, RevisionView::Tracked)
        .iter()
        .filter(|projected| run_has_visible_content(projected.run))
        .filter_map(|projected| projected.revision)
        .chain(mark)
        .find_map(legal_revision_color)
        .or(Some(Color::BLACK))
}

fn run_properties_have_revision(properties: &oxoffice_docx_oxml::properties::CT_RPr) -> bool {
    properties.change.is_some() || !properties.revision_markers.is_empty()
}

fn run_has_visible_content(run: &CT_R) -> bool {
    run.content.iter().any(|content| match content {
        RunContent::Text(text) | RunContent::DeletedText(text) => !text.text.is_empty(),
        RunContent::CommentReference { .. } => false,
        RunContent::Tab
        | RunContent::Break(_)
        | RunContent::Drawing(_)
        | RunContent::Field(_)
        | RunContent::FootnoteRef { .. }
        | RunContent::EndnoteRef { .. }
        | RunContent::Symbol { .. }
        | RunContent::SpecialCharacter(_) => true,
    })
}

fn control_has_visible_revision(control: &CT_Sdt) -> bool {
    control
        .revisions()
        .iter()
        .any(|(_, revision)| revision_is_visible(revision))
        || control.content.iter().any(|content| match content {
            SdtContent::Paragraph(paragraph) => paragraph_has_visible_revision(paragraph),
            SdtContent::ContentControl(control) => control_has_visible_revision(control),
            SdtContent::Table(table) => {
                table
                    .content_controls
                    .iter()
                    .any(|(_, _, control)| control_has_visible_revision(control))
                    || table.rows.iter().any(row_has_visible_control_revision)
            }
            SdtContent::Row(row) => row_has_visible_control_revision(row),
            SdtContent::Cell(cell) => cell_has_visible_control_revision(cell),
            SdtContent::Run(run) => run
                .properties
                .as_ref()
                .is_some_and(run_properties_have_revision),
            SdtContent::RawXml(_) => false,
        })
}

fn row_has_visible_control_revision(row: &CT_Row) -> bool {
    row.content_controls
        .iter()
        .any(|(_, _, control)| control_has_visible_revision(control))
        || row.cells.iter().any(cell_has_visible_control_revision)
}

fn cell_has_visible_control_revision(cell: &CT_Tc) -> bool {
    cell.content.iter().any(|content| match content {
        CellContent::Paragraph(paragraph) => paragraph_has_visible_revision(paragraph),
        CellContent::Table(table) => {
            table
                .content_controls
                .iter()
                .any(|(_, _, control)| control_has_visible_revision(control))
                || table.rows.iter().any(row_has_visible_control_revision)
        }
        CellContent::ContentControl(control) => control_has_visible_revision(control),
    })
}

fn revision_is_visible(revision: &CT_Revision) -> bool {
    if let Some(paragraph) = revision.content_paragraph() {
        return project_paragraph_runs(paragraph, RevisionView::Tracked)
            .iter()
            .any(|projected| run_has_visible_content(projected.run))
            || paragraph_has_visible_revision(paragraph);
    }
    match revision.content() {
        RevisionContent::Runs(runs) => {
            runs.iter().any(run_has_visible_content)
                || revision
                    .nested_revisions()
                    .iter()
                    .any(|(_, nested)| revision_is_visible(nested))
        }
        RevisionContent::Marker => revision
            .nested_revisions()
            .iter()
            .any(|(_, nested)| revision_is_visible(nested)),
        RevisionContent::PriorRunProperties(_)
        | RevisionContent::PriorParagraphProperties(_)
        | RevisionContent::PriorTableProperties(_)
        | RevisionContent::PriorSectionProperties(_) => true,
    }
}

// Borrowed admission precedes the recursive predicate, any clone and all
// deferred semantic factories. The caller's source model remains untouched.
pub(super) fn prepared_semantic_layout_input(input: &LayoutInput) -> Result<Cow<'_, LayoutInput>> {
    let validate = || -> oxoffice_docx_oxml::Result<()> {
        input.document.validate_resource_limits()?;
        for body in input.story_bodies.values() {
            body.validate_resource_limits()?;
        }
        for part in input.headers.values().chain(input.footers.values()) {
            for paragraph in &part.paragraphs {
                paragraph.validate_resource_limits()?;
            }
        }
        for notes in [input.footnotes.as_ref(), input.endnotes.as_ref()]
            .into_iter()
            .flatten()
        {
            for note in &notes.footnotes {
                for paragraph in &note.paragraphs {
                    paragraph.validate_resource_limits()?;
                }
            }
        }
        Ok(())
    };
    validate().map_err(|error| LayoutError::Layout(error.to_string()))?;
    let mut needs_projection = false;
    // This is the existing authoritative story selection, including its
    // drawing children. Raw foreign payload is never promoted to a model.
    visit_input_paragraphs(input, &mut |p| {
        needs_projection |= paragraph_has_semantic_revision(p);
    });
    // Direct-run revisions can belong to a block control without a paragraph.
    needs_projection |= body_has_semantic_revision(&input.document.body.content)
        || input
            .story_bodies
            .values()
            .any(|b| body_has_semantic_revision(&b.content));
    if !needs_projection {
        return Ok(Cow::Borrowed(input));
    }
    let mut prepared = input.clone();
    prepare_layout_body_projection(&mut prepared.document.body.content)
        .map_err(|error| LayoutError::Layout(error.to_string()))?;
    for (header, parts) in [
        (true, &mut prepared.headers),
        (false, &mut prepared.footers),
    ] {
        for (id, part) in parts {
            let story = if header {
                WordStory::Header {
                    relationship_id: id.clone(),
                }
            } else {
                WordStory::Footer {
                    relationship_id: id.clone(),
                }
            };
            if let Some(body) = prepared.story_bodies.get_mut(&story) {
                prepare_layout_body_projection(&mut body.content)
                    .map_err(|error| LayoutError::Layout(error.to_string()))?;
            } else {
                for paragraph in &mut part.paragraphs {
                    prepare_layout_paragraph_projection(paragraph)
                        .map_err(|error| LayoutError::Layout(error.to_string()))?;
                }
            }
        }
    }
    for (footnote, notes) in [
        (true, &mut prepared.footnotes),
        (false, &mut prepared.endnotes),
    ] {
        if let Some(notes) = notes {
            for note in &mut notes.footnotes {
                if note.note_type != oxoffice_docx_oxml::footnotes::NoteType::Normal {
                    continue;
                }
                let story = if footnote {
                    WordStory::Footnote { id: note.id }
                } else {
                    WordStory::Endnote { id: note.id }
                };
                if let Some(body) = prepared.story_bodies.get_mut(&story) {
                    prepare_layout_body_projection(&mut body.content)
                        .map_err(|error| LayoutError::Layout(error.to_string()))?;
                } else {
                    for paragraph in &mut note.paragraphs {
                        prepare_layout_paragraph_projection(paragraph)
                            .map_err(|error| LayoutError::Layout(error.to_string()))?;
                    }
                }
            }
        }
    }
    Ok(Cow::Owned(prepared))
}

fn paragraph_has_semantic_revision(p: &CT_P) -> bool {
    !p.revisions.is_empty()
        || p.content_controls
            .iter()
            .any(|(_, _, _, c)| control_has_semantic_revision(c))
        || p.runs.iter().any(run_has_semantic_revision)
}

fn run_has_semantic_revision(run: &CT_R) -> bool {
    run.content
        .iter()
        .filter_map(|c| match c {
            RunContent::Drawing(d) => Some(d),
            _ => None,
        })
        .chain(run.alt_drawings.iter())
        .any(|drawing| {
            drawing
                .anchor
                .as_ref()
                .and_then(|a| a.shape.as_ref())
                .is_some_and(|shape| {
                    shape.text_body.as_ref().map_or_else(
                        || shape.text.iter().any(paragraph_has_semantic_revision),
                        |b| body_has_semantic_revision(&b.content),
                    )
                })
        })
}

fn control_has_semantic_revision(control: &CT_Sdt) -> bool {
    !control.revisions().is_empty()
        || control.content.iter().any(|c| match c {
            SdtContent::Paragraph(p) => paragraph_has_semantic_revision(p),
            SdtContent::Table(t) => table_has_semantic_revision(t),
            SdtContent::Row(r) => row_has_semantic_revision(r),
            SdtContent::Cell(c) => cell_has_semantic_revision(c),
            SdtContent::Run(r) => run_has_semantic_revision(r),
            SdtContent::ContentControl(c) => control_has_semantic_revision(c),
            SdtContent::RawXml(_) => false,
        })
}

fn table_has_semantic_revision(table: &CT_Tbl) -> bool {
    table
        .content_controls
        .iter()
        .any(|(_, _, c)| control_has_semantic_revision(c))
        || table.rows.iter().any(row_has_semantic_revision)
}

fn row_has_semantic_revision(row: &CT_Row) -> bool {
    row.content_controls
        .iter()
        .any(|(_, _, c)| control_has_semantic_revision(c))
        || row.cells.iter().any(cell_has_semantic_revision)
}

fn cell_has_semantic_revision(cell: &CT_Tc) -> bool {
    cell.content.iter().any(|c| match c {
        CellContent::Paragraph(p) => paragraph_has_semantic_revision(p),
        CellContent::Table(t) => table_has_semantic_revision(t),
        CellContent::ContentControl(c) => control_has_semantic_revision(c),
    })
}

fn body_has_semantic_revision(body: &[BodyContent]) -> bool {
    body.iter().any(|c| match c {
        BodyContent::Paragraph(p) => paragraph_has_semantic_revision(p),
        BodyContent::Table(t) => table_has_semantic_revision(t),
        BodyContent::ContentControl(c) => control_has_semantic_revision(c),
        BodyContent::RawXml(_) => false,
    })
}

fn prepare_layout_paragraph_projection(value: &mut CT_P) -> oxoffice_docx_oxml::Result<()> {
    for (_, _, revision) in &mut value.revisions {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_layout_paragraph_projection(paragraph)?;
        }
    }
    for (_, _, _, control) in &mut value.content_controls {
        prepare_layout_control_projection(control)?;
    }
    for run in &mut value.runs {
        prepare_layout_run_projection(run)?;
    }
    Ok(())
}

fn prepare_layout_run_projection(value: &mut CT_R) -> oxoffice_docx_oxml::Result<()> {
    for drawing in value
        .content
        .iter_mut()
        .filter_map(|content| {
            if let RunContent::Drawing(drawing) = content {
                Some(drawing)
            } else {
                None
            }
        })
        .chain(value.alt_drawings.iter_mut())
    {
        let shape = drawing
            .anchor
            .as_mut()
            .and_then(|anchor| anchor.shape.as_mut());
        if let Some(shape) = shape {
            if let Some(body) = &mut shape.text_body {
                prepare_layout_body_projection(&mut body.content)?;
            } else {
                for paragraph in &mut shape.text {
                    prepare_layout_paragraph_projection(paragraph)?;
                }
            }
        }
    }
    Ok(())
}

fn prepare_layout_control_projection(value: &mut CT_Sdt) -> oxoffice_docx_oxml::Result<()> {
    for (_, revision) in value.physical_revisions_mut() {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_layout_paragraph_projection(paragraph)?;
        }
    }
    for child in &mut value.content {
        match child {
            SdtContent::Paragraph(p) => prepare_layout_paragraph_projection(p)?,
            SdtContent::Table(t) => prepare_layout_table_projection(t)?,
            SdtContent::Row(r) => prepare_layout_row_projection(r)?,
            SdtContent::Cell(c) => prepare_layout_cell_projection(c)?,
            SdtContent::Run(r) => prepare_layout_run_projection(r)?,
            SdtContent::ContentControl(c) => prepare_layout_control_projection(c)?,
            SdtContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn prepare_layout_table_projection(value: &mut CT_Tbl) -> oxoffice_docx_oxml::Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_layout_control_projection(control)?;
    }
    for row in &mut value.rows {
        prepare_layout_row_projection(row)?;
    }
    Ok(())
}

fn prepare_layout_row_projection(value: &mut CT_Row) -> oxoffice_docx_oxml::Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_layout_control_projection(control)?;
    }
    for cell in &mut value.cells {
        prepare_layout_cell_projection(cell)?;
    }
    Ok(())
}

fn prepare_layout_cell_projection(value: &mut CT_Tc) -> oxoffice_docx_oxml::Result<()> {
    for content in &mut value.content {
        match content {
            CellContent::Paragraph(p) => prepare_layout_paragraph_projection(p)?,
            CellContent::Table(t) => prepare_layout_table_projection(t)?,
            CellContent::ContentControl(c) => prepare_layout_control_projection(c)?,
        }
    }
    Ok(())
}

fn prepare_layout_body_projection(values: &mut [BodyContent]) -> oxoffice_docx_oxml::Result<()> {
    for value in values {
        match value {
            BodyContent::Paragraph(p) => prepare_layout_paragraph_projection(p)?,
            BodyContent::Table(t) => prepare_layout_table_projection(t)?,
            BodyContent::ContentControl(c) => prepare_layout_control_projection(c)?,
            BodyContent::RawXml(_) => {}
        }
    }
    Ok(())
}
