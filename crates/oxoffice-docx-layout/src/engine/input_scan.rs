//! Scans of the layout input: page references, sequence and section fields, note labels, bookmark text.

use std::collections::{BTreeSet, HashSet};

use oxoffice_docx_oxml::text::{
    bookmark_name_key, bookmarks_depending_on, field_ref_targets, same_bookmark_name,
};

use super::*;

pub(super) fn page_ref_id(input: &LayoutInput, name: &str) -> Option<usize> {
    bookmark_names(input)
        .page_target_ids
        .get(&bookmark_name_key(name))
        .copied()
}

/// The spelling of the bookmark a name refers to, as its start writes it:
/// Word compares bookmark names without case, links match them exactly.
pub(super) fn bookmark_spelling(input: &LayoutInput, name: &str) -> Option<String> {
    bookmark_names(input)
        .spellings
        .get(&bookmark_name_key(name))
        .cloned()
}

/// The bookmark names of the input, read once while its body and revision
/// view stay the same.
fn bookmark_names(input: &LayoutInput) -> std::borrow::Cow<'_, crate::input::BookmarkNames> {
    let owner = (
        input.document.body.content.as_ptr().addr(),
        input.revision_view,
    );
    let read = || {
        let page_targets = read_page_reference_names(input);
        let page_target_ids = page_targets
            .iter()
            .enumerate()
            .map(|(id, name)| (bookmark_name_key(name), id))
            .collect();
        let end_ids = read_bookmark_end_ids(input);
        crate::input::BookmarkNames {
            page_targets,
            page_target_ids,
            spellings: read_bookmark_spellings(input, &end_ids),
            end_ids,
        }
    };
    let (cached_owner, cached) = input.bookmark_names.0.get_or_init(|| (owner, read()));
    if *cached_owner == owner {
        std::borrow::Cow::Borrowed(cached)
    } else {
        std::borrow::Cow::Owned(read())
    }
}

/// The identifier of every bookmark end of the stories of the input.
fn read_bookmark_end_ids(input: &LayoutInput) -> HashSet<i32> {
    let block_bookmarks = input.block_bookmarks();
    let mut ids = HashSet::new();
    for paragraph in input_story_paragraphs(input).iter().flatten() {
        let (before, after) = block_bookmarks.markers(paragraph);
        for marker in paragraph.bookmark_markers.iter().chain(before).chain(after) {
            if !marker.is_start()
                && let Some(id) = marker.id()
            {
                ids.insert(id);
            }
        }
    }
    ids
}

/// Whether a bookmark start opens a bookmark a field can find by name: one
/// whose identifier has no end in any story, as LibreOffice writes beside a
/// copied form field, is no bookmark to Word.
pub(super) fn opens_bookmark(input: &LayoutInput, marker: &BookmarkMarker) -> bool {
    marker.is_start()
        && marker
            .id()
            .is_none_or(|id| bookmark_names(input).end_ids.contains(&id))
}

/// The first spelling of each bookmark that starts in the stories of the
/// input, by its folded name. A start with no end (`end_ids`) is no
/// bookmark, so it gives no spelling.
fn read_bookmark_spellings(input: &LayoutInput, end_ids: &HashSet<i32>) -> HashMap<String, String> {
    let block_bookmarks = input.block_bookmarks();
    let mut spellings = HashMap::new();
    for paragraph in input_story_paragraphs(input).iter().flatten() {
        let (before, after) = block_bookmarks.markers(paragraph);
        for marker in paragraph.bookmark_markers.iter().chain(before).chain(after) {
            if marker.is_start()
                && marker.id().is_none_or(|id| end_ids.contains(&id))
                && let Some(name) = marker.name()
            {
                spellings
                    .entry(bookmark_name_key(name))
                    .or_insert_with(|| name.to_owned());
            }
        }
    }
    spellings
}

/// The bookmarks a page target marker records, by target id: those of
/// `PAGEREF` fields first, then those an internal link goes to.
pub(crate) fn page_reference_names(input: &LayoutInput) -> Vec<String> {
    bookmark_names(input).page_targets.clone()
}

