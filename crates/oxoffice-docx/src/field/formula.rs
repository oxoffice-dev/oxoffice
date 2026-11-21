//! Values inside field instructions: formulas, switch formats, barcodes, dates and times.

use super::*;

pub(super) const MAX_FORMULA_BYTES: usize = 4096;
const MAX_FORMULA_TOKENS: usize = 512;
const MAX_FORMULA_DEPTH: usize = 32;

pub(super) fn parse_toc_level(value: &str, field: &str) -> std::result::Result<u8, String> {
    value
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|level| (1..=9).contains(level))
        .ok_or_else(|| format!("{field} level must be from 1 through 9"))
}

pub(super) fn parse_level_range(value: &str, field: &str) -> std::result::Result<(u8, u8), String> {
    let Some((start, end)) = value.split_once('-') else {
        let level = parse_toc_level(value, field)?;
        return Ok((level, level));
    };
    let start = parse_toc_level(start, field)?;
    let end = parse_toc_level(end, field)?;
    if start > end {
        return Err(format!("{field} range starts after it ends"));
    }
    Ok((start, end))
}

pub(super) fn parse_custom_styles(value: &str) -> std::result::Result<Vec<(String, u8)>, String> {
    let mut parts = value.split(',').collect::<Vec<_>>();
    if parts.last().is_some_and(|part| part.trim().is_empty()) {
        parts.pop();
    }
    if parts.len() % 2 != 0 || parts.is_empty() {
        return Err("TOC custom styles require style and level pairs".to_owned());
    }
    let mut styles = Vec::with_capacity(parts.len() / 2);
    for pair in parts.chunks_exact(2) {
        let name = pair[0].trim();
        if name.is_empty() {
            return Err("TOC custom style name must not be empty".to_owned());
        }
        styles.push((name.to_owned(), parse_toc_level(pair[1], "TOC style")?));
    }
    Ok(styles)
}

pub(super) fn parse_barcode(
    field: &str,
    switches: &[(String, Option<String>)],
    value: String,
    kind: &str,
) -> std::result::Result<BarcodeField, String> {
    let kind = parse_barcode_kind(kind).ok_or_else(|| {
        format!(
            "{field} barcode type {} is unsupported",
            kind.to_ascii_uppercase()
        )
    })?;
    if value.is_empty() || value.chars().count() > 1024 {
        return Err(format!(
            "{field} value must contain from 1 through 1024 characters"
        ));
    }
    validate_barcode_value(field, kind, &value)?;
    let mut barcode = BarcodeField {
        value,
        kind,
        height: None,
        scale: None,
        error_correction: None,
        point_of_sale_style: None,
        case_style: None,
        fix_check_digit: false,
        rotation: None,
        foreground_color: None,
        background_color: None,
        display_text: false,
        add_start_stop: false,
    };
    for (name, argument) in switches {
        match name.as_str() {
            "t" if argument.is_none() => barcode.display_text = true,
            "x" if argument.is_none() => barcode.fix_check_digit = true,
            "d" => {
                if argument.is_some() {
                    return Err(format!(
                        "field {field} switch \\d does not take an argument"
                    ));
                }
                if !matches!(kind, BarcodeKind::Nw7 | BarcodeKind::Code39) {
                    return Err(format!("{field} switch \\d requires NW7 or CODE39"));
                }
                barcode.add_start_stop = true;
            }
            "h" | "s" | "q" | "p" | "c" | "r" | "f" | "b" => {
                let Some(value) = argument.as_deref() else {
                    return Err(format!(
                        "field {field} switch \\{name} requires a text argument"
                    ));
                };
                match name.as_str() {
                    "h" => barcode.height = Some(parse_unsigned_integer(value, field, "height")?),
                    "s" => {
                        barcode.scale = Some(
                            u16::try_from(parse_bounded_integer(value, 10, 1000, field, "scale")?)
                                .expect("bounded barcode scale fits u16"),
                        )
                    }
                    "q" => {
                        if kind != BarcodeKind::Qr {
                            return Err(format!("{field} switch \\q requires QR"));
                        }
                        barcode.error_correction = Some(
                            u8::try_from(parse_bounded_integer(
                                value,
                                0,
                                3,
                                field,
                                "error correction",
                            )?)
                            .expect("bounded error correction fits u8"),
                        );
                    }
                    "p" => {
                        if !matches!(
                            kind,
                            BarcodeKind::Upca
                                | BarcodeKind::Upce
                                | BarcodeKind::Ean13
                                | BarcodeKind::Ean8
                        ) {
                            return Err(format!(
                                "{field} switch \\p requires UPCA, UPCE, EAN13, or EAN8"
                            ));
                        }
                        barcode.point_of_sale_style =
                            Some(parse_point_of_sale_style(value).ok_or_else(|| {
                                format!(
                                    "{field} point-of-sale style must be STD, SUP2, SUP5, or CASE"
                                )
                            })?);
                    }
                    "c" => {
                        if !matches!(kind, BarcodeKind::Case | BarcodeKind::Itf14) {
                            return Err(format!("{field} switch \\c requires ITF14"));
                        }
                        barcode.case_style = Some(parse_case_style(value).ok_or_else(|| {
                            format!("{field} case style must be STD, EXT, or ADD")
                        })?);
                    }
                    "r" => {
                        let rotation = parse_bounded_integer(value, 0, 3, field, "rotation")?;
                        barcode.rotation =
                            Some(u8::try_from(rotation).expect("bounded barcode rotation fits u8"));
                    }
                    "f" => barcode.foreground_color = Some(parse_barcode_color(value, field)?),
                    "b" => barcode.background_color = Some(parse_barcode_color(value, field)?),
                    _ => unreachable!(),
                }
            }
            name if argument.is_some() => {
                return Err(format!(
                    "field {field} switch \\{name} does not take an argument"
                ));
            }
            _ => return Err(format!("field {field} uses unsupported switch \\{name}")),
        }
    }
    Ok(barcode)
}

