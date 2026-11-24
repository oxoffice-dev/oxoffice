//! Patching one control in its part's bytes: date, last value and check box state, display text, fonts and prompt runs.

use super::*;

#[derive(Clone)]
pub(super) enum FullDateEdit {
    Keep,
    Set(String),
    Clear,
}

#[derive(Clone)]
pub(super) enum LastValueEdit {
    Keep,
    Set(String),
    Clear,
}

pub(super) fn apply_control_patch(
    xml: &mut Vec<u8>,
    start: usize,
    patch: &ControlPatch,
    placeholder_styles: &[String],
) -> Result<()> {
    if let Some(placeholder) = patch.placeholder {
        let bounds = sdt_bounds(xml, start)?;
        let updated =
            edit_sdt_property(xml, &bounds, PropertyEdit::Placeholder(Some(placeholder)))?;
        xml.splice(bounds, updated);
    }
    match &patch.full_date {
        FullDateEdit::Keep => {}
        FullDateEdit::Set(value) => edit_date_attribute(xml, start, Some(value))?,
        FullDateEdit::Clear => edit_date_attribute(xml, start, None)?,
    }
    match &patch.last_value {
        LastValueEdit::Keep => {}
        LastValueEdit::Set(value) => edit_last_value(xml, start, Some(value))?,
        LastValueEdit::Clear => edit_last_value(xml, start, None)?,
    }
    if let Some(checked) = patch.checked {
        edit_checked(xml, start, checked)?;
    }
    if let Some(display) = &patch.display {
        rewrite_display(
            xml,
            start,
            display,
            patch.font.as_deref(),
            patch.prompt,
            placeholder_styles,
        )?;
    }
    Ok(())
}

fn sdt_bounds(xml: &[u8], start: usize) -> Result<Range<usize>> {
    let elems = index_elements(xml)?;
    let sdt = elems
        .iter()
        .find(|elem| elem.word && elem.local == b"sdt" && elem.range.start == start)
        .ok_or_else(|| form_fault("stale form target"))?;
    Ok(sdt.range.clone())
}

fn edit_date_attribute(xml: &mut Vec<u8>, sdt_start: usize, value: Option<&str>) -> Result<()> {
    let elems = index_elements(xml)?;
    let sdt = find_sdt(&elems, sdt_start)?;
    let Some(properties) = direct_child(&elems, sdt.id, true, false, b"sdtPr") else {
        return Ok(());
    };
    let Some(date) = direct_child(&elems, properties.id, true, false, b"date") else {
        return if value.is_some() {
            Err(form_fault("date type payload is missing"))
        } else {
            Ok(())
        };
    };
    rewrite_qualified_attribute(xml, date.range.start, AttrNs::Word, "fullDate", value)
}

fn edit_last_value(xml: &mut Vec<u8>, sdt_start: usize, value: Option<&str>) -> Result<()> {
    let elems = index_elements(xml)?;
    let sdt = find_sdt(&elems, sdt_start)?;
    let Some(properties) = direct_child(&elems, sdt.id, true, false, b"sdtPr") else {
        return Ok(());
    };
    let Some(list) = direct_children(&elems, properties.id)
        .find(|elem| elem.word && matches!(elem.local.as_slice(), b"comboBox" | b"dropDownList"))
    else {
        return Ok(());
    };
    let start = list.range.start;
    rewrite_qualified_attribute(xml, start, AttrNs::Word, "lastValue", value)
}