fn read_page_reference_names(input: &LayoutInput) -> Vec<String> {
    let mut names = Vec::<String>::new();
    let mut seen = HashSet::new();
    visit_input_paragraphs(input, &mut |paragraph| {
        for projected in project_paragraph_runs(paragraph, input.revision_view) {
            for content in &projected.run.content {
                let RunContent::Field(field) = content else {
                    continue;
                };
                let mut fields = vec![field];
                while let Some(field) = fields.pop() {
                    let instruction = field.effective_instruction();
                    if instruction.name == "PAGEREF"
                        && let Some(bookmark) = field_text_argument(&instruction, 0)
                        && seen.insert(bookmark_name_key(bookmark))
                    {
                        names.push(bookmark.to_owned());
                    }
                    fields.extend(field.all_nested_fields_in_source_order().into_iter().rev());
                }
            }
        }
    });
    // Then the bookmarks an internal link goes to, whose start a marker
    // records so the link lands on the line where the bookmark starts.
    let mut add = |name: &str| {
        if !name.is_empty() && seen.insert(bookmark_name_key(name)) {
            names.push(name.to_owned());
        }
    };
    visit_input_paragraphs(input, &mut |paragraph| {
        for hyperlink in &paragraph.hyperlinks {
            if hyperlink.rel_id.is_none()
                && let Some(anchor) = hyperlink.anchor.as_deref()
            {
                add(anchor);
            }
        }
        for projected in project_paragraph_runs(paragraph, input.revision_view) {
            for content in &projected.run.content {
                let RunContent::Field(field) = content else {
                    continue;
                };
                let mut fields = vec![field];
                while let Some(field) = fields.pop() {
                    if let Some(name) = field_link(&field.effective_instruction())
                        .as_deref()
                        .and_then(|link| link.strip_prefix('#'))
                    {
                        add(name);
                    }
                    fields.extend(field.all_nested_fields_in_source_order().into_iter().rev());
                }
            }
        }
    });
    names
}

/// Where a field links to: `HYPERLINK` to its target, its `\l` anchor
/// appended (`#anchor` alone for an anchor only), and `REF`, `PAGEREF` or
/// `NOTEREF` with `\h` to `#` and their bookmark.
pub(crate) fn field_link(instruction: &FieldInstruction) -> Option<String> {
    match instruction.name.as_str() {
        "HYPERLINK" => {
            let anchor = instruction
                .switches
                .iter()
                .find(|field_switch| field_switch.name == "l")
                .and_then(|field_switch| match &field_switch.argument {
                    Some(FieldArgument::Text(anchor)) => Some(anchor.as_str()),
                    _ => None,
                })
                .filter(|anchor| !anchor.is_empty());
            let target = field_text_argument(instruction, 0).filter(|target| !target.is_empty());
            match (target, anchor) {
                (Some(target), Some(anchor)) => Some(format!("{target}#{anchor}")),
                (Some(target), None) => Some(target.to_owned()),
                (None, Some(anchor)) => Some(format!("#{anchor}")),
                (None, None) => None,
            }
        }
        "REF" | "PAGEREF" | "NOTEREF" if field_has_switch(instruction, "h") => {
            field_text_argument(instruction, 0)
                .filter(|bookmark| !bookmark.is_empty())
                .map(|bookmark| format!("#{bookmark}"))
        }
        _ => None,
    }
}

pub(super) fn document_has_sequence_fields(input: &LayoutInput) -> bool {
    let mut found = false;
    visit_input_paragraphs(input, &mut |paragraph| {
        for projected in project_paragraph_runs(paragraph, input.revision_view) {
            for content in &projected.run.content {
                if let RunContent::Field(field) = content {
                    found |= field.effective_instruction().name == "SEQ"
                        || field
                            .all_nested_fields_in_source_order()
                            .iter()
                            .any(|nested| nested.effective_instruction().name == "SEQ");
                }
            }
        }
    });
    found
}

pub(super) fn document_has_section_fields(input: &LayoutInput) -> bool {
    let mut found = false;
    visit_input_paragraphs(input, &mut |paragraph| {
        found |= project_paragraph_runs(paragraph, input.revision_view).iter().any(|projected|
            projected.run.content.iter().any(|content| matches!(content, RunContent::Field(field)
                if std::iter::once(field).chain(field.all_nested_fields_in_source_order()).any(|owner| matches!(owner.effective_instruction().name.as_str(), "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF")))));
    });
    found
}

