//! The source XML of paragraph and control properties.

use super::*;

pub(super) fn direct_property(xml: &str, local: &str) -> Result<Option<String>> {
    direct_word_element_spans(xml, local)?
        .first()
        .map(|span| {
            let source = source_child(xml, span.clone())?;
            if local == "pPr" {
                let mut reader = Reader::from_str(&source);
                if let Event::Empty(root) = reader
                    .read_event()
                    .map_err(|error| Error::Other(error.to_string()))?
                    && root.attributes().all(|attribute| {
                        attribute.is_ok_and(|attribute| {
                            attribute.key.as_ref() == b"xmlns"
                                || attribute.key.as_ref().starts_with(b"xmlns:")
                        })
                    })
                {
                    let (opening, closing) = paragraph_shell(Some(&source))?;
                    return Ok(format!("{opening}{closing}"));
                }
            }
            Ok(source)
        })
        .transpose()
}

/// Full direct source events, including raw boundaries the control parser owns.
pub(super) fn modeled_control_source_spans(
    content: &[&SdtContent],
    payload: &str,
) -> Result<Vec<Range<usize>>> {
    let physical = control_source_event_spans(payload)?;
    let mut spans = Vec::with_capacity(content.len());
    let mut cursor = 0usize;
    for child in content {
        let first = physical.get(cursor).ok_or_else(|| {
            Error::Other("source control has fewer physical owners than its model".into())
        })?;
        let mut extent = first.clone();
        if let SdtContent::Run(run) = child
            && let Some(field) = run_field(run)
            && let Some((original, _)) = field.source_replacement()?
        {
            let end = first
                .start
                .checked_add(original.len())
                .ok_or_else(|| Error::Other("control field source extent overflow".into()))?;
            if payload.as_bytes().get(first.start..end) != Some(original) {
                return Err(Error::Other(
                    "control field is not at its actual physical source boundary".into(),
                ));
            }
            let last = physical[cursor..]
                .iter()
                .position(|span| span.end == end)
                .ok_or_else(|| {
                    Error::Other("control field does not end at a complete physical owner".into())
                })?;
            extent.end = end;
            cursor += last;
        }
        spans.push(extent);
        cursor += 1;
    }
    if cursor != physical.len() {
        return Err(Error::Other(
            "source control ownership does not match its model".into(),
        ));
    }
    Ok(spans)
}

fn control_source_event_spans(xml: &str) -> Result<Vec<Range<usize>>> {
    let (opening, _) = paragraph_shell(Some(xml))?;
    let mut reader = Reader::from_str(xml);
    let mut spans = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        if before < opening.len() {
            continue;
        }
        match event {
            Event::Start(root) => {
                reader
                    .read_to_end(root.name())
                    .map_err(|error| Error::Other(error.to_string()))?;
            }
            Event::End(_) | Event::Eof => break,
            _ => {}
        }
        let after = reader.buffer_position() as usize;
        if !xml.as_bytes()[before..after]
            .iter()
            .all(u8::is_ascii_whitespace)
        {
            spans.push(before..after);
        }
    }
    Ok(spans)
}

pub(super) fn without_property_children(xml: &str, locals: &[&str]) -> Result<String> {
    let mut spans = Vec::new();
    for local in locals {
        spans.extend(direct_word_element_spans(xml, local)?);
    }
    spans.sort_by_key(|span| span.start);
    let mut output = xml.to_owned();
    for span in spans.into_iter().rev() {
        output.replace_range(span, "");
    }
    Ok(output)
}

fn property_children(xml: &str, retained: &[&str]) -> Result<PropertyChildren> {
    if xml.is_empty() {
        return Ok(Vec::new());
    }
    let word = word_element_starts(xml)?;
    let mut counts = HashMap::<String, usize>::new();
    let mut output = Vec::new();
    for span in direct_element_spans(xml)? {
        // Foreign children are source slots too. Their ordinal key relates
        // the retained source payload to the typed writer's positioned raw
        // payload without interpreting or rewriting opaque XML.
        let local = if word.contains(&span.start) && !retained.contains(&&xml[span.clone()]) {
            element_local_name(&xml[span.clone()]).to_owned()
        } else {
            "#opaque".to_owned()
        };
        let occurrence = counts.entry(local.clone()).or_default();
        output.push(((local, *occurrence), span));
        *occurrence += 1;
    }
    Ok(output)
}

