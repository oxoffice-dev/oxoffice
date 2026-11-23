//! Editing one property of a control in place: tag, alias, lock, appearance, placeholder and data binding, in schema order.

use super::*;

impl Document {
    pub(super) fn edit_control_property(
        &mut self,
        location: &ContentLocation,
        edit: PropertyEdit,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (part, mut xml, range, _) = control_item(&candidate, location)?;
        let updated = edit_sdt_property(&xml, &range, edit)?;
        let _ = xml.splice(range, updated);
        candidate.write_story_part(&part, xml)?;
        self.publish_candidate(candidate)
    }
}

pub(super) enum PropertyEdit {
    Tag(Option<String>),
    Alias(Option<String>),
    Lock(Option<ContentControlLock>),
    Appearance(Option<ContentControlAppearance>),
    Placeholder(Option<bool>),
    DataBinding {
        prefix_mappings: Option<String>,
        xpath: String,
        store_item_id: String,
    },
}

pub(super) fn edit_sdt_property(
    xml: &[u8],
    sdt_range: &Range<usize>,
    edit: PropertyEdit,
) -> Result<Vec<u8>> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, sdt_range)?;
    let sdt_pr = elems
        .iter()
        .find(|elem| elem.parent == Some(sdt.id) && elem.word && elem.local == b"sdtPr")
        .ok_or_else(|| Error::Other("content control has no properties".to_owned()))?;
    if let Some(target) = elems
        .iter()
        .find(|elem| elem.parent == Some(sdt_pr.id) && property_matches(elem, &edit))
    {
        let replacement = property_bytes(xml, target, &edit)?;
        return splice_absolute(xml, sdt_range, &target.range, &replacement);
    }
    if property_clears(&edit) {
        return Ok(xml[sdt_range.clone()].to_vec());
    }
    let markup = property_bytes_new(xml, sdt_pr, &edit)?;
    if sdt_pr.inner_end.is_none() {
        let expanded = expand_empty(&xml[sdt_pr.range.clone()], &markup)?;
        return splice_absolute(xml, sdt_range, &sdt_pr.range, &expanded);
    }
    let rank = property_insert_rank(&edit);
    let insert_at = elems
        .iter()
        .filter(|elem| elem.parent == Some(sdt_pr.id))
        .find(|elem| property_rank(elem).is_some_and(|child| child > rank))
        .map(|elem| elem.range.start)
        .unwrap_or(sdt_pr.inner_end.unwrap_or(sdt_pr.range.end));
    splice_absolute(xml, sdt_range, &(insert_at..insert_at), &markup)
}

pub(super) fn splice_absolute(
    xml: &[u8],
    outer: &Range<usize>,
    target: &Range<usize>,
    replacement: &[u8],
) -> Result<Vec<u8>> {
    if target.start < outer.start || target.end > outer.end {
        return Err(Error::Other(
            "content control property is outside its shell".to_owned(),
        ));
    }
    let mut updated = xml[outer.clone()].to_vec();
    let local = (target.start - outer.start)..(target.end - outer.start);
    updated.splice(local, replacement.iter().copied());
    Ok(updated)
}

fn property_matches(elem: &XmlElem, edit: &PropertyEdit) -> bool {
    match edit {
        PropertyEdit::Tag(_) => elem.word && elem.local == b"tag",
        PropertyEdit::Alias(_) => elem.word && elem.local == b"alias",
        PropertyEdit::Lock(_) => elem.word && elem.local == b"lock",
        PropertyEdit::Appearance(_) => elem.w15 && elem.local == b"appearance",
        PropertyEdit::Placeholder(_) => elem.word && elem.local == b"showingPlcHdr",
        PropertyEdit::DataBinding { .. } => elem.word && elem.local == b"dataBinding",
    }
}

fn property_clears(edit: &PropertyEdit) -> bool {
    matches!(
        edit,
        PropertyEdit::Tag(None)
            | PropertyEdit::Alias(None)
            | PropertyEdit::Lock(None)
            | PropertyEdit::Appearance(None)
            | PropertyEdit::Placeholder(None)
            | PropertyEdit::Placeholder(Some(false))
    )
}

fn property_insert_rank(edit: &PropertyEdit) -> u8 {
    match edit {
        PropertyEdit::Alias(_) => 1,
        PropertyEdit::Tag(_) => 2,
        PropertyEdit::Lock(_) => 4,
        PropertyEdit::Placeholder(_) => 7,
        PropertyEdit::DataBinding { .. } => 8,
        PropertyEdit::Appearance(_) => 21,
    }
}