/// Preserve each physical story boundary while visiting selected drawing owners.
fn input_story_paragraphs(input: &LayoutInput) -> Vec<Vec<&CT_P>> {
    fn body_paragraphs(body: &CT_Body, accepted: bool) -> Vec<&CT_P> {
        let mut paragraphs = Vec::new();
        for item in body_layout_items(body) {
            match item {
                MainStoryLayoutItem::Paragraph(paragraph, _) => paragraphs.push(paragraph),
                MainStoryLayoutItem::Table(table, _) => {
                    visit_table_paragraphs(table, accepted, &mut |p| paragraphs.push(p));
                }
            }
        }
        paragraphs
    }
    let accepted = input.revision_view == RevisionView::Accepted;
    let mut main = Vec::new();
    visit_document_paragraphs(input, &mut |paragraph| main.push(paragraph));
    let mut stories = vec![main];
    let mut physical_parts = std::collections::HashSet::new();
    for (header, parts) in [(true, &input.headers), (false, &input.footers)] {
        let mut parts = parts.iter().collect::<Vec<_>>();
        parts.sort_unstable_by_key(|(id, _)| *id);
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
            if let Some(part) = input.story_part_names.get(&story)
                && !physical_parts.insert((header, part))
            {
                continue;
            }
            stories.push(input.story_bodies.get(&story).map_or_else(
                || part.paragraphs.iter().collect(),
                |body| body_paragraphs(body, accepted),
            ));
        }
    }
    for (footnote, stream) in [
        (true, input.footnotes.as_ref()),
        (false, input.endnotes.as_ref()),
    ] {
        if let Some(stream) = stream {
            for note in &stream.footnotes {
                if note.note_type != oxoffice_docx_oxml::footnotes::NoteType::Normal {
                    continue;
                }
                let story = if footnote {
                    WordStory::Footnote { id: note.id }
                } else {
                    WordStory::Endnote { id: note.id }
                };
                stories.push(input.story_bodies.get(&story).map_or_else(
                    || note.paragraphs.iter().collect(),
                    |body| body_paragraphs(body, accepted),
                ));
            }
        }
    }
    let mut owner = 0;
    while owner < stories.len() {
        let mut drawings = Vec::new();
        for paragraph in &stories[owner] {
            for projected in project_paragraph_runs(paragraph, input.revision_view) {
                for drawing in projected
                    .run
                    .content
                    .iter()
                    .filter_map(|content| match content {
                        RunContent::Drawing(drawing) => Some(drawing),
                        _ => None,
                    })
                    .chain(projected.run.alt_drawings.iter())
                {
                    if let Some(shape) = drawing
                        .anchor
                        .as_ref()
                        .and_then(|anchor| anchor.shape.as_ref())
                    {
                        drawings.push(shape.text_body.as_ref().map_or_else(
                            || shape.text.iter().collect(),
                            |body| body_paragraphs(body, accepted),
                        ));
                    }
                }
            }
        }
        stories.extend(drawings);
        owner += 1;
    }
    stories
}

/// Visit every authoritative modeled paragraph without joining story ranges.
pub(super) fn visit_input_paragraphs<'a>(input: &'a LayoutInput, visit: &mut impl FnMut(&'a CT_P)) {
    for paragraphs in input_story_paragraphs(input) {
        for paragraph in paragraphs {
            visit(paragraph);
        }
    }
}

pub(super) fn field_text_argument(instruction: &FieldInstruction, index: usize) -> Option<&str> {
    match instruction.arguments.get(index) {
        Some(FieldArgument::Text(value)) => Some(value),
        Some(FieldArgument::Nested(_) | FieldArgument::Joined(_)) | None => None,
    }
}

pub(super) fn field_has_switch(instruction: &FieldInstruction, name: &str) -> bool {
    instruction
        .switches
        .iter()
        .any(|field_switch| field_switch.name == name)
}

