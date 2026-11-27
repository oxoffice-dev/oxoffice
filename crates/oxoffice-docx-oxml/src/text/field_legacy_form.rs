//! Legacy form fields: reading their form data, and writing and authoring them.

use super::*;

#[derive(Default)]
struct LegacyFormProjection {
    ff_data_last_slot: Option<usize>,
    kind_last_slot: Option<usize>,
    name: Option<String>,
    name_seen: bool,
    enabled: Option<bool>,
    enabled_seen: bool,
    calculate_on_exit: Option<bool>,
    calculate_on_exit_seen: bool,
    entry_macro_seen: bool,
    exit_macro_seen: bool,
    help_text_seen: bool,
    help_text: Option<LegacyFormInfoText>,
    status_text_seen: bool,
    kind: Option<LegacyFormFieldKind>,
    text_type_seen: bool,
    text_input_type: Option<LegacyTextInputType>,
    text_format_seen: bool,
    checkbox_size_seen: bool,
    checkbox_size: Option<LegacyCheckBoxSize>,
    default_text: Option<String>,
    default_text_seen: bool,
    checked: Option<bool>,
    checked_seen: bool,
    check_default: Option<bool>,
    check_default_seen: bool,
    selected_index: Option<usize>,
    selected_index_seen: bool,
    selected_default: Option<usize>,
    selected_default_seen: bool,
    choices: Vec<String>,
    max_length: Option<usize>,
    max_length_seen: bool,
}

pub(super) type PhysicalRunSources = Vec<Option<Vec<u8>>>;
pub(super) type InlineFieldParagraph = (
    CT_P,
    PhysicalRunSources,
    Vec<(usize, Vec<u8>)>,
    Vec<std::ops::Range<usize>>,
);