fn edit_checked(xml: &mut Vec<u8>, sdt_start: usize, checked: bool) -> Result<()> {
    let elems = index_elements(xml)?;
    let sdt = find_sdt(&elems, sdt_start)?;
    let Some(properties) = direct_child(&elems, sdt.id, true, false, b"sdtPr") else {
        return Err(form_fault("content control has no properties"));
    };
    let Some(checkbox) = direct_child(&elems, properties.id, false, true, b"checkbox") else {
        return Err(form_fault("checkbox type payload is missing"));
    };
    let value = if checked { "1" } else { "0" };
    if let Some(existing) = direct_child(&elems, checkbox.id, false, true, b"checked") {
        return rewrite_qualified_attribute(
            xml,
            existing.range.start,
            AttrNs::W14,
            "val",
            Some(value),
        );
    }
    let (_attributes, prefix, declare) =
        qualified_attributes(xml, checkbox.range.start, AttrNs::W14)?;
    let markup = match declare {
        Some(uri) => {
            format!("<{prefix}:checked xmlns:{prefix}=\"{uri}\" {prefix}:val=\"{value}\"/>")
        }
        None => format!("<{prefix}:checked {prefix}:val=\"{value}\"/>"),
    };
    insert_markup(xml, checkbox.range.start, &markup)
}

/// Write a control's display text: the value in its first `w:t`, its tabs
/// and line breaks as `w:tab` and `w:br`, every other text of the control
/// emptied, as the typed tag setters write it. The tracked insertions,
/// deletions and moves of its display content go first, as
/// `drop_content_changes` drops them in the typed model. A value over a
/// prompt takes the control's own run properties, and a block control keeps
/// no paragraph the value left empty.
fn rewrite_display(
    xml: &mut Vec<u8>,
    sdt_start: usize,
    value: &str,
    font: Option<&str>,
    prompt: Option<bool>,
    placeholder_styles: &[String],
) -> Result<()> {
    drop_display_changes(xml, sdt_start)?;
    let mut separators = Vec::new();
    {
        let elems = index_elements(xml)?;
        let sdt = find_sdt(&elems, sdt_start)?;
        for elem in &elems {
            if elem.word
                && matches!(
                    elem.local.as_slice(),
                    b"tab" | b"ptab" | b"noBreakHyphen" | b"br" | b"cr"
                )
                && accepted_display_text(&elems, sdt, elem)
            {
                separators.push(elem.range.start);
            }
        }
    }
    separators.sort_unstable();
    for start in separators.into_iter().rev() {
        let elems = index_elements(xml)?;
        let range = elems
            .iter()
            .find(|elem| elem.range.start == start)
            .ok_or_else(|| form_fault("stale form target"))?
            .range
            .clone();
        xml.splice(range, Vec::new());
    }
    let mut starts = display_text_starts(xml, sdt_start)?;
    if starts.is_empty() {
        // A control without text gets an empty run, which then takes the
        // value as the first text of any control does.
        insert_display(xml, sdt_start)?;
        starts = display_text_starts(xml, sdt_start)?;
    }
    let first = *starts
        .first()
        .ok_or_else(|| form_fault("content control has no typed text display location"))?;
    let mut content = value_content(value).into_iter().peekable();
    let head = match content.next_if(|item| matches!(item, RunContent::Text(_))) {
        Some(RunContent::Text(text)) => text.text,
        _ => String::new(),
    };
    for (index, start) in starts.iter().enumerate().rev() {
        let text = if index == 0 { head.as_str() } else { "" };
        set_element_text(xml, *start, text)?;
    }
    insert_after_text(xml, first, &content.collect::<Vec<_>>())?;
    if let Some(showing_placeholder) = prompt {
        take_display_prompt_properties(
            xml,
            sdt_start,
            first,
            showing_placeholder,
            placeholder_styles,
        )?;
    }
    if let Some(font) = font {
        // New run properties before the value's text moved it.
        let first = *display_text_starts(xml, sdt_start)?
            .first()
            .ok_or_else(|| form_fault("content control has no typed text display location"))?;
        apply_display_font(xml, first, font)?;
    }
    drop_emptied_display_paragraphs(xml, sdt_start)
}

