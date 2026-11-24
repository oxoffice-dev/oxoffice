//! Setting values by tag or alias: preparing a value, the controls of the body and of the other stories, and the custom XML they are bound to.

use super::*;

fn prepare_control_value(control: &CT_Sdt, value: &str) -> Result<ControlValue> {
    let kind = control
        .properties
        .as_ref()
        .and_then(|properties| properties.control_type);
    if kind == Some(SdtType::CheckBox) {
        let checked = match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => true,
            "false" | "0" | "no" => false,
            _ => {
                return Err(Error::Other(
                    "checkbox value must be true or false (also 1/0, yes/no)".to_owned(),
                ));
            }
        };
        let (glyph, font) = control
            .properties
            .as_ref()
            .unwrap()
            .checkbox_state(checked)?;
        return Ok(ControlValue {
            display: glyph.to_string(),
            bound: checked.to_string(),
            checkbox: Some((checked, font)),
            full_date: None,
        });
    }
    if matches!(kind, Some(SdtType::Picture | SdtType::Group)) {
        return Err(Error::Other(
            "selected content-control kind does not support a text value".to_owned(),
        ));
    }
    let (display, bound, full_date) = if let Some(properties) = &control.properties {
        properties.prepare_text_value(value)?
    } else {
        (value.to_owned(), value.to_owned(), None)
    };
    Ok(ControlValue {
        display,
        bound,
        checkbox: None,
        full_date,
    })
}

impl Document {
    /// Write the value `select` gives each control it selects. The body's
    /// controls change in the typed model. Those of the other stories change
    /// in their parts, as [`Self::fill_form`] writes a value, with the checks
    /// and display rules of the body's.
    pub(super) fn apply_content_control_values(
        &mut self,
        select: impl Fn(ContentControlRef<'_>) -> Option<String>,
    ) -> Result<usize> {
        let selected = self
            .document
            .body
            .content_controls()
            .into_iter()
            .enumerate()
            .filter_map(|(index, inner)| {
                let value = select(ContentControlRef::borrowed(inner))?;
                let binding = inner
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.data_binding.clone());
                Some(prepare_control_value(inner, &value).map(|value| (index, value, binding)))
            })
            .collect::<Result<Vec<_>>>()?;
        refuse_nested_selection(&self.document.body, &selected)?;

        let body = if selected.is_empty() {
            None
        } else {
            let replacements = ControlReplacements {
                values: selected
                    .iter()
                    .map(|(index, value, _)| (*index, value.clone()))
                    .collect(),
                placeholder_styles: placeholder_style_ids(&self.styles),
            };
            let mut staged_document = self.document.clone();
            replace_document_controls(&mut staged_document.body, &replacements)?;
            staged_document.to_xml()?;

            let mut staged_package = self.package.clone();
            let mut edits_by_part: HashMap<String, Vec<XmlEdit>> = HashMap::new();
            for (_, value, binding) in &selected {
                let Some(binding) = binding else {
                    continue;
                };
                let (part_name, edit) = resolve_binding_edit(
                    &staged_package,
                    &self.doc_part_name,
                    binding,
                    &value.bound,
                )?;
                edits_by_part.entry(part_name).or_default().push(edit);
            }
            for (part_name, edits) in edits_by_part {
                let original = staged_package.get_part(&part_name).ok_or_else(|| {
                    Error::Other(format!("custom XML part {part_name} is missing"))
                })?;
                let changed = apply_xml_edits(original, edits)?;
                staged_package.set_part(&part_name, changed);
            }
            Some((staged_document, staged_package))
        };

        // The other stories, read as fill_form reads them: a story that
        // cannot be read fails the call, as it fails fill_form.
        let mut candidate = self.clone_for_staging();
        if let Some((document, package)) = &body {
            candidate.document = document.clone();
            candidate.package = package.clone();
        }
        candidate.flush_to_package()?;
        let stories = story_controls(&candidate)?;
        let picked = stories
            .iter()
            .filter_map(|story| {
                let value = select(ContentControlRef::borrowed(&story.control))?;
                Some(prepare_control_value(&story.control, &value).map(|value| (story, value)))
            })
            .collect::<Result<Vec<_>>>()?;

        if picked.is_empty() {
            let Some((document, package)) = body else {
                return Ok(0);
            };
            self.document = document;
            self.package = package;
            self.invalidate_layout();
            return Ok(selected.len());
        }

        write_story_control_values(&mut candidate, &picked)?;
        self.commit_staged_mutation(candidate.prepare_and_reopen_staged()?);
        Ok(selected.len() + picked.len())
    }
}

/// Refuse two selected body controls of which one holds the other, as the
/// story controls are refused: the outer value would overwrite the inner one.
/// `selected` holds indices of `body.content_controls()`.
fn refuse_nested_selection(
    body: &CT_Body,
    selected: &[(usize, ControlValue, Option<CT_DataBinding>)],
) -> Result<()> {
    if selected.len() < 2 {
        return Ok(());
    }
    let controls = body.content_controls();
    for (index, _, _) in selected {
        let nested = controls[*index].nested_content_controls();
        if selected.iter().any(|(other, _, _)| {
            nested
                .iter()
                .any(|control| std::ptr::eq(*control, controls[*other]))
        }) {
            return Err(Error::Other(
                "content controls nested in one another cannot take values at once".to_owned(),
            ));
        }
    }
    Ok(())
}