pub(super) type NestedEffectiveSegment<'a> =
    (std::borrow::Cow<'a, str>, Option<&'a CT_RPr>, &'a Field);
type ParsedLegacyForm = (
    LegacyFormFieldData,
    Option<LegacyFormFieldValue>,
    Option<String>,
);
type LegacyAuthorParts = (
    &'static str,
    String,
    LegacyFormFieldData,
    LegacyFormAuthor,
    Option<LegacyFormFieldValue>,
    Option<String>,
);

pub(super) fn parse_legacy_form_data(
    raw: crate::document::WordXmlInput<'_>,
    word_prefixes: &[String],
    instruction_name: &str,
    cached_result: &str,
    result_published: bool,
) -> Result<Option<ParsedLegacyForm>> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<Option<String>>::new();
    let mut prefixes = word_prefixes.to_vec();
    let mut prefix_scopes = Vec::new();
    let mut begin_depth = None;
    let mut ff_data_depth = None;
    let mut ff_data_count = 0usize;
    let mut kind_depth = None;
    let mut leaf_depth = None;
    let mut projection = LegacyFormProjection::default();

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                if leaf_depth.is_some() {
                    return Err(OxmlError::InvalidValue(
                        "legacy form leaf property contains an element".to_owned(),
                    ));
                }
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                let word = is_word_element(
                    element.name().as_ref(),
                    local_name(element.name().as_ref()),
                    &local_prefixes,
                );
                let local = word.then(|| {
                    String::from_utf8_lossy(local_name(element.name().as_ref())).into_owned()
                });
                stack.push(local.clone());
                let local_prefixes = local_prefixes.into_owned();
                prefix_scopes.push(std::mem::replace(&mut prefixes, local_prefixes));
                let depth = stack.len() - 1;
                if local.as_deref() == Some("fldChar")
                    && begin_depth.is_none()
                    && optional_word_attribute(&element, b"fldCharType", &prefixes).as_deref()
                        == Some("begin")
                {
                    begin_depth = Some(depth);
                } else if local.as_deref() == Some("ffData") && begin_depth == depth.checked_sub(1)
                {
                    ff_data_count += 1;
                    if ff_data_count > 1 {
                        return Err(OxmlError::InvalidValue(
                            "duplicate direct w:ffData owner".to_owned(),
                        ));
                    }
                    ff_data_depth = Some(depth);
                } else if let Some(ff_depth) = ff_data_depth {
                    leaf_depth = project_legacy_form_element(
                        &element,
                        local.as_deref(),
                        depth,
                        ff_depth,
                        &prefixes,
                        &mut kind_depth,
                        &mut projection,
                    )?
                    .then_some(depth);
                }
            }
            Event::Empty(element) => {
                if leaf_depth.is_some() {
                    return Err(OxmlError::InvalidValue(
                        "legacy form leaf property contains an element".to_owned(),
                    ));
                }
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                let word = is_word_element(
                    element.name().as_ref(),
                    local_name(element.name().as_ref()),
                    &local_prefixes,
                );
                let local = word.then(|| {
                    String::from_utf8_lossy(local_name(element.name().as_ref())).into_owned()
                });
                let depth = stack.len();
                if local.as_deref() == Some("fldChar")
                    && begin_depth.is_none()
                    && optional_word_attribute(&element, b"fldCharType", &local_prefixes).as_deref()
                        == Some("begin")
                {
                    begin_depth = Some(depth);
                } else if local.as_deref() == Some("ffData") && begin_depth == depth.checked_sub(1)
                {
                    ff_data_count += 1;
                    if ff_data_count > 1 {
                        return Err(OxmlError::InvalidValue(
                            "duplicate direct w:ffData owner".to_owned(),
                        ));
                    }
                } else if let Some(ff_depth) = ff_data_depth {
                    project_legacy_form_element(
                        &element,
                        local.as_deref(),
                        depth,
                        ff_depth,
                        &local_prefixes,
                        &mut kind_depth,
                        &mut projection,
                    )?;
                    if depth == ff_depth + 1
                        && matches!(local.as_deref(), Some("checkBox" | "ddList" | "textInput"))
                    {
                        kind_depth = None;
                    }
                }
            }
            Event::End(_) => {
                let depth = stack.len().saturating_sub(1);
                if leaf_depth == Some(depth) {
                    leaf_depth = None;
                }
                if kind_depth == Some(depth) {
                    kind_depth = None;
                }
                if ff_data_depth == Some(depth) {
                    ff_data_depth = None;
                }
                let closes_begin = begin_depth == Some(depth);
                stack.pop();
                prefixes = prefix_scopes
                    .pop()
                    .unwrap_or_else(|| word_prefixes.to_vec());
                if closes_begin {
                    break;
                }
            }
            Event::Text(text)
                if (leaf_depth.is_some()
                    || is_legacy_form_element_only_container(
                        stack.len(),
                        ff_data_depth,
                        kind_depth,
                    ))
                    && text.iter().any(|byte| !byte.is_ascii_whitespace()) =>
            {
                return Err(OxmlError::InvalidValue(
                    "legacy form element-only content contains character data".to_owned(),
                ));
            }
            Event::CData(text)
                if leaf_depth.is_some()
                    || (is_legacy_form_element_only_container(
                        stack.len(),
                        ff_data_depth,
                        kind_depth,
                    ) && text.iter().any(|byte| !byte.is_ascii_whitespace())) =>
            {
                return Err(OxmlError::InvalidValue(
                    "legacy form element-only content contains character data".to_owned(),
                ));
            }
            Event::GeneralRef(reference) => {
                let in_element_only_container =
                    is_legacy_form_element_only_container(stack.len(), ff_data_depth, kind_depth);
                let non_whitespace = match reference.resolve_char_ref() {
                    Ok(Some(character)) => !character.is_ascii_whitespace(),
                    Ok(None) | Err(_) => true,
                };
                if leaf_depth.is_some() || (in_element_only_container && non_whitespace) {
                    return Err(OxmlError::InvalidValue(
                        "legacy form element-only content contains character data".to_owned(),
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    let Some(kind) = projection.kind else {
        if ff_data_count == 0 {
            return Ok(None);
        }
        return Err(OxmlError::MissingElement("w:ffData form kind".to_owned()));
    };
    let matches_instruction = matches!(
        (kind, instruction_name),
        (LegacyFormFieldKind::TextInput, "FORMTEXT")
            | (LegacyFormFieldKind::CheckBox, "FORMCHECKBOX")
            | (LegacyFormFieldKind::DropDownList, "FORMDROPDOWN")
    );
    if !matches_instruction {
        return Err(OxmlError::InvalidValue(
            "w:ffData kind contradicts its field instruction".to_owned(),
        ));
    }
    if kind == LegacyFormFieldKind::CheckBox && !projection.checkbox_size_seen {
        return Err(OxmlError::MissingElement(
            "w:checkBox requires w:size or w:sizeAuto".to_owned(),
        ));
    }
    let (value, reset_default, text_default) = match kind {
        LegacyFormFieldKind::TextInput => {
            let current = if result_published {
                cached_result.to_owned()
            } else {
                projection.default_text.clone().unwrap_or_default()
            };
            (
                LegacyFormFieldValue::Text(current),
                projection
                    .default_text
                    .clone()
                    .map(LegacyFormFieldValue::Text),
                projection.default_text,
            )
        }
        LegacyFormFieldKind::CheckBox => (
            LegacyFormFieldValue::Checked(
                projection
                    .checked
                    .or(projection.check_default)
                    .unwrap_or(false),
            ),
            projection.check_default.map(LegacyFormFieldValue::Checked),
            None,
        ),
        LegacyFormFieldKind::DropDownList => {
            let index = if projection.selected_index.is_some() {
                projection.selected_index.unwrap_or(0)
            } else {
                projection.selected_default.unwrap_or(0)
            };
            if projection.choices.get(index).is_none() {
                return Err(OxmlError::InvalidValue(
                    "w:ddList selection is outside its entry list".to_owned(),
                ));
            }
            (
                LegacyFormFieldValue::SelectedIndex(index),
                projection
                    .selected_default
                    .map(LegacyFormFieldValue::SelectedIndex),
                None,
            )
        }
    };
    Ok(Some((
        LegacyFormFieldData {
            name: projection.name,
            enabled: projection.enabled.unwrap_or(true),
            calculate_on_exit: projection.calculate_on_exit.unwrap_or(true),
            kind,
            value,
            choices: projection.choices,
            max_length: projection.max_length,
            checkbox_size: projection.checkbox_size,
            help_text: projection.help_text,
            text_input_type: projection.text_input_type,
        },
        reset_default,
        text_default,
    )))
}

/// The `w:name` of a legacy form field read without checking the rest of
/// its form data: a field whose form data does not read can still be named
/// in a diagnostic. `None` when no begin `w:ffData` holds a readable name.
pub(super) fn lenient_legacy_form_name(
    raw: crate::document::WordXmlInput<'_>,
    word_prefixes: &[String],
) -> Option<String> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut prefixes = word_prefixes.to_vec();
    let mut prefix_scopes = Vec::new();
    let mut begin_depth = None;
    let mut ff_data_depth = None;
    loop {
        let event = reader.read_event_into(&mut buffer).ok()?;
        let (element, empty) = match &event {
            Event::Start(element) => (element, false),
            Event::Empty(element) => (element, true),
            Event::End(_) => {
                depth = depth.checked_sub(1)?;
                prefixes = prefix_scopes.pop()?;
                if ff_data_depth == Some(depth) {
                    ff_data_depth = None;
                }
                if begin_depth == Some(depth) {
                    return None;
                }
                buffer.clear();
                continue;
            }
            Event::Eof => return None,
            _ => {
                buffer.clear();
                continue;
            }
        };
        let local_prefixes = word_prefixes_at(element, &prefixes).ok()?.into_owned();
        let name = element.name();
        let local = is_word_element(name.as_ref(), local_name(name.as_ref()), &local_prefixes)
            .then(|| local_name(name.as_ref()));
        match local {
            Some(b"fldChar")
                if begin_depth.is_none()
                    && optional_word_attribute(element, b"fldCharType", &local_prefixes)
                        .as_deref()
                        == Some("begin") =>
            {
                begin_depth = Some(depth);
            }
            Some(b"ffData") if begin_depth.is_some() && begin_depth == depth.checked_sub(1) => {
                ff_data_depth = Some(depth);
            }
            Some(b"name") if ff_data_depth.is_some() && ff_data_depth == depth.checked_sub(1) => {
                return legacy_form_value_attribute(element, &local_prefixes)
                    .ok()
                    .flatten()
                    .filter(|name| !name.is_empty());
            }
            _ => {}
        }
        if empty {
            if begin_depth == Some(depth) {
                return None;
            }
        } else {
            prefix_scopes.push(std::mem::replace(&mut prefixes, local_prefixes));
            depth += 1;
        }
        buffer.clear();
    }
}

pub(super) fn complex_outer_result_present(raw: crate::document::WordXmlInput<'_>) -> bool {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0i32;
    let mut saw_separate = false;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Empty(element) | Event::Start(element)) => {
                if element.name().as_ref().ends_with(b"fldChar") {
                    let kind = element.attributes().flatten().find_map(|attribute| {
                        attribute
                            .key
                            .as_ref()
                            .ends_with(b"fldCharType")
                            .then(|| {
                                oxoffice_core::xml::attribute_text(&attribute)
                                    .ok()
                                    .map(std::borrow::Cow::into_owned)
                            })
                            .flatten()
                    });
                    match kind.as_deref() {
                        Some("begin") => depth += 1,
                        Some("separate") if depth == 1 => saw_separate = true,
                        Some("end") if depth > 0 => depth -= 1,
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return false,
            _ => {}
        }
        buffer.clear();
    }
    saw_separate
}

fn project_legacy_form_element(
    element: &BytesStart<'_>,
    local: Option<&str>,
    depth: usize,
    ff_depth: usize,
    word_prefixes: &[String],
    kind_depth: &mut Option<usize>,
    projection: &mut LegacyFormProjection,
) -> Result<bool> {
    if let Some(local) = local
        && is_known_legacy_form_vocabulary(local)
    {
        let valid_position = if depth == ff_depth + 1 {
            legacy_form_ff_data_slot(Some(local)).is_some()
        } else if kind_depth.is_some_and(|kind| depth == kind + 1) {
            projection
                .kind
                .is_some_and(|kind| legacy_form_kind_child_slot(kind, local).is_some())
        } else {
            false
        };
        if !valid_position {
            return Err(OxmlError::InvalidValue(format!(
                "w:{local} is at the wrong legacy form container level"
            )));
        }
    }
    let is_leaf = (depth == ff_depth + 1
        && matches!(
            local,
            Some(
                "name"
                    | "enabled"
                    | "calcOnExit"
                    | "entryMacro"
                    | "exitMacro"
                    | "helpText"
                    | "statusText"
            )
        ))
        || (kind_depth.is_some_and(|kind| depth == kind + 1)
            && matches!(
                local,
                Some(
                    "type"
                        | "default"
                        | "maxLength"
                        | "format"
                        | "size"
                        | "sizeAuto"
                        | "checked"
                        | "result"
                        | "listEntry"
                )
            ));
    if depth == ff_depth + 1 {
        if let Some(slot) = legacy_form_ff_data_slot(local) {
            validate_legacy_form_child_order(&mut projection.ff_data_last_slot, slot, "w:ffData")?;
        }
        match local {
            Some("name") => {
                mark_legacy_form_singleton(&mut projection.name_seen, "w:name")?;
                projection.name = Some(required_bounded_legacy_form_value(
                    element,
                    word_prefixes,
                    "w:name",
                    20,
                )?);
            }
            Some("enabled") => {
                mark_legacy_form_singleton(&mut projection.enabled_seen, "w:enabled")?;
                projection.enabled = Some(legacy_form_boolean_value(
                    element,
                    word_prefixes,
                    "w:enabled",
                )?);
            }
            Some("calcOnExit") => {
                mark_legacy_form_singleton(&mut projection.calculate_on_exit_seen, "w:calcOnExit")?;
                projection.calculate_on_exit = Some(legacy_form_boolean_value(
                    element,
                    word_prefixes,
                    "w:calcOnExit",
                )?);
            }
            Some("entryMacro") => {
                mark_legacy_form_singleton(&mut projection.entry_macro_seen, "w:entryMacro")?;
                required_bounded_legacy_form_value(element, word_prefixes, "w:entryMacro", 33)?;
            }
            Some("exitMacro") => {
                mark_legacy_form_singleton(&mut projection.exit_macro_seen, "w:exitMacro")?;
                required_bounded_legacy_form_value(element, word_prefixes, "w:exitMacro", 33)?;
            }
            Some("helpText") => {
                mark_legacy_form_singleton(&mut projection.help_text_seen, "w:helpText")?;
                projection.help_text =
                    validate_legacy_info_text(element, word_prefixes, "w:helpText", 255)?;
            }
            Some("statusText") => {
                mark_legacy_form_singleton(&mut projection.status_text_seen, "w:statusText")?;
                validate_legacy_info_text(element, word_prefixes, "w:statusText", 140)?;
            }
            Some("textInput") => set_legacy_form_kind(
                projection,
                LegacyFormFieldKind::TextInput,
                depth,
                kind_depth,
            )?,
            Some("checkBox") => {
                set_legacy_form_kind(projection, LegacyFormFieldKind::CheckBox, depth, kind_depth)?
            }
            Some("ddList") => set_legacy_form_kind(
                projection,
                LegacyFormFieldKind::DropDownList,
                depth,
                kind_depth,
            )?,
            _ => {}
        }
    } else if kind_depth.is_some_and(|kind| depth == kind + 1) {
        if let (Some(kind), Some(local)) = (projection.kind, local) {
            if let Some(slot) = legacy_form_kind_child_slot(kind, local) {
                validate_legacy_form_child_order(
                    &mut projection.kind_last_slot,
                    slot,
                    "legacy form kind",
                )?;
            } else if is_known_legacy_form_kind_child(local) {
                return Err(OxmlError::InvalidValue(format!(
                    "w:{local} is not valid for this legacy form kind"
                )));
            }
        }
        match (projection.kind, local) {
            (Some(LegacyFormFieldKind::TextInput), Some("type")) => {
                mark_legacy_form_singleton(&mut projection.text_type_seen, "w:textInput/w:type")?;
                let value =
                    required_legacy_form_value(element, word_prefixes, "w:textInput/w:type")?;
                projection.text_input_type = match value.as_str() {
                    "regular" => None,
                    "number" => Some(LegacyTextInputType::Number),
                    "date" => Some(LegacyTextInputType::Date),
                    "currentTime" => Some(LegacyTextInputType::CurrentTime),
                    "currentDate" => Some(LegacyTextInputType::CurrentDate),
                    "calculated" => Some(LegacyTextInputType::Calculated),
                    _ => {
                        return Err(OxmlError::InvalidValue(
                            "invalid w:textInput/w:type token".to_owned(),
                        ));
                    }
                };
            }
            (Some(LegacyFormFieldKind::TextInput), Some("default")) => {
                mark_legacy_form_singleton(
                    &mut projection.default_text_seen,
                    "w:textInput/w:default",
                )?;
                projection.default_text = Some(required_bounded_legacy_form_value(
                    element,
                    word_prefixes,
                    "w:textInput/w:default",
                    255,
                )?);
            }
            (Some(LegacyFormFieldKind::TextInput), Some("maxLength")) => {
                mark_legacy_form_singleton(
                    &mut projection.max_length_seen,
                    "w:textInput/w:maxLength",
                )?;
                let value =
                    legacy_form_usize_value(element, word_prefixes, "w:textInput/w:maxLength")?;
                if !(1..=32_767).contains(&value) {
                    return Err(OxmlError::InvalidValue(
                        "w:textInput/w:maxLength must be between 1 and 32767".to_owned(),
                    ));
                }
                projection.max_length = Some(value);
            }
            (Some(LegacyFormFieldKind::TextInput), Some("format")) => {
                mark_legacy_form_singleton(
                    &mut projection.text_format_seen,
                    "w:textInput/w:format",
                )?;
                required_bounded_legacy_form_value(
                    element,
                    word_prefixes,
                    "w:textInput/w:format",
                    64,
                )?;
            }
            (Some(LegacyFormFieldKind::CheckBox), Some("size")) => {
                mark_legacy_form_singleton(
                    &mut projection.checkbox_size_seen,
                    "w:checkBox size choice",
                )?;
                projection.checkbox_size = Some(LegacyCheckBoxSize::HalfPoints(
                    legacy_form_hps_measure_value(element, word_prefixes, "w:checkBox/w:size")?,
                ));
            }
            (Some(LegacyFormFieldKind::CheckBox), Some("sizeAuto")) => {
                mark_legacy_form_singleton(
                    &mut projection.checkbox_size_seen,
                    "w:checkBox size choice",
                )?;
                legacy_form_boolean_value(element, word_prefixes, "w:checkBox/w:sizeAuto")?;
                projection.checkbox_size = Some(LegacyCheckBoxSize::Auto);
            }
            (Some(LegacyFormFieldKind::CheckBox), Some("checked")) => {
                mark_legacy_form_singleton(&mut projection.checked_seen, "w:checkBox/w:checked")?;
                projection.checked = Some(legacy_form_boolean_value(
                    element,
                    word_prefixes,
                    "w:checkBox/w:checked",
                )?);
            }
            (Some(LegacyFormFieldKind::CheckBox), Some("default")) => {
                mark_legacy_form_singleton(
                    &mut projection.check_default_seen,
                    "w:checkBox/w:default",
                )?;
                projection.check_default = Some(legacy_form_boolean_value(
                    element,
                    word_prefixes,
                    "w:checkBox/w:default",
                )?);
            }
            (Some(LegacyFormFieldKind::DropDownList), Some("result")) => {
                mark_legacy_form_singleton(
                    &mut projection.selected_index_seen,
                    "w:ddList/w:result",
                )?;
                projection.selected_index = Some(legacy_form_usize_value(
                    element,
                    word_prefixes,
                    "w:ddList/w:result",
                )?);
            }
            (Some(LegacyFormFieldKind::DropDownList), Some("default")) => {
                mark_legacy_form_singleton(
                    &mut projection.selected_default_seen,
                    "w:ddList/w:default",
                )?;
                let value = legacy_form_usize_value(element, word_prefixes, "w:ddList/w:default")?;
                if value > 24 {
                    return Err(OxmlError::InvalidValue(
                        "w:ddList/w:default must be between 0 and 24".to_owned(),
                    ));
                }
                projection.selected_default = Some(value);
            }
            (Some(LegacyFormFieldKind::DropDownList), Some("listEntry")) => {
                if projection.choices.len() == 25 {
                    return Err(OxmlError::InvalidValue(
                        "w:ddList contains more than 25 list entries".to_owned(),
                    ));
                }
                projection.choices.push(required_bounded_legacy_form_value(
                    element,
                    word_prefixes,
                    "w:ddList/w:listEntry",
                    255,
                )?);
            }
            _ => {}
        }
    }
    Ok(is_leaf)
}

fn legacy_form_ff_data_slot(local: Option<&str>) -> Option<usize> {
    match local? {
        "name" => Some(0),
        "enabled" => Some(1),
        "calcOnExit" => Some(2),
        "entryMacro" => Some(3),
        "exitMacro" => Some(4),
        "helpText" => Some(5),
        "statusText" => Some(6),
        "checkBox" | "ddList" | "textInput" => Some(7),
        _ => None,
    }
}

fn legacy_form_kind_child_slot(kind: LegacyFormFieldKind, local: &str) -> Option<usize> {
    match kind {
        LegacyFormFieldKind::TextInput => match local {
            "type" => Some(0),
            "default" => Some(1),
            "maxLength" => Some(2),
            "format" => Some(3),
            _ => None,
        },
        LegacyFormFieldKind::CheckBox => match local {
            "size" | "sizeAuto" => Some(0),
            "default" => Some(1),
            "checked" => Some(2),
            _ => None,
        },
        LegacyFormFieldKind::DropDownList => match local {
            "result" => Some(0),
            "default" => Some(1),
            "listEntry" => Some(2),
            _ => None,
        },
    }
}

fn is_known_legacy_form_kind_child(local: &str) -> bool {
    matches!(
        local,
        "type"
            | "default"
            | "maxLength"
            | "format"
            | "size"
            | "sizeAuto"
            | "checked"
            | "result"
            | "listEntry"
    )
}

fn is_known_legacy_form_vocabulary(local: &str) -> bool {
    local == "ffData"
        || legacy_form_ff_data_slot(Some(local)).is_some()
        || is_known_legacy_form_kind_child(local)
}

fn is_legacy_form_element_only_container(
    stack_len: usize,
    ff_data_depth: Option<usize>,
    kind_depth: Option<usize>,
) -> bool {
    ff_data_depth.is_some_and(|depth| stack_len == depth + 1)
        || kind_depth.is_some_and(|depth| stack_len == depth + 1)
}

fn validate_legacy_form_child_order(
    last_slot: &mut Option<usize>,
    slot: usize,
    owner: &str,
) -> Result<()> {
    if last_slot.is_some_and(|last| slot < last) {
        return Err(OxmlError::InvalidValue(format!(
            "out-of-order modeled child in {owner}"
        )));
    }
    *last_slot = Some(slot);
    Ok(())
}

fn mark_legacy_form_singleton(seen: &mut bool, name: &str) -> Result<()> {
    if std::mem::replace(seen, true) {
        return Err(OxmlError::InvalidValue(format!(
            "duplicate legacy form singleton {name}"
        )));
    }
    Ok(())
}

fn required_legacy_form_value(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
    name: &str,
) -> Result<String> {
    legacy_form_value_attribute(element, word_prefixes)?
        .ok_or_else(|| OxmlError::MissingElement(format!("{name} w:val attribute")))
}

fn required_bounded_legacy_form_value(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
    name: &str,
    max_characters: usize,
) -> Result<String> {
    let value = required_legacy_form_value(element, word_prefixes, name)?;
    if value.chars().count() > max_characters {
        return Err(OxmlError::InvalidValue(format!(
            "{name} exceeds {max_characters} characters"
        )));
    }
    Ok(value)
}

fn legacy_form_boolean_value(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
    name: &str,
) -> Result<bool> {
    let Some(value) = legacy_form_value_attribute(element, word_prefixes)? else {
        return Ok(true);
    };
    crate::shared::ST_OnOff::parse(&value)
        .ok_or_else(|| OxmlError::InvalidValue(format!("invalid {name} boolean token")))
}

fn legacy_form_value_attribute(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
) -> Result<Option<String>> {
    legacy_form_word_attribute(element, word_prefixes, b"val", "w:val")
}

fn legacy_form_word_attribute(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
    wanted: &[u8],
    name: &str,
) -> Result<Option<String>> {
    let mut value = None;
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let Some(separator) = key.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if key.get(separator + 1..) == Some(wanted)
            && word_prefixes
                .iter()
                .any(|prefix| prefix.as_bytes() == &key[..separator])
        {
            if value.is_some() {
                return Err(OxmlError::InvalidValue(format!(
                    "duplicate legacy form {name} attribute"
                )));
            }
            let raw = oxoffice_core::xml::normalize_attribute_white_space(std::str::from_utf8(
                &attribute.value,
            )?);
            value = Some(
                quick_xml::escape::unescape(&raw)
                    .map_err(|error| {
                        OxmlError::InvalidValue(format!(
                            "invalid legacy form {name} attribute: {error}"
                        ))
                    })?
                    .into_owned(),
            );
        }
    }
    Ok(value)
}

/// A help or status text, `None` when it has no `w:val`.
fn validate_legacy_info_text(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
    name: &str,
    max_characters: usize,
) -> Result<Option<LegacyFormInfoText>> {
    let auto_text =
        match legacy_form_word_attribute(element, word_prefixes, b"type", "w:type")?.as_deref() {
            None | Some("text") => false,
            Some("autoText") => true,
            Some(_) => {
                return Err(OxmlError::InvalidValue(format!(
                    "invalid {name} w:type token"
                )));
            }
        };
    let value = legacy_form_word_attribute(element, word_prefixes, b"val", "w:val")?;
    if value
        .as_ref()
        .is_some_and(|value| value.chars().count() > max_characters)
    {
        return Err(OxmlError::InvalidValue(format!(
            "{name} w:val exceeds {max_characters} characters"
        )));
    }
    Ok(value.map(|value| {
        if auto_text {
            LegacyFormInfoText::AutoText(value)
        } else {
            LegacyFormInfoText::Text(value)
        }
    }))
}

fn legacy_form_usize_value(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
    name: &str,
) -> Result<usize> {
    required_legacy_form_value(element, word_prefixes, name)?
        .parse()
        .map_err(|_| OxmlError::InvalidValue(format!("invalid {name} numeric token")))
}

/// A half-point measure: a count of half-points, or a number with a unit,
/// rounded to the nearest half-point.
fn legacy_form_hps_measure_value(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
    name: &str,
) -> Result<u64> {
    let value = required_legacy_form_value(element, word_prefixes, name)?;
    if let Ok(half_points) = value.parse::<u64>() {
        return Ok(half_points);
    }
    let Some(unit) = ["mm", "cm", "in", "pt", "pc", "pi"]
        .into_iter()
        .find(|unit| value.ends_with(unit))
    else {
        return Err(OxmlError::InvalidValue(format!(
            "invalid {name} measure token"
        )));
    };
    let number = &value[..value.len() - unit.len()];
    let valid_number = number.split_once('.').map_or_else(
        || !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()),
        |(integer, fraction)| {
            !integer.is_empty()
                && !fraction.is_empty()
                && integer.bytes().all(|byte| byte.is_ascii_digit())
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        },
    );
    if !valid_number {
        return Err(OxmlError::InvalidValue(format!(
            "invalid {name} measure token"
        )));
    }
    let points_per_unit = match unit {
        "mm" => 72.0 / 25.4,
        "cm" => 72.0 / 2.54,
        "in" => 72.0,
        "pt" => 1.0,
        _ => 12.0,
    };
    let half_points = number.parse::<f64>().unwrap_or(0.0) * points_per_unit * 2.0;
    Ok(half_points.round().min(u64::MAX as f64) as u64)
}