/// Remove the tracked insertions, deletions and moves of the control's
/// display content, with everything they hold, as `drop_content_changes`
/// does in the typed model: not those of a nested control, a text box, a
/// paragraph mark or properties. One that `refuse_dropped_change` refuses
/// refuses the value, before anything is removed. A hyperlink that holds
/// nothing but changes goes with them, as the typed model removes one left
/// empty.
fn drop_display_changes(xml: &mut Vec<u8>, sdt_start: usize) -> Result<()> {
    let mut changes = {
        let elems = index_elements(xml)?;
        let sdt = find_sdt(&elems, sdt_start)?;
        let selected = elems
            .iter()
            .filter(|elem| accepted_display_change(&elems, sdt, elem))
            .collect::<Vec<_>>();
        let is_selected = |elem: &XmlElem| selected.iter().any(|change| change.id == elem.id);
        selected
            .iter()
            .map(|change| match change.parent.map(|id| &elems[id]) {
                Some(parent)
                    if parent.word
                        && parent.local == b"hyperlink"
                        && direct_children(&elems, parent.id).all(is_selected) =>
                {
                    parent.range.clone()
                }
                _ => change.range.clone(),
            })
            .collect::<Vec<_>>()
    };
    changes.sort_by_key(|range| range.start);
    // A change inside another goes with it.
    let mut outermost: Vec<Range<usize>> = Vec::new();
    for range in changes {
        if outermost.last().is_none_or(|last| range.start >= last.end) {
            outermost.push(range);
        }
    }
    for range in &outermost {
        refuse_dropped_change(&xml[range.clone()])?;
    }
    for range in outermost.into_iter().rev() {
        xml.splice(range, Vec::new());
    }
    Ok(())
}

/// The starts of the control's display `w:t` elements, in document order.
fn display_text_starts(xml: &[u8], sdt_start: usize) -> Result<Vec<usize>> {
    let elems = index_elements(xml)?;
    let sdt = find_sdt(&elems, sdt_start)?;
    let mut starts: Vec<usize> = elems
        .iter()
        .filter(|elem| elem.word && elem.local == b"t" && accepted_display_text(&elems, sdt, elem))
        .map(|elem| elem.range.start)
        .collect();
    starts.sort_unstable();
    Ok(starts)
}

/// The run that holds the display element starting at `start`.
fn display_run_of(elems: &[XmlElem], start: usize) -> Result<&XmlElem> {
    let text = elems
        .iter()
        .find(|elem| elem.range.start == start)
        .ok_or_else(|| form_fault("stale form target"))?;
    let mut parent = text.parent;
    while let Some(id) = parent {
        let current = elems
            .iter()
            .find(|elem| elem.id == id)
            .ok_or_else(|| form_fault("stale form target"))?;
        if current.word && current.local == b"r" {
            return Ok(current);
        }
        parent = current.parent;
    }
    Err(form_fault(
        "content control has no typed text display location",
    ))
}

/// Write the run content that follows a value's first text, its tabs,
/// breaks and further texts, right after the `w:t` starting at `text_start`.
fn insert_after_text(xml: &mut Vec<u8>, text_start: usize, content: &[RunContent]) -> Result<()> {
    if content.is_empty() {
        return Ok(());
    }
    let elems = index_elements(xml)?;
    let run = display_run_of(&elems, text_start)?;
    let prefix = match name_prefix(element_qname(&xml[run.range.clone()])?) {
        "" => String::new(),
        prefix => format!("{prefix}:"),
    };
    let mut markup = String::new();
    for item in content {
        match item {
            RunContent::Tab => markup.push_str(&format!("<{prefix}tab/>")),
            RunContent::Text(text) => {
                let space = if needs_preserve(&text.text) {
                    " xml:space=\"preserve\""
                } else {
                    ""
                };
                let escaped = escaped_text(&text.text);
                markup.push_str(&format!("<{prefix}t{space}>{escaped}</{prefix}t>"));
            }
            _ => markup.push_str(&format!("<{prefix}br/>")),
        }
    }
    let end = elems
        .iter()
        .find(|elem| elem.range.start == text_start)
        .ok_or_else(|| form_fault("stale form target"))?
        .range
        .end;
    xml.splice(end..end, markup.into_bytes());
    Ok(())
}