/// The typed content controls of every story but the body's model, as
/// [`Document::fill_form`] finds them: those of the body's text boxes, then
/// those of the related parts in story order, each part's in document order,
/// nested controls included. A control the typed parser does not read is left
/// out, as the body's model keeps one as raw XML. `document` is flushed.
pub(super) fn story_controls(document: &Document) -> Result<Vec<StoryControl>> {
    let (controls, _, text_boxes) = document.accepted_form_regions()?;
    let main = document.doc_part_name.as_str();
    let mut parts: Vec<(String, Vec<Range<usize>>)> = Vec::new();
    for (part, range) in controls {
        // The body's own controls are those of its model.
        if part == main
            && !text_boxes.iter().any(|(owner, text_box)| {
                owner == main && text_box.start <= range.start && range.end <= text_box.end
            })
        {
            continue;
        }
        match parts.iter_mut().find(|(name, _)| *name == part) {
            Some((_, ranges)) => ranges.push(range),
            None => parts.push((part, vec![range])),
        }
    }
    let mut found = Vec::new();
    for (part, mut ranges) in parts {
        ranges.sort_by_key(|range| (range.start, range.end));
        let xml = document
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other(format!("story part {part} is missing")))?;
        for range in ranges {
            let scope = crate::document::story_namespace_scope_at(xml, range.start)?;
            let closed =
                crate::document::close_content_fragment_namespaces(&xml[range.clone()], &scope)?;
            let Some(control) = std::str::from_utf8(&closed)
                .ok()
                .and_then(|text| parse_control(text).ok())
            else {
                continue;
            };
            found.push(StoryControl {
                part: part.clone(),
                start: range.start,
                end: range.end,
                control,
            });
        }
    }
    Ok(found)
}

/// Write `picked`, values prepared as the body's are, into the controls'
/// parts of the flushed `candidate`, and their bound custom XML values. Two
/// selected controls of which one holds the other are refused, as one value
/// would overwrite the other.
fn write_story_control_values(
    candidate: &mut Document,
    picked: &[(&StoryControl, ControlValue)],
) -> Result<()> {
    let placeholder_styles = placeholder_style_ids(&candidate.styles);
    let mut parts: Vec<&str> = Vec::new();
    let mut patches = Vec::with_capacity(picked.len());
    for (story, value) in picked {
        if !parts.contains(&story.part.as_str()) {
            parts.push(&story.part);
        }
        patches.push((*story, story_value_patch(&story.control, value)?));
    }
    // From the last control to the first, so that each start holds.
    patches.sort_by_key(|(story, _)| std::cmp::Reverse(story.start));
    for part in parts {
        let in_part: Vec<&(&StoryControl, ControlPatch)> = patches
            .iter()
            .filter(|(story, _)| story.part == part)
            .collect();
        if in_part
            .windows(2)
            .any(|pair| pair[1].0.end > pair[0].0.start)
        {
            return Err(Error::Other(
                "content controls nested in one another cannot take values at once".to_owned(),
            ));
        }
        let mut xml = candidate
            .package
            .get_part(part)
            .ok_or_else(|| Error::Other(format!("story part {part} is missing")))?
            .to_vec();
        for (story, patch) in in_part {
            apply_control_patch(&mut xml, story.start, patch, &placeholder_styles)?;
        }
        candidate.write_story_part(part, xml)?;
    }
    let mut edits_by_part: HashMap<String, Vec<XmlEdit>> = HashMap::new();
    for (story, value) in picked {
        let Some(binding) = story
            .control
            .properties
            .as_ref()
            .and_then(|properties| properties.data_binding.as_ref())
        else {
            continue;
        };
        let (part_name, edit) = resolve_binding_edit(
            &candidate.package,
            &candidate.doc_part_name,
            binding,
            &value.bound,
        )?;
        edits_by_part.entry(part_name).or_default().push(edit);
    }
    for (part_name, edits) in edits_by_part {
        let original = candidate
            .package
            .get_part(&part_name)
            .ok_or_else(|| Error::Other(format!("custom XML part {part_name} is missing")))?;
        let changed = apply_xml_edits(original, edits)?;
        candidate.package.set_part(&part_name, changed);
    }
    Ok(())
}

/// The part edit that gives a story control `value`, as `replace_one_control`
/// gives one of the body: the prompt flag goes, a date takes its full date,
/// a check box its state, and the display text changes when it differs, the
/// control is a check box or its content holds tracked changes the value
/// drops.
fn story_value_patch(control: &CT_Sdt, value: &ControlValue) -> Result<ControlPatch> {
    let showing_placeholder = match &control.properties {
        Some(properties) => properties.showing_placeholder()?,
        None => false,
    };
    let mut current = String::new();
    collect_sdt_text(control, &mut current);
    let changes = drop_content_changes(&mut control.clone())?;
    let rewrite = current != value.display || value.checkbox.is_some() || changes > 0;
    Ok(ControlPatch {
        placeholder: control.properties.is_some().then_some(false),
        full_date: match &value.full_date {
            Some(full_date) => FullDateEdit::Set(full_date.clone()),
            None => FullDateEdit::Keep,
        },
        last_value: LastValueEdit::Keep,
        checked: value.checkbox.as_ref().map(|(checked, _)| *checked),
        display: rewrite.then(|| value.display.clone()),
        font: value.checkbox.as_ref().map(|(_, font)| font.clone()),
        prompt: Some(showing_placeholder),
    })
}

/// The ids of the Placeholder Text style: Word's own, and those of the
/// styles named so, as Word in another language writes them (French
/// `Textedelespacerserv`, German `Platzhaltertext`).
pub(super) fn placeholder_style_ids(styles: &CT_Styles) -> Vec<String> {
    let mut ids = vec!["PlaceholderText".to_owned()];
    for style in &styles.styles {
        if style.style_type == StyleType::Character
            && style
                .name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case("Placeholder Text"))
            && !ids.contains(&style.style_id)
        {
            ids.push(style.style_id.clone());
        }
    }
    ids
}