fn parse_barcode_kind(value: &str) -> Option<BarcodeKind> {
    match value.to_ascii_uppercase().as_str() {
        "UPCA" => Some(BarcodeKind::Upca),
        "UPCE" => Some(BarcodeKind::Upce),
        "JAN13" => Some(BarcodeKind::Jan13),
        "JAN8" => Some(BarcodeKind::Jan8),
        "EAN13" => Some(BarcodeKind::Ean13),
        "EAN8" => Some(BarcodeKind::Ean8),
        "CASE" => Some(BarcodeKind::Case),
        "ITF14" => Some(BarcodeKind::Itf14),
        "NW7" => Some(BarcodeKind::Nw7),
        "CODE39" => Some(BarcodeKind::Code39),
        "CODE128" => Some(BarcodeKind::Code128),
        "JPPOST" => Some(BarcodeKind::JpPost),
        "QR" => Some(BarcodeKind::Qr),
        _ => None,
    }
}

fn barcode_kind_name(kind: BarcodeKind) -> &'static str {
    match kind {
        BarcodeKind::Upca => "UPCA",
        BarcodeKind::Upce => "UPCE",
        BarcodeKind::Jan13 => "JAN13",
        BarcodeKind::Jan8 => "JAN8",
        BarcodeKind::Ean13 => "EAN13",
        BarcodeKind::Ean8 => "EAN8",
        BarcodeKind::Case => "CASE",
        BarcodeKind::Itf14 => "ITF14",
        BarcodeKind::Nw7 => "NW7",
        BarcodeKind::Code39 => "CODE39",
        BarcodeKind::Code128 => "CODE128",
        BarcodeKind::JpPost => "JPPOST",
        BarcodeKind::Qr => "QR",
    }
}

fn parse_point_of_sale_style(value: &str) -> Option<BarcodePointOfSaleStyle> {
    match value.to_ascii_uppercase().as_str() {
        "STD" => Some(BarcodePointOfSaleStyle::Standard),
        "SUP2" => Some(BarcodePointOfSaleStyle::SupplementalTwoDigit),
        "SUP5" => Some(BarcodePointOfSaleStyle::SupplementalFiveDigit),
        "CASE" => Some(BarcodePointOfSaleStyle::Case),
        _ => None,
    }
}

