//! Paragraph marks: where a removed or inserted paragraph records its change.

use super::*;

/// What a later final paragraph change can do with an owner already emitted.
#[derive(Clone)]
pub(super) enum EmittedOwner<'a> {
    /// A paragraph whose mark can carry the change, with its properties after
    /// acceptance and after rejection.
    Paragraph {
        accepted: PropertySide<'a>,
        rejected: PropertySide<'a>,
    },
    /// A table whose every row carries a marker of this kind (`ins`, `del`,
    /// `moveFrom` or `moveTo`), or a block content control whose content
    /// all does. A change that the same resolution removes crosses it,
    /// because that resolution removes it whole and leaves the paragraphs
    /// on either side adjacent; the other resolution keeps it.
    MarkedTable(&'static str),
    /// Source whitespace between owners, which every change crosses.
    Whitespace,
    /// A table or block content control both versions hold, which
    /// resolving the comparison's revisions keeps. A final paragraph after it carries its own change:
    /// the resolution removing its mark removes it, and the body ends with
    /// the table, as it did on that side. When existing revisions in the
    /// table remove it too, resolving them with the change, the paragraph
    /// goes with the table, and the body ends with what came before it.
    KeptTable,
    /// A block content control both versions hold, ending with a paragraph
    /// whose properties after acceptance and after rejection these are. A
    /// final paragraph change after it marks that last paragraph's mark, as
    /// Word does: removing it merges the paragraph after the control into
    /// the control.
    KeptControl {
        accepted: PropertySide<'a>,
        rejected: PropertySide<'a>,
    },
    Other,
}

impl<'a> EmittedOwner<'a> {
    /// The owner emitted for an original and an edited item, the same item
    /// when only one side has it.
    pub(super) fn of(
        original: &'a BodyContent,
        edited: &'a BodyContent,
        marked_kind: Option<&'static str>,
        metadata: &Metadata<'_>,
    ) -> Result<Self> {
        match (original, edited, marked_kind) {
            (BodyContent::Paragraph(original), BodyContent::Paragraph(edited), _) => {
                Self::paragraph(original, edited, metadata)
            }
            (_, BodyContent::Table(_) | BodyContent::ContentControl(_), Some(kind)) => {
                Ok(Self::MarkedTable(kind))
            }
            _ => Ok(Self::Other),
        }
    }

    pub(super) fn of_control(
        original: &'a SdtContent,
        edited: &'a SdtContent,
        marked_kind: Option<&'static str>,
        metadata: &Metadata<'_>,
    ) -> Result<Self> {
        match (original, edited, marked_kind) {
            (SdtContent::Paragraph(original), SdtContent::Paragraph(edited), _) => {
                Self::paragraph(original, edited, metadata)
            }
            (_, SdtContent::Table(_) | SdtContent::ContentControl(_), Some(kind)) => {
                Ok(Self::MarkedTable(kind))
            }
            _ => Ok(Self::Other),
        }
    }

    pub(super) fn of_cell(
        original: &'a CellContent,
        edited: &'a CellContent,
        marked_kind: Option<&'static str>,
        metadata: &Metadata<'_>,
    ) -> Result<Self> {
        match (original, edited, marked_kind) {
            (CellContent::Paragraph(original), CellContent::Paragraph(edited), _) => {
                Self::paragraph(original, edited, metadata)
            }
            (_, CellContent::Table(_) | CellContent::ContentControl(_), Some(kind)) => {
                Ok(Self::MarkedTable(kind))
            }
            _ => Ok(Self::Other),
        }
    }

    pub(super) fn paragraph(
        original: &'a CT_P,
        edited: &'a CT_P,
        metadata: &Metadata<'_>,
    ) -> Result<Self> {
        let (accepted, rejected) = PropertySide::matched(original, edited, metadata)?;
        // With formatting ignored, the paragraph keeps the original's
        // properties on both sides, as it is written.
        let accepted = if metadata.options.ignore_formatting {
            rejected.clone()
        } else {
            accepted
        };
        Ok(Self::Paragraph { accepted, rejected })
    }