pub(super) fn property_rank(elem: &XmlElem) -> Option<u8> {
    if elem.word {
        return match elem.local.as_slice() {
            b"rPr" => Some(0),
            b"alias" => Some(1),
            b"tag" => Some(2),
            b"id" => Some(3),
            b"lock" => Some(4),
            b"placeholder" => Some(5),
            b"temporary" => Some(6),
            b"showingPlcHdr" => Some(7),
            b"dataBinding" => Some(8),
            b"label" | b"tabIndex" => Some(9),
            b"richText" | b"text" | b"date" | b"picture" | b"comboBox" | b"dropDownList"
            | b"citation" | b"group" | b"bibliography" | b"equation" | b"docPartList"
            | b"docPartObj" => Some(20),
            _ => None,
        };
    }
    if elem.w15 {
        return match elem.local.as_slice() {
            b"appearance" => Some(21),
            b"repeatingSection" | b"repeatingSectionItem" => Some(20),
            _ => None,
        };
    }
    if elem.local.as_slice() == b"checkbox" {
        return Some(20);
    }
    None
}

fn property_bytes(xml: &[u8], target: &XmlElem, edit: &PropertyEdit) -> Result<Vec<u8>> {
    if property_clears(edit) {
        return Ok(Vec::new());
    }
    if matches!(edit, PropertyEdit::Appearance(_)) {
        return Ok(appearance_markup(edit)?.into_bytes());
    }
    let prefix = name_prefix(element_qname(&xml[target.range.clone()])?);
    Ok(property_markup(prefix, edit)?.into_bytes())
}

fn property_bytes_new(xml: &[u8], sdt_pr: &XmlElem, edit: &PropertyEdit) -> Result<Vec<u8>> {
    if matches!(edit, PropertyEdit::Appearance(_)) {
        return Ok(appearance_markup(edit)?.into_bytes());
    }
    let prefix = name_prefix(element_qname(&xml[sdt_pr.range.clone()])?);
    Ok(property_markup(prefix, edit)?.into_bytes())
}

pub(super) fn name_prefix(qname: &str) -> &str {
    match qname.split_once(':') {
        Some((prefix, _)) => prefix,
        None => "",
    }
}

fn property_markup(prefix: &str, edit: &PropertyEdit) -> Result<String> {
    match edit {
        PropertyEdit::Tag(Some(value)) => Ok(prefixed_val(prefix, "tag", value)),
        PropertyEdit::Alias(Some(value)) => Ok(prefixed_val(prefix, "alias", value)),
        PropertyEdit::Lock(Some(lock)) => Ok(prefixed_val(prefix, "lock", lock.as_str())),
        PropertyEdit::Placeholder(Some(true)) => Ok(prefixed_marker(prefix, "showingPlcHdr")),
        PropertyEdit::Appearance(_) => appearance_markup(edit),
        PropertyEdit::DataBinding {
            prefix_mappings,
            xpath,
            store_item_id,
        } => Ok(data_binding_markup(
            prefix,
            prefix_mappings.as_deref(),
            xpath,
            store_item_id,
        )),
        _ => Err(Error::Other(
            "content control property value is missing".to_owned(),
        )),
    }
}

fn data_binding_markup(
    prefix: &str,
    mappings: Option<&str>,
    xpath: &str,
    store_item_id: &str,
) -> String {
    let qualify = |local: &str| {
        if prefix.is_empty() {
            local.to_owned()
        } else {
            format!("{prefix}:{local}")
        }
    };
    let mut attrs = String::new();
    if let Some(mappings) = mappings.filter(|value| !value.is_empty()) {
        attrs.push_str(&format!(
            " {}=\"{}\"",
            qualify("prefixMappings"),
            xml_attr(mappings)
        ));
    }
    attrs.push_str(&format!(" {}=\"{}\"", qualify("xpath"), xml_attr(xpath)));
    attrs.push_str(&format!(
        " {}=\"{}\"",
        qualify("storeItemID"),
        xml_attr(store_item_id)
    ));
    format!("<{}{attrs}/>", qualify("dataBinding"))
}

fn appearance_markup(edit: &PropertyEdit) -> Result<String> {
    let PropertyEdit::Appearance(Some(appearance)) = edit else {
        return Err(Error::Other(
            "content control appearance value is missing".to_owned(),
        ));
    };
    Ok(format!(
        "<w15:appearance w15:val=\"{}\" xmlns:w15=\"{W15_NS}\"/>",
        xml_attr(appearance.as_str())
    ))
}

pub(super) fn prefixed_val(prefix: &str, local: &str, value: &str) -> String {
    if prefix.is_empty() {
        format!("<{local} val=\"{}\"/>", xml_attr(value))
    } else {
        format!("<{prefix}:{local} {prefix}:val=\"{}\"/>", xml_attr(value))
    }
}

fn prefixed_marker(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        format!("<{local}/>")
    } else {
        format!("<{prefix}:{local}/>")
    }
}
