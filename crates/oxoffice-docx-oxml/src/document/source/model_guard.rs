//! The model resource guard: admitted children, owner costs and model maxima.

use super::*;

pub(crate) fn admitted_word_child(parent: &[u8], child: &[u8]) -> bool {
    match parent {
        b"document" => child == b"body",
        b"body" | b"hdr" | b"ftr" | b"comment" | b"footnote" | b"endnote" | b"docPartBody"
        | b"txbxContent" | b"tc" => matches!(child, b"p" | b"tbl" | b"sdt"),
        b"comments" => child == b"comment",
        b"footnotes" => child == b"footnote",
        b"endnotes" => child == b"endnote",
        b"glossaryDocument" => child == b"docParts",
        b"docParts" => child == b"docPart",
        b"docPart" => child == b"docPartBody",
        b"tbl" => matches!(child, b"tr" | b"sdt"),
        b"tr" => matches!(child, b"tc" | b"sdt"),
        b"p" => matches!(
            child,
            b"r" | b"sdt"
                | b"hyperlink"
                | b"fldSimple"
                | b"ruby"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
        ),
        b"r" => matches!(child, b"drawing" | b"pict" | b"object"),
        b"sdt" => child == b"sdtContent",
        // The actual owner-specific SdtOwner predicate decides content below.
        b"sdtContent" => true,
        b"hyperlink" => matches!(
            child,
            b"r" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
        ),
        b"fldSimple" => matches!(
            child,
            b"r" | b"fldSimple"
                | b"hyperlink"
                | b"sdt"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
                | b"ruby"
        ),
        b"rt" | b"rubyBase" => child == b"r",
        b"ruby" => matches!(child, b"rt" | b"rubyBase"),
        b"ins" | b"del" | b"moveFrom" | b"moveTo" => matches!(
            child,
            b"r" | b"sdt" | b"hyperlink" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
        ),
        _ => false,
    }
}

pub(crate) fn word_owner_cost(path_cost: usize, increment: usize) -> Result<usize> {
    path_cost.checked_add(increment).filter(|cost| *cost <= 256).ok_or_else(|| {
        OxmlError::InvalidValue(
            "Word owner nesting exceeds weighted limit 256 (content control 1, typed text box 16)".into(),
        )
    })
}

