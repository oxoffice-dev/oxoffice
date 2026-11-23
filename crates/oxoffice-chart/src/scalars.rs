//! Scalar elements: parsing and writing their values, lexical booleans, once-only slots and the errors they raise.

use super::*;

pub(super) fn parse_u32_scalar(xml: &[u8], local: &str) -> Result<(u32, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed = value
        .parse::<u32>()
        .map_err(|_| invalid_attribute(local, "val", value))?;
    Ok((parsed, markup))
}

pub(super) fn required_u32_attribute(
    element: &BytesStart<'_>,
    local: &str,
    attribute: &[u8],
) -> Result<u32> {
    let value = attribute_value(element, attribute)?.ok_or_else(|| {
        invalid_attribute(
            local,
            &String::from_utf8_lossy(attribute),
            "<missing>".to_owned(),
        )
    })?;
    value
        .parse::<u32>()
        .map_err(|_| invalid_attribute(local, &String::from_utf8_lossy(attribute), value))
}

pub(super) fn optional_markup<T>(
    value: Option<(T, ScalarMarkup)>,
) -> (Option<T>, Option<ScalarMarkup>) {
    value
        .map(|(value, markup)| (Some(value), Some(markup)))
        .unwrap_or((None, None))
}

pub(super) fn optional_scalar_markup_eq(
    left: &Option<ScalarMarkup>,
    right: &Option<ScalarMarkup>,
) -> bool {
    let empty = ScalarMarkup::default();
    left.as_ref().unwrap_or(&empty) == right.as_ref().unwrap_or(&empty)
}

pub(super) const fn bool_lexical(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

pub(super) fn parse_bool_lexical(element: &str, attribute: &str, value: &str) -> Result<bool> {
    match value {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(invalid_attribute(element, attribute, value.to_owned())),
    }
}

pub(super) fn parse_bool_value(xml: &[u8], local: &str) -> Result<(bool, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = match value.as_deref().unwrap_or("1") {
        "1" | "true" => true,
        "0" | "false" => false,
        invalid => return Err(invalid_attribute(local, "val", invalid.to_owned())),
    };
    Ok((value, markup))
}

pub(super) fn parse_enum_value(xml: &[u8], local: &str) -> Result<(DispBlanksAs, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed =
        DispBlanksAs::parse(&value).ok_or_else(|| invalid_attribute(local, "val", value))?;
    Ok((parsed, markup))
}

pub(super) fn scalar_value(xml: &[u8], expected: &str) -> Result<(Option<String>, ScalarMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Empty(element)
                if matches_local_name(element.name().as_ref(), expected.as_bytes()) =>
            {
                reject_conflicting_prefix(&element, b"c", C_NS)?;
                return Ok((
                    attribute_value(&element, b"val")?,
                    ScalarMarkup {
                        raw_attributes: capture_attributes_except(&element, b"val")?,
                        raw_content: Vec::new(),
                    },
                ));
            }
            Event::Start(element)
                if matches_local_name(element.name().as_ref(), expected.as_bytes()) =>
            {
                reject_conflicting_prefix(&element, b"c", C_NS)?;
                let value = attribute_value(&element, b"val")?;
                let raw_attributes = capture_attributes_except(&element, b"val")?;
                let mut raw_content = Vec::new();
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::End(end)
                            if matches_local_name(end.name().as_ref(), expected.as_bytes()) =>
                        {
                            return Ok((
                                value,
                                ScalarMarkup {
                                    raw_attributes,
                                    raw_content,
                                },
                            ));
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => raw_content.push(capture_event(event)?),
                        Event::Eof => return Err(missing_end(expected)),
                        _ => return Err(ChartError::UnexpectedElement(expected.to_owned())),
                    }
                    inner.clear();
                }
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => return Err(ChartError::MissingElement(expected.to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<()> {
    if slot.is_some() {
        return Err(ChartError::DuplicateElement(name.to_owned()));
    }
    *slot = Some(value);
    Ok(())
}

pub(super) fn mark_once(seen: &mut bool, name: &str) -> Result<()> {
    if *seen {
        return Err(ChartError::DuplicateElement(name.to_owned()));
    }
    *seen = true;
    Ok(())
}

pub(super) fn write_scalar<W: Write>(
    writer: &mut Writer<W>,
    name: &str,
    value: &str,
    markup: Option<&ScalarMarkup>,
) -> Result<()> {
    let mut start = BytesStart::new(name);
    start.push_attribute(("val", value));
    if let Some(markup) = markup {
        push_attributes(&mut start, &markup.raw_attributes);
    }
    let has_content = markup.is_some_and(|markup| !markup.raw_content.is_empty());
    if !has_content {
        writer
            .write_event(Event::Empty(start))
            .map_err(OxmlError::from)?;
        return Ok(());
    }
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    if let Some(markup) = markup {
        for raw in &markup.raw_content {
            writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
        }
    }
    writer
        .write_event(Event::End(BytesEnd::new(name)))
        .map_err(OxmlError::from)?;
    Ok(())
}

pub(super) fn invalid_attribute(element: &str, attribute: &str, value: String) -> ChartError {
    ChartError::InvalidAttribute {
        element: element.to_owned(),
        attribute: attribute.to_owned(),
        value,
    }
}

pub(super) fn element_name(element: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(element.name().as_ref()).into_owned()
}

pub(super) fn missing_end(element: &str) -> ChartError {
    ChartError::Xml(OxmlError::MissingElement(format!("closing {element}")))
}
