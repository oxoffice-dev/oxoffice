//! Planning a fill or a reset of a form and applying the plan: prepared text, value and placeholder patches, legacy fields and bound values.

use super::*;

pub(super) struct LogicalPatch {
    part: String,
    ordinal: usize,
    patch: ControlPatch,
}

pub(super) struct LegacyWrite {
    part: String,
    ordinal: usize,
    value: crate::LegacyFormFieldValue,
}

pub(super) fn prepare_control_text(
    control: &PhysicalControl,
    value: &str,
) -> Result<(String, String, Option<String>)> {
    let markup = match control.kind {
        ControlKind::Date => date_shell(control)?,
        ControlKind::Combo => list_shell("comboBox", &control.list_items)?,
        ControlKind::DropDown => list_shell("dropDownList", &control.list_items)?,
        _ => {
            return Ok((value.to_owned(), value.to_owned(), None));
        }
    };
    let parsed = parse_control(&markup)?;
    parsed
        .properties
        .as_ref()
        .ok_or_else(|| form_fault("form value metadata is missing"))?
        .prepare_text_value(value)
        .map_err(Error::from)
}

fn date_shell(control: &PhysicalControl) -> Result<String> {
    let facts = control
        .date
        .as_ref()
        .ok_or_else(|| form_fault("unsupported date metadata"))?;
    if let Some(issue) = date_format_issue(facts) {
        return Err(Error::Other(issue));
    }
    if date_metadata_unsupported(control) {
        return Err(form_fault("unsupported date metadata"));
    }
    let mut children = String::new();
    if let Some(format) = &facts.format {
        children.push_str(&prefixed_val("w", "dateFormat", format));
    }
    if let Some(language) = &facts.language {
        children.push_str(&prefixed_val("w", "lid", language));
    }
    if let Some(store) = &facts.store {
        children.push_str(&prefixed_val("w", "storeMappedDataAs", store));
    }
    if let Some(calendar) = &facts.calendar {
        children.push_str(&prefixed_val("w", "calendar", calendar));
    }
    Ok(format!(
        "<w:sdt xmlns:w=\"{W_NS}\"><w:sdtPr><w:date>{children}</w:date></w:sdtPr><w:sdtContent/></w:sdt>"
    ))
}

fn list_shell(local: &str, items: &[ListChoice]) -> Result<String> {
    let mut children = String::new();
    for item in items {
        let mut attributes = String::new();
        if let Some(display) = &item.display {
            attributes.push_str(&format!(" w:displayText=\"{}\"", xml_attr(display)));
        }
        if let Some(value) = &item.value {
            attributes.push_str(&format!(" w:value=\"{}\"", xml_attr(value)));
        }
        children.push_str(&format!("<w:listItem{attributes}/>"));
    }
    Ok(format!(
        "<w:sdt xmlns:w=\"{W_NS}\"><w:sdtPr><w:{local}>{children}</w:{local}></w:sdtPr><w:sdtContent/></w:sdt>"
    ))
}

pub(super) fn plan_fill(
    document: &Document,
    state: &FormState,
    assignments: &[FormFieldAssignment],
    selections: &[Selection],
) -> Result<FormPlan> {
    if assignments.iter().any(|assignment| {
        assignments
            .iter()
            .filter(|other| other.target == assignment.target)
            .count()
            > 1
    }) {
        return Err(form_fault("duplicate form target"));
    }
    let mut plan = empty_plan(state, selections);
    for assignment in assignments {
        let selection = selections
            .iter()
            .find(|selection| selection.target == assignment.target)
            .ok_or_else(|| form_fault("unknown form assignment"))?;
        check_value(state, &selection.owners, &assignment.value, false)?;
        plan.counts.push(selection.owners.len());
        for owner in &selection.owners {
            push_write(
                state,
                owner,
                &FormResetValue::Value(assignment.value.clone()),
                &mut plan,
            )?;
        }
    }
    preview_bindings(document, &state.doc_part, &plan.bindings)?;
    Ok(plan)
}

pub(super) fn plan_reset(
    document: &Document,
    state: &FormState,
    rules: &[FormFieldRule],
    selections: &[Selection],
) -> Result<FormPlan> {
    let mut plan = empty_plan(state, selections);
    for (rule, selection) in rules.iter().zip(selections) {
        plan.counts.push(selection.owners.len());
        for owner in &selection.owners {
            push_write(state, owner, &rule.reset, &mut plan)?;
        }
    }
    preview_bindings(document, &state.doc_part, &plan.bindings)?;
    Ok(plan)
}

fn empty_plan(state: &FormState, selections: &[Selection]) -> FormPlan {
    let watched_legacy = selections
        .iter()
        .flat_map(|selection| selection.owners.iter())
        .filter_map(|owner| match owner {
            Owner::Legacy(index) => Some((
                state.legacy[*index].part.clone(),
                state.legacy[*index].ordinal,
            )),
            Owner::Control(_) => None,
        })
        .collect();
    FormPlan {
        counts: Vec::new(),
        legacy: Vec::new(),
        patches: Vec::new(),
        bindings: Vec::new(),
        watched_legacy,
    }
}

