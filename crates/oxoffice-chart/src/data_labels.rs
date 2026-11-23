//! Data labels: positions, parsing and writing.

use super::*;

/// Placement of a chart data label relative to its data point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataLabelPosition {
    BestFit,
    Bottom,
    Center,
    InsideBase,
    InsideEnd,
    Left,
    OutsideEnd,
    Right,
    Top,
}

impl DataLabelPosition {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "bestFit" => Some(Self::BestFit),
            "b" => Some(Self::Bottom),
            "ctr" => Some(Self::Center),
            "inBase" => Some(Self::InsideBase),
            "inEnd" => Some(Self::InsideEnd),
            "l" => Some(Self::Left),
            "outEnd" => Some(Self::OutsideEnd),
            "r" => Some(Self::Right),
            "t" => Some(Self::Top),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::BestFit => "bestFit",
            Self::Bottom => "b",
            Self::Center => "ctr",
            Self::InsideBase => "inBase",
            Self::InsideEnd => "inEnd",
            Self::Left => "l",
            Self::OutsideEnd => "outEnd",
            Self::Right => "r",
            Self::Top => "t",
        }
    }
}

impl CT_DLbls {
    /// Labels that show the percentage and nothing else, with all six flags
    /// written, as PowerPoint writes them: readers disagree on an absent
    /// flag, and LibreOffice reads one as shown.
    pub(super) fn percentage_only() -> Self {
        let stated = || Some(ScalarMarkup::default());
        Self {
            show_percent: true,
            show_legend_key_markup: stated(),
            show_value_markup: stated(),
            show_category_name_markup: stated(),
            show_series_name_markup: stated(),
            show_percent_markup: stated(),
            show_bubble_size_markup: stated(),
            ..Self::default()
        }
    }
}

