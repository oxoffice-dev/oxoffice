//! Selecting form fields by rule, checking values and resets against them, and resolving their bound values.

use super::*;

pub(super) fn select_rules(
    document: &Document,
    state: &mut FormState,
    rules: &[FormFieldRule],
) -> Result<Vec<Selection>> {
    if rules.iter().any(|rule| {
        rules
            .iter()
            .filter(|other| other.target == rule.target)
            .count()
            > 1
    }) {
        return Err(form_fault("duplicate form target"));
    }
    let mut selections = Vec::new();
    for rule in rules {
        let owners = resolve_target(state, &rule.target)?;
        check_reset(state, &owners, &rule.reset)?;
        selections.push(Selection {
            target: rule.target.clone(),
            owners,
        });
    }
    ensure_disjoint(state, &selections)?;
    resolve_bindings(document, state, &selections)?;
    let mut reset_bindings = Vec::new();
    for (rule, selection) in rules.iter().zip(&selections) {
        for owner in &selection.owners {
            if let Owner::Control(index) = owner
                && let Some(value) = reset_binding_value(&state.controls[*index], &rule.reset)?
            {
                let binding = state.controls[*index]
                    .binding
                    .clone()
                    .ok_or_else(|| form_fault("form binding is incomplete"))?;
                reset_bindings.push(BindingWrite { binding, value });
            }
        }
    }
    preview_bindings(document, &state.doc_part, &reset_bindings)?;
    Ok(selections)
}

fn reset_binding_value(
    control: &PhysicalControl,
    reset: &FormResetValue,
) -> Result<Option<String>> {
    if control.binding.is_none() {
        return Ok(None);
    }
    match reset {
        FormResetValue::Placeholder { .. } => Ok(Some(String::new())),
        FormResetValue::Value(FormFieldValue::Checked(checked)) => Ok(Some(checked.to_string())),
        FormResetValue::Value(FormFieldValue::Text(text)) => {
            if control.kind == ControlKind::Date && date_metadata_unsupported(control) {
                return Ok(Some(text.clone()));
            }
            let (_, bound, _) = prepare_control_text(control, text)?;
            Ok(Some(bound))
        }
        FormResetValue::Value(FormFieldValue::SelectedIndex(_)) => {
            Err(form_fault("form value kind is not valid for the target"))
        }
    }
}

fn resolve_target(state: &FormState, target: &FormFieldTarget) -> Result<Vec<Owner>> {
    match target {
        FormFieldTarget::ContentControlTag(tag) => {
            let matches = state
                .controls
                .iter()
                .enumerate()
                .filter(|(_, control)| control.tag.as_deref() == Some(tag.as_str()))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matches.is_empty() {
                return Err(form_fault("unknown form target"));
            }
            let selected = matches;
            for index in &selected {
                classify_control(&state.controls[*index])?;
            }
            let kind = state.controls[selected[0]].kind;
            if selected
                .iter()
                .any(|index| state.controls[*index].kind != kind)
            {
                return Err(form_fault("form target kinds differ"));
            }
            Ok(selected.into_iter().map(Owner::Control).collect())
        }
        FormFieldTarget::Legacy {
            source_part,
            ordinal,
        } => {
            let index = state
                .legacy
                .iter()
                .position(|field| field.part == *source_part && field.ordinal == *ordinal)
                .ok_or_else(|| form_fault("unknown form target"))?;
            Ok(vec![Owner::Legacy(index)])
        }
    }
}

fn classify_control(control: &PhysicalControl) -> Result<()> {
    if control.duplicate_tag {
        return Err(form_fault("content control identity is duplicated"));
    }
    match control.kind {
        ControlKind::Structural => Err(form_fault(
            "structural form target does not accept a scalar value",
        )),
        ControlKind::Inconsistent => Err(form_fault("content control type is inconsistent")),
        _ => Ok(()),
    }
}