/// Give the run of a value written over a prompt the control's own
/// `w:sdtPr/w:rPr` (ECMA-376 17.5.2.27), copied with the namespaces it needs,
/// or, when the control has none, drop the prompt's Placeholder Text style,
/// as `take_prompt_properties` does in the typed model.
fn take_display_prompt_properties(
    xml: &mut Vec<u8>,
    sdt_start: usize,
    text_start: usize,
    showing_placeholder: bool,
    placeholder_styles: &[String],
) -> Result<()> {
    let elems = index_elements(xml)?;
    let sdt = find_sdt(&elems, sdt_start)?;
    let run = display_run_of(&elems, text_start)?;
    let run_properties = direct_child(&elems, run.id, true, false, b"rPr");
    let style = run_properties
        .and_then(|properties| direct_child(&elems, properties.id, true, false, b"rStyle"));
    let styled = style
        .and_then(|style| style.val.as_deref())
        .is_some_and(|style| placeholder_styles.iter().any(|id| id == style));
    if !styled && !showing_placeholder {
        return Ok(());
    }
    let sdt_properties = direct_child(&elems, sdt.id, true, false, b"sdtPr");
    let control_properties = sdt_properties
        .and_then(|properties| direct_child(&elems, properties.id, true, false, b"rPr"));
    if let (Some(sdt_properties), Some(control_properties)) = (sdt_properties, control_properties) {
        let copy = copied_run_properties(xml, sdt_properties, control_properties, run)?;
        let range = run_properties.map_or(run.open_end..run.open_end, |properties| {
            properties.range.clone()
        });
        xml.splice(range, copy);
    } else if let (true, Some(properties), Some(style)) = (styled, run_properties, style) {
        let range = if direct_children(&elems, properties.id).count() == 1 {
            properties.range.clone()
        } else {
            style.range.clone()
        };
        xml.splice(range, Vec::new());
    }
    Ok(())
}

/// The control's `w:rPr`, to go into a run of its content, with the
/// namespace declarations in scope at its `w:sdtPr` that are not at the run.
fn copied_run_properties(
    xml: &[u8],
    sdt_properties: &XmlElem,
    properties: &XmlElem,
    run: &XmlElem,
) -> Result<Vec<u8>> {
    let at_run = in_scope_declarations(xml, run.range.start)?;
    let mut reader = Reader::from_reader(&xml[properties.range.start..properties.open_end]);
    let own: Vec<Vec<u8>> = match reader.read_event() {
        Ok(Event::Start(start) | Event::Empty(start)) => start
            .attributes()
            .with_checks(false)
            .flatten()
            .map(|attribute| attribute.key.as_ref().to_vec())
            .collect(),
        _ => Vec::new(),
    };
    let missing: String = in_scope_declarations(xml, sdt_properties.range.start)?
        .into_iter()
        .filter(|declaration| {
            !at_run.contains(declaration) && !own.contains(&declaration.0.as_bytes().to_vec())
        })
        .map(|(name, uri)| format!(" {name}=\"{}\"", xml_attr(&uri)))
        .collect();
    let mut tag = xml[properties.range.start..properties.open_end].to_vec();
    if !missing.is_empty() {
        insert_into_start_tag(&mut tag, &missing)?;
    }
    tag.extend_from_slice(&xml[properties.open_end..properties.range.end]);
    Ok(tag)
}

/// Remove the paragraphs of a block control that the value left empty, as
/// `drop_emptied_paragraphs` does in the typed model: the first stays if all
/// are empty.
fn drop_emptied_display_paragraphs(xml: &mut Vec<u8>, sdt_start: usize) -> Result<()> {
    let elems = index_elements(xml)?;
    let sdt = find_sdt(&elems, sdt_start)?;
    let Some(content) = direct_child(&elems, sdt.id, true, false, b"sdtContent") else {
        return Ok(());
    };
    let paragraphs: Vec<&XmlElem> = direct_children(&elems, content.id)
        .filter(|elem| elem.word && elem.local == b"p")
        .collect();
    if paragraphs.len() < 2 {
        return Ok(());
    }
    let mut emptied = Vec::new();
    for paragraph in &paragraphs {
        if display_paragraph_is_empty(xml, &elems, paragraph)? {
            emptied.push(paragraph.range.clone());
        }
    }
    if emptied.len() == paragraphs.len() {
        emptied.remove(0);
    }
    for range in emptied.into_iter().rev() {
        xml.splice(range, Vec::new());
    }
    Ok(())
}