impl CT_DLbls {
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &chart_namespace_defaults())
    }

    pub(super) fn from_xml_with_namespaces(
        xml: &[u8],
        inherited: &NamespaceBindings,
    ) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) if matches_local_name(element.name().as_ref(), b"dLbls") => {
                    chart_root_prefix(&element)?;
                    if !element_is_in_namespace(&element, C_NS, inherited)? {
                        return Err(ChartError::UnexpectedElement(element_name(&element)));
                    }
                    return Self::from_element(&mut reader, &element, inherited);
                }
                Event::Empty(element) if matches_local_name(element.name().as_ref(), b"dLbls") => {
                    chart_root_prefix(&element)?;
                    let namespaces = chart_bindings(inherited, &element)?;
                    if !element_is_in_namespace(&element, C_NS, inherited)? {
                        return Err(ChartError::UnexpectedElement(element_name(&element)));
                    }
                    let (raw_attributes, _) = capture_fixed_root_attributes(
                        &element,
                        &["xmlns:c", "xmlns:a", "xmlns:r"],
                    )?;
                    return Ok(Self {
                        raw_attributes,
                        namespace_declarations: standalone_namespace_declarations(&namespaces)?,
                        ..Self::default()
                    });
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("c:dLbls".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &NamespaceBindings,
    ) -> Result<Self> {
        reject_conflicting_prefix(start, b"a", A_NS)?;
        let namespaces = chart_bindings(inherited, start)?;
        require_fixed_namespace(&namespaces, b"c", C_NS, start)?;
        let (raw_attributes, _) =
            capture_fixed_root_attributes(start, &["xmlns:c", "xmlns:a", "xmlns:r"])?;
        let namespace_declarations = standalone_namespace_declarations(&namespaces)?;
        let mut state = DataLabelsParseState::new(namespaces);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = chart_child_local(&element, &state.namespaces)?;
                    let raw = capture_element(reader, &element)?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw)?;
                }
                Event::Empty(element) => {
                    let name = chart_child_local(&element, &state.namespaces)?;
                    let raw = capture_empty_element(&element)?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw)?;
                }
                event @ (Event::Text(_)
                | Event::CData(_)
                | Event::Comment(_)
                | Event::PI(_)
                | Event::GeneralRef(_)) => state.capture_event(capture_event(event)?),
                Event::End(element) if matches_local_name(element.name().as_ref(), b"dLbls") => {
                    return state.finish(raw_attributes, namespace_declarations);
                }
                Event::Eof => return Err(missing_end("c:dLbls")),
                _ => {}
            }
            buffer.clear();
        }
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml(&mut writer, true)?;
        Ok(writer.into_inner())
    }

    pub(super) fn write_xml(&self, writer: &mut Writer<Vec<u8>>, standalone: bool) -> Result<()> {
        self.validate()?;
        let mut start = BytesStart::new("c:dLbls");
        if standalone {
            start.push_attribute(("xmlns:c", C_NS));
            start.push_attribute(("xmlns:a", A_NS));
            start.push_attribute(("xmlns:r", R_NS));
        }
        push_attributes(&mut start, &self.namespace_declarations);
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(number_format) = &self.number_format {
            number_format.write_xml(writer)?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        if let Some(position) = self.position {
            write_scalar(
                writer,
                "c:dLblPos",
                position.as_str(),
                self.position_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(2))?;
        write_optional_bool(
            writer,
            "c:showLegendKey",
            self.show_legend_key,
            self.show_legend_key_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(3))?;
        write_optional_bool(
            writer,
            "c:showVal",
            self.show_value,
            self.show_value_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(4))?;
        write_optional_bool(
            writer,
            "c:showCatName",
            self.show_category_name,
            self.show_category_name_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(5))?;
        write_optional_bool(
            writer,
            "c:showSerName",
            self.show_series_name,
            self.show_series_name_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(6))?;
        write_optional_bool(
            writer,
            "c:showPercent",
            self.show_percent,
            self.show_percent_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(7))?;
        write_optional_bool(
            writer,
            "c:showBubbleSize",
            self.show_bubble_size,
            self.show_bubble_size_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(8))?;
        if let Some(separator) = &self.separator {
            write_text(
                writer,
                "c:separator",
                separator,
                self.separator_markup
                    .as_ref()
                    .unwrap_or(&TextMarkup::default()),
            )?;
        }
        emit_raw(writer, self.raw_children.at(9))?;
        writer
            .write_event(Event::End(BytesEnd::new("c:dLbls")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<()> {
        if let Some(number_format) = &self.number_format {
            number_format.validate()?;
        }
        if let Some(separator) = &self.separator {
            validate_xml_text(separator, "c:separator")?;
        }
        Ok(())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}

struct DataLabelsParseState {
    namespaces: NamespaceBindings,
    number_format: Option<NumberFormat>,
    position: Option<(DataLabelPosition, ScalarMarkup)>,
    show_legend_key: Option<(bool, ScalarMarkup)>,
    show_value: Option<(bool, ScalarMarkup)>,
    show_category_name: Option<(bool, ScalarMarkup)>,
    show_series_name: Option<(bool, ScalarMarkup)>,
    show_percent: Option<(bool, ScalarMarkup)>,
    show_bubble_size: Option<(bool, ScalarMarkup)>,
    separator: Option<(String, TextMarkup)>,
    raw_children: OrderedRawChildren,
    boundary: usize,
}

impl DataLabelsParseState {
    fn new(namespaces: NamespaceBindings) -> Self {
        Self {
            namespaces,
            number_format: None,
            position: None,
            show_legend_key: None,
            show_value: None,
            show_category_name: None,
            show_series_name: None,
            show_percent: None,
            show_bubble_size: None,
            separator: None,
            raw_children: OrderedRawChildren::default(),
            boundary: 0,
        }
    }

    fn capture_event(&mut self, raw: Vec<u8>) {
        self.raw_children.push(self.boundary, raw);
    }

    fn parse_child(&mut self, name: &[u8], raw: Vec<u8>) -> Result<()> {
        match name {
            b"numFmt" => {
                set_once(
                    &mut self.number_format,
                    NumberFormat::from_xml(&raw)?,
                    "c:numFmt",
                )?;
                self.boundary = self.boundary.max(1);
            }
            b"dLblPos" => {
                let (value, markup) = scalar_value(&raw, "dLblPos")?;
                let lexical = value
                    .ok_or_else(|| invalid_attribute("dLblPos", "val", "<missing>".to_owned()))?;
                let position = DataLabelPosition::parse(&lexical)
                    .ok_or_else(|| invalid_attribute("dLblPos", "val", lexical))?;
                set_once(&mut self.position, (position, markup), "c:dLblPos")?;
                self.boundary = self.boundary.max(2);
            }
            b"showLegendKey" => {
                set_once(
                    &mut self.show_legend_key,
                    parse_bool_value(&raw, "showLegendKey")?,
                    "c:showLegendKey",
                )?;
                self.boundary = self.boundary.max(3);
            }
            b"showVal" => {
                set_once(
                    &mut self.show_value,
                    parse_bool_value(&raw, "showVal")?,
                    "c:showVal",
                )?;
                self.boundary = self.boundary.max(4);
            }
            b"showCatName" => {
                set_once(
                    &mut self.show_category_name,
                    parse_bool_value(&raw, "showCatName")?,
                    "c:showCatName",
                )?;
                self.boundary = self.boundary.max(5);
            }
            b"showSerName" => {
                set_once(
                    &mut self.show_series_name,
                    parse_bool_value(&raw, "showSerName")?,
                    "c:showSerName",
                )?;
                self.boundary = self.boundary.max(6);
            }
            b"showPercent" => {
                set_once(
                    &mut self.show_percent,
                    parse_bool_value(&raw, "showPercent")?,
                    "c:showPercent",
                )?;
                self.boundary = self.boundary.max(7);
            }
            b"showBubbleSize" => {
                set_once(
                    &mut self.show_bubble_size,
                    parse_bool_value(&raw, "showBubbleSize")?,
                    "c:showBubbleSize",
                )?;
                self.boundary = self.boundary.max(8);
            }
            b"separator" => {
                set_once(
                    &mut self.separator,
                    parse_text_element(&raw, b"separator", &self.namespaces)?,
                    "c:separator",
                )?;
                self.boundary = self.boundary.max(9);
            }
            _ => {
                let boundary = data_labels_raw_boundary(name, self.boundary);
                self.raw_children.push(boundary, raw);
                self.boundary = self.boundary.max(boundary);
            }
        }
        Ok(())
    }

    fn finish(
        self,
        raw_attributes: XmlAttributes,
        namespace_declarations: XmlAttributes,
    ) -> Result<CT_DLbls> {
        let (position, position_markup) = optional_markup(self.position);
        let (show_legend_key, show_legend_key_markup) = optional_bool_markup(self.show_legend_key);
        let (show_value, show_value_markup) = optional_bool_markup(self.show_value);
        let (show_category_name, show_category_name_markup) =
            optional_bool_markup(self.show_category_name);
        let (show_series_name, show_series_name_markup) =
            optional_bool_markup(self.show_series_name);
        let (show_percent, show_percent_markup) = optional_bool_markup(self.show_percent);
        let (show_bubble_size, show_bubble_size_markup) =
            optional_bool_markup(self.show_bubble_size);
        let (separator, separator_markup) = self
            .separator
            .map(|(value, markup)| (Some(value), Some(markup)))
            .unwrap_or((None, None));
        let labels = CT_DLbls {
            number_format: self.number_format,
            position,
            separator,
            show_legend_key,
            show_value,
            show_category_name,
            show_series_name,
            show_percent,
            show_bubble_size,
            position_markup,
            show_legend_key_markup,
            show_value_markup,
            show_category_name_markup,
            show_series_name_markup,
            show_percent_markup,
            show_bubble_size_markup,
            separator_markup,
            raw_attributes,
            namespace_declarations,
            raw_children: raw_children_in_schema_order(&self.raw_children, 9),
        };
        labels.validate()?;
        Ok(labels)
    }
}

fn data_labels_raw_boundary(name: &[u8], current: usize) -> usize {
    match name {
        b"dLbl" | b"delete" => 0,
        b"spPr" | b"txPr" => 1,
        b"showLeaderLines" | b"leaderLines" | b"extLst" => 9,
        _ => current,
    }
}

fn optional_bool_markup(value: Option<(bool, ScalarMarkup)>) -> (bool, Option<ScalarMarkup>) {
    value
        .map(|(value, markup)| (value, Some(markup)))
        .unwrap_or((false, None))
}

fn write_optional_bool(
    writer: &mut Writer<Vec<u8>>,
    tag: &str,
    value: bool,
    markup: Option<&ScalarMarkup>,
) -> Result<()> {
    if value || markup.is_some() {
        write_scalar(writer, tag, bool_lexical(value), markup)?;
    }
    Ok(())
}