fn check_reset(state: &FormState, owners: &[Owner], reset: &FormResetValue) -> Result<()> {
    match reset {
        FormResetValue::Placeholder { text } => {
            reject_control_text("form placeholder", Some(text))?;
            if owners.iter().any(|owner| !placeholder_owner(state, owner)) {
                return Err(form_fault("placeholder reset is not valid for the target"));
            }
            Ok(())
        }
        FormResetValue::Value(value) => check_value(state, owners, value, true),
    }
}

fn placeholder_owner(state: &FormState, owner: &Owner) -> bool {
    match owner {
        Owner::Legacy(_) => false,
        Owner::Control(index) => matches!(
            state.controls[*index].kind,
            ControlKind::Rich
                | ControlKind::Plain
                | ControlKind::Date
                | ControlKind::Combo
                | ControlKind::DropDown
        ),
    }
}

pub(super) fn check_value(
    state: &FormState,
    owners: &[Owner],
    value: &FormFieldValue,
    legacy_default: bool,
) -> Result<()> {
    for owner in owners {
        match owner {
            Owner::Control(index) => check_control_value(&state.controls[*index], value)?,
            Owner::Legacy(index) => {
                check_legacy_value(&state.legacy[*index], value, legacy_default)?
            }
        }
    }
    Ok(())
}

fn check_control_value(control: &PhysicalControl, value: &FormFieldValue) -> Result<()> {
    match control.kind {
        ControlKind::CheckBox => match value {
            FormFieldValue::Checked(_) => Ok(()),
            _ => Err(form_fault("form value kind is not valid for the target")),
        },
        ControlKind::Rich | ControlKind::Plain => match value {
            FormFieldValue::Text(text) => {
                reject_control_text("form value", Some(text))?;
                Ok(())
            }
            _ => Err(form_fault("form value kind is not valid for the target")),
        },
        ControlKind::Date | ControlKind::Combo | ControlKind::DropDown => match value {
            FormFieldValue::Text(text) => {
                reject_control_text("form value", Some(text))?;
                if control.kind != ControlKind::Date || !date_metadata_unsupported(control) {
                    prepare_control_text(control, text)?;
                }
                Ok(())
            }
            FormFieldValue::SelectedIndex(_) => {
                Err(form_fault("form value kind is not valid for the target"))
            }
            FormFieldValue::Checked(_) => {
                Err(form_fault("form value kind is not valid for the target"))
            }
        },
        ControlKind::Structural => Err(form_fault(
            "structural form target does not accept a scalar value",
        )),
        ControlKind::Inconsistent => Err(form_fault("content control type is inconsistent")),
    }
}

fn check_legacy_value(
    field: &LegacyField,
    value: &FormFieldValue,
    require_default: bool,
) -> Result<()> {
    let legacy = match (field.kind, value) {
        (crate::LegacyFormFieldKind::TextInput, FormFieldValue::Text(text)) => {
            reject_control_text("form value", Some(text))?;
            crate::LegacyFormFieldValue::Text(text.clone())
        }
        (crate::LegacyFormFieldKind::CheckBox, FormFieldValue::Checked(checked)) => {
            crate::LegacyFormFieldValue::Checked(*checked)
        }
        (crate::LegacyFormFieldKind::DropDownList, FormFieldValue::SelectedIndex(index)) => {
            if *index >= field.choices.len() {
                return Err(form_fault("selected index is outside the legacy list"));
            }
            crate::LegacyFormFieldValue::SelectedIndex(*index)
        }
        _ => return Err(form_fault("form value kind is not valid for the target")),
    };
    if require_default && legacy != expected_legacy_reset(field) {
        return Err(form_fault("legacy reset value is not the explicit default"));
    }
    Ok(())
}

fn expected_legacy_reset(field: &LegacyField) -> crate::LegacyFormFieldValue {
    if let Some(value) = &field.default_value {
        return value.clone();
    }
    match field.kind {
        crate::LegacyFormFieldKind::TextInput => crate::LegacyFormFieldValue::Text(String::new()),
        crate::LegacyFormFieldKind::CheckBox => crate::LegacyFormFieldValue::Checked(false),
        crate::LegacyFormFieldKind::DropDownList => crate::LegacyFormFieldValue::SelectedIndex(0),
    }
}

