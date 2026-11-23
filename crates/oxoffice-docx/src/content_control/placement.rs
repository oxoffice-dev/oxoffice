//! What a parent can hold: parent slots, the payload checks of an unwrap, a move or a clone, and cells that must keep a paragraph.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum ParentSlot {
    Block,
    Table,
    Row,
    Inline,
    Other,
}

pub(super) fn parent_can_hold(xml: &[u8], sdt_range: &Range<usize>, inner: &[u8]) -> Result<bool> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, sdt_range)?;
    inner_matches_slot(parent_slot(&elems, sdt.id), inner)
}

pub(super) fn parent_slot(elems: &[XmlElem], id: usize) -> ParentSlot {
    let Some(elem) = elems.iter().find(|elem| elem.id == id) else {
        return ParentSlot::Other;
    };
    let Some(parent_id) = elem.parent else {
        return ParentSlot::Other;
    };
    let Some(parent) = elems.iter().find(|elem| elem.id == parent_id) else {
        return ParentSlot::Other;
    };
    element_slot(elems, parent)
}

pub(super) fn control_parent_is_block(xml: &[u8], range: &Range<usize>) -> Result<bool> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, range)?;
    Ok(matches!(parent_slot(&elems, sdt.id), ParentSlot::Block))
}

fn wrap_word_root(inner: &[u8]) -> Vec<u8> {
    let mut wrapped = Vec::with_capacity(inner.len() + W_NS.len() + 32);
    wrapped.extend_from_slice(b"<w:root xmlns:w=\"");
    wrapped.extend_from_slice(W_NS.as_bytes());
    wrapped.extend_from_slice(b"\">");
    wrapped.extend_from_slice(inner);
    wrapped.extend_from_slice(b"</w:root>");
    wrapped
}

fn inner_matches_slot(slot: ParentSlot, inner: &[u8]) -> Result<bool> {
    if matches!(slot, ParentSlot::Other) {
        return Ok(false);
    }
    if payload_has_nonspace_text(inner)? {
        return Ok(false);
    }
    let wrapped = wrap_word_root(inner);
    let elems = index_elements(&wrapped)?;
    let Some(root) = elems.iter().find(|elem| elem.parent.is_none()) else {
        return Ok(true);
    };
    transparent_children_match(slot, &wrapped, &elems, root.id)
}

