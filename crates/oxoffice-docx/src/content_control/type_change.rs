//! Changing a control's type: payload checks and markup, picture relationships, and replacing the type element.

use super::*;

const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

pub(super) fn validate_type_payload(payload: &ContentControlTypePayload) -> Result<()> {
    match payload {
        // The control is written without a language (`w:lid`): English names.
        ContentControlTypePayload::Date { format } => check_word_date_format(format, None, None)?,
        ContentControlTypePayload::ComboBox { items }
        | ContentControlTypePayload::DropDownList { items } => validate_list_items(items)?,
        ContentControlTypePayload::Picture { relationship_id } => {
            if relationship_id.is_empty() {
                return Err(Error::Other(
                    "content control picture relationship is missing".to_owned(),
                ));
            }
            oxoffice_core::xml::reject_non_xml_characters(
                "content control picture relationship",
                relationship_id,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_list_items(items: &[(String, String)]) -> Result<()> {
    if items.is_empty() {
        return Err(Error::Other("content control list is empty".to_owned()));
    }
    let mut seen = HashSet::new();
    for (display, value) in items {
        oxoffice_core::xml::reject_non_xml_characters("content control list display", display)?;
        oxoffice_core::xml::reject_non_xml_characters("content control list value", value)?;
        if !seen.insert(value.clone()) {
            return Err(Error::Other(
                "content control list value is repeated".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(super) fn type_markup(payload: &ContentControlTypePayload) -> Result<String> {
    match payload {
        ContentControlTypePayload::RichText => Ok("<w:richText/>".to_owned()),
        ContentControlTypePayload::PlainText => Ok("<w:text/>".to_owned()),
        ContentControlTypePayload::Date { format } => Ok(format!(
            "<w:date><w:dateFormat w:val=\"{}\"/><w:storeMappedDataAs w:val=\"text\"/><w:calendar w:val=\"gregorian\"/></w:date>",
            xml_attr(format)
        )),
        ContentControlTypePayload::CheckBox => Ok(format!(
            "<w14:checkbox xmlns:w14=\"{W14_NS}\"><w14:checked w14:val=\"0\"/><w14:checkedState w14:val=\"2612\" w14:font=\"MS Gothic\"/><w14:uncheckedState w14:val=\"2610\" w14:font=\"MS Gothic\"/></w14:checkbox>"
        )),
        ContentControlTypePayload::ComboBox { items } => Ok(list_markup("w:comboBox", items)),
        ContentControlTypePayload::DropDownList { items } => {
            Ok(list_markup("w:dropDownList", items))
        }
        ContentControlTypePayload::Picture { .. } => Ok("<w:picture/>".to_owned()),
        ContentControlTypePayload::Group => Ok("<w:group/>".to_owned()),
        ContentControlTypePayload::RepeatingSection => {
            Ok(format!("<w15:repeatingSection xmlns:w15=\"{W15_NS}\"/>"))
        }
        ContentControlTypePayload::RepeatingSectionItem => Ok(format!(
            "<w15:repeatingSectionItem xmlns:w15=\"{W15_NS}\"/>"
        )),
    }
}

fn list_markup(name: &str, items: &[(String, String)]) -> String {
    let mut xml = format!("<{name}>");
    for (display, value) in items {
        xml.push_str(&format!(
            "<w:listItem w:displayText=\"{}\" w:value=\"{}\"/>",
            xml_attr(display),
            xml_attr(value)
        ));
    }
    xml.push_str("</");
    xml.push_str(name);
    xml.push('>');
    xml
}

pub(super) fn part_owns_image(document: &Document, part: &str, id: &str) -> bool {
    document.package.get_part_rels(part).is_some_and(|rels| {
        rels.items
            .iter()
            .any(|rel| rel.id == id && rel.rel_type == oxoffice_opc::relationship::rel_types::IMAGE)
    })
}

pub(super) fn content_embeds_relationship(
    xml: &[u8],
    sdt_range: &Range<usize>,
    rel_id: &str,
) -> Result<bool> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, sdt_range)?;
    let Some(content) = sdt_content_elem(&elems, sdt) else {
        return Ok(false);
    };
    let Some(inner_end) = content.inner_end else {
        return Ok(false);
    };
    drawing_embeds(xml, content.open_end..inner_end, rel_id)
}

fn drawing_embeds(xml: &[u8], inner: Range<usize>, rel_id: &str) -> Result<bool> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut drawing_depth = 0i32;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| {
                Error::Other(format!(
                    "content control picture content is invalid: {error}"
                ))
            })?;
        let word = is_word(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        let in_content = before >= inner.start && after <= inner.end;
        match event {
            Event::Start(element) => {
                let drawing = word && element.local_name().as_ref() == b"drawing";
                if in_content && drawing {
                    drawing_depth += 1;
                }
                if in_content && drawing_depth > 0 && embed_matches(&reader, &element, rel_id)? {
                    return Ok(true);
                }
            }
            Event::Empty(element) => {
                let drawing = word && element.local_name().as_ref() == b"drawing";
                if in_content
                    && (drawing_depth > 0 || drawing)
                    && embed_matches(&reader, &element, rel_id)?
                {
                    return Ok(true);
                }
            }
            Event::End(element) => {
                if in_content
                    && word
                    && element.local_name().as_ref() == b"drawing"
                    && drawing_depth > 0
                {
                    drawing_depth -= 1;
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn embed_matches(reader: &NsReader<&[u8]>, element: &BytesStart<'_>, rel_id: &str) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!(
                "content control picture content is invalid: {error}"
            ))
        })?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        let relationships =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == REL_NS.as_bytes());
        // A linked picture names its relationship with `r:link`.
        if relationships && matches!(local.as_ref(), b"embed" | b"link") {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| {
                    Error::Other(format!(
                        "content control picture content is invalid: {error}"
                    ))
                })?;
            if value.as_ref() == rel_id {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn replace_control_type_markup(
    xml: &[u8],
    sdt_range: &Range<usize>,
    markup: &[u8],
) -> Result<Vec<u8>> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, sdt_range)?;
    let sdt_pr = elems
        .iter()
        .find(|elem| elem.parent == Some(sdt.id) && elem.word && elem.local == b"sdtPr")
        .ok_or_else(|| Error::Other("content control has no properties".to_owned()))?;
    let mut type_ranges: Vec<Range<usize>> = elems
        .iter()
        .filter(|elem| elem.parent == Some(sdt_pr.id) && is_type_child(elem))
        .map(|elem| elem.range.clone())
        .collect();
    type_ranges.sort_by_key(|range| range.start);
    if sdt_pr.inner_end.is_none() {
        let expanded = expand_empty(&xml[sdt_pr.range.clone()], markup)?;
        return splice_absolute(xml, sdt_range, &sdt_pr.range, &expanded);
    }
    let insert_at = type_ranges
        .first()
        .map(|range| range.start)
        .unwrap_or_else(|| {
            elems
                .iter()
                .filter(|elem| elem.parent == Some(sdt_pr.id))
                .find(|elem| property_rank(elem).is_some_and(|child| child > 20))
                .map(|elem| elem.range.start)
                .unwrap_or(sdt_pr.inner_end.unwrap_or(sdt_pr.range.end))
        });
    let mut updated = Vec::new();
    let mut cursor = sdt_range.start;
    let mut inserted = false;
    let mut skips = type_ranges.into_iter().peekable();
    while cursor < sdt_range.end {
        if !inserted && cursor == insert_at {
            updated.extend_from_slice(markup);
            inserted = true;
        }
        if skips.peek().is_some_and(|skip| cursor == skip.start) {
            cursor = skips.next().unwrap().end;
            continue;
        }
        updated.push(xml[cursor]);
        cursor += 1;
    }
    if !inserted {
        updated.extend_from_slice(markup);
    }
    Ok(updated)
}

pub(super) fn is_type_child(elem: &XmlElem) -> bool {
    if elem.word {
        return matches!(
            elem.local.as_slice(),
            b"richText"
                | b"text"
                | b"date"
                | b"picture"
                | b"comboBox"
                | b"dropDownList"
                | b"citation"
                | b"group"
                | b"bibliography"
                | b"equation"
                | b"docPartList"
                | b"docPartObj"
        );
    }
    if elem.w15 {
        return matches!(
            elem.local.as_slice(),
            b"repeatingSection" | b"repeatingSectionItem"
        );
    }
    elem.w14 && elem.local.as_slice() == b"checkbox"
}
