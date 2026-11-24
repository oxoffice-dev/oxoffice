//! Reading a form: its physical controls and legacy fields, with their kind, binding, check box, date and list facts and display text.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ControlKind {
    Rich,
    Plain,
    Date,
    Combo,
    DropDown,
    CheckBox,
    Structural,
    Inconsistent,
}

#[derive(Clone, Copy)]
pub(super) enum Owner {
    Control(usize),
    Legacy(usize),
}

struct QualAttr {
    local: String,
    word: bool,
    w14: bool,
    value: String,
}

pub(super) fn flushed_form_document(document: &Document) -> Result<Document> {
    let mut staged = document.clone_for_staging();
    staged.flush_to_package()?;
    Ok(staged)
}

pub(super) fn publish_form_document(document: &mut Document, candidate: Document) -> Result<()> {
    let mut original = document.clone_for_staging();
    let mut staged = candidate.clone_for_staging();
    if original.to_bytes()? == staged.to_bytes()? {
        return Ok(());
    }
    let reopened = candidate.prepare_and_reopen_staged()?;
    document.commit_staged_mutation(reopened);
    Ok(())
}

pub(super) fn read_form_state(document: &Document) -> Result<FormState> {
    let (control_ranges, _carriers, text_boxes) = document.accepted_form_regions()?;
    let mut parts = Vec::new();
    for (part, _) in control_ranges
        .iter()
        .chain(_carriers.iter())
        .chain(text_boxes.iter())
    {
        if !parts.iter().any(|seen: &String| seen == part) {
            parts.push(part.clone());
        }
    }
    let mut loaded = Vec::new();
    for part in &parts {
        let xml = document
            .package
            .get_part(part)
            .ok_or_else(|| form_fault(format!("form story part {part} is missing")))?
            .to_vec();
        let elems = index_elements(&xml)?;
        loaded.push((part.clone(), xml, elems));
    }
    let mut grouped: Vec<(String, Vec<std::ops::Range<usize>>)> = Vec::new();
    for (part, range) in &control_ranges {
        if let Some((_, ranges)) = grouped.iter_mut().find(|(name, _)| name == part) {
            ranges.push(range.clone());
        } else {
            grouped.push((part.clone(), vec![range.clone()]));
        }
    }
    for (_, ranges) in &mut grouped {
        ranges.sort_by(|left, right| left.start.cmp(&right.start).then(left.end.cmp(&right.end)));
        ranges.dedup();
    }
    let mut controls = Vec::new();
    for (part, ranges) in &grouped {
        let (_, xml, elems) = loaded
            .iter()
            .find(|(name, _, _)| name == part)
            .ok_or_else(|| form_fault(format!("form story part {part} is missing")))?;
        for (ordinal, range) in ranges.iter().enumerate() {
            let elem = elems
                .iter()
                .find(|elem| elem.word && elem.local == b"sdt" && elem.range.start == range.start)
                .ok_or_else(|| form_fault("stale form target"))?;
            controls.push(read_physical_control(xml, elems, elem, part, ordinal)?);
        }
    }
    let anchors = read_legacy_anchors(document)?;
    let legacy = document
        .legacy_form_fields_with_defaults()?
        .into_iter()
        .map(|(field, default_value)| LegacyField {
            part: field.source_part,
            ordinal: field.ordinal,
            kind: field.kind,
            value: field.value,
            default_value,
            choices: field.choices,
        })
        .collect();
    Ok(FormState {
        doc_part: document.doc_part_name.clone(),
        controls,
        legacy,
        anchors,
    })
}