pub(super) fn transparent_children_match(
    slot: ParentSlot,
    xml: &[u8],
    elems: &[XmlElem],
    parent_id: usize,
) -> Result<bool> {
    let custom_parent = elems
        .iter()
        .any(|elem| elem.id == parent_id && elem.word && elem.local == b"customXml");
    for child in elems.iter().filter(|elem| elem.parent == Some(parent_id)) {
        if custom_parent && child.word && child.local == b"customXmlPr" {
            continue;
        }
        if child.word && child.local == b"customXml" {
            if let Some(end) = child.inner_end
                && (payload_has_nonspace_text(&xml[child.open_end..end])?
                    || !transparent_children_match(slot, xml, elems, child.id)?)
            {
                return Ok(false);
            }
        } else if child.word && child.local == b"sdt" {
            if !sdt_payload_matches(slot, xml, elems, child)? {
                return Ok(false);
            }
        } else if !child_allowed(slot, child.word, &child.local) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn sdt_payload_matches(
    slot: ParentSlot,
    xml: &[u8],
    elems: &[XmlElem],
    sdt: &XmlElem,
) -> Result<bool> {
    if (sdt_is_repeating_section(elems, sdt) || sdt_is_repeating_item(elems, sdt))
        && !repeating_slot_allowed(slot)
    {
        return Ok(false);
    }
    let mut contents = Vec::new();
    for child in elems.iter().filter(|elem| elem.parent == Some(sdt.id)) {
        match (child.word, child.local.as_slice()) {
            (true, b"sdtPr" | b"sdtEndPr") => {}
            (true, b"sdtContent") => contents.push(child),
            _ => return Ok(false),
        }
    }
    for content in contents {
        let Some(inner_end) = content.inner_end else {
            continue;
        };
        let inner = &xml[content.open_end..inner_end];
        if payload_has_nonspace_text(inner)?
            || !transparent_children_match(slot, xml, elems, content.id)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn payload_has_nonspace_text(inner: &[u8]) -> Result<bool> {
    let wrapped = wrap_word_root(inner);
    let mut reader = Reader::from_reader(wrapped.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                if depth == 0 {
                    return Err(Error::Other(
                        "content control unwrap content is invalid".to_owned(),
                    ));
                }
                depth -= 1;
            }
            Ok(Event::Text(text)) if depth == 1 && !xml_space(text.as_ref()) => return Ok(true),
            Ok(Event::CData(text)) if depth == 1 && !xml_space(text.as_ref()) => return Ok(true),
            Ok(Event::GeneralRef(reference)) if depth == 1 => {
                let character = decode_one_xml_reference(&reference).map_err(|detail| {
                    Error::Other(format!(
                        "content control unwrap content is invalid: {detail}"
                    ))
                })?;
                if !matches!(character, ' ' | '\t' | '\n' | '\r') {
                    return Ok(true);
                }
            }
            Ok(Event::Eof) => return Ok(false),
            Ok(_) => {}
            Err(error) => {
                return Err(Error::Other(format!(
                    "content control unwrap content is invalid: {error}"
                )));
            }
        }
        buffer.clear();
    }
}

fn child_allowed(slot: ParentSlot, word: bool, local: &[u8]) -> bool {
    if !word {
        return false;
    }
    match slot {
        ParentSlot::Block => matches!(
            local,
            b"p" | b"tbl"
                | b"sdt"
                | b"bookmarkStart"
                | b"bookmarkEnd"
                | b"proofErr"
                | b"commentRangeStart"
                | b"commentRangeEnd"
                | b"permStart"
                | b"permEnd"
                | b"customXml"
        ),
        ParentSlot::Table => {
            matches!(
                local,
                b"tr" | b"sdt" | b"bookmarkStart" | b"bookmarkEnd" | b"proofErr" | b"customXml"
            )
        }
        ParentSlot::Row => {
            matches!(
                local,
                b"tc" | b"sdt" | b"bookmarkStart" | b"bookmarkEnd" | b"proofErr" | b"customXml"
            )
        }
        ParentSlot::Inline => matches!(
            local,
            b"r" | b"sdt"
                | b"hyperlink"
                | b"fldSimple"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
                | b"bookmarkStart"
                | b"bookmarkEnd"
                | b"commentRangeStart"
                | b"commentRangeEnd"
                | b"proofErr"
                | b"customXml"
        ),
        ParentSlot::Other => false,
    }
}

pub(super) fn xml_space(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
}

pub(super) fn destination_accepts_control(
    parent_xml: &[u8],
    parent_range: &Range<usize>,
    source_xml: &[u8],
    source_range: &Range<usize>,
) -> Result<()> {
    let elems = index_elements(parent_xml)?;
    let parent = element_at(&elems, parent_range)?;
    let in_section = ancestor_repeating_section(&elems, parent.id).is_some();
    if control_matches_context(
        element_slot(&elems, parent),
        in_section,
        source_xml,
        source_range,
    )? {
        Ok(())
    } else {
        Err(Error::Other(
            "content control placement does not match its parent".to_owned(),
        ))
    }
}

fn element_slot(elems: &[XmlElem], elem: &XmlElem) -> ParentSlot {
    if !elem.word {
        return ParentSlot::Other;
    }
    match elem.local.as_slice() {
        b"sdt" | b"sdtContent" | b"customXml" => parent_slot(elems, elem.id),
        b"p" | b"hyperlink" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo" => {
            ParentSlot::Inline
        }
        b"tbl" => ParentSlot::Table,
        b"tr" => ParentSlot::Row,
        b"body" | b"hdr" | b"ftr" | b"footnote" | b"endnote" | b"comment" | b"txbxContent"
        | b"tc" => ParentSlot::Block,
        _ => ParentSlot::Other,
    }
}

pub(super) fn nested_destination(
    document: &Document,
    destination: &ContentLocation,
) -> Result<PartRange> {
    if !Document::location_is_end(destination)
        && destination.item_kind() == StoryItemKind::Paragraph
    {
        return paragraph_parent_range(document, destination);
    }
    if !Document::location_is_end(destination) && destination.item_kind() == StoryItemKind::Table {
        return match destination.index_path().len() {
            1 => table_parent_range(document, destination),
            2 => row_parent_range(document, destination),
            _ => Err(Error::Other(
                "content control placement does not match its parent".to_owned(),
            )),
        };
    }
    Err(Error::Other(
        "content control placement does not match its parent".to_owned(),
    ))
}

pub(super) fn ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

pub(super) fn shift_after_removal(dest: &Range<usize>, removed: &Range<usize>) -> Range<usize> {
    if dest.start >= removed.end {
        let shift = removed.len();
        dest.start - shift..dest.end - shift
    } else {
        dest.clone()
    }
}

pub(super) fn ensure_cell_paragraph(
    document: &Document,
    story: &crate::StoryId,
    edited: &[u8],
    original_len: usize,
) -> Result<()> {
    if story.kind() != StoryKind::TableCell {
        return Ok(());
    }
    let delta = edited.len() as isize - original_len as isize;
    let (_xml, span) = document.story_owner_span(story)?;
    let end = usize::try_from(span.end as isize + delta)
        .map_err(|_| Error::Other("content control cell span is invalid".to_owned()))?;
    if span.start > end || end > edited.len() {
        return Err(Error::Other(
            "content control cell span is invalid".to_owned(),
        ));
    }
    if !word_paragraph_in_span(edited, span.start..end)? {
        return Err(Error::Other(
            "cannot remove the final required table-cell paragraph".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn refuse_cell_without_paragraph(
    document: &Document,
    story: &crate::StoryId,
    xml: &[u8],
    range: &Range<usize>,
    replacement: &[u8],
) -> Result<()> {
    if story.kind() != StoryKind::TableCell {
        return Ok(());
    }
    let mut edited = xml.to_vec();
    edited.splice(range.clone(), replacement.iter().copied());
    ensure_cell_paragraph(document, story, &edited, xml.len())
}

fn word_paragraph_in_span(xml: &[u8], span: Range<usize>) -> Result<bool> {
    let elems = index_elements(xml)?;
    Ok(elems.iter().any(|elem| {
        elem.word
            && elem.local == b"p"
            && elem.range.start >= span.start
            && elem.range.end <= span.end
    }))
}