fn parse_case_style(value: &str) -> Option<BarcodeCaseStyle> {
    match value.to_ascii_uppercase().as_str() {
        "STD" => Some(BarcodeCaseStyle::Standard),
        "EXT" => Some(BarcodeCaseStyle::Extended),
        "ADD" => Some(BarcodeCaseStyle::Add),
        _ => None,
    }
}

fn parse_unsigned_integer(
    value: &str,
    field: &str,
    label: &str,
) -> std::result::Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("{field} {label} must be a nonnegative integer"))
}

fn parse_bounded_integer(
    value: &str,
    minimum: u32,
    maximum: u32,
    field: &str,
    label: &str,
) -> std::result::Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| (minimum..=maximum).contains(value))
        .ok_or_else(|| format!("{field} {label} must be from {minimum} through {maximum}"))
}

fn validate_barcode_value(
    field: &str,
    kind: BarcodeKind,
    value: &str,
) -> std::result::Result<(), String> {
    let digit_range = match kind {
        BarcodeKind::Ean8 | BarcodeKind::Jan8 => Some(7..=8),
        BarcodeKind::Ean13 | BarcodeKind::Jan13 => Some(12..=13),
        BarcodeKind::Upca => Some(11..=12),
        BarcodeKind::Upce => Some(6..=8),
        BarcodeKind::Case | BarcodeKind::Itf14 => Some(13..=14),
        _ => None,
    };
    if let Some(range) = digit_range
        && (!value.bytes().all(|byte| byte.is_ascii_digit()) || !range.contains(&value.len()))
    {
        return Err(format!(
            "{field} {} value has an invalid digit count or character",
            barcode_kind_name(kind)
        ));
    }
    if kind == BarcodeKind::Code39
        && !value.bytes().all(|byte| {
            byte.is_ascii_uppercase()
                || byte.is_ascii_digit()
                || matches!(byte, b' ' | b'-' | b'.' | b'$' | b'/' | b'+' | b'%')
        })
    {
        return Err(format!(
            "{field} CODE39 value contains an unsupported character"
        ));
    }
    Ok(())
}

fn parse_barcode_color(value: &str, field: &str) -> std::result::Result<u32, String> {
    let parsed = if let Some(hexadecimal) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u32::from_str_radix(hexadecimal, 16).ok()
    } else {
        value.parse::<u32>().ok()
    };
    parsed
        .filter(|value| *value <= 0xFF_FFFF)
        .ok_or_else(|| format!("{field} barcode colour must be from 0 through 0xFFFFFF"))
}

pub(super) struct FormulaParser {
    characters: Vec<char>,
    index: usize,
    token_count: usize,
    depth: usize,
}

impl FormulaParser {
    pub(super) fn new(input: &str) -> std::result::Result<Self, String> {
        if input.len() > MAX_FORMULA_BYTES {
            return Err("formula exceeds the 4096-byte limit".to_owned());
        }
        if !input.is_ascii() {
            return Err("formula contains unsupported non-ASCII syntax".to_owned());
        }
        Ok(Self {
            characters: input.chars().collect(),
            index: 0,
            token_count: 0,
            depth: 0,
        })
    }

    pub(super) fn parse(mut self) -> std::result::Result<f64, String> {
        let value = self.parse_comparison()?;
        self.skip_whitespace();
        if self.index != self.characters.len() {
            return Err("formula contains unsupported or trailing syntax".to_owned());
        }
        if !value.is_finite() {
            return Err("formula result is outside the finite numeric range".to_owned());
        }
        Ok(value)
    }

    fn parse_comparison(&mut self) -> std::result::Result<f64, String> {
        let left = self.parse_additive()?;
        self.skip_whitespace();
        let operator = ["<=", ">=", "<>", "=", "<", ">"]
            .into_iter()
            .find(|operator| self.remaining().starts_with(operator));
        let Some(operator) = operator else {
            return Ok(left);
        };
        self.index += operator.len();
        self.count_token()?;
        let right = self.parse_additive()?;
        Ok(
            if match operator {
                "=" => left == right,
                "<>" => left != right,
                "<" => left < right,
                "<=" => left <= right,
                ">" => left > right,
                ">=" => left >= right,
                _ => unreachable!(),
            } {
                1.0
            } else {
                0.0
            },
        )
    }