fn ensure_disjoint(state: &FormState, selections: &[Selection]) -> Result<()> {
    let mut controls = Vec::new();
    let mut legacy = Vec::new();
    for selection in selections {
        for owner in &selection.owners {
            match owner {
                Owner::Control(index) => {
                    if controls.contains(index) {
                        return Err(form_fault("duplicate form target"));
                    }
                    if controls.iter().any(|previous| {
                        controls_overlap(&state.controls[*index], &state.controls[*previous])
                    }) {
                        return Err(form_fault("overlapping form targets"));
                    }
                    controls.push(*index);
                }
                Owner::Legacy(index) => {
                    if legacy.contains(index) {
                        return Err(form_fault("duplicate form target"));
                    }
                    legacy.push(*index);
                }
            }
        }
    }
    for legacy_index in legacy {
        let field = &state.legacy[legacy_index];
        if controls.iter().any(|index| {
            control_contains_anchor(
                &state.controls[*index],
                state,
                field.part.as_str(),
                field.ordinal,
            )
        }) {
            return Err(form_fault("overlapping form targets"));
        }
    }
    Ok(())
}

fn controls_overlap(left: &PhysicalControl, right: &PhysicalControl) -> bool {
    left.part == right.part && left.start < right.end && right.start < left.end
}

fn control_contains_anchor(
    control: &PhysicalControl,
    state: &FormState,
    part: &str,
    ordinal: usize,
) -> bool {
    state.anchors.iter().any(|anchor| {
        anchor.part == part
            && anchor.ordinal == ordinal
            && control.part == anchor.part
            && anchor.start >= control.start
            && anchor.end <= control.end
    })
}

fn resolve_bindings(
    document: &Document,
    state: &mut FormState,
    selections: &[Selection],
) -> Result<()> {
    let mut indexes = Vec::new();
    for selection in selections {
        for owner in &selection.owners {
            if let Owner::Control(index) = owner {
                indexes.push(*index);
            }
        }
    }
    indexes.sort_unstable();
    indexes.dedup();
    for index in indexes {
        let Some(binding) = state.controls[index].binding.clone() else {
            continue;
        };
        if binding.store_item_id.is_empty() || binding.xpath.is_empty() {
            return Err(form_fault("form binding is incomplete"));
        }
        let text = read_bound_scalar(document, &state.doc_part, &binding)?;
        state.controls[index].bound_text = Some(text);
    }
    Ok(())
}

fn read_bound_scalar(document: &Document, doc_part: &str, binding: &BindingRef) -> Result<String> {
    let (_, selection, xml) = locate_binding(document, doc_part, binding)?;
    if selection.empty {
        return Ok(String::new());
    }
    let raw = xml
        .get(selection.replace_start..selection.replace_end)
        .ok_or_else(|| form_fault("content-control custom XML selection is out of bounds"))?;
    let text = std::str::from_utf8(raw).map_err(|error| {
        form_fault(format!(
            "content-control custom XML text is invalid: {error}"
        ))
    })?;
    quick_xml::escape::unescape(&oxoffice_core::xml_text::normalize_line_ends(text))
        .map(|value| value.into_owned())
        .map_err(|error| {
            form_fault(format!(
                "content-control custom XML text is invalid: {error}"
            ))
        })
}

pub(super) fn locate_binding<'a>(
    document: &'a Document,
    doc_part: &str,
    binding: &BindingRef,
) -> Result<(String, XmlSelection, &'a [u8])> {
    let mappings = parse_prefix_mappings(binding.prefix_mappings.as_deref().unwrap_or(""))?;
    let steps = parse_xpath(&binding.xpath, &mappings)?;
    let part = resolve_custom_xml_part(&document.package, doc_part, &binding.store_item_id)?;
    let xml = document
        .package
        .get_part(&part)
        .ok_or_else(|| form_fault(format!("custom XML part {part} is missing")))?;
    let selection = select_xml_element(xml, &steps)?;
    Ok((part, selection, xml))
}
