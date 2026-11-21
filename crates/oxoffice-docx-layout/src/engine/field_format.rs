//! Field result formatting: numeric pictures, PAGE formats, roman, alphabetic and ordinal numbers.

use super::*;

/// Apply the numeric picture phase, before civil date and general formatting.
pub fn format_numeric_field_picture(
    value: &str,
    picture: &str,
) -> std::result::Result<String, String> {
    let number = value
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .ok_or_else(|| "numeric field value is not a finite number".to_owned())?;
    format_numeric_picture(number, picture)
}

/// Apply ordered general switches after the numeric and civil date phases.
pub fn format_numeric_field_general(
    instruction: &FieldInstruction,
    value: &str,
) -> std::result::Result<String, String> {
    let mut output = value.to_owned();
    for switch in instruction
        .switches
        .iter()
        .filter(|switch| switch.name == "*")
    {
        let Some(FieldArgument::Text(format)) = &switch.argument else {
            return Err("general format requires a text argument".into());
        };
        output = if matches!(
            instruction.name.as_str(),
            "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
        ) && format.eq_ignore_ascii_case("alphabetic")
        {
            page_alphabetic(parse_positive_integer(&output)?, format == "ALPHABETIC")?
        } else {
            apply_general_format(&output, format)?
        };
    }
    Ok(output)
}

pub(super) fn validate_page_field_instruction(
    instruction: &FieldInstruction,
) -> std::result::Result<(), String> {
    let page_ref = instruction.name == "PAGEREF";
    if !matches!(
        instruction.name.as_str(),
        "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
    ) {
        return Err("not a page-family field".into());
    }
    if !instruction.quotes_are_balanced() {
        return Err(format!("field {} has unclosed quoting", instruction.name));
    }
    if instruction.arguments.len() != usize::from(page_ref)
        || page_ref && !matches!(instruction.arguments.first(), Some(FieldArgument::Text(_)))
    {
        return Err(format!(
            "field {} requires {} positional operands",
            instruction.name,
            usize::from(page_ref)
        ));
    }
    for switch in &instruction.switches {
        let format = matches!(switch.name.as_str(), "*" | "#");
        // `\p` writes "on page N", "above" or "below", which the page
        // formats do not produce.
        if !format && !(page_ref && switch.name == "h") {
            return Err(format!("unsupported switch \\{}", switch.name));
        }
        if format && !matches!(switch.argument, Some(FieldArgument::Text(_))) {
            return Err(format!(
                "field {} switch \\{} requires a text argument",
                instruction.name, switch.name
            ));
        }
        if !format && switch.argument.is_some() {
            return Err(format!(
                "field {} switch \\{} does not take an argument",
                instruction.name, switch.name
            ));
        }
    }
    Ok(())
}

/// Whether a PAGEREF result holds more than a page number, and so keeps its
/// stored display in the layout and on update. LibreOffice writes a whole
/// table of contents entry (its number, its title and its page, between
/// tabs or spaces) as the result of the entry's PAGEREF. Word draws that
/// stored result as it is until its fields are updated, and a page number
/// alone holds no space or tab.
#[doc(hidden)]
pub fn page_reference_keeps_stored_display(cached_result: &str) -> bool {
    cached_result.contains(char::is_whitespace)
}

/// Shared bounded page-family policy for layout and saved-cache consumers.
#[doc(hidden)]
pub fn format_page_field_result(
    instruction: &FieldInstruction,
    value: usize,
    section_format: Option<&str>,
) -> std::result::Result<String, String> {
    validate_page_field_instruction(instruction)?;
    let section_format = section_format
        .filter(|format| supported_section_page_number_format(format))
        .ok_or_else(|| "unsupported section page number format".to_owned())?;
    let numeric_override = instruction.switches.iter().any(|switch| switch.name == "#" || switch.name == "*" && matches!(&switch.argument, Some(FieldArgument::Text(format)) if matches!(format.to_ascii_lowercase().as_str(), "arabic" | "roman" | "alphabetic" | "ordinal")));
    let mut base = value.to_string();
    if !numeric_override && section_format != "decimal" {
        base = match section_format {
            "upperRoman" => roman(parse_positive_integer(&base)?, true)?,
            "lowerRoman" => roman(parse_positive_integer(&base)?, false)?,
            "upperLetter" => page_alphabetic(parse_positive_integer(&base)?, true)?,
            "lowerLetter" => page_alphabetic(parse_positive_integer(&base)?, false)?,
            _ => unreachable!(),
        };
    }
    format_numeric_field_display(instruction, base)
}