/// Apply only modeled Word-child deltas to an actual source property shell.
/// Foreign children, root attributes, QName and their bindings stay together.
pub(super) fn patch_property_source(
    before: &str,
    after: &str,
    source: Option<&str>,
    ids: &IdAllocator,
) -> Result<String> {
    let Some(source) = source else {
        return ids.generated(after.to_owned());
    };
    if before == after {
        return Ok(source.to_owned());
    }
    let source_children = property_children(source, &[])?;
    let retained = source_children
        .iter()
        .filter(|(key, _)| key.0 == "#opaque")
        .map(|(_, span)| &source[span.clone()])
        .collect::<Vec<_>>();
    let before_children = property_children(before, &retained)?;
    let after_children = property_children(after, &retained)?;
    let mut output = source.to_owned();
    let mut replacements = Vec::new();
    for (key, span) in &source_children {
        if key.0 == "#opaque" {
            continue;
        }
        let left = before_children
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, span)| &before[span.clone()]);
        let right = after_children
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, span)| &after[span.clone()]);
        if left == right {
            continue;
        }
        let replacement = if matches!(key.0.as_str(), "pPr" | "rPr" | "tblPr" | "sectPr") {
            let child = source_child(source, span.clone())?;
            patch_property_source(left.unwrap_or(""), right.unwrap_or(""), Some(&child), ids)?
        } else {
            match right {
                Some(child) => ids.generated(child.to_owned())?,
                None => String::new(),
            }
        };
        replacements.push((span.clone(), replacement));
    }
    for (span, replacement) in replacements.into_iter().rev() {
        output.replace_range(span, &replacement);
    }
    for (index, (key, span)) in after_children.iter().enumerate() {
        if source_children
            .iter()
            .any(|(candidate, _)| candidate == key)
        {
            continue;
        }
        let child = source_child(after, span.clone())?;
        let existing = property_children(&output, &retained)?;
        let next = after_children[index + 1..]
            .iter()
            .find_map(|(candidate, _)| {
                existing
                    .iter()
                    .find(|(key, _)| key == candidate)
                    .map(|(_, span)| span.start)
            });
        if let Some(start) = next {
            output.insert_str(start, &child);
        } else {
            output = append_word_child(&output, element_local_name(source), &child)?;
        }
    }
    Ok(output)
}

pub(super) fn preserve_prior_property_sources(current: &str, source: &str) -> Result<String> {
    let local = element_local_name(current);
    let change_local = match local {
        "pPr" => "pPrChange",
        "rPr" => "rPrChange",
        "tblPr" => "tblPrChange",
        "sectPr" => "sectPrChange",
        _ => return Ok(current.to_owned()),
    };
    let mut output = current.to_owned();
    if let Some(change) = direct_property(&output, change_local)? {
        let previous = if local == "pPr" {
            without_property_children(source, &["rPr", "sectPr", "pPrChange"])?
        } else {
            source.to_owned()
        };
        let change = replace_property_slot(&change, local, &previous)?;
        output = replace_property_slot(&output, change_local, &change)?;
    }
    for local in ["rPr", "sectPr"] {
        if let (Some(child), Some(prior)) = (
            direct_property(&output, local)?,
            direct_property(source, local)?,
        ) {
            let child = preserve_prior_property_sources(&child, &prior)?;
            output = replace_property_slot(&output, local, &child)?;
        }
    }
    Ok(output)
}

pub(super) fn replace_property_slot(xml: &str, local: &str, replacement: &str) -> Result<String> {
    let spans = direct_word_element_spans(xml, local)?;
    if let Some(span) = spans.first() {
        let mut output = xml.to_owned();
        output.replace_range(span.clone(), replacement);
        return Ok(output);
    }
    if replacement.is_empty() {
        return Ok(xml.to_owned());
    }
    let later = match local {
        "rPr" => ["sectPr", "pPrChange"].as_slice(),
        "sectPr" => ["pPrChange"].as_slice(),
        _ => [].as_slice(),
    };
    let at = later
        .iter()
        .map(|local| direct_word_element_spans(xml, local))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .map(|span| span.start)
        .min();
    if let Some(at) = at {
        let mut output = xml.to_owned();
        output.insert_str(at, replacement);
        Ok(output)
    } else {
        append_word_child(xml, element_local_name(xml), replacement)
    }
}

impl<'a> PropertySide<'a> {
    pub(super) fn of(paragraph: &'a CT_P, metadata: &Metadata<'_>) -> Result<Self> {
        let xml = if let Some(source) = metadata.paragraph_source(paragraph) {
            direct_property(source, "pPr")?
        } else {
            paragraph
                .properties
                .as_ref()
                .map(property_xml)
                .transpose()?
                .filter(|xml| !xml.is_empty())
                .map(|xml| metadata.ids.generated(xml))
                .transpose()?
        };
        Ok(Self {
            typed: paragraph.properties.as_ref(),
            xml,
        })
    }

    pub(super) fn matched(
        original: &'a CT_P,
        edited: &'a CT_P,
        metadata: &Metadata<'_>,
    ) -> Result<(Self, Self)> {
        let rejected = Self::of(original, metadata)?;
        let mut accepted = Self::of(edited, metadata)?;
        let before = original
            .properties
            .as_ref()
            .map(property_xml)
            .transpose()?
            .unwrap_or_default();
        let mut current = edited.properties.clone().unwrap_or_default();
        let left = original.properties.as_ref().cloned().unwrap_or_default();
        current.revision_xml = left.revision_xml;
        current.revision_xml_positions = left.revision_xml_positions;
        current.numbering_revision_xml = left.numbering_revision_xml;
        current.numbering_revision_xml_positions = left.numbering_revision_xml_positions;
        current.numbering_revision_position = left.numbering_revision_position;
        let mut mark = CT_R {
            properties: current.rpr.take(),
            ..CT_R::new("")
        };
        let original_mark = CT_R {
            properties: original.properties.as_ref().and_then(|p| p.rpr.clone()),
            ..CT_R::new("")
        };
        preserve_unmodeled_run_properties(&mut mark, &original_mark);
        current.rpr = mark
            .properties
            .filter(|properties| *properties != oxoffice_docx_oxml::properties::CT_RPr::default());
        let after = property_xml(&current)?;
        let xml = patch_property_source(&before, &after, rejected.xml.as_deref(), &metadata.ids)?;
        accepted.xml = (!xml.is_empty()).then_some(xml);
        Ok((accepted, rejected))
    }
}