fn push_write(
    state: &FormState,
    owner: &Owner,
    reset: &FormResetValue,
    plan: &mut FormPlan,
) -> Result<()> {
    match owner {
        Owner::Legacy(index) => {
            let FormResetValue::Value(value) = reset else {
                return Err(form_fault("placeholder reset is not valid for the target"));
            };
            plan.legacy.push(LegacyWrite {
                part: state.legacy[*index].part.clone(),
                ordinal: state.legacy[*index].ordinal,
                value: legacy_value(&state.legacy[*index], value)?,
            });
            Ok(())
        }
        Owner::Control(index) => {
            let control = &state.controls[*index];
            let (patch, bound) = match reset {
                FormResetValue::Value(value) => value_patch(control, value)?,
                FormResetValue::Placeholder { text } => placeholder_patch(control, text)?,
            };
            if let Some(value) = bound {
                let binding = control
                    .binding
                    .clone()
                    .ok_or_else(|| form_fault("form binding is incomplete"))?;
                plan.bindings.push(BindingWrite { binding, value });
            }
            plan.patches.push(LogicalPatch {
                part: control.part.clone(),
                ordinal: control.ordinal,
                patch,
            });
            Ok(())
        }
    }
}

fn legacy_value(
    field: &LegacyField,
    value: &FormFieldValue,
) -> Result<crate::LegacyFormFieldValue> {
    match (field.kind, value) {
        (crate::LegacyFormFieldKind::TextInput, FormFieldValue::Text(text)) => {
            Ok(crate::LegacyFormFieldValue::Text(text.clone()))
        }
        (crate::LegacyFormFieldKind::CheckBox, FormFieldValue::Checked(checked)) => {
            Ok(crate::LegacyFormFieldValue::Checked(*checked))
        }
        (crate::LegacyFormFieldKind::DropDownList, FormFieldValue::SelectedIndex(index)) => {
            Ok(crate::LegacyFormFieldValue::SelectedIndex(*index))
        }
        _ => Err(form_fault("form value kind is not valid for the target")),
    }
}

fn value_patch(
    control: &PhysicalControl,
    value: &FormFieldValue,
) -> Result<(ControlPatch, Option<String>)> {
    let mut font = None;
    let (display, bound, full_date, checked) = match control.kind {
        ControlKind::CheckBox => {
            let FormFieldValue::Checked(checked) = value else {
                return Err(form_fault("form value kind is not valid for the target"));
            };
            let (glyph, state_font, fault) = if *checked {
                (
                    control.checked_glyph.clone(),
                    control.checked_font.clone(),
                    control.checked_fault.clone(),
                )
            } else {
                (
                    control.unchecked_glyph.clone(),
                    control.unchecked_font.clone(),
                    control.unchecked_fault.clone(),
                )
            };
            if let Some(fault) = fault {
                return Err(form_fault(fault));
            }
            let glyph =
                glyph.ok_or_else(|| form_fault("checkbox state glyph is not a Unicode scalar"))?;
            font = state_font;
            (glyph, checked.to_string(), None, Some(*checked))
        }
        ControlKind::Rich
        | ControlKind::Plain
        | ControlKind::Date
        | ControlKind::Combo
        | ControlKind::DropDown => {
            let FormFieldValue::Text(text) = value else {
                return Err(form_fault("form value kind is not valid for the target"));
            };
            let (display, bound, full_date) = prepare_control_text(control, text)?;
            (display, bound, full_date, None)
        }
        ControlKind::Structural => {
            return Err(form_fault(
                "structural form target does not accept a scalar value",
            ));
        }
        ControlKind::Inconsistent => {
            return Err(form_fault("content control type is inconsistent"));
        }
    };
    let patch = ControlPatch {
        placeholder: Some(false),
        full_date: match full_date {
            Some(value) => FullDateEdit::Set(value),
            None if control.kind == ControlKind::Date => FullDateEdit::Clear,
            None => FullDateEdit::Keep,
        },
        last_value: if matches!(control.kind, ControlKind::Combo | ControlKind::DropDown) {
            LastValueEdit::Set(bound.clone())
        } else {
            LastValueEdit::Keep
        },
        checked,
        display: Some(display),
        font,
        prompt: Some(control.placeholder),
    };
    let bound = control.binding.as_ref().map(|_| bound);
    Ok((patch, bound))
}