fn read_physical_control(
    xml: &[u8],
    elems: &[XmlElem],
    sdt: &XmlElem,
    part: &str,
    ordinal: usize,
) -> Result<PhysicalControl> {
    let properties = direct_child(elems, sdt.id, true, false, b"sdtPr");
    let mut tags = Vec::new();
    let mut types = Vec::new();
    let mut placeholder = false;
    let mut placeholder_broken = false;
    let mut binding = None;
    let mut saw_binding = false;
    if let Some(properties) = properties {
        for child in direct_children(elems, properties.id) {
            if child.word && child.local == b"tag" {
                tags.push(child.val.clone());
            } else if child.word && child.local == b"showingPlcHdr" {
                match parse_on_off(child.val.as_deref()) {
                    Some(flag) => placeholder = flag,
                    None => placeholder_broken = true,
                }
            } else if child.word && child.local == b"dataBinding" {
                if saw_binding {
                    binding = None;
                } else {
                    saw_binding = true;
                    binding = Some(read_binding(xml, child)?);
                }
            } else if is_type_child(child) {
                types.push(child);
            }
        }
    }
    let kind = match types.as_slice() {
        [] => ControlKind::Rich,
        [one] => control_kind(one),
        _ => ControlKind::Inconsistent,
    };
    let type_elem = if types.len() == 1 {
        Some(types[0])
    } else {
        None
    };
    let checkbox = match type_elem {
        Some(elem) if kind == ControlKind::CheckBox => read_checkbox(xml, elems, elem)?,
        _ => CheckboxRead {
            checked: None,
            checked_broken: false,
            glyph_broken: false,
            checked_glyph: None,
            unchecked_glyph: None,
            checked_font: None,
            unchecked_font: None,
            checked_fault: None,
            unchecked_fault: None,
        },
    };
    let date = match type_elem {
        Some(elem) if kind == ControlKind::Date => Some(read_date(xml, elems, elem)?),
        _ => None,
    };
    let list_items = match type_elem {
        Some(elem) if matches!(kind, ControlKind::Combo | ControlKind::DropDown) => {
            read_list_items(xml, elems, elem)?
        }
        _ => Vec::new(),
    };
    Ok(PhysicalControl {
        part: part.to_owned(),
        start: sdt.range.start,
        end: sdt.range.end,
        ordinal,
        tag: tags.first().and_then(Clone::clone),
        kind,
        placeholder,
        placeholder_broken,
        display: direct_display(xml, elems, sdt)?,
        duplicate_tag: tags.len() > 1 || (saw_binding && binding.is_none()),
        binding,
        bound_text: None,
        checked: checkbox.checked,
        checked_broken: checkbox.checked_broken,
        glyph_broken: checkbox.glyph_broken,
        checked_glyph: checkbox.checked_glyph,
        unchecked_glyph: checkbox.unchecked_glyph,
        checked_font: checkbox.checked_font,
        unchecked_font: checkbox.unchecked_font,
        checked_fault: checkbox.checked_fault,
        unchecked_fault: checkbox.unchecked_fault,
        date,
        list_items,
    })
}

fn control_kind(elem: &XmlElem) -> ControlKind {
    if elem.word {
        return match elem.local.as_slice() {
            b"richText" => ControlKind::Rich,
            b"text" => ControlKind::Plain,
            b"date" => ControlKind::Date,
            b"comboBox" => ControlKind::Combo,
            b"dropDownList" => ControlKind::DropDown,
            b"picture" | b"group" | b"citation" | b"bibliography" | b"equation"
            | b"docPartList" | b"docPartObj" => ControlKind::Structural,
            _ => ControlKind::Inconsistent,
        };
    }
    if elem.w14 && elem.local.as_slice() == b"checkbox" {
        return ControlKind::CheckBox;
    }
    if elem.w15
        && matches!(
            elem.local.as_slice(),
            b"repeatingSection" | b"repeatingSectionItem"
        )
    {
        return ControlKind::Structural;
    }
    ControlKind::Inconsistent
}

fn read_binding(xml: &[u8], elem: &XmlElem) -> Result<BindingRef> {
    let attributes = attributes_of(xml, elem.range.start)?;
    let word = |local: &str| {
        attributes
            .iter()
            .find(|attr| attr.word && attr.local == local)
            .map(|attr| attr.value.clone())
    };
    Ok(BindingRef {
        store_item_id: word("storeItemID").unwrap_or_default(),
        xpath: word("xpath").unwrap_or_default(),
        prefix_mappings: word("prefixMappings"),
    })
}