    /// The owner of a block content control both versions hold: kept like a
    /// table, and whose last paragraph takes a final paragraph change after
    /// it when both versions end the control with one.
    pub(super) fn kept_control(
        original: &'a CT_Sdt,
        edited: &'a CT_Sdt,
        metadata: &Metadata<'_>,
    ) -> Result<Self> {
        match (original.content.last(), edited.content.last()) {
            (Some(SdtContent::Paragraph(original)), Some(SdtContent::Paragraph(edited))) => {
                let (accepted, rejected) = PropertySide::matched(original, edited, metadata)?;
                Ok(Self::KeptControl { accepted, rejected })
            }
            _ => Ok(Self::KeptTable),
        }
    }
}

/// The aligned owner after an inserted, deleted or moved one at `position`
/// that the resolution removing its paragraph mark keeps. A table on the same
/// side is crossed, because that resolution removes every row of it.
///
/// When that owner is a paragraph or a table, the paragraph at `position`
/// carries its own change: removing its mark merges it into that paragraph,
/// or, before a table, removes the paragraph, which has no content left.
/// Otherwise its mark ends the story, and [`final_paragraph`] applies.
pub(super) fn next_kept_owner(
    aligned: &[(Option<usize>, Option<usize>)],
    position: usize,
    is_table: impl Fn((Option<usize>, Option<usize>)) -> bool,
) -> Option<(Option<usize>, Option<usize>)> {
    let (left, right) = aligned[position];
    aligned[position + 1..]
        .iter()
        .copied()
        .find(|&(next_left, next_right)| {
            next_left.is_some() != left.is_some()
                || next_right.is_some() != right.is_some()
                || !is_table((next_left, next_right))
        })
}

/// Whether accepting, rather than rejecting, removes content marked `kind`.
fn accept_removes(kind: &str) -> bool {
    matches!(kind, "del" | "moveFrom")
}

/// The first and last positions of the paragraphs in the run of owners
/// that one side alone holds (`one_sided` gives the owner) and that ends
/// the body. The tables and block controls between them are crossed: the mark before the
/// first paragraph carries the change of the last one, whose own mark ends
/// the story.
pub(super) fn trailing_paragraphs<'d>(
    aligned: &[(Option<usize>, Option<usize>)],
    one_sided: impl Fn((Option<usize>, Option<usize>)) -> Option<&'d BodyContent>,
) -> Option<(usize, usize)> {
    let paragraphs = aligned
        .iter()
        .enumerate()
        .rev()
        .map_while(|(position, &pair)| {
            one_sided(pair)
                .filter(|content| {
                    matches!(
                        content,
                        BodyContent::Paragraph(_)
                            | BodyContent::Table(_)
                            | BodyContent::ContentControl(_)
                    )
                })
                .map(|content| (position, content))
        })
        .filter(|(_, content)| matches!(content, BodyContent::Paragraph(_)))
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    paragraphs.last().copied().zip(paragraphs.first().copied())
}

