//! Repeating sections and their items: wrapping content into an item, finding sections and items, the checks that keep them whole, fresh ids for a copy.

use super::*;

pub(super) enum RepeatingContent {
    Empty,
    NotBlock,
    AlreadyItem,
    Wrap,
}

pub(super) fn repeating_content(xml: &[u8], sdt_range: &Range<usize>) -> Result<RepeatingContent> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, sdt_range)?;
    let Some(content) = sdt_content_elem(&elems, sdt) else {
        return Ok(RepeatingContent::Empty);
    };
    let Some(inner_end) = content.inner_end else {
        return Ok(RepeatingContent::Empty);
    };
    let children: Vec<&XmlElem> = elems
        .iter()
        .filter(|elem| elem.parent == Some(content.id))
        .collect();
    let inner = &xml[content.open_end..inner_end];
    if inner_has_nonspace_text(inner)? {
        return Ok(RepeatingContent::NotBlock);
    }
    if children.is_empty() {
        return Ok(RepeatingContent::Empty);
    }
    if children.len() == 1
        && children[0].word
        && children[0].local == b"sdt"
        && sdt_is_repeating_item(&elems, children[0])
    {
        return Ok(RepeatingContent::AlreadyItem);
    }
    if !transparent_children_match(ParentSlot::Block, xml, &elems, content.id)
        .map_err(|error| Error::Other(format!("repeating section content is invalid: {error}")))?
    {
        return Ok(RepeatingContent::NotBlock);
    }
    Ok(RepeatingContent::Wrap)
}

fn inner_has_nonspace_text(inner: &[u8]) -> Result<bool> {
    let mut wrapped = Vec::new();
    wrapped.extend_from_slice(b"<w:root xmlns:w=\"");
    wrapped.extend_from_slice(W_NS.as_bytes());
    wrapped.extend_from_slice(b"\">");
    wrapped.extend_from_slice(inner);
    wrapped.extend_from_slice(b"</w:root>");
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
                        "repeating section content is not a block sequence".to_owned(),
                    ));
                }
                depth -= 1;
            }
            Ok(Event::Text(text)) if depth == 1 && !xml_space(text.as_ref()) => return Ok(true),
            Ok(Event::CData(text)) if depth == 1 && !xml_space(text.as_ref()) => return Ok(true),
            Ok(Event::GeneralRef(reference)) if depth == 1 => {
                let character = decode_one_xml_reference(&reference).map_err(|detail| {
                    Error::Other(format!("repeating section content is invalid: {detail}"))
                })?;
                if !matches!(character, ' ' | '\t' | '\n' | '\r') {
                    return Ok(true);
                }
            }
            Ok(Event::Eof) => return Ok(false),
            Ok(_) => {}
            Err(error) => {
                return Err(Error::Other(format!(
                    "repeating section content is invalid: {error}"
                )));
            }
        }
        buffer.clear();
    }
}

pub(super) fn wrap_repeating_item(
    xml: &[u8],
    sdt_range: &Range<usize>,
    id: i32,
) -> Result<(Vec<u8>, Range<usize>)> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, sdt_range)?;
    let content = sdt_content_elem(&elems, sdt)
        .ok_or_else(|| Error::Other("repeating section has no item".to_owned()))?;
    let inner_end = content
        .inner_end
        .ok_or_else(|| Error::Other("repeating section has no item".to_owned()))?;
    let inner = &xml[content.open_end..inner_end];
    let inner = std::str::from_utf8(inner)
        .map_err(|error| Error::Other(format!("repeating section content is invalid: {error}")))?;
    let wrapper = format!(
        "<w:sdt><w:sdtPr><w:id w:val=\"{id}\"/><w15:repeatingSectionItem xmlns:w15=\"{W15_NS}\"/></w:sdtPr><w:sdtContent>{inner}</w:sdtContent></w:sdt>"
    );
    let growth = wrapper.len() - (inner_end - content.open_end);
    let mut updated = xml.to_vec();
    updated.splice(content.open_end..inner_end, wrapper.into_bytes());
    Ok((updated, sdt_range.start..sdt_range.end + growth))
}