struct CheckboxRead {
    checked: Option<bool>,
    checked_broken: bool,
    glyph_broken: bool,
    checked_glyph: Option<String>,
    unchecked_glyph: Option<String>,
    checked_font: Option<String>,
    unchecked_font: Option<String>,
    checked_fault: Option<String>,
    unchecked_fault: Option<String>,
}

fn read_checkbox(xml: &[u8], elems: &[XmlElem], checkbox: &XmlElem) -> Result<CheckboxRead> {
    let checked_elem = direct_child(elems, checkbox.id, false, true, b"checked");
    let (checked, broken) = if let Some(elem) = checked_elem {
        match w14_val(xml, elem)? {
            Some(flag) => (Some(flag), false),
            None => (Some(false), true),
        }
    } else {
        (Some(false), false)
    };
    let checked_state = checkbox_policy(xml, checkbox, true);
    let unchecked_state = checkbox_policy(xml, checkbox, false);
    let (checked_glyph, checked_font, checked_fault) = checkbox_state_parts(checked_state);
    let (unchecked_glyph, unchecked_font, unchecked_fault) = checkbox_state_parts(unchecked_state);
    Ok(CheckboxRead {
        checked,
        checked_broken: broken,
        glyph_broken: checked_fault.is_some() || unchecked_fault.is_some(),
        checked_glyph,
        unchecked_glyph,
        checked_font,
        unchecked_font,
        checked_fault,
        unchecked_fault,
    })
}

fn checkbox_state_parts(
    state: Result<(char, String)>,
) -> (Option<String>, Option<String>, Option<String>) {
    match state {
        Ok((glyph, font)) => (Some(glyph.to_string()), Some(font), None),
        Err(error) => (None, None, Some(error.to_string())),
    }
}

fn checkbox_policy(xml: &[u8], checkbox: &XmlElem, checked: bool) -> Result<(char, String)> {
    let element = std::str::from_utf8(&xml[checkbox.range.clone()])
        .map_err(|error| form_fault(format!("checkbox type payload is invalid: {error}")))?;
    let declarations = in_scope_xmlns(xml, checkbox.range.start)?;
    let (_attributes, prefix, declare) =
        qualified_attributes(xml, checkbox.range.start, AttrNs::Word)?;
    let mut markup = format!("<{prefix}:sdt");
    if let Some(uri) = declare {
        let marker = format!(" xmlns:{prefix}=\"");
        if !declarations.contains(&marker) {
            markup.push_str(&format!(" xmlns:{prefix}=\"{uri}\""));
        }
    }
    markup.push_str(&declarations);
    markup.push_str(&format!(
        "><{prefix}:sdtPr>{element}</{prefix}:sdtPr><{prefix}:sdtContent/></{prefix}:sdt>"
    ));
    let parsed = parse_control(&markup)?;
    parsed
        .properties
        .ok_or_else(|| form_fault("checkbox type payload is missing"))?
        .checkbox_state(checked)
        .map_err(Error::from)
}

fn read_date(xml: &[u8], elems: &[XmlElem], date: &XmlElem) -> Result<DateFacts> {
    let attributes = attributes_of(xml, date.range.start)?;
    let full_date = attributes
        .iter()
        .find(|attr| attr.word && attr.local == "fullDate")
        .map(|attr| attr.value.clone());
    let mut format = None;
    let mut language = None;
    let mut store = None;
    let mut calendar = None;
    let mut duplicate = false;
    for child in direct_children(elems, date.id) {
        if !child.word {
            continue;
        }
        let slot = match child.local.as_slice() {
            b"dateFormat" => &mut format,
            b"lid" => &mut language,
            b"storeMappedDataAs" => &mut store,
            b"calendar" => &mut calendar,
            _ => continue,
        };
        if slot.is_some() {
            duplicate = true;
        }
        *slot = child.val.clone();
    }
    Ok(DateFacts {
        format,
        language,
        store,
        calendar,
        full_date,
        duplicate,
    })
}