pub(super) fn numbered_ref_text(
    instruction: &FieldInstruction,
    bookmark: &str,
    bookmark_text: String,
    numbering: &NumberingState,
    source: Option<SourceNodeId>,
    position: std::result::Result<Option<&str>, String>,
) -> std::result::Result<String, String> {
    let position = position?;
    let mode = if field_has_switch(instruction, "w") {
        Some("full")
    } else if field_has_switch(instruction, "r") {
        Some("relative")
    } else if field_has_switch(instruction, "n") {
        Some("level")
    } else {
        None
    };
    if mode.is_none() && !field_has_switch(instruction, "p") {
        return Ok(bookmark_text);
    }
    let source_is_main_story = source.is_some_and(|source| numbering.is_main_story_source(source));
    let marker = numbering
        .bookmark_source(bookmark)
        .and_then(|_| numbering.bookmark(bookmark))
        .map(|(_, marker)| marker);
    let mut value = match (mode, marker) {
        (Some(_), Some(marker)) => marker.numbered_reference_text(
            instruction,
            source
                .filter(|_| source_is_main_story)
                .and_then(|source| numbering.source(source)),
        )?,
        (Some(_), None)
            if numbering.bookmark_source(bookmark).is_some_and(|source| {
                numbering
                    .navigation
                    .get(&source)
                    .is_some_and(|facts| facts.suppressed_numbering)
            }) =>
        {
            "0".into()
        }
        (Some(_), None) => bookmark_text,
        (None, _) => String::new(),
    };
    if let Some(position) = position {
        if !value.is_empty() {
            value.push(' ');
        }
        value.push_str(position);
    }
    Ok(value)
}

pub(super) fn document_has_ref_projection(input: &LayoutInput) -> bool {
    let paragraph_has_ref = |paragraph: &CT_P| {
        project_paragraph_runs(paragraph, input.revision_view)
            .into_iter()
            .any(|projected| {
                projected.run.content.iter().any(|content| {
                    let RunContent::Field(field) = content else {
                        return false;
                    };
                    let instruction = field.effective_instruction();
                    instruction.name == "STYLEREF"
                        || instruction.name == "REF"
                            && ["n", "r", "w", "p"]
                                .into_iter()
                                .any(|name| field_has_switch(&instruction, name))
                })
            })
    };
    let mut found = false;
    visit_input_paragraphs(input, &mut |paragraph| {
        found |= paragraph_has_ref(paragraph);
    });
    found
}

pub(super) fn document_has_page_ref(input: &LayoutInput, name: &str) -> bool {
    page_ref_id(input, name).is_some()
}

/// Visit every paragraph of the main story. The accepted view leaves out the
/// rows that `CT_Row::accepted_view_removes` reports, as accepting does.
fn visit_document_paragraphs<'a>(input: &'a LayoutInput, visit: &mut impl FnMut(&'a CT_P)) {
    let accepted = input.revision_view == RevisionView::Accepted;
    for content in &input.document.body.content {
        match content {
            BodyContent::Paragraph(paragraph) => visit(paragraph),
            BodyContent::Table(table) => visit_table_paragraphs(table, accepted, visit),
            BodyContent::ContentControl(control) => {
                visit_control_paragraphs(control, BlockControlOwner::Body, accepted, visit)
            }
            BodyContent::RawXml(_) => {}
        }
    }
}

pub(super) fn input_uses_page_note_restart(input: &LayoutInput) -> bool {
    input.note_defaults.iter().flatten().any(|policy| policy.num_restart.as_deref() == Some("eachPage"))
        || input.document.body.content.iter().any(|content| {
            matches!(content, BodyContent::Paragraph(paragraph) if paragraph.properties.as_ref().and_then(|properties| properties.sect_pr.as_ref()).is_some_and(|section| {
                [section.footnote_pr.as_deref(), section.endnote_pr.as_deref()].into_iter().flatten().any(|policy| policy.num_restart.as_deref() == Some("eachPage"))
            }))
        })
        || input.document.body.sect_pr.as_ref().is_some_and(|section| {
            [section.footnote_pr.as_deref(), section.endnote_pr.as_deref()].into_iter().flatten().any(|policy| policy.num_restart.as_deref() == Some("eachPage"))
        })
}

pub(super) fn note_reference_pages(result: &LayoutResult) -> HashMap<NoteRef, usize> {
    let mut pages = HashMap::new();
    for page in &result.pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            let reference = match element {
                PositionedElement::Text(run) => run.note,
                PositionedElement::MultilingualText(run) => run.note,
                _ => None,
            };
            if let Some(reference) = reference {
                pages.entry(reference).or_insert(page.page_number);
            }
        });
    }
    pages
}