fn set_legacy_form_kind(
    projection: &mut LegacyFormProjection,
    kind: LegacyFormFieldKind,
    depth: usize,
    kind_depth: &mut Option<usize>,
) -> Result<()> {
    if projection.kind.replace(kind).is_some() {
        return Err(OxmlError::InvalidValue(
            "w:ffData contains more than one form kind".to_owned(),
        ));
    }
    *kind_depth = Some(depth);
    Ok(())
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

pub(super) fn rewrite_legacy_form_source(
    raw: &[u8],
    word_prefixes: &[String],
    form: &LegacyFormFieldData,
    work: &WordOperationWork,
) -> Result<Vec<u8>> {
    let emission = work.field_xml_emission();
    if emission.enabled {
        let (output, _outer) = emission.new_source_record_output();
        let mut writer = Writer::new(output);
        write_legacy_form_source(&mut writer, raw, word_prefixes, form, &emission)?;
        Ok(writer.into_inner().into_mapped_parts().0)
    } else {
        let mut writer = Writer::new(Vec::new());
        write_legacy_form_source(&mut writer, raw, word_prefixes, form, &emission)?;
        Ok(writer.into_inner())
    }
}

fn write_legacy_form_source<W: std::io::Write>(
    writer: &mut Writer<W>,
    raw: &[u8],
    word_prefixes: &[String],
    form: &LegacyFormFieldData,
    emission: &FieldXmlEmission,
) -> Result<()> {
    let (kind_name, value_name, value) = match (&form.kind, &form.value) {
        (LegacyFormFieldKind::TextInput, LegacyFormFieldValue::Text(value)) => {
            ("textInput", "default", value.clone())
        }
        (LegacyFormFieldKind::CheckBox, LegacyFormFieldValue::Checked(value)) => (
            "checkBox",
            "checked",
            if *value { "1" } else { "0" }.to_owned(),
        ),
        (LegacyFormFieldKind::DropDownList, LegacyFormFieldValue::SelectedIndex(value)) => {
            ("ddList", "result", value.to_string())
        }
        _ => {
            return Err(OxmlError::InvalidValue(
                "legacy form value kind does not match the field kind".to_owned(),
            ));
        }
    };
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut prefixes = word_prefixes.to_vec();
    let mut prefix_scopes = Vec::new();
    let mut stack = Vec::<Option<String>>::new();
    let mut begin_depth = None;
    let mut begin_seen = false;
    let mut ff_depth = None;
    let mut kind_depth = None;
    let mut kind_prefix = "w".to_owned();
    let mut wrote_value = false;

    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                let local = word_local_name(&element, &local_prefixes);
                let depth = stack.len();
                if kind_depth.is_some_and(|kind| depth == kind + 1)
                    && !wrote_value
                    && form.kind == LegacyFormFieldKind::TextInput
                    && matches!(local.as_deref(), Some("maxLength" | "format"))
                {
                    write_legacy_form_value_element(
                        writer.get_mut(),
                        &kind_prefix,
                        value_name,
                        &value,
                        emission,
                    )?;
                    wrote_value = true;
                }
                if kind_depth.is_some_and(|kind| depth == kind + 1)
                    && !wrote_value
                    && form.kind == LegacyFormFieldKind::DropDownList
                    && local.as_deref() != Some(value_name)
                {
                    write_legacy_form_value_element(
                        writer.get_mut(),
                        &kind_prefix,
                        value_name,
                        &value,
                        emission,
                    )?;
                    wrote_value = true;
                }
                if kind_depth.is_some_and(|kind| depth == kind + 1)
                    && local.as_deref() == Some(value_name)
                {
                    write_patched_value_event(
                        writer.get_mut(),
                        &element,
                        &local_prefixes,
                        &value,
                        false,
                        emission,
                    )?;
                    wrote_value = true;
                } else {
                    writer.write_event(Event::Start(element.clone()))?;
                }
                stack.push(local.clone());
                let local_prefixes = local_prefixes.into_owned();
                prefix_scopes.push(std::mem::replace(&mut prefixes, local_prefixes));
                if local.as_deref() == Some("fldChar")
                    && !begin_seen
                    && optional_word_attribute(&element, b"fldCharType", &prefixes).as_deref()
                        == Some("begin")
                {
                    begin_depth = Some(depth);
                    begin_seen = true;
                } else if local.as_deref() == Some("ffData") && begin_depth == depth.checked_sub(1)
                {
                    ff_depth = Some(depth);
                } else if ff_depth.is_some_and(|ff| depth == ff + 1)
                    && local.as_deref() == Some(kind_name)
                {
                    kind_depth = Some(depth);
                    kind_prefix = qname_prefix(element.name().as_ref())
                        .unwrap_or("w")
                        .to_owned();
                }
            }
            Event::Empty(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                let local = word_local_name(&element, &local_prefixes);
                let depth = stack.len();
                if ff_depth.is_some_and(|ff| depth == ff + 1) && local.as_deref() == Some(kind_name)
                {
                    kind_prefix = qname_prefix(element.name().as_ref())
                        .unwrap_or("w")
                        .to_owned();
                    writer.write_event(Event::Start(element.clone()))?;
                    write_legacy_form_value_element(
                        writer.get_mut(),
                        &kind_prefix,
                        value_name,
                        &value,
                        emission,
                    )?;
                    writer.write_event(Event::End(element.to_end()))?;
                    wrote_value = true;
                    continue;
                }
                if kind_depth.is_some_and(|kind| depth == kind + 1)
                    && !wrote_value
                    && form.kind == LegacyFormFieldKind::TextInput
                    && matches!(local.as_deref(), Some("maxLength" | "format"))
                {
                    write_legacy_form_value_element(
                        writer.get_mut(),
                        &kind_prefix,
                        value_name,
                        &value,
                        emission,
                    )?;
                    wrote_value = true;
                }
                if kind_depth.is_some_and(|kind| depth == kind + 1)
                    && !wrote_value
                    && form.kind == LegacyFormFieldKind::DropDownList
                    && local.as_deref() != Some(value_name)
                {
                    write_legacy_form_value_element(
                        writer.get_mut(),
                        &kind_prefix,
                        value_name,
                        &value,
                        emission,
                    )?;
                    wrote_value = true;
                }
                if kind_depth.is_some_and(|kind| depth == kind + 1)
                    && local.as_deref() == Some(value_name)
                {
                    write_patched_value_event(
                        writer.get_mut(),
                        &element,
                        &local_prefixes,
                        &value,
                        true,
                        emission,
                    )?;
                    wrote_value = true;
                } else {
                    crate::raw_xml::write_empty(writer, &element, reader.empty_writing())?;
                }
            }
            Event::End(element) => {
                let depth = stack.len().saturating_sub(1);
                if kind_depth == Some(depth) {
                    if !wrote_value {
                        write_legacy_form_value_element(
                            writer.get_mut(),
                            &kind_prefix,
                            value_name,
                            &value,
                            emission,
                        )?;
                        wrote_value = true;
                    }
                    kind_depth = None;
                }
                if ff_depth == Some(depth) {
                    ff_depth = None;
                }
                if begin_depth == Some(depth) {
                    begin_depth = None;
                }
                writer.write_event(Event::End(element))?;
                stack.pop();
                prefixes = prefix_scopes
                    .pop()
                    .unwrap_or_else(|| word_prefixes.to_vec());
            }
            Event::Eof => break,
            event => writer.write_event(event)?,
        }
    }
    if !wrote_value {
        return Err(OxmlError::MissingElement(format!(
            "legacy form {kind_name} value"
        )));
    }
    Ok(())
}