    fn parse_additive(&mut self) -> std::result::Result<f64, String> {
        let mut value = self.parse_multiplicative()?;
        loop {
            self.skip_whitespace();
            let operator = self.peek();
            if !matches!(operator, Some('+') | Some('-')) {
                return Ok(value);
            }
            self.index += 1;
            self.count_token()?;
            let right = self.parse_multiplicative()?;
            value = if operator == Some('+') {
                value + right
            } else {
                value - right
            };
            self.ensure_finite(value)?;
        }
    }

    fn parse_multiplicative(&mut self) -> std::result::Result<f64, String> {
        let mut value = self.parse_power()?;
        loop {
            self.skip_whitespace();
            let operator = self.peek();
            if !matches!(operator, Some('*') | Some('/')) {
                return Ok(value);
            }
            self.index += 1;
            self.count_token()?;
            let right = self.parse_power()?;
            if operator == Some('/') && right == 0.0 {
                return Err("formula divides by zero".to_owned());
            }
            value = match operator {
                Some('*') => value * right,
                Some('/') => value / right,
                _ => unreachable!(),
            };
            self.ensure_finite(value)?;
        }
    }

    fn parse_power(&mut self) -> std::result::Result<f64, String> {
        let value = self.parse_unary()?;
        self.skip_whitespace();
        if self.peek() != Some('^') {
            return Ok(value);
        }
        self.index += 1;
        self.count_token()?;
        let exponent = self.parse_power()?;
        let result = value.powf(exponent);
        self.ensure_finite(result)?;
        Ok(result)
    }

    fn parse_unary(&mut self) -> std::result::Result<f64, String> {
        self.skip_whitespace();
        match self.peek() {
            Some('+') => {
                self.index += 1;
                self.count_token()?;
                self.parse_unary()
            }
            Some('-') => {
                self.index += 1;
                self.count_token()?;
                Ok(-self.parse_unary()?)
            }
            _ => self.parse_percentage(),
        }
    }

    fn parse_percentage(&mut self) -> std::result::Result<f64, String> {
        let mut value = self.parse_primary()?;
        loop {
            self.skip_whitespace();
            if self.peek() != Some('%') {
                return Ok(value);
            }
            self.index += 1;
            self.count_token()?;
            value /= 100.0;
            self.ensure_finite(value)?;
        }
    }

    fn parse_primary(&mut self) -> std::result::Result<f64, String> {
        self.skip_whitespace();
        if self.peek() == Some('(') {
            self.index += 1;
            self.count_token()?;
            self.depth += 1;
            if self.depth > MAX_FORMULA_DEPTH {
                return Err("formula exceeds the 32-level nesting limit".to_owned());
            }
            let value = self.parse_comparison()?;
            self.skip_whitespace();
            if self.peek() != Some(')') {
                return Err("formula has an unclosed parenthesis".to_owned());
            }
            self.index += 1;
            self.count_token()?;
            self.depth -= 1;
            return Ok(value);
        }
        let start = self.index;
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_digit() || matches!(character, '.' | ','))
        {
            self.index += 1;
        }
        if start == self.index {
            return Err(
                if self
                    .peek()
                    .is_some_and(|character| character.is_ascii_alphabetic())
                {
                    "formula functions are unsupported".to_owned()
                } else {
                    "formula requires a numeric operand".to_owned()
                },
            );
        }
        self.count_token()?;
        let number = self.characters[start..self.index]
            .iter()
            .filter(|character| **character != ',')
            .collect::<String>();
        number
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| "formula contains an invalid or out-of-range number".to_owned())
    }

    fn remaining(&self) -> String {
        self.characters[self.index..].iter().collect()
    }

    fn peek(&self) -> Option<char> {
        self.characters.get(self.index).copied()
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.index += 1;
        }
    }

    fn count_token(&mut self) -> std::result::Result<(), String> {
        self.token_count += 1;
        if self.token_count > MAX_FORMULA_TOKENS {
            Err("formula exceeds the 512-token limit".to_owned())
        } else {
            Ok(())
        }
    }

    fn ensure_finite(&self, value: f64) -> std::result::Result<(), String> {
        if value.is_finite() {
            Ok(())
        } else {
            Err("formula result is outside the finite numeric range".to_owned())
        }
    }
}