fn read_list_items(xml: &[u8], elems: &[XmlElem], list: &XmlElem) -> Result<Vec<ListChoice>> {
    let mut items = Vec::new();
    for child in direct_children(elems, list.id) {
        if !(child.word && child.local == b"listItem") {
            continue;
        }
        let attributes = attributes_of(xml, child.range.start)?;
        let attr = |local: &str| {
            attributes
                .iter()
                .find(|attr| attr.word && attr.local == local)
                .map(|attr| attr.value.clone())
        };
        items.push(ListChoice {
            value: attr("value"),
            display: attr("displayText"),
        });
    }
    Ok(items)
}

fn read_legacy_anchors(document: &Document) -> Result<Vec<LegacyAnchor>> {
    Ok(document
        .legacy_form_physical_anchors()?
        .into_iter()
        .map(|anchor| LegacyAnchor {
            part: anchor.part,
            ordinal: anchor.ordinal,
            start: anchor.start,
            end: anchor.end,
        })
        .collect())
}

fn direct_display(xml: &[u8], elems: &[XmlElem], sdt: &XmlElem) -> Result<String> {
    let mut parts = Vec::new();
    for elem in elems {
        let Some(piece) = display_piece(xml, elem)? else {
            continue;
        };
        if !accepted_display_text(elems, sdt, elem) {
            continue;
        }
        parts.push((elem.range.start, piece));
    }
    parts.sort_by_key(|part| part.0);
    Ok(parts.into_iter().map(|part| part.1).collect())
}

fn display_piece(xml: &[u8], elem: &XmlElem) -> Result<Option<String>> {
    if !elem.word {
        return Ok(None);
    }
    match elem.local.as_slice() {
        b"t" => Ok(Some(element_text(xml, elem)?)),
        b"tab" | b"ptab" => Ok(Some("\t".to_owned())),
        b"noBreakHyphen" => Ok(Some("-".to_owned())),
        b"br" | b"cr" => Ok(Some("\n".to_owned())),
        _ => Ok(None),
    }
}

pub(super) fn accepted_display_text(elems: &[XmlElem], sdt: &XmlElem, text: &XmlElem) -> bool {
    accepted_owning_content(elems, sdt, text.parent, true)
}

/// Whether `change` is a tracked insertion, deletion or move among the
/// control's display content, where its paragraphs and runs are: not one of
/// a property or a paragraph mark, a nested control or a text box.
pub(super) fn accepted_display_change(elems: &[XmlElem], sdt: &XmlElem, change: &XmlElem) -> bool {
    change.word
        && matches!(
            change.local.as_slice(),
            b"ins" | b"del" | b"moveFrom" | b"moveTo"
        )
        && accepted_owning_content(elems, sdt, change.parent, false)
}

/// Whether `change` is the tracked insertion, deletion or move of the mark
/// of one of the control's display paragraphs.
pub(super) fn accepted_display_mark_change(
    elems: &[XmlElem],
    sdt: &XmlElem,
    change: &XmlElem,
) -> bool {
    let parent = |elem: &XmlElem, local: &[u8]| {
        elem.parent
            .map(|id| &elems[id])
            .filter(|parent| parent.word && parent.local == local)
    };
    change.word
        && matches!(
            change.local.as_slice(),
            b"ins" | b"del" | b"moveFrom" | b"moveTo"
        )
        && parent(change, b"rPr")
            .and_then(|properties| parent(properties, b"pPr"))
            .and_then(|properties| parent(properties, b"p"))
            .is_some_and(|paragraph| accepted_display_paragraph(elems, sdt, paragraph))
}

pub(super) fn accepted_display_paragraph(
    elems: &[XmlElem],
    sdt: &XmlElem,
    paragraph: &XmlElem,
) -> bool {
    accepted_owning_content(elems, sdt, Some(paragraph.id), false)
}