fn word_local_name(element: &BytesStart<'_>, word_prefixes: &[String]) -> Option<String> {
    let name = element.name();
    let local = local_name(name.as_ref());
    is_word_element(name.as_ref(), local, word_prefixes)
        .then(|| String::from_utf8_lossy(local).into_owned())
}

fn qname_prefix(name: &[u8]) -> Option<&str> {
    let separator = name.iter().position(|byte| *byte == b':')?;
    std::str::from_utf8(&name[..separator]).ok()
}

pub(super) fn field_publication_bytes(
    field: &Field,
    work: &mut WordOperationWork,
) -> Result<Vec<u8>> {
    let _source_scope = root_binding_scope(0);
    // Bound the existing recursive unchanged predicate before using it as the
    // actual writer's source/generated comparison classification. These same
    // admission/reference checks grant no additional operation input or work.
    crate::document::admit_word_model(crate::document::WordOwner::Field(field))?;
    field.normalization_work_budget()?;
    let incoming_source = work
        .retained_inputs
        .contains(&(Arc::as_ptr(&field.source) as usize))
        || work
            .field_origins
            .lock()
            .expect("field origin registry poisoned")
            .incoming_record(field);
    if field.is_unchanged() && incoming_source {
        // The actual unchanged parsed branch transports a proved incoming
        // retained record. Parsed metadata alone cannot make generated bytes
        // another original/model input or a free comparison copy.
        let mut writer = Writer::new(Vec::new());
        write_field(&mut writer, field, None, work)?;
        Ok(writer.into_inner())
    } else {
        let emission = work.field_xml_emission();
        let mut writer = Writer::new(FieldXmlRecord::new(&emission));
        write_field(&mut writer, field, None, work)?;
        Ok(writer.into_inner().into_bytes())
    }
}