pub(super) fn note_labels(
    input: &LayoutInput,
    note_pages: Option<&HashMap<NoteRef, usize>>,
) -> Result<HashMap<NoteRef, String>> {
    let mut labels = HashMap::new();
    let mut preparation_error = None;
    let mut counts = [None::<u32>, None::<u32>];
    let mut last_pages = [None::<usize>, None::<usize>];
    let mut section_ends = input
        .document
        .body
        .content
        .iter()
        .enumerate()
        .filter_map(|(index, content)| match content {
            BodyContent::Paragraph(paragraph) => paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
                .map(|section| (index, Some(section))),
            _ => None,
        })
        .collect::<Vec<_>>();
    section_ends.push((
        input.document.body.content.len(),
        input.document.body.sect_pr.as_ref(),
    ));
    let mut first_index = 0;
    for (end, section) in section_ends {
        let policies = [
            section.and_then(|section| section.footnote_pr.as_deref()),
            section.and_then(|section| section.endnote_pr.as_deref()),
        ];
        for (family, policy) in policies.iter().enumerate() {
            let restart = policy
                .and_then(|policy| policy.num_restart.as_deref())
                .or_else(|| {
                    input.note_defaults[family]
                        .as_ref()
                        .and_then(|policy| policy.num_restart.as_deref())
                });
            if first_index > 0 && restart == Some("eachSect") && policy.is_some() {
                counts[family] = None;
            }
        }
        for content in input
            .document
            .body
            .content
            .iter()
            .take(end + 1)
            .skip(first_index)
        {
            let mut visit = |paragraph: &CT_P| {
                for projected in project_paragraph_runs(paragraph, input.revision_view) {
                    let references = projected
                        .run
                        .content
                        .iter()
                        .map(|content| content_note_references(content, input.revision_view))
                        .collect::<Result<Vec<_>>>();
                    let references = match references {
                        Ok(references) => references,
                        Err(error) => {
                            preparation_error = Some(error);
                            return;
                        }
                    };
                    for item in references.into_iter().flatten() {
                        let (family, stream, id, custom_mark) = match &item {
                            RunContent::FootnoteRef { id, custom_mark } => {
                                (0, NoteStream::Footnote, *id, custom_mark.as_deref())
                            }
                            RunContent::EndnoteRef { id, custom_mark } => {
                                (1, NoteStream::Endnote, *id, custom_mark.as_deref())
                            }
                            _ => continue,
                        };
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            labels.entry(NoteRef { stream, id })
                        {
                            if let Some(mark) = custom_mark {
                                entry.insert(mark.to_owned());
                                continue;
                            }
                            let section_policy = policies[family];
                            let default_policy = input.note_defaults[family].as_ref();
                            let restart = section_policy
                                .and_then(|policy| policy.num_restart.as_deref())
                                .or_else(|| {
                                    default_policy.and_then(|policy| policy.num_restart.as_deref())
                                });
                            let page = note_pages
                                .and_then(|pages| pages.get(&NoteRef { stream, id }))
                                .copied();
                            if restart == Some("eachPage")
                                && page.is_some()
                                && last_pages[family] != page
                            {
                                counts[family] = None;
                            }
                            if page.is_some() {
                                last_pages[family] = page;
                            }
                            let start = section_policy
                                .and_then(|policy| policy.num_start)
                                .or_else(|| default_policy.and_then(|policy| policy.num_start))
                                .unwrap_or(1);
                            let ordinal =
                                counts[family].map_or(start, |value| value.saturating_add(1));
                            counts[family] = Some(ordinal);
                            let format = section_policy
                                .and_then(|policy| policy.num_fmt.as_deref())
                                .or_else(|| {
                                    default_policy.and_then(|policy| policy.num_fmt.as_deref())
                                })
                                .map(ST_NumberFormat::from_str);
                            entry.insert(note_mark(ordinal, format, family));
                        }
                    }
                }
            };
            match content {
                BodyContent::Paragraph(paragraph) => visit(paragraph),
                BodyContent::Table(table) => visit_table_paragraphs(
                    table,
                    input.revision_view == RevisionView::Accepted,
                    &mut visit,
                ),
                BodyContent::ContentControl(control) => visit_control_paragraphs(
                    control,
                    BlockControlOwner::Body,
                    input.revision_view == RevisionView::Accepted,
                    &mut visit,
                ),
                BodyContent::RawXml(_) => {}
            }
        }
        first_index = end.saturating_add(1);
    }
    if let Some(error) = preparation_error {
        return Err(error);
    }
    Ok(labels)
}