fn placeholder_patch(
    control: &PhysicalControl,
    text: &str,
) -> Result<(ControlPatch, Option<String>)> {
    if !matches!(
        control.kind,
        ControlKind::Rich
            | ControlKind::Plain
            | ControlKind::Date
            | ControlKind::Combo
            | ControlKind::DropDown
    ) {
        return Err(form_fault("placeholder reset is not valid for the target"));
    }
    let patch = ControlPatch {
        placeholder: Some(true),
        full_date: if control.kind == ControlKind::Date {
            FullDateEdit::Clear
        } else {
            FullDateEdit::Keep
        },
        last_value: if matches!(control.kind, ControlKind::Combo | ControlKind::DropDown) {
            LastValueEdit::Clear
        } else {
            LastValueEdit::Keep
        },
        checked: None,
        display: Some(text.to_owned()),
        font: None,
        prompt: None,
    };
    let bound = control.binding.as_ref().map(|_| String::new());
    Ok((patch, bound))
}

pub(super) fn preview_bindings(
    document: &Document,
    doc_part: &str,
    writes: &[BindingWrite],
) -> Result<()> {
    let _ = apply_binding_groups(document, doc_part, writes)?;
    Ok(())
}

fn apply_binding_groups(
    document: &Document,
    doc_part: &str,
    writes: &[BindingWrite],
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut grouped: Vec<(String, Vec<XmlEdit>)> = Vec::new();
    for write in writes {
        let (part, selection, xml) = locate_binding(document, doc_part, &write.binding)?;
        let replacement = if selection.empty {
            expand_empty_element(xml, &selection, &write.value)?
        } else {
            quick_xml::escape::escape(&write.value)
                .into_owned()
                .into_bytes()
        };
        let edit = XmlEdit {
            start: selection.replace_start,
            end: selection.replace_end,
            replacement,
        };
        if let Some((_, edits)) = grouped.iter_mut().find(|(name, _)| name == &part) {
            edits.push(edit);
        } else {
            grouped.push((part, vec![edit]));
        }
    }
    let mut updated = Vec::new();
    for (part, edits) in grouped {
        let xml = document
            .package
            .get_part(&part)
            .ok_or_else(|| form_fault(format!("custom XML part {part} is missing")))?;
        updated.push((part, apply_xml_edits(xml, edits)?));
    }
    Ok(updated)
}

pub(super) fn apply_form_plan(
    candidate: &mut Document,
    state: &FormState,
    plan: &FormPlan,
) -> Result<()> {
    ensure_legacy_stable(state, plan)?;
    for write in &plan.legacy {
        candidate.set_legacy_form_field_value(&write.part, write.ordinal, write.value.clone())?;
    }
    candidate.flush_to_package()?;
    let fresh = read_form_state(candidate)?;
    let mut buffers: Vec<(String, Vec<u8>)> = Vec::new();
    let mut located = Vec::new();
    for patch in &plan.patches {
        let control = fresh
            .controls
            .iter()
            .find(|control| control.part == patch.part && control.ordinal == patch.ordinal)
            .ok_or_else(|| form_fault("stale form target"))?;
        if !buffers.iter().any(|(part, _)| part == &patch.part) {
            let xml = candidate
                .package
                .get_part(&patch.part)
                .ok_or_else(|| form_fault(format!("form story part {} is missing", patch.part)))?
                .to_vec();
            buffers.push((patch.part.clone(), xml));
        }
        located.push((patch.part.clone(), control.start, patch.patch.clone()));
    }
    located.sort_by_key(|item| std::cmp::Reverse(item.1));
    let placeholder_styles = placeholder_style_ids(&candidate.styles);
    for (part, start, patch) in &located {
        let xml = buffers
            .iter_mut()
            .find(|(name, _)| name == part)
            .map(|(_, xml)| xml)
            .ok_or_else(|| form_fault("form story part is missing"))?;
        apply_control_patch(xml, *start, patch, &placeholder_styles)?;
    }
    for (part, xml) in buffers {
        candidate.write_story_part(&part, xml)?;
    }
    for (part, xml) in apply_binding_groups(candidate, &candidate.doc_part_name, &plan.bindings)? {
        candidate.package.set_part(&part, xml);
    }
    Ok(())
}

fn ensure_legacy_stable(state: &FormState, plan: &FormPlan) -> Result<()> {
    let mut parts = Vec::new();
    for patch in &plan.patches {
        if !parts.iter().any(|part| part == &patch.part) {
            parts.push(patch.part.clone());
        }
    }
    for part in parts {
        let mut contained = Vec::new();
        for patch in plan.patches.iter().filter(|patch| patch.part == part) {
            let Some(control) = state
                .controls
                .iter()
                .find(|control| control.part == patch.part && control.ordinal == patch.ordinal)
            else {
                continue;
            };
            for anchor in &state.anchors {
                if anchor.part == part && anchor.start >= control.start && anchor.end <= control.end
                {
                    contained.push(anchor.ordinal);
                }
            }
        }
        let Some(minimum) = contained.into_iter().min() else {
            continue;
        };
        if plan
            .watched_legacy
            .iter()
            .any(|(watched, ordinal)| watched == &part && *ordinal >= minimum)
        {
            return Err(form_fault("legacy form target would be reindexed"));
        }
    }
    Ok(())
}