/// The position in `output` of the owner whose paragraph mark
/// [`mark_previous_paragraph`] marks for a final `kind` change.
pub(super) fn previous_mark_position(
    output: &[(EmittedOwner<'_>, String)],
    kind: &str,
) -> Option<usize> {
    output.iter().rposition(|(owner, _)| match owner {
        EmittedOwner::MarkedTable(marked) => accept_removes(marked) != accept_removes(kind),
        EmittedOwner::Whitespace => false,
        EmittedOwner::Paragraph { .. }
        | EmittedOwner::KeptTable
        | EmittedOwner::KeptControl { .. }
        | EmittedOwner::Other => true,
    })
}

/// Whether the owner a final `kind` change would mark is a table this
/// change's resolution keeps: one both versions hold, or one the other
/// resolution removes (a table this resolution removes is crossed before
/// it). The final paragraph then carries its own change.
pub(super) fn after_kept_table(
    output: &[(EmittedOwner<'_>, String)],
    kind: &str,
    location: &str,
) -> bool {
    // Only the body may end with a table; every other story ends with a
    // paragraph.
    location == "body"
        && matches!(
            previous_mark_position(output, kind).map(|position| &output[position].0),
            Some(EmittedOwner::KeptTable | EmittedOwner::MarkedTable(_))
        )
}

/// Mark the paragraph before a final inserted or deleted paragraph, and
/// return its properties on the side that removes that mark, which
/// [`final_paragraph`] gives the paragraph the two merge into.
pub(super) fn mark_previous_paragraph<'a>(
    output: &mut [(EmittedOwner<'a>, String)],
    kind: &str,
    bindings: &[(String, String)],
    metadata: &mut Metadata<'_>,
) -> Result<PropertySide<'a>> {
    let Some((accepted, rejected, control, paragraph)) =
        previous_mark_position(output, kind).and_then(|position| mark_owner(&mut output[position]))
    else {
        return Err(Error::Other(
            "comparison needs an adjacent paragraph for a final paragraph change".to_owned(),
        ));
    };
    let marker = metadata
        .ids
        .marker(kind, metadata.author, metadata.timestamp)?;
    let scoped = source_fragment(paragraph, bindings)?;
    *paragraph = marked_owner_xml(&scoped, control, &marker, &metadata.ids)?;
    Ok(if kind == "del" {
        accepted.clone()
    } else {
        rejected.clone()
    })
}

/// The properties of an emitted owner whose paragraph mark can carry a
/// final paragraph change, after acceptance and after rejection, whether it
/// is a block control (the mark is its last paragraph's), and its XML.
#[allow(clippy::type_complexity)]
fn mark_owner<'a, 'o>(
    (owner, xml): &'o mut (EmittedOwner<'a>, String),
) -> Option<(
    &'o PropertySide<'a>,
    &'o PropertySide<'a>,
    bool,
    &'o mut String,
)> {
    match owner {
        EmittedOwner::Paragraph { accepted, rejected } => Some((accepted, rejected, false, xml)),
        EmittedOwner::KeptControl { accepted, rejected } => Some((accepted, rejected, true, xml)),
        _ => None,
    }
}

/// `owner` with `marker` on its paragraph mark: its own, or for a block
/// `control` the mark of its last paragraph.
fn marked_owner_xml(owner: &str, control: bool, marker: &str, ids: &IdAllocator) -> Result<String> {
    if !control {
        return marked_paragraph_xml(owner, marker, ids);
    }
    let refusal = || {
        Error::Other("comparison could not find the last paragraph of a content control".to_owned())
    };
    let content_span = direct_word_element_spans(owner, "sdtContent")?
        .into_iter()
        .next()
        .ok_or_else(refusal)?;
    // Marked in place: the content keeps the namespaces the control's root
    // declares.
    let content = &owner[content_span.clone()];
    let other_end = ["tbl", "sdt"]
        .into_iter()
        .map(|local| direct_word_element_spans(content, local))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .map(|span| span.end)
        .max()
        .unwrap_or(0);
    let last = direct_word_element_spans(content, "p")?
        .pop()
        .filter(|span| span.start >= other_end)
        .ok_or_else(refusal)?;
    let marked = marked_paragraph_xml(&content[last.clone()], marker, ids)?;
    let mut updated = owner.to_owned();
    updated.replace_range(
        content_span.start + last.start..content_span.start + last.end,
        &marked,
    );
    Ok(updated)
}

pub(super) fn marked_paragraph_xml(
    paragraph: &str,
    marker: &str,
    ids: &IdAllocator,
) -> Result<String> {
    let paragraph_properties = direct_word_element_spans(paragraph, "pPr")?;
    if let Some(properties_span) = paragraph_properties.first() {
        let properties = source_child(paragraph, properties_span.clone())?;
        let updated = marked_paragraph_properties_xml(&properties, marker, ids)?;
        let mut marked = paragraph.to_owned();
        marked.replace_range(properties_span.clone(), &updated);
        return Ok(marked);
    }
    let properties = ids.generated(format!("<w:pPr><w:rPr>{marker}</w:rPr></w:pPr>"))?;
    insert_first_child(paragraph, &properties)
}

/// Put a paragraph-mark revision marker into serialized `w:pPr` in schema
/// order: first in `w:rPr`, and a new `w:rPr` before `w:sectPr` and
/// `w:pPrChange`.
pub(super) fn marked_paragraph_properties_xml(
    properties: &str,
    marker: &str,
    ids: &IdAllocator,
) -> Result<String> {
    if let Some(run_span) = direct_word_element_spans(properties, "rPr")?.first() {
        let run_properties = source_child(properties, run_span.clone())?;
        let updated_run = with_mark_revision(&run_properties, marker)?;
        let mut updated = properties.to_owned();
        updated.replace_range(run_span.clone(), &updated_run);
        return Ok(updated);
    }
    let run_properties = ids.generated(format!("<w:rPr>{marker}</w:rPr>"))?;
    let later = direct_word_element_spans(properties, "sectPr")?
        .into_iter()
        .chain(direct_word_element_spans(properties, "pPrChange")?)
        .map(|span| span.start)
        .min();
    if let Some(start) = later {
        let mut updated = properties.to_owned();
        updated.insert_str(start, &run_properties);
        return Ok(updated);
    }
    append_word_child(properties, "pPr", &run_properties)
}

/// Emit a paragraph of the run that ends the body, with `trailing` the
/// properties [`final_paragraph`] merges, whether a kept table precedes the
/// run, and whether the run paragraph emitted last is moved.
///
/// The first paragraph marks the paragraph before the run, unless a kept
/// table precedes it: with its own change when it is the only one, else
/// with a mark of the run's resolution under a fresh id, so that resolving
/// one move of the run alone merges nothing else. Each paragraph but the
/// last carries its own change. The last keeps its mark as the story
/// terminator; when it is moved, its move also goes on the mark of the
/// paragraph before it in the run, which resolving that move alone merges
/// into it, unless that paragraph is moved too: a mark holds one move
/// marker of a kind, and resolving the last move alone then leaves its
/// paragraph empty.
pub(super) fn trailing_paragraph<'a>(
    output: &mut Vec<(EmittedOwner<'a>, String)>,
    run: TrailingRun<'a, '_>,
    trailing: &mut (PropertySide<'a>, bool, bool),
    refuse_held_mark: &dyn Fn(Option<usize>) -> Result<()>,
    bindings: &[(String, String)],
    metadata: &mut Metadata<'_>,
) -> Result<()> {
    let (merged, after_table, previous_moved) = (&mut trailing.0, &mut trailing.1, &mut trailing.2);
    if run.first {
        let (kind, move_id) = if run.single {
            (run.kind, run.move_id)
        } else if accept_removes(run.kind) {
            ("del", None)
        } else {
            ("ins", None)
        };
        *after_table = after_kept_table(output, kind, run.location);
        if !*after_table {
            refuse_held_mark(previous_mark_position(output, kind))?;
            *merged = mark_before_final_paragraph(output, kind, move_id, bindings, metadata)?;
        }
    }
    if run.next_takes_mark || *after_table {
        output.push(unmatched_body_owner(
            run.content,
            run.kind,
            run.move_id,
            run.carried,
            metadata,
        )?);
        *previous_moved = run.move_id.is_some();
        return Ok(());
    }
    if let Some(id) = run.move_id
        && !run.single
        && !*previous_moved
    {
        mark_previous_paragraph_with_id(output, run.kind, id, bindings, metadata)?;
    }
    output.push(final_paragraph(
        run.paragraph,
        run.kind,
        run.move_id,
        run.carried,
        merged.clone(),
        metadata,
    )?);
    Ok(())
}

/// A body owner one side alone holds, written with its own `kind` marks:
/// moved under `move_id`, deleted or inserted, `carried` or not (see
/// [`deleted_paragraph`]).
pub(super) fn unmatched_body_owner<'a>(
    content: &'a BodyContent,
    kind: &'static str,
    move_id: Option<i32>,
    carried: bool,
    metadata: &mut Metadata<'_>,
) -> Result<(EmittedOwner<'a>, String)> {
    let xml = match (move_id, kind) {
        (Some(id), _) => moved_body_content(content, kind, id, metadata)?,
        (None, "del") => deleted_body_content(content, carried, metadata)?,
        (None, _) => inserted_body_content(content, carried, metadata)?,
    };
    Ok((
        EmittedOwner::of(content, content, Some(kind), metadata)?,
        xml,
    ))
}

/// Mark the paragraph before a final `kind` paragraph, with the id of its
/// move pair when it is moved, and return what [`final_paragraph`] merges.
pub(super) fn mark_before_final_paragraph<'a>(
    output: &mut [(EmittedOwner<'a>, String)],
    kind: &str,
    move_id: Option<i32>,
    bindings: &[(String, String)],
    metadata: &mut Metadata<'_>,
) -> Result<PropertySide<'a>> {
    match move_id {
        Some(id) => mark_previous_paragraph_with_id(output, kind, id, bindings, metadata),
        None => mark_previous_paragraph(output, kind, bindings, metadata),
    }
}

/// [`mark_previous_paragraph`] for a final moved paragraph, whose move
/// pair shares `id`.
fn mark_previous_paragraph_with_id<'a>(
    output: &mut [(EmittedOwner<'a>, String)],
    kind: &str,
    id: i32,
    bindings: &[(String, String)],
    metadata: &Metadata<'_>,
) -> Result<PropertySide<'a>> {
    let Some((accepted, rejected, control, paragraph)) =
        previous_mark_position(output, kind).and_then(|position| mark_owner(&mut output[position]))
    else {
        return Err(Error::Other(
            "comparison needs an adjacent paragraph for a final paragraph move".to_owned(),
        ));
    };
    let marker = metadata.ids.generated(IdAllocator::marker_with_id(
        kind,
        metadata.author,
        metadata.timestamp,
        id,
    ))?;
    let scoped = source_fragment(paragraph, bindings)?;
    *paragraph = marked_owner_xml(&scoped, control, &marker, &metadata.ids)?;
    Ok(if kind == "moveFrom" {
        accepted.clone()
    } else {
        rejected.clone()
    })
}

/// A final inserted, deleted or moved paragraph. Its own mark stays as the
/// story terminator, and the mark of the paragraph before it carries the
/// change. A move wraps the runs with the `move_id` of its pair.
///
/// Removing that mark merges the paragraph before into this one, and the
/// merged paragraph keeps these properties. `merged` holds the properties the
/// paragraph before has on that side, so this paragraph carries the accepted
/// properties with a `w:pPrChange` and a mark `w:rPrChange` that hold the
/// rejected ones, and both resolutions keep each paragraph's properties.
///
/// Its simple fields are written as for [`deleted_paragraph`], unless it is
/// moved or `carried`.
pub(super) fn final_paragraph<'a>(
    paragraph: &'a CT_P,
    kind: &str,
    move_id: Option<i32>,
    carried: bool,
    merged: PropertySide<'a>,
    metadata: &mut Metadata<'_>,
) -> Result<(EmittedOwner<'a>, String)> {
    let own = PropertySide::of(paragraph, metadata)?;
    let (accepted, rejected) = if matches!(kind, "del" | "moveFrom") {
        (merged, own.clone())
    } else {
        (own.clone(), merged)
    };
    let (mut output, closing) = paragraph_shell(metadata.paragraph_source(paragraph))?;
    output.push_str(&final_paragraph_properties_xml(
        &own, &accepted, &rejected, metadata,
    )?);
    let (mut children, mut published) = paragraph_children_xml(
        paragraph,
        kind == "del",
        move_id.is_none() && !carried,
        metadata,
    )?;
    if move_id.is_none() {
        place_shared_bookmarks(&mut children, &mut published, kind == "del", metadata)?;
    }
    output.push_str(&crate::revision::tracked_paragraph_children(
        &children,
        kind == "del",
        false,
        &published,
        &mut |content| match move_id {
            Some(id) => metadata.ids.generated(IdAllocator::revision_with_id(
                kind,
                metadata.author,
                metadata.timestamp,
                content,
                id,
            )),
            None => metadata
                .ids
                .revision(kind, metadata.author, metadata.timestamp, content),
        },
    )?);
    output.push_str(&closing);
    Ok((
        EmittedOwner::Paragraph { accepted, rejected },
        metadata.ids.generated(output)?,
    ))
}