pub(super) fn format_formula_value(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    if value.fract() == 0.0 && value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        return format!("{value:.0}");
    }
    format!("{value:.14e}")
        .parse::<f64>()
        .unwrap_or(value)
        .to_string()
}

pub(super) fn text_argument(instruction: &FieldInstruction, index: usize) -> Option<&str> {
    instruction.arguments.get(index).and_then(argument_text)
}

pub(super) fn argument_text(argument: &FieldArgument) -> Option<&str> {
    match argument {
        FieldArgument::Text(value) => Some(value),
        FieldArgument::Nested(_) => None,
    }
}

pub(super) fn has_switch(instruction: &FieldInstruction, name: &str) -> bool {
    instruction
        .switches
        .iter()
        .any(|switch| switch.name == name)
}

pub(super) fn switch_text<'a>(instruction: &'a FieldInstruction, name: &str) -> Option<&'a str> {
    instruction.switches.iter().find_map(|switch| {
        (switch.name == name)
            .then_some(switch.argument.as_ref())
            .flatten()
            .and_then(argument_text)
    })
}

pub(super) fn unsupported_switch(instruction: &FieldInstruction) -> Option<&str> {
    let allowed: &[&str] = match instruction.name.as_str() {
        "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" => &["*", "#"],
        "REF" => &["h", "n", "r", "t", "w", "p", "f", "d", "*", "#"],
        "PAGEREF" => &["h", "p", "*", "#"],
        "IF" => &["*", "#"],
        "SEQ" => &["n", "c", "h", "r", "s", "*", "#"],
        "DOCPROPERTY" | "DOCVARIABLE" => &["*", "#", "@"],
        "STYLEREF" => &["l", "n", "r", "t", "w", "p", "*", "#"],
        "INCLUDETEXT" => &["!", "c", "*", "#"],
        "DATE" | "TIME" => &["@", "*"],
        "FILENAME" => &["p", "*"],
        "AUTHOR" => &["*"],
        "MERGEFIELD" => &["b", "f", "m", "v", "*", "#", "@"],
        "=" => &["*", "#"],
        "NEXT" | "NEXTIF" | "SKIPIF" | "MERGEREC" | "MERGESEQ" => &[],
        _ => return None,
    };
    instruction
        .switches
        .iter()
        .find(|switch| !allowed.contains(&switch.name.as_str()))
        .map(|switch| switch.name.as_str())
}

