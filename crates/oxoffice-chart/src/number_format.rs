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

    /// One cached value as Excel displays it with this code, dates counted
    /// in `options.date_system` (the chart space's,
    /// [`CT_ChartSpace::date_system`]). An error for a code the chart does
    /// not support ([`NumberFormat::is_supported`]).
    pub fn format_value(&self, value: f64, options: &FormatOptions) -> Result<String> {
        if !value.is_finite() {
            return Err(ChartError::InvalidValue {
                element: "c:numFmt value".to_owned(),
                value: value.to_string(),
            });
        }
        let unsupported = || ChartError::InvalidValue {
            element: "c:numFmt/@formatCode projection".into(),
            value: self.format_code.clone(),
        };
        let format =
            oxoffice_numfmt::NumberFormat::parse(&self.format_code).map_err(|_| unsupported())?;
        if format.shows_unknown_names() {
            return Err(unsupported());
        }
        Ok(format.format_number(value, options).text)
    }

    /// Whether the chart draws this code as Excel does: not when Excel
    /// cannot read it, nor when it asks for month or day names in a language
    /// `oxoffice-numfmt` has no names for (`[$-40D]mmmm`, Hebrew). Their
    /// values show as General, with a diagnostic.
    pub fn is_supported(&self) -> bool {
        oxoffice_numfmt::NumberFormat::parse(&self.format_code)
            .is_ok_and(|format| !format.shows_unknown_names())
    }

    pub(super) fn validate(&self) -> Result<()> {
        validate_xml_text(&self.format_code, "c:numFmt/@formatCode")
    }
}