/// Read effective imported numbering after qualifying the modeled first owner.
/// Qualified absence is decimal. Unsupported chapter or unqualified source is None.
/// Direct Word duplicates overlay explicit attributes without changing the model.
#[doc(hidden)]
pub fn section_page_number_format(section: &CT_SectPr) -> Option<String> {
    effective_section_page_number(section).format
}

fn supported_section_page_number_format(format: &str) -> bool {
    matches!(
        format,
        "decimal" | "upperRoman" | "lowerRoman" | "upperLetter" | "lowerLetter"
    )
}

pub(super) fn effective_section_page_number(section: &CT_SectPr) -> EffectiveSectionPageNumber {
    let start = section
        .page_number
        .as_ref()
        .and_then(|number| number.start)
        .map(|value| value as usize);
    let unsupported = || EffectiveSectionPageNumber {
        start,
        format: None,
    };
    if section.page_number.is_none() {
        return EffectiveSectionPageNumber {
            start: None,
            format: Some("decimal".into()),
        };
    }
    // Positively admit the actual modeled owner before any opaque duplicate can
    // contribute. The writer owns canonical w and retained root binding records.
    let mut owner = section.clone();
    let root_records = section
        .extra_xml
        .iter()
        .zip(&section.extra_xml_positions)
        .filter(|(raw, position)| CT_SectPr::raw_position_is_root_attributes(position, raw))
        .map(|(raw, position)| (raw.clone(), position.clone()))
        .collect::<Vec<_>>();
    (owner.extra_xml, owner.extra_xml_positions) = root_records.into_iter().unzip();
    if read_effective_section_page_number(&owner, start).is_none() {
        return unsupported();
    }
    // This is an analysis-only projection of the current writer output. Typed
    // getters and writes still address the first owner. Invalid later state
    // discards the entire recovery and retains the legacy start as compatibility.
    read_effective_section_page_number(section, start).unwrap_or_else(unsupported)
}