pub(super) fn legacy_text_value_only_change(
    before: &Option<LegacyFormFieldData>,
    after: &Option<LegacyFormFieldData>,
) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => {
            before.kind == LegacyFormFieldKind::TextInput
                && after.kind == LegacyFormFieldKind::TextInput
                && before.name == after.name
                && before.enabled == after.enabled
                && before.calculate_on_exit == after.calculate_on_exit
                && before.choices == after.choices
                && before.max_length == after.max_length
                && before.value != after.value
        }
        _ => false,
    }
}

pub(super) fn legacy_form_canonical_display(
    form: &LegacyFormFieldData,
    value: &LegacyFormFieldValue,
) -> Result<String> {
    match (&form.kind, value) {
        (LegacyFormFieldKind::TextInput, LegacyFormFieldValue::Text(value)) => {
            if form
                .max_length
                .is_some_and(|maximum| value.chars().count() > maximum)
            {
                return Err(OxmlError::InvalidValue(
                    "legacy text form value exceeds w:maxLength".to_owned(),
                ));
            }
            oxoffice_core::xml::reject_non_xml_characters("legacy form text", value)?;
            Ok(value.clone())
        }
        (LegacyFormFieldKind::CheckBox, LegacyFormFieldValue::Checked(checked)) => {
            Ok(if *checked { "☒" } else { "☐" }.to_owned())
        }
        (LegacyFormFieldKind::DropDownList, LegacyFormFieldValue::SelectedIndex(index)) => {
            form.choices.get(*index).cloned().ok_or_else(|| {
                OxmlError::InvalidValue("legacy drop-down selection is out of range".to_owned())
            })
        }
        _ => Err(OxmlError::InvalidValue(
            "legacy form value kind does not match the field kind".to_owned(),
        )),
    }
}