fn accepted_owning_content(
    elems: &[XmlElem],
    sdt: &XmlElem,
    mut parent: Option<usize>,
    require_run: bool,
) -> bool {
    if nearest_sdt_id(elems, parent) != Some(sdt.id) {
        return false;
    }
    if descendant_text_box(elems, sdt.id, parent) {
        return false;
    }
    let mut saw_run = false;
    let mut saw_content = false;
    while let Some(id) = parent {
        if id == sdt.id {
            return saw_content && (!require_run || saw_run);
        }
        let Some(current) = elems.iter().find(|elem| elem.id == id) else {
            return false;
        };
        // Drawing discovery omits fldSimple because that walk does not expose a
        // drawing inside a simple field. Owned display still includes the cache.
        if !(current.word && accepted_display_container(current.local.as_slice())) {
            return false;
        }
        if current.local == b"r" {
            saw_run = true;
        }
        if current.local == b"sdtContent" {
            if current.parent != Some(sdt.id) {
                return false;
            }
            saw_content = true;
        }
        parent = current.parent;
    }
    false
}

fn accepted_display_container(local: &[u8]) -> bool {
    local == b"fldSimple" || crate::field::word_accepted_form_container(local)
}

pub(super) fn element_text(xml: &[u8], elem: &XmlElem) -> Result<String> {
    let Some(inner_end) = elem.inner_end else {
        return Ok(String::new());
    };
    let raw = xml
        .get(elem.open_end..inner_end)
        .ok_or_else(|| form_fault("content control text is out of bounds"))?;
    let text = std::str::from_utf8(raw)
        .map_err(|error| form_fault(format!("content control text is invalid: {error}")))?;
    quick_xml::escape::unescape(&oxoffice_core::xml_text::normalize_line_ends(text))
        .map(|value| value.into_owned())
        .map_err(|error| form_fault(format!("content control text is invalid: {error}")))
}

fn attributes_of(xml: &[u8], start: usize) -> Result<Vec<QualAttr>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (_namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| form_fault(format!("content control XML is invalid: {error}")))?;
        if before == start {
            let (Event::Start(element) | Event::Empty(element)) = event else {
                return Ok(Vec::new());
            };
            let mut attributes = Vec::new();
            for attribute in element.attributes() {
                let attribute = attribute.map_err(|error| {
                    form_fault(format!("content control attribute is invalid: {error}"))
                })?;
                let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                let value = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|error| {
                        form_fault(format!(
                            "content control attribute value is invalid: {error}"
                        ))
                    })?;
                attributes.push(QualAttr {
                    local: String::from_utf8_lossy(local.as_ref()).into_owned(),
                    word: is_word(&namespace),
                    w14: is_w14(&namespace),
                    value: value.into_owned(),
                });
            }
            return Ok(attributes);
        }
        if before > start {
            return Err(form_fault("content control attribute position was missed"));
        }
        buffer.clear();
    }
}

fn parse_on_off(value: Option<&str>) -> Option<bool> {
    match value {
        None => Some(true),
        Some(value) => oxoffice_docx_oxml::shared::ST_OnOff::parse(value),
    }
}

fn w14_val(xml: &[u8], elem: &XmlElem) -> Result<Option<bool>> {
    let text = w14_text(xml, elem, "val")?;
    Ok(match text {
        None => Some(true),
        Some(value) => parse_on_off(Some(&value)),
    })
}

fn w14_text(xml: &[u8], elem: &XmlElem, local: &str) -> Result<Option<String>> {
    Ok(attributes_of(xml, elem.range.start)?
        .into_iter()
        .find(|attr| attr.w14 && attr.local == local)
        .map(|attr| attr.value))
}

fn nearest_sdt_id(elems: &[XmlElem], mut parent: Option<usize>) -> Option<usize> {
    while let Some(id) = parent {
        let current = elems.iter().find(|elem| elem.id == id)?;
        if current.word && current.local == b"sdt" {
            return Some(current.id);
        }
        parent = current.parent;
    }
    None
}

fn descendant_text_box(elems: &[XmlElem], sdt_id: usize, mut parent: Option<usize>) -> bool {
    while let Some(id) = parent {
        if id == sdt_id {
            return false;
        }
        let elem = &elems[id];
        if elem.word && elem.local == b"txbxContent" {
            return true;
        }
        parent = elem.parent;
    }
    false
}
