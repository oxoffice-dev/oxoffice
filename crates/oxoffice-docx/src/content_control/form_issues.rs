//! The current problems of a form field: required and empty, kind, date and check box or list disagreements.

use super::*;

pub(super) fn current_issues(
    state: &FormState,
    rules: &[FormFieldRule],
    selections: &[Selection],
) -> Vec<FormValidationIssue> {
    let mut issues = Vec::new();
    for (rule, selection) in rules.iter().zip(selections) {
        for owner in &selection.owners {
            if let Some(message) = owner_issue(state, owner, rule.required) {
                let (source_part, ordinal) = owner_coordinates(state, owner);
                issues.push(FormValidationIssue {
                    target: rule.target.clone(),
                    source_part,
                    ordinal,
                    message: message.to_owned(),
                });
            }
        }
    }
    issues
}

fn owner_coordinates(state: &FormState, owner: &Owner) -> (String, usize) {
    match owner {
        Owner::Control(index) => (
            state.controls[*index].part.clone(),
            state.controls[*index].ordinal,
        ),
        Owner::Legacy(index) => (
            state.legacy[*index].part.clone(),
            state.legacy[*index].ordinal,
        ),
    }
}

fn owner_issue(state: &FormState, owner: &Owner, required: bool) -> Option<&'static str> {
    match owner {
        Owner::Legacy(index) => legacy_issue(&state.legacy[*index], required),
        Owner::Control(index) => control_issue(&state.controls[*index], required),
    }
}

fn legacy_issue(field: &LegacyField, required: bool) -> Option<&'static str> {
    let empty = matches!(&field.value, crate::LegacyFormFieldValue::Text(text) if is_xml_whitespace(text.as_bytes()));
    if required && empty {
        Some("required value is empty")
    } else {
        None
    }
}

fn control_issue(control: &PhysicalControl, required: bool) -> Option<&'static str> {
    if control.placeholder_broken || control.checked_broken || control.glyph_broken {
        return Some("current form value is invalid");
    }
    if let Some(message) = kind_issue(control) {
        return Some(message);
    }
    if control.placeholder
        && control
            .bound_text
            .as_ref()
            .is_some_and(|text| !text.is_empty())
    {
        return Some("bound scalar is not empty");
    }
    if required && control_is_empty(control) {
        return Some("required value is empty");
    }
    None
}

fn kind_issue(control: &PhysicalControl) -> Option<&'static str> {
    match control.kind {
        ControlKind::Date => date_issue(control),
        ControlKind::CheckBox if checkbox_disagrees(control) => {
            Some("current form value is invalid")
        }
        ControlKind::Combo | ControlKind::DropDown
            if !control.placeholder && list_disagrees(control) =>
        {
            Some("current form value is invalid")
        }
        ControlKind::Rich | ControlKind::Plain
            if !control.placeholder && text_bound_disagrees(control) =>
        {
            Some("current form value is invalid")
        }
        _ => None,
    }
}

fn date_issue(control: &PhysicalControl) -> Option<&'static str> {
    if date_metadata_unsupported(control) {
        return Some("unsupported date metadata");
    }
    if control.placeholder {
        return None;
    }
    let facts = control.date.as_ref()?;
    // A time typed in Word into a control whose format shows one is checked
    // as a date is; a time under a format that shows none is a wrong value.
    let Some((full_date, at)) = facts
        .full_date
        .as_deref()
        .and_then(|full_date| Some((full_date, WordDateTime::parse_full_date(full_date)?)))
    else {
        return Some("current form value is invalid");
    };
    let Ok((display, bound, prepared)) = prepare_control_text(control, &at.value()) else {
        return Some("current form value is invalid");
    };
    if control.display != display || prepared.as_deref() != Some(full_date) {
        return Some("current form value is invalid");
    }
    if control.binding.is_some() && control.bound_text.as_deref() != Some(bound.as_str()) {
        return Some("current form value is invalid");
    }
    None
}

pub(super) fn date_metadata_unsupported(control: &PhysicalControl) -> bool {
    let Some(facts) = &control.date else {
        return true;
    };
    facts.duplicate
        || !matches!(
            facts.calendar.as_deref(),
            None | Some("gregorian" | "gregorianUs")
        )
        || date_format_issue(facts).is_some()
        || !matches!(
            facts.store.as_deref(),
            None | Some("text" | "date" | "dateTime")
        )
}

/// Why the control's date format cannot be shown, if it cannot.
pub(super) fn date_format_issue(facts: &DateFacts) -> Option<String> {
    let Some(format) = facts.format.as_deref() else {
        return Some("missing date format".to_owned());
    };
    check_word_date_format(format, facts.language.as_deref(), facts.calendar.as_deref())
        .err()
        .map(|error| error.to_string())
}

fn checkbox_disagrees(control: &PhysicalControl) -> bool {
    let Some(checked) = control.checked else {
        return true;
    };
    let glyph = if checked {
        control.checked_glyph.as_deref()
    } else {
        control.unchecked_glyph.as_deref()
    };
    if !control.display.is_empty() && glyph != Some(control.display.as_str()) {
        return true;
    }
    if glyph.is_none() && !control.display.is_empty() {
        return true;
    }
    control
        .bound_text
        .as_ref()
        .is_some_and(|text| text != &checked.to_string())
}

fn list_disagrees(control: &PhysicalControl) -> bool {
    let authority = if control.binding.is_some() {
        control.bound_text.as_deref().unwrap_or("")
    } else {
        control.display.as_str()
    };
    match prepare_control_text(control, authority) {
        Ok((display, bound, _)) => {
            control.display != display
                || control.binding.is_some()
                    && control.bound_text.as_deref() != Some(bound.as_str())
        }
        Err(_) => true,
    }
}

fn text_bound_disagrees(control: &PhysicalControl) -> bool {
    control
        .bound_text
        .as_ref()
        .is_some_and(|text| text != &control.display)
}

fn control_is_empty(control: &PhysicalControl) -> bool {
    match control.kind {
        ControlKind::CheckBox => control.placeholder,
        ControlKind::Rich
        | ControlKind::Plain
        | ControlKind::Date
        | ControlKind::Combo
        | ControlKind::DropDown => {
            control.placeholder || is_xml_whitespace(control.display.as_bytes())
        }
        _ => false,
    }
}