fn bounded_legacy_text(label: &str, value: &str, max_characters: usize) -> Result<()> {
    oxoffice_core::xml::reject_non_xml_characters(label, value)?;
    if value.chars().count() > max_characters {
        return Err(OxmlError::InvalidValue(format!(
            "{label} exceeds {max_characters} characters"
        )));
    }
    Ok(())
}

fn validate_legacy_info(
    label: &str,
    text: &LegacyFormInfoText,
    max_characters: usize,
) -> Result<()> {
    let value = match text {
        LegacyFormInfoText::Text(value) | LegacyFormInfoText::AutoText(value) => value,
    };
    bounded_legacy_text(label, value, max_characters)
}

pub(super) fn validate_legacy_form_authoring(
    options: &LegacyFormFieldOptions,
    definition: &LegacyFormFieldDefinition,
) -> Result<()> {
    if let Some(name) = &options.name {
        bounded_legacy_text("legacy form name", name, 20)?;
    }
    if let Some(help) = &options.help_text {
        validate_legacy_info("legacy form help", help, 255)?;
    }
    if let Some(status) = &options.status_text {
        validate_legacy_info("legacy form status", status, 140)?;
    }
    match definition {
        LegacyFormFieldDefinition::TextInput {
            default,
            value,
            max_length,
            format,
            ..
        } => {
            if let Some(default) = default {
                bounded_legacy_text("legacy form default", default, 255)?;
            }
            bounded_legacy_text("legacy form text", value, 255)?;
            if let Some(format) = format {
                bounded_legacy_text("legacy form format", format, 64)?;
            }
            if let Some(max_length) = max_length
                && !(1..=32_767).contains(max_length)
            {
                return Err(OxmlError::InvalidValue(
                    "w:textInput/w:maxLength must be between 1 and 32767".to_owned(),
                ));
            }
            if max_length.is_some_and(|maximum| value.chars().count() > maximum) {
                return Err(OxmlError::InvalidValue(
                    "legacy text form value exceeds w:maxLength".to_owned(),
                ));
            }
        }
        LegacyFormFieldDefinition::CheckBox { .. } => {}
        LegacyFormFieldDefinition::DropDownList {
            choices,
            default_index,
            selected_index,
        } => {
            if !(1..=25).contains(&choices.len()) {
                return Err(OxmlError::InvalidValue(
                    "w:ddList must contain between 1 and 25 list entries".to_owned(),
                ));
            }
            for choice in choices {
                bounded_legacy_text("legacy form list entry", choice, 255)?;
            }
            if *default_index >= choices.len() || *selected_index >= choices.len() {
                return Err(OxmlError::InvalidValue(
                    "legacy drop-down selection is out of range".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn legacy_form_author_parts(
    options: &LegacyFormFieldOptions,
    definition: &LegacyFormFieldDefinition,
) -> Result<LegacyAuthorParts> {
    let mut author = LegacyFormAuthor {
        help_text: options.help_text.clone(),
        status_text: options.status_text.clone(),
        text_type: None,
        text_default: None,
        format: None,
        checkbox_size: None,
        checkbox_default: None,
        list_default: None,
    };
    let (name, kind, value, choices, max_length, display, reset_default, text_default) =
        match definition {
            LegacyFormFieldDefinition::TextInput {
                input_type,
                default,
                value,
                max_length,
                format,
            } => {
                author.text_type = Some(*input_type);
                author.text_default = default.clone();
                author.format = format.clone();
                (
                    "FORMTEXT",
                    LegacyFormFieldKind::TextInput,
                    LegacyFormFieldValue::Text(value.clone()),
                    Vec::new(),
                    *max_length,
                    value.clone(),
                    default.clone().map(LegacyFormFieldValue::Text),
                    default.clone(),
                )
            }
            LegacyFormFieldDefinition::CheckBox {
                size,
                default,
                checked,
            } => {
                author.checkbox_size = Some(*size);
                author.checkbox_default = Some(*default);
                (
                    "FORMCHECKBOX",
                    LegacyFormFieldKind::CheckBox,
                    LegacyFormFieldValue::Checked(*checked),
                    Vec::new(),
                    None,
                    if *checked { "☒" } else { "☐" }.to_owned(),
                    Some(LegacyFormFieldValue::Checked(*default)),
                    None,
                )
            }
            LegacyFormFieldDefinition::DropDownList {
                choices,
                default_index,
                selected_index,
            } => {
                author.list_default = Some(*default_index);
                (
                    "FORMDROPDOWN",
                    LegacyFormFieldKind::DropDownList,
                    LegacyFormFieldValue::SelectedIndex(*selected_index),
                    choices.clone(),
                    None,
                    choices[*selected_index].clone(),
                    Some(LegacyFormFieldValue::SelectedIndex(*default_index)),
                    None,
                )
            }
        };
    let data = LegacyFormFieldData {
        name: options.name.clone(),
        enabled: options.enabled,
        calculate_on_exit: options.calculate_on_exit,
        kind,
        value,
        choices,
        max_length,
        checkbox_size: author.checkbox_size,
        help_text: author.help_text.clone(),
        text_input_type: author
            .text_type
            .filter(|input_type| *input_type != LegacyTextInputType::Regular),
    };
    Ok((name, display, data, author, reset_default, text_default))
}

fn write_legacy_word_empty<W: std::io::Write>(
    writer: &mut Writer<W>,
    name: &str,
    value: Option<&str>,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    let mut element = FieldXmlStart::word("w", name, emission)?;
    if let Some(value) = value {
        element.push("w:val", value)?;
    }
    element.write(writer, true)?;
    Ok(())
}

fn write_legacy_info<W: std::io::Write>(
    writer: &mut Writer<W>,
    name: &str,
    text: &LegacyFormInfoText,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    let (kind, value) = match text {
        LegacyFormInfoText::Text(value) => ("text", value.as_str()),
        LegacyFormInfoText::AutoText(value) => ("autoText", value.as_str()),
    };
    let mut element = FieldXmlStart::word("w", name, emission)?;
    element.push("w:type", kind)?;
    element.push("w:val", value)?;
    element.write(writer, true)?;
    Ok(())
}

pub(super) fn write_legacy_form_ff_data<W: std::io::Write>(
    writer: &mut Writer<W>,
    field: &Field,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    let Some(form) = &field.legacy_form else {
        return Ok(());
    };
    let author = field.legacy_author.clone().unwrap_or(LegacyFormAuthor {
        help_text: None,
        status_text: None,
        text_type: None,
        text_default: field.legacy_text_default.clone(),
        format: None,
        checkbox_size: None,
        checkbox_default: None,
        list_default: None,
    });
    FieldXmlStart::new("w:ffData", emission)?.write(writer, false)?;
    if let Some(name) = &form.name {
        write_legacy_word_empty(writer, "name", Some(name), emission)?;
    }
    write_legacy_word_empty(
        writer,
        "enabled",
        Some(if form.enabled { "1" } else { "0" }),
        emission,
    )?;
    write_legacy_word_empty(
        writer,
        "calcOnExit",
        Some(if form.calculate_on_exit { "1" } else { "0" }),
        emission,
    )?;
    if let Some(help) = &author.help_text {
        write_legacy_info(writer, "helpText", help, emission)?;
    }
    if let Some(status) = &author.status_text {
        write_legacy_info(writer, "statusText", status, emission)?;
    }
    let mut decimal = itoa::Buffer::new();
    match form.kind {
        LegacyFormFieldKind::TextInput => {
            FieldXmlStart::new("w:textInput", emission)?.write(writer, false)?;
            let text_type = author
                .text_type
                .unwrap_or(LegacyTextInputType::Regular)
                .as_str();
            write_legacy_word_empty(writer, "type", Some(text_type), emission)?;
            if let Some(default) = &author.text_default {
                write_legacy_word_empty(writer, "default", Some(default), emission)?;
            }
            if let Some(max_length) = form.max_length {
                write_legacy_word_empty(
                    writer,
                    "maxLength",
                    Some(decimal.format(max_length)),
                    emission,
                )?;
            }
            if let Some(format) = &author.format {
                write_legacy_word_empty(writer, "format", Some(format), emission)?;
            }
            writer.write_event(Event::End(BytesEnd::new("w:textInput")))?;
        }
        LegacyFormFieldKind::CheckBox => {
            FieldXmlStart::new("w:checkBox", emission)?.write(writer, false)?;
            match author.checkbox_size.unwrap_or(LegacyCheckBoxSize::Auto) {
                LegacyCheckBoxSize::Auto => {
                    write_legacy_word_empty(writer, "sizeAuto", None, emission)?
                }
                LegacyCheckBoxSize::HalfPoints(size) => {
                    write_legacy_word_empty(writer, "size", Some(decimal.format(size)), emission)?
                }
            }
            let default = author.checkbox_default.unwrap_or(false);
            write_legacy_word_empty(
                writer,
                "default",
                Some(if default { "1" } else { "0" }),
                emission,
            )?;
            let checked = matches!(form.value, LegacyFormFieldValue::Checked(true));
            write_legacy_word_empty(
                writer,
                "checked",
                Some(if checked { "1" } else { "0" }),
                emission,
            )?;
            writer.write_event(Event::End(BytesEnd::new("w:checkBox")))?;
        }
        LegacyFormFieldKind::DropDownList => {
            FieldXmlStart::new("w:ddList", emission)?.write(writer, false)?;
            let selected = match form.value {
                LegacyFormFieldValue::SelectedIndex(index) => index,
                _ => 0,
            };
            write_legacy_word_empty(writer, "result", Some(decimal.format(selected)), emission)?;
            if let Some(default) = author.list_default {
                write_legacy_word_empty(
                    writer,
                    "default",
                    Some(decimal.format(default)),
                    emission,
                )?;
            }
            for choice in &form.choices {
                write_legacy_word_empty(writer, "listEntry", Some(choice), emission)?;
            }
            writer.write_event(Event::End(BytesEnd::new("w:ddList")))?;
        }
    }
    writer.write_event(Event::End(BytesEnd::new("w:ffData")))?;
    Ok(())
}

pub(super) fn write_missing_legacy_result_region<W: std::io::Write>(
    writer: &mut Writer<W>,
    source_run_start: &BytesStart<'_>,
    display: &str,
    canonical_word_binding: bool,
    work: &WordOperationWork,
) -> Result<()> {
    writer.write_event(Event::End(source_run_start.to_end()))?;
    let word_declaration = canonical_word_binding.then_some(crate::namespace::W_NS);
    write_word_run_start(writer, word_declaration, Some(&work.field_xml_emission()))?;
    let emission = work.field_xml_emission();
    let mut separate = FieldXmlStart::new("w:fldChar", Some(&emission))?;
    separate.push("w:fldCharType", "separate")?;
    separate.write(writer, true)?;
    writer.write_event(Event::End(BytesEnd::new("w:r")))?;
    write_word_run_start(writer, word_declaration, Some(&work.field_xml_emission()))?;
    write_field_result_content_with_binding(writer, display, canonical_word_binding, work)?;
    writer.write_event(Event::End(BytesEnd::new("w:r")))?;
    // The source end event still closes this run. Resume its actual QName and
    // local namespace scope without copying non-namespace owner attributes.
    let mut retained = Vec::new();
    for attribute in source_run_start.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
            retained.push(attribute);
        }
    }
    let name =
        std::str::from_utf8(&source_run_start.as_ref()[..source_run_start.name().as_ref().len()])?;
    let mut resumed = FieldXmlStart::new(name, Some(&emission))?;
    for attribute in retained {
        resumed.push_raw_attribute(attribute)?;
    }
    resumed.write(writer, false)?;
    Ok(())
}

fn write_legacy_form_value_element<W: std::io::Write>(
    output: &mut W,
    prefix: &str,
    name: &str,
    value: &str,
    emission: &FieldXmlEmission,
) -> Result<()> {
    let pieces = [
        FieldSourcePiece::Literal(b"<"),
        FieldSourcePiece::Raw(prefix.as_bytes()),
        FieldSourcePiece::Literal(b":"),
        FieldSourcePiece::Raw(name.as_bytes()),
        FieldSourcePiece::Literal(b" xmlns:"),
        FieldSourcePiece::Raw(prefix.as_bytes()),
        FieldSourcePiece::Literal(
            concat!(
                "=\"",
                "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
                "\" "
            )
            .as_bytes(),
        ),
        FieldSourcePiece::Raw(prefix.as_bytes()),
        FieldSourcePiece::Literal(b":val=\""),
        FieldSourcePiece::Escaped(value),
        FieldSourcePiece::Literal(b"\"/>"),
    ];
    emission.write_xml_pieces(output, &pieces)?;
    Ok(())
}

fn write_patched_value_event<W: std::io::Write>(
    output: &mut W,
    element: &BytesStart<'_>,
    word_prefixes: &[String],
    value: &str,
    empty: bool,
    emission: &FieldXmlEmission,
) -> Result<()> {
    let payload = element.as_ref();
    let mut selected = None;
    for attribute in element.attributes() {
        let attribute = attribute?;
        if is_word_attribute(attribute.key.as_ref(), b"val", word_prefixes)
            && let Some(range) = lexical_attribute_value_range(payload, attribute.key.as_ref())
        {
            selected = Some(range);
            break;
        }
    }
    let mut pieces = vec![FieldSourcePiece::Literal(b"<")];
    let prefix;
    if let Some(range) = selected {
        pieces.extend([
            FieldSourcePiece::Raw(&payload[..range.start]),
            FieldSourcePiece::Escaped(value),
            FieldSourcePiece::Raw(&payload[range.end..]),
        ]);
    } else {
        let preferred = qname_prefix(&payload[..element.name().as_ref().len()]).unwrap_or("w");
        let (chosen, declare) = usable_qualified_word_prefix(word_prefixes, payload, preferred);
        prefix = chosen;
        pieces.push(FieldSourcePiece::Raw(payload));
        if declare {
            pieces.extend([
                FieldSourcePiece::Literal(b" xmlns:"),
                FieldSourcePiece::Raw(prefix.as_bytes()),
                FieldSourcePiece::Literal(
                    concat!(
                        "=\"",
                        "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
                        "\""
                    )
                    .as_bytes(),
                ),
            ]);
        }
        pieces.extend([
            FieldSourcePiece::Literal(b" "),
            FieldSourcePiece::Raw(prefix.as_bytes()),
            FieldSourcePiece::Literal(b":val=\""),
            FieldSourcePiece::Escaped(value),
            FieldSourcePiece::Literal(b"\""),
        ]);
    }
    pieces.push(FieldSourcePiece::Literal(if empty { b"/>" } else { b">" }));
    emission.write_xml_pieces(output, &pieces)?;
    Ok(())
}

#[cfg(test)]
mod legacy_checkbox_size_tests {
    use super::*;

    /// The checkbox size of a Word legacy checkbox with this `w:checkBox` size.
    fn checkbox_size(size: &str) -> Option<LegacyCheckBoxSize> {
        let xml = format!(
            r#"<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Check1"/><w:enabled/><w:calcOnExit w:val="0"/><w:checkBox>{size}<w:default w:val="0"/></w:checkBox></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMCHECKBOX </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
        );
        let paragraph = CT_P::from_xml_fragment(xml.as_bytes()).unwrap();
        paragraph
            .runs
            .iter()
            .flat_map(|run| &run.content)
            .find_map(|content| match content {
                RunContent::Field(field) => field.legacy_form.as_ref()?.checkbox_size,
                _ => None,
            })
    }

    #[test]
    fn a_checkbox_size_reads_half_points_and_unit_measures() {
        assert_eq!(
            checkbox_size(r#"<w:size w:val="24"/>"#),
            Some(LegacyCheckBoxSize::HalfPoints(24))
        );
        assert_eq!(
            checkbox_size(r#"<w:size w:val="12pt"/>"#),
            Some(LegacyCheckBoxSize::HalfPoints(24))
        );
        assert_eq!(
            checkbox_size(r#"<w:size w:val="0.5in"/>"#),
            Some(LegacyCheckBoxSize::HalfPoints(72))
        );
        assert_eq!(
            checkbox_size(r#"<w:size w:val="1cm"/>"#),
            Some(LegacyCheckBoxSize::HalfPoints(57)),
            "28.35 points"
        );
        assert_eq!(
            checkbox_size(r#"<w:size w:val="2pc"/>"#),
            Some(LegacyCheckBoxSize::HalfPoints(48))
        );
        assert_eq!(
            checkbox_size(r#"<w:size w:val="1pi"/>"#),
            Some(LegacyCheckBoxSize::HalfPoints(24))
        );
        assert_eq!(
            checkbox_size(r#"<w:size w:val="10mm"/>"#),
            Some(LegacyCheckBoxSize::HalfPoints(57)),
            "28.35 points"
        );
        assert_eq!(
            checkbox_size("<w:sizeAuto/>"),
            Some(LegacyCheckBoxSize::Auto)
        );
    }
}

#[cfg(test)]
mod legacy_form_read_tests {
    use super::*;

    /// The legacy form data of the one field of a Word-shaped paragraph.
    fn legacy_form(ff_data: &str, instruction: &str) -> LegacyFormFieldData {
        let xml = format!(
            r#"<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:r><w:fldChar w:fldCharType="begin"><w:ffData>{ff_data}</w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>value</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
        );
        let paragraph = CT_P::from_xml_fragment(xml.as_bytes()).unwrap();
        paragraph
            .runs
            .iter()
            .flat_map(|run| &run.content)
            .find_map(|content| match content {
                RunContent::Field(field) => field.legacy_form.clone(),
                _ => None,
            })
            .expect("a legacy form field")
    }

    #[test]
    fn a_legacy_text_field_keeps_its_help_text_and_input_type() {
        let data = legacy_form(
            r#"<w:name w:val="Today"/><w:enabled/><w:calcOnExit w:val="0"/><w:helpText w:type="text" w:val="The signing date"/><w:textInput><w:type w:val="currentDate"/><w:format w:val="dd/MM/yyyy"/></w:textInput>"#,
            "FORMTEXT",
        );
        assert_eq!(
            data.help_text,
            Some(LegacyFormInfoText::Text("The signing date".to_owned()))
        );
        assert_eq!(data.text_input_type, Some(LegacyTextInputType::CurrentDate));
        assert_eq!(data.checkbox_size, None);
    }

    #[test]
    fn help_text_from_autotext_and_an_unstated_type_are_kept_as_written() {
        let data = legacy_form(
            r#"<w:name w:val="Name"/><w:enabled/><w:calcOnExit w:val="0"/><w:helpText w:type="autoText" w:val="NameHelp"/><w:textInput><w:maxLength w:val="40"/></w:textInput>"#,
            "FORMTEXT",
        );
        assert_eq!(
            data.help_text,
            Some(LegacyFormInfoText::AutoText("NameHelp".to_owned()))
        );
        assert_eq!(data.text_input_type, None, "unstated means regular");
        assert_eq!(data.max_length, Some(40));
    }

    #[test]
    fn an_authored_legacy_text_field_keeps_its_help_text_and_input_type() {
        let options = LegacyFormFieldOptions {
            name: Some("Today".to_owned()),
            enabled: true,
            calculate_on_exit: false,
            help_text: Some(LegacyFormInfoText::Text("The signing date".to_owned())),
            status_text: None,
        };
        let definition = LegacyFormFieldDefinition::TextInput {
            input_type: LegacyTextInputType::CurrentDate,
            default: None,
            value: String::new(),
            max_length: None,
            format: None,
        };
        let field = Field::from_legacy_form(options, definition).unwrap();
        let data = field.legacy_form.as_ref().unwrap();
        assert_eq!(
            data.help_text,
            Some(LegacyFormInfoText::Text("The signing date".to_owned()))
        );
        assert_eq!(data.text_input_type, Some(LegacyTextInputType::CurrentDate));
    }

    #[test]
    fn authored_regular_text_reads_none_like_parsed_regular_text() {
        let options = LegacyFormFieldOptions {
            name: Some("Name".to_owned()),
            enabled: true,
            calculate_on_exit: false,
            help_text: None,
            status_text: None,
        };
        let definition = LegacyFormFieldDefinition::TextInput {
            input_type: LegacyTextInputType::Regular,
            default: None,
            value: String::new(),
            max_length: None,
            format: None,
        };
        let field = Field::from_legacy_form(options, definition).unwrap();
        assert_eq!(field.legacy_form.as_ref().unwrap().text_input_type, None);
    }

    #[test]
    fn regular_text_reads_the_same_stated_or_not() {
        let stated = legacy_form(
            r#"<w:name w:val="A"/><w:enabled/><w:calcOnExit w:val="0"/><w:textInput><w:type w:val="regular"/></w:textInput>"#,
            "FORMTEXT",
        );
        let unstated = legacy_form(
            r#"<w:name w:val="A"/><w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
            "FORMTEXT",
        );
        assert_eq!(stated.text_input_type, None);
        assert_eq!(unstated.text_input_type, None);
    }
}