fn read_effective_section_page_number(
    section: &CT_SectPr,
    modeled_start: Option<usize>,
) -> Option<EffectiveSectionPageNumber> {
    use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
    let mut writer = quick_xml::Writer::new(Vec::new());
    section.to_xml(&mut writer).ok()?;
    let mut xml =
        b"<pageSource xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">"
            .to_vec();
    xml.extend(writer.into_inner());
    xml.extend_from_slice(b"</pageSource>");
    let mut reader = NsReader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut owners = 0usize;
    let mut start = modeled_start;
    let mut format = "decimal".to_owned();
    let mut unsupported_format = None;
    loop {
        let event = reader.read_event_into(&mut buffer).ok()?;
        let opens = matches!(&event, Event::Start(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                if depth == 2 && element.local_name().as_ref() == b"pgNumType" {
                    let namespace = reader.resolver().resolve_element(element.name()).0;
                    if matches!(namespace, ResolveResult::Unknown(_)) {
                        return None;
                    }
                    if matches!(namespace, ResolveResult::Bound(namespace) if namespace.as_ref() == b"http://schemas.openxmlformats.org/wordprocessingml/2006/main")
                    {
                        owners += 1;
                        for attribute in element.attributes() {
                            let attribute = attribute.ok()?;
                            if attribute.key.as_ref() == b"xmlns"
                                || attribute.key.as_ref().starts_with(b"xmlns:")
                            {
                                continue;
                            }
                            let namespace = reader.resolver().resolve_attribute(attribute.key).0;
                            if matches!(namespace, ResolveResult::Unknown(_)) {
                                return None;
                            }
                            if !matches!(namespace, ResolveResult::Bound(namespace) if namespace.as_ref() == b"http://schemas.openxmlformats.org/wordprocessingml/2006/main")
                            {
                                continue;
                            }
                            match attribute.key.local_name().as_ref() {
                                b"fmt" => {
                                    format = attribute
                                        .decoded_and_normalized_value(
                                            quick_xml::XmlVersion::Implicit1_0,
                                            element.decoder(),
                                        )
                                        .ok()?
                                        .into_owned();
                                    if !supported_section_page_number_format(&format)
                                        && unsupported_format.is_none()
                                    {
                                        unsupported_format = Some(format.clone());
                                    }
                                }
                                b"start" if owners > 1 => {
                                    let value = attribute
                                        .decoded_and_normalized_value(
                                            quick_xml::XmlVersion::Implicit1_0,
                                            element.decoder(),
                                        )
                                        .ok()?;
                                    start = Some(value.parse::<u32>().ok()? as usize);
                                }
                                b"chapStyle" | b"chapSep" => return None,
                                _ => {}
                            }
                        }
                    }
                }
                if opens {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1)?;
            }
            Event::Eof => {
                return (owners > 0).then(|| EffectiveSectionPageNumber {
                    start,
                    format: Some(unsupported_format.unwrap_or(format)),
                });
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn apply_general_format(value: &str, format: &str) -> std::result::Result<String, String> {
    if format == "ALPHABETIC" {
        return parse_positive_integer(value).map(|value| alphabetic(value, true));
    }
    if format == "ROMAN" {
        return parse_positive_integer(value).and_then(|value| roman(value, true));
    }
    match format.to_ascii_lowercase().as_str() {
        "upper" => Ok(value.to_uppercase()),
        "lower" => Ok(value.to_lowercase()),
        "firstcap" => Ok(capitalize_first(value)),
        "caps" => Ok(value
            .split_inclusive(char::is_whitespace)
            .map(capitalize_first)
            .collect()),
        "arabic" => parse_positive_integer(value).map(|value| value.to_string()),
        "alphabetic" => parse_positive_integer(value).map(|value| alphabetic(value, false)),
        "roman" => parse_positive_integer(value).and_then(|value| roman(value, false)),
        "ordinal" => parse_positive_integer(value).map(ordinal),
        "mergeformat" | "charformat" => Ok(value.to_owned()),
        other => Err(format!("general format {other} is unsupported")),
    }
}

fn parse_positive_integer(value: &str) -> std::result::Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "general numeric format requires a positive integer".to_owned())
}

fn capitalize_first(value: &str) -> String {
    let mut characters = value.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

fn page_alphabetic(value: u32, upper: bool) -> std::result::Result<String, String> {
    // Word uses A..Z, AA..ZZ, AAA..ZZZ for alphabetic page fields.
    // Keep hostile page-number restarts from allocating an unbounded cache.
    let count = (value - 1) / 26 + 1;
    if count > 4096 {
        return Err("alphabetic page result exceeds 4096 characters".to_owned());
    }
    let base = if upper { b'A' } else { b'a' };
    Ok(char::from(base + ((value - 1) % 26) as u8)
        .to_string()
        .repeat(count as usize))
}

fn alphabetic(mut value: u32, upper: bool) -> String {
    let mut output = Vec::new();
    while value > 0 {
        value -= 1;
        let base = if upper { b'A' } else { b'a' };
        output.push((base + (value % 26) as u8) as char);
        value /= 26;
    }
    output.iter().rev().collect()
}

fn roman(mut value: u32, upper: bool) -> std::result::Result<String, String> {
    if value > 3999 {
        return Err("Roman format supports values from 1 through 3999".to_owned());
    }
    let values = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut output = String::new();
    for (number, numeral) in values {
        while value >= number {
            output.push_str(numeral);
            value -= number;
        }
    }
    Ok(if upper { output } else { output.to_lowercase() })
}

fn ordinal(value: u32) -> String {
    let suffix = if (11..=13).contains(&(value % 100)) {
        "th"
    } else {
        match value % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{value}{suffix}")
}

fn split_picture_sections(picture: &str) -> std::result::Result<Vec<String>, String> {
    let mut sections = vec![String::new()];
    let mut quote = None;
    for character in picture.chars() {
        match (character, quote) {
            ('"' | '\'', None) => {
                quote = Some(character);
                sections.last_mut().unwrap().push(character);
            }
            (character, Some(active)) if character == active => {
                quote = None;
                sections.last_mut().unwrap().push(character);
            }
            (';', None) => sections.push(String::new()),
            (other, _) => sections.last_mut().unwrap().push(other),
        }
    }
    if quote.is_some() || sections.len() > 3 {
        Err("numeric picture has invalid sections or quoting".to_owned())
    } else {
        Ok(sections)
    }
}

fn format_numeric_picture(value: f64, picture: &str) -> std::result::Result<String, String> {
    let sections = split_picture_sections(picture)?;
    let (section, magnitude, implicit_negative) = if value < 0.0 {
        if let Some(section) = sections.get(1) {
            (section.as_str(), -value, false)
        } else {
            (sections[0].as_str(), -value, true)
        }
    } else if value == 0.0 && sections.len() == 3 {
        (sections[2].as_str(), 0.0, false)
    } else {
        (sections[0].as_str(), value, false)
    };
    let placeholders = numeric_placeholder_indices(section);
    let Some(first) = placeholders.first().copied() else {
        return unquote_picture(section);
    };
    let last = placeholders.last().copied().unwrap() + 1;
    let prefix = unquote_picture(&section[..first])?;
    let suffix = unquote_picture(&section[last..])?;
    let (core, literals) = extract_numeric_literals(&section[first..last])?;
    let mut halves = core.split('.');
    let integer_picture = halves.next().unwrap_or_default();
    let decimal_picture = halves.next().unwrap_or_default();
    if halves.next().is_some()
        || integer_picture
            .chars()
            .any(|character| !matches!(character, '0' | '#' | ','))
        || decimal_picture
            .chars()
            .any(|character| !matches!(character, '0' | '#'))
    {
        return Err("numeric picture contains unsupported tokens".to_owned());
    }
    let maximum_decimals = decimal_picture.chars().count();
    let minimum_decimals = decimal_picture
        .chars()
        .filter(|character| *character == '0')
        .count();
    let integer_placeholders = integer_picture
        .chars()
        .filter(|character| matches!(character, '0' | '#'))
        .collect::<Vec<_>>();
    let decimal_placeholders = decimal_picture.chars().collect::<Vec<_>>();
    let formatted = format!("{magnitude:.maximum_decimals$}");
    let (integer, mut decimal) = formatted
        .split_once('.')
        .map(|(integer, decimal)| (integer.to_owned(), decimal.to_owned()))
        .unwrap_or((formatted, String::new()));
    while decimal.len() > minimum_decimals && decimal.ends_with('0') {
        decimal.pop();
    }
    let mut integer = integer;
    if !integer_placeholders.contains(&'0') && integer == "0" {
        integer.clear();
    }
    let missing_integer = integer_placeholders.len().saturating_sub(integer.len());
    let padding = integer_placeholders[..missing_integer]
        .iter()
        .map(|placeholder| if *placeholder == '0' { '0' } else { ' ' })
        .collect::<String>();
    if integer_picture.contains(',') {
        integer = group_digits(&integer);
    }
    let decimal_padding = decimal_placeholders[decimal.len()..]
        .iter()
        .map(|placeholder| if *placeholder == '0' { '0' } else { ' ' })
        .collect::<String>();
    let number = if decimal_placeholders.is_empty() {
        format!("{padding}{integer}")
    } else {
        format!("{padding}{integer}.{decimal}{decimal_padding}")
    };
    let number = insert_numeric_literals(
        &number,
        integer_picture
            .chars()
            .chain(decimal_picture.chars())
            .filter(|character| matches!(character, '0' | '#'))
            .count(),
        &literals,
    );
    Ok(format!(
        "{}{}{}{}",
        if implicit_negative { "-" } else { "" },
        prefix,
        number,
        suffix
    ))
}

fn numeric_placeholder_indices(value: &str) -> Vec<usize> {
    let mut quote = None;
    let mut indices = Vec::new();
    for (index, character) in value.char_indices() {
        match (character, quote) {
            ('"' | '\'', None) => quote = Some(character),
            (character, Some(active)) if character == active => quote = None,
            ('0' | '#', None) => indices.push(index),
            _ => {}
        }
    }
    indices
}

fn extract_numeric_literals(
    value: &str,
) -> std::result::Result<(String, Vec<(usize, String)>), String> {
    let mut pattern = String::new();
    let mut literals = Vec::new();
    let mut literal = String::new();
    let mut quote = None;
    let mut placeholder_count = 0usize;
    for character in value.chars() {
        match (character, quote) {
            ('"' | '\'', None) => quote = Some(character),
            (character, Some(active)) if character == active => {
                literals.push((placeholder_count, std::mem::take(&mut literal)));
                quote = None;
            }
            (character, Some(_)) => literal.push(character),
            (character @ ('0' | '#'), None) => {
                pattern.push(character);
                placeholder_count += 1;
            }
            (character, None) => pattern.push(character),
        }
    }
    if quote.is_some() {
        Err("numeric picture has unclosed quoting".to_owned())
    } else {
        Ok((pattern, literals))
    }
}

fn insert_numeric_literals(
    number: &str,
    placeholder_count: usize,
    literals: &[(usize, String)],
) -> String {
    if literals.is_empty() {
        return number.to_owned();
    }
    let slot_count = number
        .chars()
        .filter(|character| character.is_ascii_digit() || *character == ' ')
        .count();
    let extra_leading = slot_count.saturating_sub(placeholder_count);
    let mut output = String::new();
    let mut digits_written = 0usize;
    for (position, literal) in literals
        .iter()
        .filter(|(position, _)| extra_leading + position == 0)
    {
        let _ = position;
        output.push_str(literal);
    }
    for character in number.chars() {
        output.push(character);
        if character.is_ascii_digit() || character == ' ' {
            digits_written += 1;
            for (_, literal) in literals
                .iter()
                .filter(|(position, _)| extra_leading + position == digits_written)
            {
                output.push_str(literal);
            }
        }
    }
    output
}

fn unquote_picture(value: &str) -> std::result::Result<String, String> {
    let mut output = String::new();
    let mut quote = None;
    for character in value.chars() {
        match (character, quote) {
            ('"' | '\'', None) => quote = Some(character),
            (character, Some(active)) if character == active => quote = None,
            (character, _) => output.push(character),
        }
    }
    if quote.is_some() {
        Err("numeric picture has unclosed quoting".to_owned())
    } else {
        Ok(output)
    }
}

fn group_digits(value: &str) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if index > 0 && (value.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

pub(super) fn format_sequence_display(
    instruction: &FieldInstruction,
    value: String,
) -> std::result::Result<String, String> {
    if value == "Error! Main Document Only."
        || (value.is_empty()
            && field_has_switch(instruction, "h")
            && !field_has_switch(instruction, "*"))
    {
        return Ok(value);
    }
    format_numeric_field_display(instruction, value)
}

pub(super) fn format_numeric_field_display(
    instruction: &FieldInstruction,
    value: String,
) -> std::result::Result<String, String> {
    let picture = instruction
        .switches
        .iter()
        .find(|switch| switch.name == "#")
        .and_then(|switch| switch.argument.as_ref())
        .and_then(|argument| match argument {
            FieldArgument::Text(value) => Some(value.as_str()),
            _ => None,
        });
    let value = if let Some(picture) = picture {
        format_numeric_field_picture(&value, picture)?
    } else {
        value
    };
    format_numeric_field_general(instruction, &value)
}