pub(super) fn sdt_content_elem<'a>(elems: &'a [XmlElem], sdt: &XmlElem) -> Option<&'a XmlElem> {
    elems
        .iter()
        .find(|elem| elem.parent == Some(sdt.id) && elem.word && elem.local == b"sdtContent")
}

fn sdt_has_direct_type(
    elems: &[XmlElem],
    sdt: &XmlElem,
    local: &[u8],
    w15: bool,
    w14: bool,
) -> bool {
    let Some(properties) = elems
        .iter()
        .find(|elem| elem.parent == Some(sdt.id) && elem.word && elem.local == b"sdtPr")
    else {
        return false;
    };
    elems.iter().any(|elem| {
        elem.parent == Some(properties.id)
            && elem.local.as_slice() == local
            && if w15 {
                elem.w15
            } else if w14 {
                elem.w14
            } else {
                elem.word
            }
    })
}

pub(super) fn sdt_is_repeating_section(elems: &[XmlElem], sdt: &XmlElem) -> bool {
    sdt_has_direct_type(elems, sdt, b"repeatingSection", true, false)
}

pub(super) fn sdt_is_repeating_item(elems: &[XmlElem], sdt: &XmlElem) -> bool {
    sdt_has_direct_type(elems, sdt, b"repeatingSectionItem", true, false)
}

pub(super) fn ancestor_repeating_section(elems: &[XmlElem], sdt_id: usize) -> Option<&XmlElem> {
    let mut current = elems.iter().find(|elem| elem.id == sdt_id)?;
    while let Some(parent_id) = current.parent {
        let parent = elems.iter().find(|elem| elem.id == parent_id)?;
        if parent.word && parent.local == b"sdt" && sdt_is_repeating_section(elems, parent) {
            return Some(parent);
        }
        current = parent;
    }
    None
}

fn direct_repeating_items<'a>(elems: &'a [XmlElem], section: &XmlElem) -> Vec<&'a XmlElem> {
    let Some(content) = sdt_content_elem(elems, section) else {
        return Vec::new();
    };
    elems
        .iter()
        .filter(|elem| {
            elem.parent == Some(content.id)
                && elem.word
                && elem.local == b"sdt"
                && sdt_is_repeating_item(elems, elem)
        })
        .collect()
}

pub(super) fn first_repeating_item_range(
    elems: &[XmlElem],
    section: &XmlElem,
) -> Option<Range<usize>> {
    let content = sdt_content_elem(elems, section)?;
    let end = content.inner_end.unwrap_or(content.range.end);
    elems
        .iter()
        .filter(|elem| {
            elem.word
                && elem.local == b"sdt"
                && elem.range.start >= content.open_end
                && elem.range.end <= end
                && sdt_is_repeating_item(elems, elem)
        })
        .min_by_key(|elem| elem.range.start)
        .map(|elem| elem.range.clone())
}

pub(super) fn repeating_slot_allowed(slot: ParentSlot) -> bool {
    matches!(
        slot,
        ParentSlot::Block | ParentSlot::Table | ParentSlot::Row
    )
}

fn direct_item_section<'a>(elems: &'a [XmlElem], item: &XmlElem) -> Option<&'a XmlElem> {
    if !sdt_is_repeating_item(elems, item) {
        return None;
    }
    let section = ancestor_repeating_section(elems, item.id)?;
    direct_repeating_items(elems, section)
        .iter()
        .any(|child| child.id == item.id)
        .then_some(section)
}

fn section_type_can_retire(elems: &[XmlElem], section: &XmlElem) -> bool {
    if !sdt_is_repeating_section(elems, section)
        || ancestor_repeating_section(elems, section.id).is_some()
    {
        return true;
    }
    !elems.iter().any(|item| {
        item.word
            && item.local == b"sdt"
            && section.range.start < item.range.start
            && item.range.end < section.range.end
            && sdt_is_repeating_item(elems, item)
            && ancestor_repeating_section(elems, item.id)
                .is_some_and(|owner| owner.id == section.id)
    })
}