/// Concrete borrowed owners used by the iterative pre-recursion guard.
#[derive(Clone, Copy)]
pub(crate) enum WordOwner<'a> {
    Body(&'a CT_Body),
    Paragraph(&'a CT_P),
    Run(&'a crate::text::CT_R),
    Control(&'a CT_Sdt),
    Table(&'a CT_Tbl),
    Row(&'a crate::table::CT_Row),
    Cell(&'a crate::table::CT_Tc),
    Field(&'a crate::text::Field),
    Revision(&'a CT_Revision),
    Drawing(&'a crate::drawing::CT_Drawing),
    Anchor(&'a crate::drawing::CT_Anchor),
}

impl<'a> WordOwner<'a> {
    pub(crate) fn key(self) -> (u8, usize) {
        match self {
            Self::Body(value) => (0, std::ptr::from_ref(value) as usize),
            Self::Paragraph(value) => (1, std::ptr::from_ref(value) as usize),
            Self::Run(value) => (2, std::ptr::from_ref(value) as usize),
            Self::Control(value) => (3, std::ptr::from_ref(value) as usize),
            Self::Table(value) => (4, std::ptr::from_ref(value) as usize),
            Self::Row(value) => (5, std::ptr::from_ref(value) as usize),
            Self::Cell(value) => (6, std::ptr::from_ref(value) as usize),
            Self::Field(value) => (7, std::ptr::from_ref(value) as usize),
            Self::Revision(value) => (8, std::ptr::from_ref(value) as usize),
            Self::Drawing(value) => (9, std::ptr::from_ref(value) as usize),
            Self::Anchor(value) => (10, std::ptr::from_ref(value) as usize),
        }
    }
    fn local_depth(self) -> (usize, usize) {
        match self {
            Self::Control(_) => (1, 0),
            Self::Table(_) => (0, 1),
            Self::Field(field) => (usize::from(field.has_ordered_result_owner()), 0),
            Self::Anchor(anchor)
                if anchor
                    .shape
                    .as_ref()
                    .is_some_and(|shape| shape.text_body.is_some() || !shape.text.is_empty()) =>
            {
                (16, 0)
            }
            Self::Drawing(drawing)
                if drawing
                    .inline
                    .as_ref()
                    .and_then(|inline| inline.shape.as_ref())
                    .is_some_and(|shape| shape.text_body.is_some() || !shape.text.is_empty()) =>
            {
                (16, 0)
            }
            _ => (0, 0),
        }
    }
    pub(crate) fn resource_children(self) -> Vec<Self> {
        use crate::content_control::SdtContent;
        use crate::table::CellContent;
        use crate::text::RunContent;
        let mut pending = Vec::new();
        let mut push = |child| pending.push(child);
        match self {
            WordOwner::Body(body) => {
                for child in &body.content {
                    match child {
                        BodyContent::Paragraph(p) => push(WordOwner::Paragraph(p)),
                        BodyContent::Table(t) => push(WordOwner::Table(t)),
                        BodyContent::ContentControl(s) => push(WordOwner::Control(s)),
                        BodyContent::RawXml(_) => {}
                    }
                }
            }
            WordOwner::Paragraph(p) => {
                for run in &p.runs {
                    push(WordOwner::Run(run));
                }
                for (_, _, _, control) in &p.content_controls {
                    push(WordOwner::Control(control));
                }
                for (_, _, revision) in &p.revisions {
                    push(WordOwner::Revision(revision));
                }
                for ruby in &p.rubies {
                    for run in &ruby.ruby_text {
                        push(WordOwner::Run(run));
                    }
                }
            }
            WordOwner::Run(run) => {
                for child in &run.content {
                    match child {
                        RunContent::Drawing(d) => push(WordOwner::Drawing(d)),
                        RunContent::Field(f) => push(WordOwner::Field(f)),
                        _ => {}
                    }
                }
                for drawing in &run.alt_drawings {
                    push(WordOwner::Drawing(drawing));
                }
            }
            WordOwner::Control(control) => {
                for run in control.original_resource_runs() {
                    pending.push(WordOwner::Run(run));
                }
                for (_, revision) in &control.revisions {
                    pending.push(WordOwner::Revision(revision));
                }
                for child in &control.content {
                    let child = match child {
                        SdtContent::Paragraph(p) => WordOwner::Paragraph(p),
                        SdtContent::Table(t) => WordOwner::Table(t),
                        SdtContent::Row(r) => WordOwner::Row(r),
                        SdtContent::Cell(c) => WordOwner::Cell(c),
                        SdtContent::Run(r) => WordOwner::Run(r),
                        SdtContent::ContentControl(s) => WordOwner::Control(s),
                        SdtContent::RawXml(_) => continue,
                    };
                    pending.push(child);
                }
            }
            WordOwner::Table(table) => {
                for row in &table.rows {
                    pending.push(WordOwner::Row(row));
                }
                for (_, _, control) in &table.content_controls {
                    pending.push(WordOwner::Control(control));
                }
            }
            WordOwner::Row(row) => {
                for cell in &row.cells {
                    push(WordOwner::Cell(cell));
                }
                for (_, _, control) in &row.content_controls {
                    push(WordOwner::Control(control));
                }
            }
            WordOwner::Cell(cell) => {
                for child in &cell.content {
                    match child {
                        CellContent::Paragraph(p) => push(WordOwner::Paragraph(p)),
                        CellContent::Table(t) => push(WordOwner::Table(t)),
                        CellContent::ContentControl(s) => push(WordOwner::Control(s)),
                    }
                }
            }
            WordOwner::Field(field) => {
                for child in field.resource_owner_children() {
                    pending.push(child);
                }
            }
            WordOwner::Revision(revision) => {
                if let Some(p) = revision.content_paragraph() {
                    push(WordOwner::Paragraph(p));
                }
                if let crate::revision::RevisionContent::Runs(runs) = revision.content() {
                    for run in runs {
                        push(WordOwner::Run(run));
                    }
                }
                for (_, revision) in revision.nested_revisions() {
                    push(WordOwner::Revision(revision));
                }
            }
            WordOwner::Drawing(drawing) => {
                if let Some(anchor) = &drawing.anchor {
                    push(WordOwner::Anchor(anchor));
                }
                // A text box placed in line with text is a typed text box
                // too: its weight is the drawing's own.
                if let Some(shape) = drawing.inline.as_ref().and_then(|i| i.shape.as_ref()) {
                    if let Some(body) = &shape.text_body {
                        pending.push(WordOwner::Body(body));
                    }
                    for p in &shape.text {
                        pending.push(WordOwner::Paragraph(p));
                    }
                }
            }
            WordOwner::Anchor(anchor) => {
                if let Some(shape) = &anchor.shape
                    && (shape.text_body.is_some() || !shape.text.is_empty())
                {
                    if let Some(body) = &shape.text_body {
                        pending.push(WordOwner::Body(body));
                    }
                    // All stored models are cloned, including the authored
                    // legacy paragraphs if an authoritative body is present.
                    for p in &shape.text {
                        pending.push(WordOwner::Paragraph(p));
                    }
                }
            }
        }
        pending
    }
}

enum ModelAdmissionStep<'a> {
    Enter(WordOwner<'a>, usize, usize),
    Finish((u8, usize), (usize, usize), Vec<WordOwner<'a>>),
}

fn checked_model_table_depth(depth: usize, increment: usize) -> Result<usize> {
    depth
        .checked_add(increment)
        .filter(|depth| *depth <= crate::table::MAX_RECOGNIZED_TABLE_NESTING)
        .ok_or_else(|| {
            OxmlError::InvalidValue("recognized model nesting exceeds table limit".into())
        })
}

pub(crate) fn admit_word_model(root: WordOwner<'_>) -> Result<()> {
    word_model_maxima(root).map(|_| ())
}

pub(crate) fn word_model_maxima(root: WordOwner<'_>) -> Result<(usize, usize)> {
    word_model_children_maxima(&[root], 0)
}

/// Guard actual prospective children under their real not-yet-built parent.
/// The memo is shared across every alternative, not renewed for each child.
pub(crate) fn word_model_children_maxima(
    roots: &[WordOwner<'_>],
    parent_cost: usize,
) -> Result<(usize, usize)> {
    word_owner_cost(0, parent_cost)?;
    // None means an actually entered active node. Merely queued siblings are
    // not marked active, so shared siblings cannot masquerade as back edges.
    let mut summaries = std::collections::HashMap::<(u8, usize), Option<(usize, usize)>>::new();
    let mut pending = roots
        .iter()
        .copied()
        .map(|root| ModelAdmissionStep::Enter(root, parent_cost, 0))
        .collect::<Vec<_>>();
    while let Some(step) = pending.pop() {
        match step {
            ModelAdmissionStep::Enter(owner, cost, tables) => {
                let key = owner.key();
                let completed = match summaries.get(&key) {
                    Some(Some(summary)) => Some(*summary),
                    Some(None) => {
                        return Err(OxmlError::InvalidValue(
                            "Word owner resource graph contains an active cycle".into(),
                        ));
                    }
                    None => None,
                };
                if let Some((weighted, table_depth)) = completed
                    && cost.checked_add(weighted).is_some_and(|value| value <= 256)
                    && tables
                        .checked_add(table_depth)
                        .is_some_and(|value| value <= crate::table::MAX_RECOGNIZED_TABLE_NESTING)
                {
                    continue;
                }
                let local = owner.local_depth();
                // Check this actual entry before enumerating any invalid tail.
                let cost = word_owner_cost(cost, local.0)?;
                let tables = checked_model_table_depth(tables, local.1)?;
                if completed.is_none() {
                    summaries.insert(key, None);
                }
                let children = owner.resource_children();
                if completed.is_none() && children.is_empty() {
                    pending.push(ModelAdmissionStep::Finish(key, local, children));
                } else if completed.is_none() {
                    let finish = pending.len();
                    pending.push(ModelAdmissionStep::Finish(key, local, Vec::new()));
                    pending.extend(
                        children
                            .iter()
                            .copied()
                            .map(|child| ModelAdmissionStep::Enter(child, cost, tables)),
                    );
                    // Extension only appends. Install the original children
                    // before the loop can process this Finish or any Enter.
                    let ModelAdmissionStep::Finish(_, _, retained) = &mut pending[finish] else {
                        unreachable!("new summary Finish is unchanged while queuing children");
                    };
                    *retained = children;
                } else {
                    // Preserve the former append/LIFO visitation order. On an
                    // over-budget memo hit, fitting children are skipped by their
                    // maxima and the actual first offending entry still refuses.
                    pending.extend(
                        children
                            .into_iter()
                            .map(|child| ModelAdmissionStep::Enter(child, cost, tables)),
                    );
                }
            }
            ModelAdmissionStep::Finish(key, local, children) => {
                let mut maxima = (0, 0);
                for child in children {
                    let Some(Some(summary)) = summaries.get(&child.key()) else {
                        return Err(OxmlError::InvalidValue(
                            "Word owner resource summary is incomplete".into(),
                        ));
                    };
                    maxima.0 = maxima.0.max(summary.0);
                    maxima.1 = maxima.1.max(summary.1);
                }
                let summary = (
                    word_owner_cost(local.0, maxima.0)?,
                    checked_model_table_depth(local.1, maxima.1)?,
                );
                summaries.insert(key, Some(summary));
            }
        }
    }
    let mut maxima = (0, 0);
    for root in roots {
        let Some(Some(summary)) = summaries.get(&root.key()) else {
            return Err(OxmlError::InvalidValue(
                "Word owner resource summary is incomplete".into(),
            ));
        };
        maxima.0 = maxima.0.max(summary.0);
        maxima.1 = maxima.1.max(summary.1);
    }
    Ok(maxima)
}