/// The mark of the `ordinal`th note of `family` (0 footnotes, 1 endnotes)
/// in `format`. Without a format Word numbers footnotes 1, 2, 3 and
/// endnotes i, ii, iii; a format the layout cannot draw falls back to
/// decimal rather than an empty mark.
fn note_mark(ordinal: u32, format: Option<ST_NumberFormat>, family: usize) -> String {
    let format = match format {
        Some(format) if style_resolver::number_format_has_visible_renderer(&format) => format,
        Some(_) => ST_NumberFormat::Decimal,
        None if family == 1 => ST_NumberFormat::LowerRoman,
        None => ST_NumberFormat::Decimal,
    };
    style_resolver::format_number(ordinal, format)
}

fn visit_table_paragraphs<'a>(table: &'a CT_Tbl, accepted: bool, visit: &mut impl FnMut(&'a CT_P)) {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(position, _, _)| *position == boundary)
        {
            visit_control_paragraphs(control, BlockControlOwner::Table, accepted, visit);
        }
        if let Some(row) = table.rows.get(boundary) {
            visit_row_paragraphs(row, accepted, visit);
        }
    }
}

fn visit_row_paragraphs<'a>(row: &'a CT_Row, accepted: bool, visit: &mut impl FnMut(&'a CT_P)) {
    if accepted && row.accepted_view_removes() {
        return;
    }
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(position, _, _)| *position == boundary)
        {
            visit_control_paragraphs(control, BlockControlOwner::Row, accepted, visit);
        }
        if let Some(cell) = row.cells.get(boundary) {
            visit_cell_paragraphs(cell, accepted, visit);
        }
    }
}

fn visit_cell_paragraphs<'a>(cell: &'a CT_Tc, accepted: bool, visit: &mut impl FnMut(&'a CT_P)) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => visit(paragraph),
            CellContent::Table(table) => visit_table_paragraphs(table, accepted, visit),
            CellContent::ContentControl(control) => {
                visit_control_paragraphs(control, BlockControlOwner::Cell, accepted, visit)
            }
        }
    }
}

fn visit_control_paragraphs<'a>(
    control: &'a CT_Sdt,
    owner: BlockControlOwner,
    accepted: bool,
    visit: &mut impl FnMut(&'a CT_P),
) {
    for content in &control.content {
        match (owner, content) {
            (
                BlockControlOwner::Body | BlockControlOwner::Cell,
                SdtContent::Paragraph(paragraph),
            ) => visit(paragraph),
            (BlockControlOwner::Body | BlockControlOwner::Cell, SdtContent::Table(table)) => {
                visit_table_paragraphs(table, accepted, visit)
            }
            (BlockControlOwner::Table, SdtContent::Row(row)) => {
                visit_row_paragraphs(row, accepted, visit)
            }
            (BlockControlOwner::Row, SdtContent::Cell(cell)) => {
                visit_cell_paragraphs(cell, accepted, visit)
            }
            (_, SdtContent::ContentControl(control)) => {
                visit_control_paragraphs(control, owner, accepted, visit)
            }
            _ => {}
        }
    }
}

fn bookmark_range_in_paragraphs(
    input: &LayoutInput,
    name: &str,
    paragraphs: &[&CT_P],
) -> Option<((usize, usize), (usize, usize))> {
    type OrderedBodyRunPosition = (usize, usize, usize);
    type BookmarkStart<'a> = (Option<&'a str>, OrderedBodyRunPosition);
    let mut starts: HashMap<i32, Vec<BookmarkStart<'_>>> = HashMap::new();
    let mut ends: HashMap<i32, Vec<OrderedBodyRunPosition>> = HashMap::new();
    let mut encounter = 0usize;
    let block_bookmarks = input.block_bookmarks();
    let current_bookmarks = paragraphs
        .iter()
        .map(|paragraph| block_bookmarks.paragraph_projection(paragraph))
        .collect::<Vec<_>>();
    for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
        let projected_run_count = project_paragraph_runs(paragraph, input.revision_view).len();
        for marker in &current_bookmarks[paragraph_index] {
            let Some(id) = marker.id() else {
                continue;
            };
            let marker_run_index = match input.revision_view {
                RevisionView::Accepted => marker.projected_run_index(),
                RevisionView::Tracked => marker.tracked_run_index(),
            };
            if marker_run_index > projected_run_count {
                return None;
            }
            let position = (paragraph_index, marker_run_index, encounter);
            encounter += 1;
            if marker.is_start() {
                starts
                    .entry(id)
                    .or_default()
                    .push((marker.name(), position));
            } else {
                ends.entry(id).or_default().push(position);
            }
        }
    }
    let candidates = starts
        .iter()
        .filter_map(|(id, starts)| {
            let ends = ends.get(id)?;
            (starts.len() == 1
                && starts[0]
                    .0
                    .is_some_and(|start| same_bookmark_name(start, name))
                && ends.len() == 1)
                .then_some((starts[0].1, ends[0]))
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return None;
    }
    let (ordered_start, ordered_end) = candidates[0];
    if ordered_start > ordered_end {
        return None;
    }
    let start = (ordered_start.0, ordered_start.1);
    let end = (ordered_end.0, ordered_end.1);
    Some((start, end))
}