pub(super) fn validate_repeating_type_change(
    xml: &[u8],
    range: &Range<usize>,
    payload: &ContentControlTypePayload,
) -> Result<()> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, range)?;
    let item = matches!(payload, ContentControlTypePayload::RepeatingSectionItem);
    let section = matches!(payload, ContentControlTypePayload::RepeatingSection);
    if item && ancestor_repeating_section(&elems, sdt.id).is_none() {
        return Err(Error::Other(
            "repeating section item requires a repeating section parent".to_owned(),
        ));
    }
    if (item || section) && !repeating_slot_allowed(parent_slot(&elems, sdt.id)) {
        return Err(Error::Other(
            "repeating control requires block, row, or cell placement".to_owned(),
        ));
    }
    if !item && direct_item_section(&elems, sdt).is_some() {
        return Err(Error::Other(
            "repeating section must retain repeating section items".to_owned(),
        ));
    }
    if !section && !section_type_can_retire(&elems, sdt) {
        return Err(Error::Other(
            "cannot remove a repeating section type while its items require it".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn control_matches_context(
    slot: ParentSlot,
    in_section: bool,
    xml: &[u8],
    range: &Range<usize>,
) -> Result<bool> {
    if matches!(slot, ParentSlot::Other) {
        return Ok(false);
    }
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, range)?;
    if !sdt_payload_matches(slot, xml, &elems, sdt)? {
        return Ok(false);
    }
    if in_section {
        return Ok(true);
    }
    Ok(!elems.iter().any(|item| {
        item.word
            && item.local == b"sdt"
            && range.start <= item.range.start
            && item.range.end <= range.end
            && sdt_is_repeating_item(&elems, item)
            && ancestor_repeating_section(&elems, item.id)
                .is_none_or(|owner| owner.range.start < range.start || owner.range.end > range.end)
    }))
}

pub(super) fn story_inside_repeating_section(
    document: &Document,
    story: &crate::StoryId,
) -> Result<bool> {
    let (xml, range) = document.story_owner_span(story)?;
    let elems = index_elements(&xml)?;
    Ok(elems.iter().any(|section| {
        section.word
            && section.local == b"sdt"
            && section.range.start < range.start
            && range.end < section.range.end
            && sdt_is_repeating_section(&elems, section)
    }))
}

pub(super) fn unwrap_keeps_repeating_context(xml: &[u8], range: &Range<usize>) -> Result<bool> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, range)?;
    if !section_type_can_retire(&elems, sdt) {
        return Ok(false);
    }
    let Some(owner) = direct_item_section(&elems, sdt) else {
        return Ok(true);
    };
    let Some(content) = sdt_content_elem(&elems, sdt) else {
        return Ok(direct_repeating_items(&elems, owner).len() > 1);
    };
    let children: Vec<_> = elems
        .iter()
        .filter(|elem| elem.parent == Some(content.id))
        .collect();
    if children
        .iter()
        .any(|child| !child.word || child.local != b"sdt" || !sdt_is_repeating_item(&elems, child))
    {
        return Ok(false);
    }
    Ok(!children.is_empty() || direct_repeating_items(&elems, owner).len() > 1)
}

pub(super) fn last_repeating_item(xml: &[u8], sdt_range: &Range<usize>) -> Result<bool> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, sdt_range)?;
    let Some(section) = direct_item_section(&elems, sdt) else {
        return Ok(false);
    };
    Ok(direct_repeating_items(&elems, section).len() == 1)
}

pub(super) fn freshen_control(document: &mut Document, sdt_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut wrapped = format!(
        "<w:document xmlns:w=\"{W_NS}\" xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\"><w:body>"
    )
    .into_bytes();
    wrapped.extend_from_slice(sdt_bytes);
    wrapped.extend_from_slice(b"</w:body></w:document>");
    let updated = crate::field::freshen_content_fragment_identities(document, &wrapped)?;
    let elems = index_elements(&updated)?;
    let sdt = elems
        .iter()
        .find(|elem| elem.word && elem.local == b"sdt")
        .ok_or_else(|| Error::Other("content control clone did not keep its shell".to_owned()))?;
    Ok(updated[sdt.range.clone()].to_vec())
}
