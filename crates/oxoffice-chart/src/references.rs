//! Formula references and their caches: parsing and writing points, text and wrappers, and the checks on cached values.

use super::*;

pub(super) fn parse_reference(
    xml: &[u8],
    reference_local: &[u8],
    cache_local: &[u8],
    numeric: bool,
    inherited: &NamespaceBindings,
) -> Result<ParsedReference> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element)
                if matches_local_name(element.name().as_ref(), reference_local) =>
            {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let raw_attributes = capture_fixed_attributes(&element, &["xmlns:c"])?;
                let mut formula: Option<(String, TextMarkup)> = None;
                let mut cache: Option<(Option<String>, Vec<String>, CacheMarkup)> = None;
                let mut raw_children = OrderedRawChildren::default();
                let mut boundary = 0usize;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            match name.as_deref().unwrap_or_default() {
                                b"f" => {
                                    set_once(
                                        &mut formula,
                                        parse_text_element(&raw, b"f", &namespaces)?,
                                        "c:f",
                                    )?;
                                    boundary = boundary.max(1);
                                }
                                name if name == cache_local => {
                                    set_once(
                                        &mut cache,
                                        parse_cache(&raw, cache_local, numeric, &namespaces)?,
                                        &format!("c:{}", String::from_utf8_lossy(cache_local)),
                                    )?;
                                    boundary = boundary.max(2);
                                }
                                _ => raw_children.push(boundary, raw),
                            }
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            match name.as_deref().unwrap_or_default() {
                                b"f" => {
                                    set_once(
                                        &mut formula,
                                        parse_text_element(&raw, b"f", &namespaces)?,
                                        "c:f",
                                    )?;
                                    boundary = boundary.max(1);
                                }
                                name if name == cache_local => {
                                    set_once(
                                        &mut cache,
                                        parse_cache(&raw, cache_local, numeric, &namespaces)?,
                                        &format!("c:{}", String::from_utf8_lossy(cache_local)),
                                    )?;
                                    boundary = boundary.max(2);
                                }
                                _ => raw_children.push(boundary, raw),
                            }
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(boundary, capture_event(event)?);
                        }
                        Event::End(end)
                            if matches_local_name(end.name().as_ref(), reference_local) =>
                        {
                            let (formula, formula_markup) = formula
                                .ok_or_else(|| ChartError::MissingElement("c:f".to_owned()))?;
                            validate_formula(&formula, "c:f")?;
                            let (format_code, values, cache_markup) = cache.ok_or_else(|| {
                                ChartError::MissingElement(format!(
                                    "c:{}",
                                    String::from_utf8_lossy(cache_local)
                                ))
                            })?;
                            let markup = ReferenceMarkup {
                                raw_attributes,
                                raw_children: raw_children_in_schema_order(&raw_children, 2),
                                formula: formula_markup,
                                cache_attributes: cache_markup.raw_attributes,
                                cache_children: cache_markup.raw_children,
                                format_code: cache_markup.format_code,
                                format_code_omitted: cache_markup.format_code_omitted,
                                point_count: cache_markup.point_count,
                                declared_point_count: cache_markup.declared_point_count,
                                point_indexes: cache_markup.point_indexes,
                                points: cache_markup.points,
                            };
                            return Ok(ParsedReference {
                                formula,
                                format_code,
                                values,
                                markup,
                            });
                        }
                        Event::Eof => {
                            return Err(missing_end(&format!(
                                "c:{}",
                                String::from_utf8_lossy(reference_local)
                            )));
                        }
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element)
                if matches_local_name(element.name().as_ref(), reference_local) =>
            {
                return Err(ChartError::MissingElement("c:f".to_owned()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(reference_local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

struct CacheMarkup {
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
    format_code: Option<TextMarkup>,
    format_code_omitted: bool,
    point_count: ScalarMarkup,
    declared_point_count: Option<u32>,
    point_indexes: Vec<u32>,
    points: Vec<PointMarkup>,
}

fn parse_cache(
    xml: &[u8],
    cache_local: &[u8],
    numeric: bool,
    inherited: &NamespaceBindings,
) -> Result<(Option<String>, Vec<String>, CacheMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), cache_local) => {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let raw_attributes = capture_attributes(&element)?;
                let mut state = CacheParseState::new(numeric, namespaces);
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &state.namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            state.parse_child(name.as_deref().unwrap_or_default(), raw)?;
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &state.namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            state.parse_child(name.as_deref().unwrap_or_default(), raw)?;
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            state.capture_event(capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), cache_local) => {
                            return state.finish(raw_attributes);
                        }
                        Event::Eof => {
                            return Err(missing_end(&format!(
                                "c:{}",
                                String::from_utf8_lossy(cache_local)
                            )));
                        }
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), cache_local) => {
                return Err(ChartError::MissingElement("c:ptCount".to_owned()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(cache_local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

struct CacheParseState {
    numeric: bool,
    namespaces: NamespaceBindings,
    point_base: usize,
    boundary: usize,
    format_code: Option<(String, TextMarkup)>,
    point_count: Option<(u32, ScalarMarkup)>,
    points: Vec<(u32, String, PointMarkup)>,
    raw_children: OrderedRawChildren,
}

impl CacheParseState {
    fn new(numeric: bool, namespaces: NamespaceBindings) -> Self {
        Self {
            numeric,
            namespaces,
            point_base: usize::from(numeric) + 1,
            boundary: 0,
            format_code: None,
            point_count: None,
            points: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }

    fn capture_event(&mut self, raw: Vec<u8>) {
        self.raw_children.push(self.boundary, raw);
    }

    fn parse_child(&mut self, name: &[u8], raw: Vec<u8>) -> Result<()> {
        match name {
            b"formatCode" if self.numeric => {
                set_once(
                    &mut self.format_code,
                    parse_text_element(&raw, b"formatCode", &self.namespaces)?,
                    "c:formatCode",
                )?;
                self.boundary = self.boundary.max(1);
            }
            b"ptCount" => {
                set_once(
                    &mut self.point_count,
                    parse_u32_scalar(&raw, "ptCount")?,
                    "c:ptCount",
                )?;
                self.boundary = self.boundary.max(self.point_base);
            }
            b"pt" => {
                self.points.push(parse_point(&raw, &self.namespaces)?);
                self.boundary = self.boundary.max(self.point_base + self.points.len());
            }
            _ => self.raw_children.push(self.boundary, raw),
        }
        Ok(())
    }

    fn finish(
        self,
        raw_attributes: XmlAttributes,
    ) -> Result<(Option<String>, Vec<String>, CacheMarkup)> {
        let (declared, point_count_markup) = self
            .point_count
            .ok_or_else(|| ChartError::MissingElement("c:ptCount".to_owned()))?;
        let actual = u32::try_from(self.points.len()).map_err(|_| ChartError::InvalidValue {
            element: "c:ptCount".to_owned(),
            value: self.points.len().to_string(),
        })?;
        if declared < actual {
            return Err(invalid_attribute("ptCount", "val", declared.to_string()));
        }
        let mut values = Vec::with_capacity(self.points.len());
        let mut point_indexes = Vec::with_capacity(self.points.len());
        let mut point_markup = Vec::with_capacity(self.points.len());
        let mut previous = None;
        for (index, value, markup) in self.points {
            if index >= declared || previous.is_some_and(|last| index <= last) {
                return Err(invalid_attribute("pt", "idx", index.to_string()));
            }
            previous = Some(index);
            values.push(value);
            point_indexes.push(index);
            point_markup.push(markup);
        }
        let (format_code, format_markup) = self
            .format_code
            .map(|(value, markup)| (Some(value), Some(markup)))
            .unwrap_or((None, None));
        let format_code_omitted = self.numeric && format_code.is_none();
        Ok((
            format_code,
            values,
            CacheMarkup {
                raw_attributes,
                raw_children: raw_children_in_schema_order(
                    &self.raw_children,
                    self.point_base + point_markup.len(),
                ),
                format_code: format_markup,
                format_code_omitted,
                point_count: point_count_markup,
                declared_point_count: Some(declared),
                point_indexes,
                points: point_markup,
            },
        ))
    }
}

fn parse_point(xml: &[u8], inherited: &NamespaceBindings) -> Result<(u32, String, PointMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), b"pt") => {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let index = required_u32_attribute(&element, "pt", b"idx")?;
                let raw_attributes = capture_attributes_except(&element, b"idx")?;
                let mut value: Option<(String, TextMarkup)> = None;
                let mut raw_children = OrderedRawChildren::default();
                let mut boundary = 0usize;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            if name.as_deref() == Some(b"v") {
                                set_once(
                                    &mut value,
                                    parse_text_element(&raw, b"v", &namespaces)?,
                                    "c:v",
                                )?;
                                boundary = 1;
                            } else {
                                raw_children.push(boundary, raw);
                            }
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            if name.as_deref() == Some(b"v") {
                                set_once(
                                    &mut value,
                                    parse_text_element(&raw, b"v", &namespaces)?,
                                    "c:v",
                                )?;
                                boundary = 1;
                            } else {
                                raw_children.push(boundary, raw);
                            }
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(boundary, capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), b"pt") => {
                            let (value, value_markup) = value
                                .ok_or_else(|| ChartError::MissingElement("c:v".to_owned()))?;
                            return Ok((
                                index,
                                value,
                                PointMarkup {
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 1),
                                    value: value_markup,
                                },
                            ));
                        }
                        Event::Eof => return Err(missing_end("c:pt")),
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), b"pt") => {
                return Err(ChartError::MissingElement("c:v".to_owned()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => return Err(ChartError::MissingElement("c:pt".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn parse_text_element(
    xml: &[u8],
    local: &[u8],
    inherited: &NamespaceBindings,
) -> Result<(String, TextMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), local) => {
                typed_rewrite_bindings(&element, inherited)?;
                let raw_attributes = capture_attributes(&element)?;
                let mut value = String::new();
                let mut raw_children = OrderedRawChildren::default();
                let mut seen_text = false;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Text(text) => {
                            let decoded =
                                text.xml10_content()
                                    .map_err(|error| ChartError::InvalidValue {
                                        element: format!("c:{}", String::from_utf8_lossy(local)),
                                        value: error.to_string(),
                                    })?;
                            value.push_str(&decoded);
                            seen_text = true;
                        }
                        Event::CData(text) => {
                            let decoded =
                                text.xml10_content()
                                    .map_err(|error| ChartError::InvalidValue {
                                        element: format!("c:{}", String::from_utf8_lossy(local)),
                                        value: error.to_string(),
                                    })?;
                            value.push_str(&decoded);
                            seen_text = true;
                        }
                        Event::GeneralRef(reference) => {
                            value.push_str(&resolve_entity(&reference));
                            seen_text = true;
                        }
                        event @ (Event::Comment(_) | Event::PI(_)) => {
                            raw_children.push(usize::from(seen_text), capture_event(event)?);
                        }
                        Event::Start(child) | Event::Empty(child) => {
                            return Err(ChartError::UnexpectedElement(element_name(&child)));
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), local) => {
                            return Ok((
                                value,
                                TextMarkup {
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 1),
                                },
                            ));
                        }
                        Event::Eof => {
                            return Err(missing_end(&format!(
                                "c:{}",
                                String::from_utf8_lossy(local)
                            )));
                        }
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), local) => {
                typed_rewrite_bindings(&element, inherited)?;
                return Ok((
                    String::new(),
                    TextMarkup {
                        raw_attributes: capture_attributes(&element)?,
                        raw_children: OrderedRawChildren::default(),
                    },
                ));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn parse_wrapper(
    xml: &[u8],
    local: &[u8],
    choices: &[&[u8]],
    inherited: &NamespaceBindings,
) -> Result<ParsedWrapper> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), local) => {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let raw_attributes = capture_attributes(&element)?;
                let mut choice = None;
                let mut raw_children = OrderedRawChildren::default();
                let mut boundary = 0usize;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            if let Some(name) =
                                name.filter(|name| choices.contains(&name.as_slice()))
                            {
                                if choice.is_some() {
                                    return Err(ChartError::DuplicateElement(format!(
                                        "c:{} reference",
                                        String::from_utf8_lossy(local)
                                    )));
                                }
                                choice = Some((name, raw));
                                boundary = 1;
                            } else {
                                raw_children.push(boundary, raw);
                            }
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            if let Some(name) =
                                name.filter(|name| choices.contains(&name.as_slice()))
                            {
                                if choice.is_some() {
                                    return Err(ChartError::DuplicateElement(format!(
                                        "c:{} reference",
                                        String::from_utf8_lossy(local)
                                    )));
                                }
                                choice = Some((name, raw));
                                boundary = 1;
                            } else {
                                raw_children.push(boundary, raw);
                            }
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(boundary, capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), local) => {
                            return Ok(ParsedWrapper {
                                choice,
                                markup: WrapperMarkup {
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 1),
                                },
                                namespaces,
                            });
                        }
                        Event::Eof => {
                            return Err(missing_end(&format!(
                                "c:{}",
                                String::from_utf8_lossy(local)
                            )));
                        }
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), local) => {
                let namespaces = chart_bindings(inherited, &element)?;
                return Ok(ParsedWrapper {
                    choice: None,
                    markup: WrapperMarkup {
                        raw_attributes: capture_attributes(&element)?,
                        raw_children: OrderedRawChildren::default(),
                    },
                    namespaces,
                });
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn write_string_cache(
    writer: &mut Writer<Vec<u8>>,
    values: &[String],
    markup: &ReferenceMarkup,
) -> Result<()> {
    let (declared_count, indexes) = cache_layout(markup, values.len())?;
    let mut start = BytesStart::new("c:strCache");
    push_attributes(&mut start, &markup.cache_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.cache_children.at(0))?;
    write_scalar(
        writer,
        "c:ptCount",
        &declared_count.to_string(),
        Some(&markup.point_count),
    )?;
    emit_cache_point_raw(writer, markup, 1, values.len(), 0)?;
    for (position, value) in values.iter().enumerate() {
        let default_markup = PointMarkup::default();
        write_point(
            writer,
            indexes[position],
            value,
            markup.points.get(position).unwrap_or(&default_markup),
        )?;
        emit_cache_point_raw(writer, markup, 1, values.len(), position + 1)?;
    }
    writer
        .write_event(Event::End(BytesEnd::new("c:strCache")))
        .map_err(OxmlError::from)?;
    Ok(())
}

pub(super) fn write_numeric_cache(
    writer: &mut Writer<Vec<u8>>,
    data: &NumericData,
    markup: &ReferenceMarkup,
) -> Result<()> {
    let (declared_count, indexes) = cache_layout(markup, data.values.len())?;
    let mut start = BytesStart::new("c:numCache");
    push_attributes(&mut start, &markup.cache_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.cache_children.at(0))?;
    if !markup.format_code_omitted || data.format_code != "General" {
        let default_format_markup = TextMarkup::default();
        write_text(
            writer,
            "c:formatCode",
            &data.format_code,
            markup
                .format_code
                .as_ref()
                .unwrap_or(&default_format_markup),
        )?;
    }
    emit_raw(writer, markup.cache_children.at(1))?;
    write_scalar(
        writer,
        "c:ptCount",
        &declared_count.to_string(),
        Some(&markup.point_count),
    )?;
    emit_cache_point_raw(writer, markup, 2, data.values.len(), 0)?;
    for (position, value) in data.values.iter().enumerate() {
        let default_markup = PointMarkup::default();
        write_point(
            writer,
            indexes[position],
            &value.to_string(),
            markup.points.get(position).unwrap_or(&default_markup),
        )?;
        emit_cache_point_raw(writer, markup, 2, data.values.len(), position + 1)?;
    }
    writer
        .write_event(Event::End(BytesEnd::new("c:numCache")))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn emit_cache_point_raw(
    writer: &mut Writer<Vec<u8>>,
    markup: &ReferenceMarkup,
    point_base: usize,
    current_count: usize,
    completed_points: usize,
) -> Result<()> {
    let original_count = markup.points.len();
    if current_count == 0 {
        for boundary in point_base..=point_base + original_count {
            emit_raw(writer, markup.cache_children.at(boundary))?;
        }
    } else if completed_points == 0 {
        if original_count > 0 {
            emit_raw(writer, markup.cache_children.at(point_base))?;
        }
    } else if completed_points < current_count {
        if completed_points < original_count {
            emit_raw(
                writer,
                markup.cache_children.at(point_base + completed_points),
            )?;
        }
    } else {
        let first_tail = if original_count == 0 {
            point_base
        } else {
            point_base + current_count.min(original_count)
        };
        for boundary in first_tail..=point_base + original_count {
            emit_raw(writer, markup.cache_children.at(boundary))?;
        }
    }
    Ok(())
}

fn write_point(
    writer: &mut Writer<Vec<u8>>,
    index: u32,
    value: &str,
    markup: &PointMarkup,
) -> Result<()> {
    let index = index.to_string();
    let mut start = BytesStart::new("c:pt");
    start.push_attribute(("idx", index.as_str()));
    push_attributes(&mut start, &markup.raw_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.raw_children.at(0))?;
    write_text(writer, "c:v", value, &markup.value)?;
    emit_raw(writer, markup.raw_children.at(1))?;
    writer
        .write_event(Event::End(BytesEnd::new("c:pt")))
        .map_err(OxmlError::from)?;
    Ok(())
}

pub(super) fn cache_layout(
    markup: &ReferenceMarkup,
    value_count: usize,
) -> Result<(u32, Vec<u32>)> {
    if markup.point_indexes.len() == value_count
        && let Some(declared) = markup.declared_point_count
    {
        return Ok((declared, markup.point_indexes.clone()));
    }
    let declared = u32::try_from(value_count).map_err(|_| ChartError::InvalidValue {
        element: "c:ptCount".to_owned(),
        value: value_count.to_string(),
    })?;
    Ok((declared, (0..declared).collect()))
}

pub(super) fn write_text(
    writer: &mut Writer<Vec<u8>>,
    tag: &str,
    value: &str,
    markup: &TextMarkup,
) -> Result<()> {
    let mut start = BytesStart::new(tag);
    push_attributes(&mut start, &markup.raw_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.raw_children.at(0))?;
    writer
        .write_event(Event::Text(BytesText::from_escaped(escape_text(value))))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.raw_children.at(1))?;
    writer
        .write_event(Event::End(BytesEnd::new(tag)))
        .map_err(OxmlError::from)?;
    Ok(())
}

pub(super) fn write_wrapper_start(
    writer: &mut Writer<Vec<u8>>,
    tag: &str,
    markup: &WrapperMarkup,
) -> Result<()> {
    let mut start = BytesStart::new(tag);
    push_attributes(&mut start, &markup.raw_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    Ok(emit_raw(writer, markup.raw_children.at(0))?)
}

pub(super) fn write_wrapper_end(
    writer: &mut Writer<Vec<u8>>,
    tag: &str,
    markup: &WrapperMarkup,
) -> Result<()> {
    emit_raw(writer, markup.raw_children.at(1))?;
    writer
        .write_event(Event::End(BytesEnd::new(tag)))
        .map_err(OxmlError::from)?;
    Ok(())
}

pub(super) fn validate_formula(formula: &str, element: &str) -> Result<()> {
    if formula.trim().is_empty() {
        return Err(ChartError::InvalidValue {
            element: element.to_owned(),
            value: formula.to_owned(),
        });
    }
    validate_xml_text(formula, element)
}

pub(super) fn validate_format_code(format_code: &str) -> Result<()> {
    validate_xml_text(format_code, "c:formatCode")
}

pub(super) fn validate_xml_text(value: &str, element: &str) -> Result<()> {
    if value.chars().all(|character| {
        matches!(
            character,
            '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
        )
    }) {
        return Ok(());
    }
    Err(ChartError::InvalidValue {
        element: element.to_owned(),
        value: value.to_owned(),
    })
}

pub(super) fn validate_numeric_values(values: &[f64]) -> Result<()> {
    if let Some(value) = values.iter().find(|value| !value.is_finite()) {
        return Err(ChartError::InvalidValue {
            element: "c:v".to_owned(),
            value: value.to_string(),
        });
    }
    Ok(())
}

pub(super) fn validate_point_count(count: usize, element: &str) -> Result<()> {
    u32::try_from(count)
        .map(|_| ())
        .map_err(|_| ChartError::InvalidValue {
            element: element.to_owned(),
            value: count.to_string(),
        })
}