/// The runs of the bookmark `name` as the revision view shows them, one row
/// per paragraph, when exactly one bookmark has that name in any case.
fn bookmark_value_runs(input: &LayoutInput, name: &str) -> Option<Vec<Vec<CT_R>>> {
    let stories = input_story_paragraphs(input);
    let block_bookmarks = input.block_bookmarks();
    let names = bookmark_names(input);
    let named_starts = stories
        .iter()
        .flatten()
        .flat_map(|p| {
            let (before, after) = block_bookmarks.markers(p);
            p.bookmark_markers.iter().chain(before).chain(after)
        })
        .filter(|marker| {
            marker.is_start()
                && marker.id().is_none_or(|id| names.end_ids.contains(&id))
                && marker
                    .name()
                    .is_some_and(|marker| same_bookmark_name(marker, name))
        })
        .count();
    if named_starts != 1 {
        return None;
    }
    let mut matches = Vec::new();
    for paragraphs in stories {
        if let Some((start, end)) = bookmark_range_in_paragraphs(input, name, &paragraphs) {
            let mut rows = Vec::new();
            for (paragraph_index, paragraph) in
                paragraphs.iter().enumerate().take(end.0 + 1).skip(start.0)
            {
                rows.push(
                    project_paragraph_runs(paragraph, input.revision_view)
                        .into_iter()
                        .enumerate()
                        .filter(|(index, _)| {
                            (paragraph_index, *index) >= start && (paragraph_index, *index) < end
                        })
                        .map(|(_, projected)| projected.run.clone())
                        .collect::<Vec<_>>(),
                );
            }
            matches.push(rows);
        }
    }
    (matches.len() == 1).then(|| matches.remove(0))
}

/// Whether exactly one bookmark is named `name` and has a range, the
/// condition for [`bookmark_text`] to give a value.
pub(super) fn bookmark_exists(input: &LayoutInput, name: &str) -> bool {
    bookmark_value_runs(input, name).is_some()
}

/// The folded REF targets of every field a reader sees in `rows`.
fn rows_ref_targets(rows: &[Vec<CT_R>]) -> BTreeSet<String> {
    let mut targets = BTreeSet::new();
    for run in rows.iter().flatten() {
        run.accepted_text_keeping(&mut |field| {
            field_ref_targets(field, &mut targets);
            true
        });
    }
    targets
}

/// The value a REF to `name` copies: the text a reader sees in the bookmark,
/// less the display of every field whose result depends on the bookmark
/// itself, as `update_fields` reads it.
pub(super) fn bookmark_text(input: &LayoutInput, name: &str) -> Option<String> {
    let rows = bookmark_value_runs(input, name)?;
    let own = rows_ref_targets(&rows);
    let key = bookmark_name_key(name);
    let depending = bookmarks_depending_on(name, &mut |current| {
        if current == key {
            return own.clone();
        }
        bookmark_value_runs(input, current)
            .map(|other| rows_ref_targets(&other))
            .unwrap_or_default()
    });
    Some(
        rows.iter()
            .map(|row| {
                row.iter()
                    .map(|run| {
                        run.accepted_text_keeping(&mut |field| {
                            let mut targets = BTreeSet::new();
                            field_ref_targets(field, &mut targets);
                            !targets.iter().any(|target| depending.contains(target))
                        })
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}