pub(super) fn validate_instruction_shape(
    instruction: &FieldInstruction,
) -> std::result::Result<(), String> {
    if !instruction.quotes_are_balanced() {
        return Err(format!("field {} has unclosed quoting", instruction.name));
    }
    let argument_range = match instruction.name.as_str() {
        "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "DATE" | "TIME" | "FILENAME"
        | "AUTHOR" => 0..=0,
        "REF" | "PAGEREF" | "SEQ" | "DOCPROPERTY" | "DOCVARIABLE" | "STYLEREF" | "MERGEFIELD" => {
            1..=1
        }
        "IF" => 5..=5,
        "INCLUDETEXT" => 1..=2,
        "=" => 1..=usize::MAX,
        "TOC" => 0..=0,
        "TC" => 1..=1,
        "DISPLAYBARCODE" | "MERGEBARCODE" => 2..=2,
        "NEXT" | "MERGEREC" | "MERGESEQ" => 0..=0,
        "NEXTIF" | "SKIPIF" => 3..=3,
        _ => return Ok(()),
    };
    if !argument_range.contains(&instruction.arguments.len()) {
        let expected = if argument_range.start() == argument_range.end() {
            argument_range.start().to_string()
        } else {
            format!(
                "{} through {}",
                argument_range.start(),
                argument_range.end()
            )
        };
        return Err(format!(
            "field {} requires {expected} positional operands",
            instruction.name
        ));
    }

    if matches!(
        instruction.name.as_str(),
        "TOC" | "TC" | "DISPLAYBARCODE" | "MERGEBARCODE"
    ) {
        return Ok(());
    }

    for switch in &instruction.switches {
        let requires_text = matches!(switch.name.as_str(), "*" | "#" | "@")
            || instruction.name == "REF" && switch.name == "d"
            || instruction.name == "SEQ" && matches!(switch.name.as_str(), "r" | "s")
            || instruction.name == "MERGEFIELD" && matches!(switch.name.as_str(), "b" | "f")
            || instruction.name == "INCLUDETEXT" && switch.name == "c";
        match (&switch.argument, requires_text) {
            (Some(FieldArgument::Text(_)), true) | (None, false) => {}
            (_, true) => {
                return Err(format!(
                    "field {} switch \\{} requires a text argument",
                    instruction.name, switch.name
                ));
            }
            (Some(_), false) => {
                return Err(format!(
                    "field {} switch \\{} does not take an argument",
                    instruction.name, switch.name
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn compare_if(left: &str, operator: &str, right: &str) -> Option<bool> {
    let numeric = left
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .zip(right.parse::<f64>().ok().filter(|value| value.is_finite()));
    let ordering = numeric
        .map(|(left, right)| left.total_cmp(&right))
        .unwrap_or_else(|| left.to_lowercase().cmp(&right.to_lowercase()));
    match operator {
        "=" => Some(if right.contains(['*', '?']) {
            wildcard_matches(right, left)
        } else {
            ordering.is_eq()
        }),
        "<>" => Some(if right.contains(['*', '?']) {
            !wildcard_matches(right, left)
        } else {
            !ordering.is_eq()
        }),
        "<" => Some(ordering.is_lt()),
        "<=" => Some(!ordering.is_gt()),
        ">" => Some(ordering.is_gt()),
        ">=" => Some(!ordering.is_lt()),
        _ => None,
    }
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let mut expression = String::from("(?is)^");
    for character in pattern.chars() {
        match character {
            '*' => expression.push_str(".*"),
            '?' => expression.push('.'),
            other => expression.push_str(&regex::escape(&other.to_string())),
        }
    }
    expression.push('$');
    regex::Regex::new(&expression).is_ok_and(|regex| regex.is_match(value))
}

pub(super) fn custom_property_text(value: &CustomPropertyValue) -> Option<String> {
    match value {
        CustomPropertyValue::Lpstr(value)
        | CustomPropertyValue::Lpwstr(value)
        | CustomPropertyValue::FileTime(value) => Some(value.clone()),
        CustomPropertyValue::I4(value) => Some(value.to_string()),
        CustomPropertyValue::R8(value) if value.is_finite() => Some(value.to_string()),
        CustomPropertyValue::Bool(value) => Some(value.to_string()),
        CustomPropertyValue::Empty => Some(String::new()),
        CustomPropertyValue::R8(_) | CustomPropertyValue::Raw(_) => None,
    }
}

pub(super) fn normalized_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) fn lexical_file_name(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
}

pub(super) fn apply_formats(
    instruction: &FieldInstruction,
    value: &str,
    date_time: Option<FieldDateTime>,
) -> std::result::Result<String, String> {
    let mut output = value.to_owned();
    if let Some(picture) = switch_text(instruction, "#") {
        output = oxoffice_docx_layout::engine::format_numeric_field_picture(value, picture)?;
    }
    if let Some(picture) = switch_text(instruction, "@") {
        let date_time = if matches!(instruction.name.as_str(), "DATE" | "TIME") {
            date_time.ok_or_else(|| {
                "date-time formatting requires an explicit date and time".to_owned()
            })?
        } else {
            parse_field_date_time(value).ok_or_else(|| {
                "date-time field value is not a supported civil date and time".to_owned()
            })?
        };
        output = format_date_time(date_time, picture)?;
    }
    oxoffice_docx_layout::engine::format_numeric_field_general(instruction, &output)
}

pub(super) fn valid_date_time(value: FieldDateTime) -> bool {
    value.month >= 1
        && value.month <= 12
        && value.day >= 1
        && value.day <= days_in_month(value.year, value.month)
        && value.hour < 24
        && value.minute < 60
        && value.second < 60
}

fn parse_field_date_time(value: &str) -> Option<FieldDateTime> {
    let value = value.trim().strip_suffix('Z').unwrap_or(value.trim());
    let (date, time) = value
        .split_once('T')
        .or_else(|| value.split_once(' '))
        .map_or((value, None), |(date, time)| (date, Some(time)));
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse().ok()?;
    let month = date_parts.next()?.parse().ok()?;
    let day = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let (hour, minute, second) = if let Some(time) = time {
        let mut time_parts = time.split(':');
        let hour = time_parts.next()?.parse().ok()?;
        let minute = time_parts.next()?.parse().ok()?;
        let second = time_parts.next()?.split('.').next()?.parse().ok()?;
        if time_parts.next().is_some() {
            return None;
        }
        (hour, minute, second)
    } else {
        (0, 0, 0)
    };
    let parsed = FieldDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    };
    valid_date_time(parsed).then_some(parsed)
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 400 == 0 || year % 4 == 0 && year % 100 != 0 => 29,
        2 => 28,
        _ => 31,
    }
}

pub(super) fn format_date_time(
    value: FieldDateTime,
    picture: &str,
) -> std::result::Result<String, String> {
    if !valid_date_time(value) {
        return Err("field date and time is invalid".to_owned());
    }
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let weekdays = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let characters = picture.chars().collect::<Vec<_>>();
    let mut output = String::new();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] == '"' {
            index += 1;
            while index < characters.len() && characters[index] != '"' {
                output.push(characters[index]);
                index += 1;
            }
            if index == characters.len() {
                return Err("date-time picture has unclosed quoting".to_owned());
            }
            index += 1;
            continue;
        }
        if characters[index] == '\\' {
            index += 1;
            let Some(character) = characters.get(index) else {
                return Err("date-time picture ends with an escape".to_owned());
            };
            output.push(*character);
            index += 1;
            continue;
        }
        let rest = characters[index..].iter().collect::<String>();
        if rest.to_ascii_uppercase().starts_with("AM/PM") {
            output.push_str(if value.hour < 12 { "AM" } else { "PM" });
            index += 5;
            continue;
        }
        let token = characters[index];
        if !matches!(token, 'y' | 'M' | 'd' | 'H' | 'h' | 'm' | 's') {
            output.push(token);
            index += 1;
            continue;
        }
        let mut count = 1;
        while index + count < characters.len() && characters[index + count] == token {
            count += 1;
        }
        match token {
            'y' if count == 2 => output.push_str(&format!("{:02}", value.year.rem_euclid(100))),
            'y' => output.push_str(&format!("{:04}", value.year)),
            'M' if count == 1 => output.push_str(&value.month.to_string()),
            'M' if count == 2 => output.push_str(&format!("{:02}", value.month)),
            'M' if count == 3 => output.push_str(&months[value.month as usize - 1][..3]),
            'M' => output.push_str(months[value.month as usize - 1]),
            'd' if count == 1 => output.push_str(&value.day.to_string()),
            'd' if count == 2 => output.push_str(&format!("{:02}", value.day)),
            'd' if count == 3 => output.push_str(&weekdays[weekday(value)][..3]),
            'd' => output.push_str(weekdays[weekday(value)]),
            'H' if count == 1 => output.push_str(&value.hour.to_string()),
            'H' => output.push_str(&format!("{:02}", value.hour)),
            'h' if count == 1 => output.push_str(&(value.hour % 12).max(1).to_string()),
            'h' => output.push_str(&format!("{:02}", (value.hour % 12).max(1))),
            'm' if count == 1 => output.push_str(&value.minute.to_string()),
            'm' => output.push_str(&format!("{:02}", value.minute)),
            's' if count == 1 => output.push_str(&value.second.to_string()),
            's' => output.push_str(&format!("{:02}", value.second)),
            _ => unreachable!(),
        }
        index += count;
    }
    Ok(output)
}

fn weekday(value: FieldDateTime) -> usize {
    let mut year = i64::from(value.year);
    let mut month = i64::from(value.month);
    if month < 3 {
        month += 12;
        year -= 1;
    }
    let year_of_century = year.rem_euclid(100);
    let century = year.div_euclid(100);
    let h = (i64::from(value.day)
        + (13 * (month + 1)) / 5
        + year_of_century
        + year_of_century / 4
        + century / 4
        + 5 * century)
        .rem_euclid(7);
    ((h + 6) % 7) as usize
}