/// Whether a paragraph holds only runs of empty text, as
/// `is_empty_paragraph` reads one in the typed model: attributes, proofing
/// marks and last rendered page breaks are no content, but a section break
/// and a tracked paragraph mark or property change in its `w:pPr` are.
fn display_paragraph_is_empty(xml: &[u8], elems: &[XmlElem], paragraph: &XmlElem) -> Result<bool> {
    for child in direct_children(elems, paragraph.id) {
        if !child.word {
            return Ok(false);
        }
        match child.local.as_slice() {
            b"pPr" => {
                let keeps_state = elems.iter().any(|inner| {
                    inner.range.start > child.range.start
                        && inner.range.end <= child.range.end
                        && inner.word
                        && matches!(
                            inner.local.as_slice(),
                            b"sectPr"
                                | b"pPrChange"
                                | b"rPrChange"
                                | b"ins"
                                | b"del"
                                | b"moveFrom"
                                | b"moveTo"
                        )
                });
                if keeps_state {
                    return Ok(false);
                }
            }
            b"r" => {
                for item in direct_children(elems, child.id) {
                    let empty_text = item.local == b"t" && element_text(xml, item)?.is_empty();
                    let no_content =
                        matches!(item.local.as_slice(), b"rPr" | b"lastRenderedPageBreak");
                    if !item.word || !(no_content || empty_text) {
                        return Ok(false);
                    }
                }
            }
            b"proofErr" => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn apply_display_font(xml: &mut Vec<u8>, text_start: usize, font: &str) -> Result<()> {
    let elems = index_elements(xml)?;
    let run = display_run_of(&elems, text_start)?;
    let run_start = run.range.start;
    let properties = direct_child(&elems, run.id, true, false, b"rPr");
    if properties.is_none() {
        let markup = display_font_properties(font);
        let at = run.open_end;
        xml.splice(at..at, markup.into_bytes());
        return Ok(());
    }
    let properties = properties.ok_or_else(|| form_fault("stale form target"))?;
    let fonts = direct_child(&elems, properties.id, true, false, b"rFonts");
    if fonts.is_none() {
        let markup = display_font_element(font);
        let at = properties.open_end;
        xml.splice(at..at, markup.into_bytes());
    }
    let elems = index_elements(xml)?;
    let run = elems
        .iter()
        .find(|elem| elem.word && elem.local == b"r" && elem.range.start == run_start)
        .ok_or_else(|| form_fault("stale form target"))?;
    let properties = direct_child(&elems, run.id, true, false, b"rPr")
        .ok_or_else(|| form_fault("content control has no typed text display location"))?;
    let fonts = direct_child(&elems, properties.id, true, false, b"rFonts")
        .ok_or_else(|| form_fault("content control has no typed text display location"))?;
    for local in ["ascii", "hAnsi", "eastAsia", "cs"] {
        rewrite_qualified_attribute(xml, fonts.range.start, AttrNs::Word, local, Some(font))?;
    }
    let elems = index_elements(xml)?;
    let fonts = elems
        .iter()
        .find(|elem| elem.word && elem.local == b"rFonts" && elem.range.start == fonts.range.start)
        .ok_or_else(|| form_fault("stale form target"))?;
    let start = fonts.range.start;
    for local in ["asciiTheme", "hAnsiTheme", "eastAsiaTheme", "cstheme"] {
        rewrite_qualified_attribute(xml, start, AttrNs::Word, local, None)?;
    }
    Ok(())
}

fn display_font_properties(font: &str) -> String {
    let fonts = display_font_element(font);
    format!("<w:rPr xmlns:w=\"{W_NS}\">{fonts}</w:rPr>")
}

fn display_font_element(font: &str) -> String {
    let font = xml_attr(font);
    format!(
        "<w:rFonts xmlns:w=\"{W_NS}\" w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:eastAsia=\"{font}\" w:cs=\"{font}\"/>"
    )
}

/// Give a control without display text an empty run: in its first
/// paragraph, in a new paragraph for a block control without one, or in its
/// content for an inline control.
fn insert_display(xml: &mut Vec<u8>, sdt_start: usize) -> Result<()> {
    let elems = index_elements(xml)?;
    let sdt = find_sdt(&elems, sdt_start)?;
    let inline = ancestor_local(&elems, sdt.parent, b"p");
    let prefix = display_prefix(xml, &elems, sdt)?;
    let markup = display_run(&prefix);
    if !inline {
        if let Some(paragraph) = elems.iter().find(|elem| {
            elem.word
                && elem.local == b"p"
                && elem.range.start >= sdt.range.start
                && elem.range.end <= sdt.range.end
                && accepted_display_paragraph(&elems, sdt, elem)
        }) {
            return insert_markup(xml, paragraph.range.start, &markup);
        }
        let paragraph = if prefix.is_empty() {
            format!("<p>{markup}</p>")
        } else {
            format!("<{prefix}:p>{markup}</{prefix}:p>")
        };
        return insert_in_content(xml, sdt_start, &paragraph);
    }
    insert_in_content(xml, sdt_start, &markup)
}

fn display_prefix(xml: &[u8], elems: &[XmlElem], sdt: &XmlElem) -> Result<String> {
    let content = direct_child(elems, sdt.id, true, false, b"sdtContent")
        .ok_or_else(|| form_fault("content control has no typed text display location"))?;
    Ok(name_prefix(element_qname(&xml[content.range.clone()])?).to_owned())
}

fn display_run(prefix: &str) -> String {
    if prefix.is_empty() {
        "<r><t></t></r>".to_owned()
    } else {
        format!("<{prefix}:r><{prefix}:t></{prefix}:t></{prefix}:r>")
    }
}

fn insert_in_content(xml: &mut Vec<u8>, sdt_start: usize, markup: &str) -> Result<()> {
    let elems = index_elements(xml)?;
    let sdt = find_sdt(&elems, sdt_start)?;
    let content = direct_child(&elems, sdt.id, true, false, b"sdtContent")
        .ok_or_else(|| form_fault("content control has no typed text display location"))?;
    insert_markup(xml, content.range.start, markup)
}

fn insert_markup(xml: &mut Vec<u8>, start: usize, markup: &str) -> Result<()> {
    let elems = index_elements(xml)?;
    let elem = elems
        .iter()
        .find(|elem| elem.range.start == start)
        .ok_or_else(|| form_fault("stale form target"))?;
    let range = elem.range.clone();
    let replacement = insert_into_element(xml, &range, markup.as_bytes())?;
    xml.splice(range, replacement);
    Ok(())
}

fn set_element_text(xml: &mut Vec<u8>, start: usize, value: &str) -> Result<()> {
    let elems = index_elements(xml)?;
    let elem = elems
        .iter()
        .find(|elem| elem.range.start == start)
        .ok_or_else(|| form_fault("stale form target"))?;
    let escaped = escaped_text(value);
    if let Some(inner) = elem.inner_end {
        let open = elem.open_end;
        xml.splice(open..inner, escaped.into_bytes());
    } else {
        let range = elem.range.clone();
        let replacement = expand_empty(&xml[range.clone()], escaped.as_bytes())?;
        xml.splice(range, replacement);
    }
    if needs_preserve(value) {
        ensure_preserve(xml, start)?;
    }
    Ok(())
}

fn ensure_preserve(xml: &mut Vec<u8>, start: usize) -> Result<()> {
    rewrite_qualified_attribute(xml, start, AttrNs::Xml, "space", Some("preserve"))
}

fn escaped_text(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

fn needs_preserve(value: &str) -> bool {
    value.starts_with(char::is_whitespace) || value.ends_with(char::is_whitespace)
}

fn find_sdt(elems: &[XmlElem], start: usize) -> Result<&XmlElem> {
    elems
        .iter()
        .find(|elem| elem.word && elem.local == b"sdt" && elem.range.start == start)
        .ok_or_else(|| form_fault("stale form target"))
}
