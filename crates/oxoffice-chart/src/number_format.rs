//! Number formats of axes and labels, and the numeric sections of a format code.

use super::*;

/// Number format applied to axis values and labels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumberFormat {
    pub format_code: String,
    pub source_linked: bool,
    raw_attributes: XmlAttributes,
    raw_content: Vec<Vec<u8>>,
}

impl NumberFormat {
    pub fn new(format_code: String, source_linked: bool) -> Result<Self> {
        let number_format = Self {
            format_code,
            source_linked,
            raw_attributes: Vec::new(),
            raw_content: Vec::new(),
        };
        number_format.validate()?;
        Ok(number_format)
    }

    pub(super) fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Empty(element) if matches_local_name(element.name().as_ref(), b"numFmt") => {
                    return Self::from_element(&element, Vec::new());
                }
                Event::Start(element) if matches_local_name(element.name().as_ref(), b"numFmt") => {
                    let mut raw_content = Vec::new();
                    let mut inner = Vec::new();
                    loop {
                        match reader
                            .read_event_into(&mut inner)
                            .map_err(OxmlError::from)?
                        {
                            Event::End(end)
                                if matches_local_name(end.name().as_ref(), b"numFmt") =>
                            {
                                return Self::from_element(&element, raw_content);
                            }
                            event @ (Event::Text(_)
                            | Event::CData(_)
                            | Event::Comment(_)
                            | Event::PI(_)
                            | Event::GeneralRef(_)) => raw_content.push(capture_event(event)?),
                            Event::Eof => return Err(missing_end("c:numFmt")),
                            _ => return Err(ChartError::UnexpectedElement("c:numFmt".to_owned())),
                        }
                        inner.clear();
                    }
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("c:numFmt".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(element: &BytesStart<'_>, raw_content: Vec<Vec<u8>>) -> Result<Self> {
        reject_conflicting_prefix(element, b"c", C_NS)?;
        let format_code = attribute_value(element, b"formatCode")?
            .ok_or_else(|| invalid_attribute("numFmt", "formatCode", "<missing>".to_owned()))?;
        let source_linked = attribute_value(element, b"sourceLinked")?
            .ok_or_else(|| invalid_attribute("numFmt", "sourceLinked", "<missing>".to_owned()))?;
        let source_linked = parse_bool_lexical("numFmt", "sourceLinked", &source_linked)?;
        let number_format = Self {
            format_code,
            source_linked,
            raw_attributes: capture_attributes_excluding(
                element,
                &[b"formatCode", b"sourceLinked"],
            )?,
            raw_content,
        };
        number_format.validate()?;
        Ok(number_format)
    }

    pub(super) fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.validate()?;
        let mut start = BytesStart::new("c:numFmt");
        start.push_attribute(("formatCode", self.format_code.as_str()));
        start.push_attribute(("sourceLinked", bool_lexical(self.source_linked)));
        push_attributes(&mut start, &self.raw_attributes);
        if self.raw_content.is_empty() {
            writer
                .write_event(Event::Empty(start))
                .map_err(OxmlError::from)?;
            return Ok(());
        }
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        for raw in &self.raw_content {
            writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("c:numFmt")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    /// Projects one cached value through the supported deterministic subset.
    pub fn format_value(&self, value: f64) -> Result<String> {
        if !value.is_finite() {
            return Err(ChartError::InvalidValue {
                element: "c:numFmt value".to_owned(),
                value: value.to_string(),
            });
        }
        if self.format_code == "General" {
            return Ok(value.to_string());
        }

        let sections =
            numeric_sections(&self.format_code).ok_or_else(|| ChartError::InvalidValue {
                element: "c:numFmt/@formatCode projection".into(),
                value: self.format_code.clone(),
            })?;
        let index = if value < 0.0 && sections.len() >= 2 {
            1
        } else if value == 0.0 && sections.len() >= 3 {
            2
        } else {
            0
        };
        let mut text = sections[index].format(value.abs())?;
        if value < 0.0 && sections.len() == 1 && !text.starts_with('-') {
            text.insert(0, '-');
        }
        Ok(text)
    }

    pub(super) fn validate(&self) -> Result<()> {
        validate_xml_text(&self.format_code, "c:numFmt/@formatCode")
    }
}

#[derive(Clone, PartialEq)]
enum NumericToken {
    Digit(bool),
    Point,
    Comma,
    Percent,
    Literal(String),
}
pub(super) struct NumericSection {
    tokens: Vec<NumericToken>,
    group: usize,
    scale: usize,
}
pub(super) fn numeric_sections(code: &str) -> Option<Vec<NumericSection>> {
    if code.is_empty() {
        return None;
    }
    let mut parts = vec![String::new()];
    let mut chars = code.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        if c == '\\' || !quoted && matches!(c, '_' | '*') {
            parts.last_mut()?.push(c);
            parts.last_mut()?.push(chars.next()?);
            continue;
        }
        if c == '"' {
            if quoted && chars.peek() == Some(&'"') {
                parts.last_mut()?.push_str("\"\"");
                chars.next();
                continue;
            }
            quoted = !quoted;
        }
        if c == ';' && !quoted {
            parts.push(String::new());
        } else {
            parts.last_mut()?.push(c);
        }
    }
    if quoted || parts.len() > 4 {
        return None;
    }
    parts.truncate(3);
    if parts.len() > 1 && parts.iter().any(|part| part == "General") {
        return None;
    }
    let sections = parts
        .iter()
        .map(|part| numeric_section(part))
        .collect::<Option<Vec<_>>>()?;
    if !sections.iter().any(|section| {
        section
            .tokens
            .iter()
            .any(|t| matches!(t, NumericToken::Digit(_)))
    }) {
        return None;
    }
    Some(sections)
}
fn numeric_section(code: &str) -> Option<NumericSection> {
    let mut chars = code.chars().peekable();
    let mut tokens = Vec::new();
    let mut point = false;
    while let Some(c) = chars.next() {
        let token = match c {
            '"' => {
                let mut literal = String::new();
                loop {
                    match chars.next()? {
                        '"' if chars.peek() == Some(&'"') => {
                            chars.next();
                            literal.push('"');
                        }
                        '"' => break,
                        c => literal.push(c),
                    }
                }
                NumericToken::Literal(literal)
            }
            '\\' => NumericToken::Literal(chars.next()?.to_string()),
            '_' => {
                chars.next()?;
                NumericToken::Literal(" ".into())
            }
            '*' => {
                chars.next()?;
                continue;
            }
            '[' => {
                let mut bracket = String::new();
                loop {
                    let c = chars.next()?;
                    if c == ']' {
                        break;
                    }
                    bracket.push(c);
                }
                if let Some(currency) = bracket.strip_prefix('$') {
                    let symbol = currency
                        .rsplit_once('-')
                        .filter(|(_, suffix)| {
                            !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_hexdigit())
                        })
                        .map_or(currency, |(symbol, _)| symbol);
                    NumericToken::Literal(symbol.into())
                } else if [
                    "red", "blue", "green", "yellow", "black", "white", "cyan", "magenta",
                ]
                .contains(&bracket.to_ascii_lowercase().as_str())
                {
                    continue;
                } else {
                    return None;
                }
            }
            '0' | '#' | '?' => NumericToken::Digit(c == '0'),
            '.' => {
                if point {
                    return None;
                }
                point = true;
                NumericToken::Point
            }
            ',' => NumericToken::Comma,
            '%' => NumericToken::Percent,
            c if matches!(c.to_ascii_lowercase(), 'y' | 'm' | 'd' | 'h' | 's') => return None,
            'A' | 'a'
                if {
                    let mut following = chars.clone();
                    following.next() == Some('/') && matches!(following.next(), Some('P' | 'p'))
                } =>
            {
                return None;
            }
            'E' | 'e' if matches!(chars.peek(), Some('+' | '-')) => return None,
            '/' => {
                if matches!(tokens.last(), Some(NumericToken::Digit(_)))
                    && matches!(chars.peek(), Some('0' | '#' | '?'))
                {
                    return None;
                }
                NumericToken::Literal("/".into())
            }
            c => NumericToken::Literal(c.to_string()),
        };
        tokens.push(token);
    }
    let decimal = tokens
        .iter()
        .position(|t| *t == NumericToken::Point)
        .unwrap_or(tokens.len());
    let integer_digits = tokens[..decimal]
        .iter()
        .enumerate()
        .filter_map(|(i, t)| matches!(t, NumericToken::Digit(_)).then_some(i))
        .collect::<Vec<_>>();
    let mut group = 0;
    let mut scale = 0;
    for (i, token) in tokens[..decimal].iter_mut().enumerate() {
        if *token != NumericToken::Comma {
            continue;
        }
        let left = integer_digits.iter().any(|at| *at < i);
        let right = integer_digits.iter().filter(|at| **at > i).count();
        if left && right > 0 {
            group = right;
            *token = NumericToken::Literal(String::new());
        } else if left {
            scale += 1;
            *token = NumericToken::Literal(String::new());
        } else {
            *token = NumericToken::Literal(",".into());
        }
    }
    // Fractional commas are ordinary literal commas, not scale/group directives.
    for token in &mut tokens[decimal..] {
        if *token == NumericToken::Comma {
            *token = NumericToken::Literal(",".into());
        }
    }
    Some(NumericSection {
        tokens,
        group,
        scale,
    })
}
impl NumericSection {
    fn format(&self, value: f64) -> Result<String> {
        let mut scaled = value;
        for token in &self.tokens {
            if *token == NumericToken::Percent {
                scaled *= 100.0;
            }
        }
        for _ in 0..self.scale {
            scaled /= 1000.0;
        }
        if !scaled.is_finite() {
            return Err(ChartError::InvalidValue {
                element: "c:numFmt value".into(),
                value: scaled.to_string(),
            });
        }
        let point = self
            .tokens
            .iter()
            .position(|t| *t == NumericToken::Point)
            .unwrap_or(self.tokens.len());
        let precision = self.tokens[point..]
            .iter()
            .filter(|t| matches!(t, NumericToken::Digit(_)))
            .count();
        let rounded = format!("{scaled:.precision$}");
        let (integer, fraction) = rounded.split_once('.').unwrap_or((&rounded, ""));
        let integer_places = self.tokens[..point]
            .iter()
            .filter(|t| matches!(t, NumericToken::Digit(_)))
            .count();
        let required = self.tokens[..point]
            .iter()
            .filter(|t| **t == NumericToken::Digit(true))
            .count();
        let integer = if integer == "0" && required == 0 {
            String::new()
        } else {
            format!("{integer:0>required$}")
        };
        let extra = integer.len().saturating_sub(integer_places);
        let mut integer_at = 0;
        let mut fraction_at = 0;
        let mut places = integer_places;
        let mut out = String::new();
        let mut fractional_end = fraction.len();
        for token in self.tokens[point..].iter().rev() {
            if let NumericToken::Digit(required) = token {
                if !required
                    && fractional_end > 0
                    && fraction.as_bytes()[fractional_end - 1] == b'0'
                {
                    fractional_end -= 1;
                } else {
                    break;
                }
            }
        }
        for (at, token) in self.tokens.iter().enumerate() {
            match token {
                NumericToken::Digit(required) if at < point => {
                    let take = if integer_at == 0 { extra + 1 } else { 1 };
                    if integer.len().saturating_sub(integer_at) >= places {
                        for _ in 0..take {
                            if integer_at < integer.len() {
                                out.push(integer.as_bytes()[integer_at] as char);
                                integer_at += 1;
                                let remaining = integer.len() - integer_at;
                                if self.group > 0 && remaining > 0 && remaining % self.group == 0 {
                                    out.push(',');
                                }
                            }
                        }
                    } else if *required {
                        out.push('0');
                    }
                    places = places.saturating_sub(1);
                }
                NumericToken::Digit(required) => {
                    if fraction_at < fractional_end || *required {
                        out.push(
                            fraction
                                .as_bytes()
                                .get(fraction_at)
                                .copied()
                                .unwrap_or(b'0') as char,
                        );
                    }
                    fraction_at += 1;
                }
                NumericToken::Point => {
                    if fractional_end > 0 {
                        out.push('.');
                    }
                }
                NumericToken::Literal(text) => out.push_str(text),
                NumericToken::Percent => out.push('%'),
                NumericToken::Comma => {}
            }
        }
        Ok(out)
    }
}
