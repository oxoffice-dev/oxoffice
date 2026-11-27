//! Axes: kinds, positions, tick marks, scaling and grid lines, with their parsing and writing.

use super::*;

/// A producer-compatible ChartML axis identifier.
///
/// ECMA-376 names an unsigned value, but PowerPoint emits signed 32-bit
/// lexical values in real chart parts. The accepted domain covers both forms.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AxisId(i64);

impl AxisId {
    pub fn new(value: i64) -> Result<Self> {
        if (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ChartError::InvalidValue {
                element: "c:axId".to_owned(),
                value: value.to_string(),
            })
        }
    }

    pub const fn value(self) -> i64 {
        self.0
    }
}

/// The concrete ChartML root used by an axis.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AxisKind {
    Category,
    Value,
    Date,
    Series,
}

impl AxisKind {
    pub(super) fn parse(local: &[u8]) -> Option<Self> {
        match local {
            b"catAx" => Some(Self::Category),
            b"valAx" => Some(Self::Value),
            b"dateAx" => Some(Self::Date),
            b"serAx" => Some(Self::Series),
            _ => None,
        }
    }

    const fn local(self) -> &'static str {
        match self {
            Self::Category => "catAx",
            Self::Value => "valAx",
            Self::Date => "dateAx",
            Self::Series => "serAx",
        }
    }
}

/// Direction of values along an axis scale.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Orientation {
    #[default]
    MinMax,
    MaxMin,
}

impl Orientation {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "minMax" => Some(Self::MinMax),
            "maxMin" => Some(Self::MaxMin),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::MinMax => "minMax",
            Self::MaxMin => "maxMin",
        }
    }
}

/// Which side of the plot area carries an axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AxisPosition {
    Bottom,
    Left,
    Right,
    Top,
}

impl AxisPosition {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "b" => Some(Self::Bottom),
            "l" => Some(Self::Left),
            "r" => Some(Self::Right),
            "t" => Some(Self::Top),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Bottom => "b",
            Self::Left => "l",
            Self::Right => "r",
            Self::Top => "t",
        }
    }
}

/// Placement of a major or minor tick relative to its axis line.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TickMark {
    Cross,
    Inside,
    #[default]
    None,
    Outside,
}

impl TickMark {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "cross" => Some(Self::Cross),
            "in" => Some(Self::Inside),
            "none" => Some(Self::None),
            "out" => Some(Self::Outside),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Cross => "cross",
            Self::Inside => "in",
            Self::None => "none",
            Self::Outside => "out",
        }
    }
}

/// Placement of tick labels relative to an axis.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TickLabelPosition {
    High,
    Low,
    #[default]
    NextTo,
    None,
}

impl TickLabelPosition {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "high" => Some(Self::High),
            "low" => Some(Self::Low),
            "nextTo" => Some(Self::NextTo),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
            Self::NextTo => "nextTo",
            Self::None => "none",
        }
    }
}

/// The modelled values of `c:scaling`.
#[derive(Clone, Debug)]
pub struct Scaling {
    pub log_base: Option<f64>,
    pub orientation: Orientation,
    pub maximum: Option<f64>,
    pub minimum: Option<f64>,
    log_base_markup: Option<ScalarMarkup>,
    orientation_markup: Option<ScalarMarkup>,
    maximum_markup: Option<ScalarMarkup>,
    minimum_markup: Option<ScalarMarkup>,
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
}

impl PartialEq for Scaling {
    fn eq(&self, other: &Self) -> bool {
        self.log_base == other.log_base
            && self.orientation == other.orientation
            && self.maximum == other.maximum
            && self.minimum == other.minimum
            && optional_scalar_markup_eq(&self.log_base_markup, &other.log_base_markup)
            && optional_scalar_markup_eq(&self.orientation_markup, &other.orientation_markup)
            && optional_scalar_markup_eq(&self.maximum_markup, &other.maximum_markup)
            && optional_scalar_markup_eq(&self.minimum_markup, &other.minimum_markup)
            && self.raw_attributes == other.raw_attributes
            && self.raw_children == other.raw_children
    }
}

impl Default for Scaling {
    fn default() -> Self {
        Self {
            log_base: None,
            orientation: Orientation::MinMax,
            maximum: None,
            minimum: None,
            log_base_markup: None,
            orientation_markup: None,
            maximum_markup: None,
            minimum_markup: None,
            raw_attributes: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }
}

/// Shape properties on a major or minor gridline container.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartLines {
    pub sp_pr: Option<CT_ShapeProperties>,
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct AxisIdMarkup {
    scalar: ScalarMarkup,
    parsed: AxisId,
    lexical: String,
}

/// The common modelled sequence of one ChartML axis.
#[derive(Clone, Debug)]
pub struct Axis {
    pub kind: AxisKind,
    pub id: AxisId,
    pub scaling: Scaling,
    pub deleted: bool,
    pub position: AxisPosition,
    pub major_gridlines: Option<ChartLines>,
    pub minor_gridlines: Option<ChartLines>,
    pub title: Option<CT_Title>,
    pub number_format: Option<NumberFormat>,
    pub major_tick_mark: TickMark,
    pub minor_tick_mark: TickMark,
    pub tick_label_position: TickLabelPosition,
    pub sp_pr: Option<CT_ShapeProperties>,
    pub tx_pr: Option<CT_TextBody>,
    pub cross_axis: AxisId,
    parsed_kind: Option<AxisKind>,
    id_markup: AxisIdMarkup,
    delete_markup: Option<ScalarMarkup>,
    position_markup: ScalarMarkup,
    major_tick_mark_markup: Option<ScalarMarkup>,
    minor_tick_mark_markup: Option<ScalarMarkup>,
    tick_label_position_markup: Option<ScalarMarkup>,
    cross_axis_markup: AxisIdMarkup,
    raw_attributes: XmlAttributes,
    namespace_declarations: XmlAttributes,
    raw_children: OrderedRawChildren,
}

impl PartialEq for Axis {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.id == other.id
            && axis_id_markup_eq(&self.id_markup, &other.id_markup)
            && self.scaling == other.scaling
            && self.deleted == other.deleted
            && optional_scalar_markup_eq(&self.delete_markup, &other.delete_markup)
            && self.position == other.position
            && self.position_markup == other.position_markup
            && self.major_gridlines == other.major_gridlines
            && self.minor_gridlines == other.minor_gridlines
            && self.title == other.title
            && self.number_format == other.number_format
            && self.major_tick_mark == other.major_tick_mark
            && optional_scalar_markup_eq(
                &self.major_tick_mark_markup,
                &other.major_tick_mark_markup,
            )
            && self.minor_tick_mark == other.minor_tick_mark
            && optional_scalar_markup_eq(
                &self.minor_tick_mark_markup,
                &other.minor_tick_mark_markup,
            )
            && self.tick_label_position == other.tick_label_position
            && optional_scalar_markup_eq(
                &self.tick_label_position_markup,
                &other.tick_label_position_markup,
            )
            && self.sp_pr == other.sp_pr
            && self.tx_pr == other.tx_pr
            && self.cross_axis == other.cross_axis
            && axis_id_markup_eq(&self.cross_axis_markup, &other.cross_axis_markup)
            && self.raw_attributes == other.raw_attributes
            && self.namespace_declarations == other.namespace_declarations
            && self.raw_children == other.raw_children
    }
}

impl Axis {
    pub fn new(kind: AxisKind, id: AxisId, position: AxisPosition, cross_axis: AxisId) -> Self {
        Self {
            kind,
            id,
            scaling: Scaling::default(),
            deleted: false,
            position,
            major_gridlines: None,
            minor_gridlines: None,
            title: None,
            number_format: None,
            major_tick_mark: TickMark::None,
            minor_tick_mark: TickMark::None,
            tick_label_position: TickLabelPosition::NextTo,
            sp_pr: None,
            tx_pr: None,
            cross_axis,
            parsed_kind: None,
            id_markup: default_axis_id_markup(id),
            delete_markup: None,
            position_markup: ScalarMarkup::default(),
            major_tick_mark_markup: None,
            minor_tick_mark_markup: None,
            tick_label_position_markup: None,
            cross_axis_markup: default_axis_id_markup(cross_axis),
            raw_attributes: Vec::new(),
            namespace_declarations: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }

    /// Sets axis visibility and writes the explicit ChartML deletion flag.
    pub fn set_deleted(&mut self, deleted: bool) {
        self.deleted = deleted;
        self.delete_markup.get_or_insert_with(ScalarMarkup::default);
    }

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
                Event::Start(element) => {
                    let name = element.name();
                    let local = local_name(name.as_ref());
                    let Some(kind) = AxisKind::parse(local) else {
                        return Err(ChartError::UnexpectedElement(element_name(&element)));
                    };
                    chart_root_prefix(&element)?;
                    if !element_is_in_namespace(&element, C_NS, inherited)? {
                        return Err(ChartError::UnexpectedElement(element_name(&element)));
                    }
                    return Self::from_element(&mut reader, &element, inherited, kind);
                }
                Event::Empty(element) => {
                    let name = element.name();
                    let local = local_name(name.as_ref());
                    if AxisKind::parse(local).is_some() {
                        return Err(ChartError::MissingElement("c:axId".to_owned()));
                    }
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("ChartML axis".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &NamespaceBindings,
        kind: AxisKind,
    ) -> Result<Self> {
        reject_conflicting_prefix(start, b"a", A_NS)?;
        reject_conflicting_prefix(start, b"r", R_NS)?;
        let namespaces = chart_bindings(inherited, start)?;
        require_fixed_namespace(&namespaces, b"c", C_NS, start)?;
        require_fixed_namespace(&namespaces, b"a", A_NS, start)?;
        require_fixed_namespace(&namespaces, b"r", R_NS, start)?;
        let (raw_attributes, _) =
            capture_fixed_root_attributes(start, &["xmlns:c", "xmlns:a", "xmlns:r"])?;
        let namespace_declarations = standalone_namespace_declarations(&namespaces)?;
        let mut state = AxisParseState::default();
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = chart_child_local(&element, &namespaces)?;
                    let raw = capture_element(reader, &element)?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw, &namespaces)?;
                }
                Event::Empty(element) => {
                    let name = chart_child_local(&element, &namespaces)?;
                    let raw = capture_empty(&element, reader.empty_writing())?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw, &namespaces)?;
                }
                event @ (Event::Text(_)
                | Event::CData(_)
                | Event::Comment(_)
                | Event::PI(_)
                | Event::GeneralRef(_)) => state.capture_event(capture_event(event)?),
                Event::End(element)
                    if matches_local_name(element.name().as_ref(), kind.local().as_bytes()) =>
                {
                    break;
                }
                Event::Eof => return Err(missing_end(&format!("c:{}", kind.local()))),
                _ => {}
            }
            buffer.clear();
        }
        state.finish(kind, raw_attributes, namespace_declarations)
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut writer = Writer::new(Vec::new());
        let root = format!("c:{}", self.kind.local());
        let mut start = BytesStart::new(&root);
        start.push_attribute(("xmlns:c", C_NS));
        start.push_attribute(("xmlns:a", A_NS));
        start.push_attribute(("xmlns:r", R_NS));
        push_attributes(&mut start, &self.namespace_declarations);
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(&mut writer, self.raw_children.at(0))?;
        write_axis_id(&mut writer, "c:axId", self.id, &self.id_markup)?;
        emit_raw(&mut writer, self.raw_children.at(1))?;
        self.scaling.write_xml(&mut writer)?;
        emit_raw(&mut writer, self.raw_children.at(2))?;
        if self.deleted || self.delete_markup.is_some() {
            write_scalar(
                &mut writer,
                "c:delete",
                bool_lexical(self.deleted),
                self.delete_markup.as_ref(),
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(3))?;
        write_scalar(
            &mut writer,
            "c:axPos",
            self.position.as_str(),
            Some(&self.position_markup),
        )?;
        emit_raw(&mut writer, self.raw_children.at(4))?;
        if let Some(lines) = &self.major_gridlines {
            lines.write_xml(&mut writer, "c:majorGridlines")?;
        }
        emit_raw(&mut writer, self.raw_children.at(5))?;
        if let Some(lines) = &self.minor_gridlines {
            lines.write_xml(&mut writer, "c:minorGridlines")?;
        }
        emit_raw(&mut writer, self.raw_children.at(6))?;
        if let Some(title) = &self.title {
            title.write_xml(&mut writer)?;
        }
        emit_raw(&mut writer, self.raw_children.at(7))?;
        if let Some(number_format) = &self.number_format {
            number_format.write_xml(&mut writer)?;
        }
        emit_raw(&mut writer, self.raw_children.at(8))?;
        if self.major_tick_mark != TickMark::None || self.major_tick_mark_markup.is_some() {
            write_scalar(
                &mut writer,
                "c:majorTickMark",
                self.major_tick_mark.as_str(),
                self.major_tick_mark_markup.as_ref(),
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(9))?;
        if self.minor_tick_mark != TickMark::None || self.minor_tick_mark_markup.is_some() {
            write_scalar(
                &mut writer,
                "c:minorTickMark",
                self.minor_tick_mark.as_str(),
                self.minor_tick_mark_markup.as_ref(),
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(10))?;
        if self.tick_label_position != TickLabelPosition::NextTo
            || self.tick_label_position_markup.is_some()
        {
            write_scalar(
                &mut writer,
                "c:tickLblPos",
                self.tick_label_position.as_str(),
                self.tick_label_position_markup.as_ref(),
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(11))?;
        if let Some(properties) = &self.sp_pr {
            properties.write_xml_as(&mut writer, "c:spPr")?;
        }
        emit_raw(&mut writer, self.raw_children.at(12))?;
        if let Some(text) = &self.tx_pr {
            text.write_xml_as(&mut writer, "c:txPr")?;
        }
        emit_raw(&mut writer, self.raw_children.at(13))?;
        write_axis_id(
            &mut writer,
            "c:crossAx",
            self.cross_axis,
            &self.cross_axis_markup,
        )?;
        emit_raw(&mut writer, self.raw_children.at(14))?;
        writer
            .write_event(Event::End(BytesEnd::new(&root)))
            .map_err(OxmlError::from)?;
        Ok(writer.into_inner())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }

    fn validate(&self) -> Result<()> {
        if let Some(parsed_kind) = self.parsed_kind
            && self.kind != parsed_kind
        {
            return Err(ChartError::InvalidValue {
                element: "ChartML axis root".to_owned(),
                value: format!(
                    "parsed c:{} cannot be relabelled c:{}",
                    parsed_kind.local(),
                    self.kind.local()
                ),
            });
        }
        AxisId::new(self.id.value())?;
        AxisId::new(self.cross_axis.value())?;
        self.scaling.validate()?;
        if let Some(number_format) = &self.number_format {
            number_format.validate()?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct AxisParseState {
    id: Option<(AxisId, AxisIdMarkup)>,
    scaling: Option<Scaling>,
    deleted: Option<(bool, ScalarMarkup)>,
    position: Option<(AxisPosition, ScalarMarkup)>,
    major_gridlines: Option<ChartLines>,
    minor_gridlines: Option<ChartLines>,
    title: Option<CT_Title>,
    number_format: Option<NumberFormat>,
    major_tick_mark: Option<(TickMark, ScalarMarkup)>,
    minor_tick_mark: Option<(TickMark, ScalarMarkup)>,
    tick_label_position: Option<(TickLabelPosition, ScalarMarkup)>,
    sp_pr: Option<CT_ShapeProperties>,
    tx_pr: Option<CT_TextBody>,
    cross_axis: Option<(AxisId, AxisIdMarkup)>,
    raw_children: OrderedRawChildren,
    boundary: usize,
}

impl AxisParseState {
    fn capture_event(&mut self, raw: Vec<u8>) {
        self.raw_children.push(self.boundary, raw);
    }

    fn parse_child(
        &mut self,
        name: &[u8],
        raw: Vec<u8>,
        namespaces: &NamespaceBindings,
    ) -> Result<()> {
        match name {
            b"axId" => {
                set_once(&mut self.id, parse_axis_id_scalar(&raw, "axId")?, "c:axId")?;
                self.boundary = self.boundary.max(1);
            }
            b"scaling" => {
                set_once(
                    &mut self.scaling,
                    Scaling::from_xml(&raw, namespaces)?,
                    "c:scaling",
                )?;
                self.boundary = self.boundary.max(2);
            }
            b"delete" => {
                set_once(
                    &mut self.deleted,
                    parse_bool_value(&raw, "delete")?,
                    "c:delete",
                )?;
                self.boundary = self.boundary.max(3);
            }
            b"axPos" => {
                set_once(&mut self.position, parse_axis_position(&raw)?, "c:axPos")?;
                self.boundary = self.boundary.max(4);
            }
            b"majorGridlines" => {
                set_once(
                    &mut self.major_gridlines,
                    ChartLines::from_xml(&raw, b"majorGridlines", namespaces)?,
                    "c:majorGridlines",
                )?;
                self.boundary = self.boundary.max(5);
            }
            b"minorGridlines" => {
                set_once(
                    &mut self.minor_gridlines,
                    ChartLines::from_xml(&raw, b"minorGridlines", namespaces)?,
                    "c:minorGridlines",
                )?;
                self.boundary = self.boundary.max(6);
            }
            b"title" => {
                set_once(&mut self.title, CT_Title::from_xml(&raw)?, "c:title")?;
                self.boundary = self.boundary.max(7);
            }
            b"numFmt" => {
                set_once(
                    &mut self.number_format,
                    NumberFormat::from_xml(&raw)?,
                    "c:numFmt",
                )?;
                self.boundary = self.boundary.max(8);
            }
            b"majorTickMark" => {
                set_once(
                    &mut self.major_tick_mark,
                    parse_tick_mark(&raw, "majorTickMark")?,
                    "c:majorTickMark",
                )?;
                self.boundary = self.boundary.max(9);
            }
            b"minorTickMark" => {
                set_once(
                    &mut self.minor_tick_mark,
                    parse_tick_mark(&raw, "minorTickMark")?,
                    "c:minorTickMark",
                )?;
                self.boundary = self.boundary.max(10);
            }
            b"tickLblPos" => {
                set_once(
                    &mut self.tick_label_position,
                    parse_tick_label_position(&raw)?,
                    "c:tickLblPos",
                )?;
                self.boundary = self.boundary.max(11);
            }
            b"spPr" => {
                parse_chart_shape_properties(&raw, namespaces, &mut self.sp_pr)?;
                self.boundary = self.boundary.max(12);
            }
            b"txPr" => {
                reject_conflicting_prefix_in_xml(&raw, b"a", A_NS)?;
                let inherited = namespaces
                    .iter()
                    .map(|(prefix, uri)| {
                        Ok((
                            std::str::from_utf8(prefix)
                                .map_err(OxmlError::from)?
                                .to_owned(),
                            uri.clone(),
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let text = CT_TextBody::from_xml_as_with_namespaces(&raw, b"txPr", &inherited)?;
                let mut writer = Writer::new(Vec::new());
                text.write_xml_as(&mut writer, "c:txPr")?;
                reject_rewritten_foreign_elements(&raw, &writer.into_inner(), namespaces, b"txPr")?;
                set_once(&mut self.tx_pr, text, "c:txPr")?;
                self.boundary = self.boundary.max(13);
            }
            b"crossAx" => {
                set_once(
                    &mut self.cross_axis,
                    parse_axis_id_scalar(&raw, "crossAx")?,
                    "c:crossAx",
                )?;
                self.boundary = self.boundary.max(14);
            }
            _ => self.raw_children.push(self.boundary, raw),
        }
        Ok(())
    }

    fn finish(
        self,
        kind: AxisKind,
        raw_attributes: XmlAttributes,
        namespace_declarations: XmlAttributes,
    ) -> Result<Axis> {
        let (id, id_markup) = self
            .id
            .ok_or_else(|| ChartError::MissingElement("c:axId".to_owned()))?;
        let scaling = self
            .scaling
            .ok_or_else(|| ChartError::MissingElement("c:scaling".to_owned()))?;
        let (position, position_markup) = self
            .position
            .ok_or_else(|| ChartError::MissingElement("c:axPos".to_owned()))?;
        let (cross_axis, cross_axis_markup) = self
            .cross_axis
            .ok_or_else(|| ChartError::MissingElement("c:crossAx".to_owned()))?;
        let (deleted, delete_markup) = self
            .deleted
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((false, None));
        let (major_tick_mark, major_tick_mark_markup) = self
            .major_tick_mark
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((TickMark::None, None));
        let (minor_tick_mark, minor_tick_mark_markup) = self
            .minor_tick_mark
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((TickMark::None, None));
        let (tick_label_position, tick_label_position_markup) = self
            .tick_label_position
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((TickLabelPosition::NextTo, None));
        let axis = Axis {
            kind,
            id,
            scaling,
            deleted,
            position,
            major_gridlines: self.major_gridlines,
            minor_gridlines: self.minor_gridlines,
            title: self.title,
            number_format: self.number_format,
            major_tick_mark,
            minor_tick_mark,
            tick_label_position,
            sp_pr: self.sp_pr,
            tx_pr: self.tx_pr,
            cross_axis,
            parsed_kind: Some(kind),
            id_markup,
            delete_markup,
            position_markup,
            major_tick_mark_markup,
            minor_tick_mark_markup,
            tick_label_position_markup,
            cross_axis_markup,
            raw_attributes,
            namespace_declarations,
            raw_children: raw_children_in_schema_order(&self.raw_children, 14),
        };
        axis.validate()?;
        Ok(axis)
    }
}

impl Scaling {
    fn from_xml(xml: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element)
                    if matches_local_name(element.name().as_ref(), b"scaling") =>
                {
                    let namespaces = typed_rewrite_bindings(&element, inherited)?;
                    let raw_attributes = capture_attributes(&element)?;
                    let mut log_base = None;
                    let mut orientation = None;
                    let mut maximum = None;
                    let mut minimum = None;
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
                                parse_scaling_child(
                                    name.as_deref().unwrap_or_default(),
                                    raw,
                                    &mut log_base,
                                    &mut orientation,
                                    &mut maximum,
                                    &mut minimum,
                                    &mut raw_children,
                                    &mut boundary,
                                )?;
                            }
                            Event::Empty(child) => {
                                let name = chart_child_local(&child, &namespaces)?;
                                let raw = capture_empty(&child, reader.empty_writing())?;
                                parse_scaling_child(
                                    name.as_deref().unwrap_or_default(),
                                    raw,
                                    &mut log_base,
                                    &mut orientation,
                                    &mut maximum,
                                    &mut minimum,
                                    &mut raw_children,
                                    &mut boundary,
                                )?;
                            }
                            event @ (Event::Text(_)
                            | Event::CData(_)
                            | Event::Comment(_)
                            | Event::PI(_)
                            | Event::GeneralRef(_)) => {
                                raw_children.push(boundary, capture_event(event)?);
                            }
                            Event::End(end)
                                if matches_local_name(end.name().as_ref(), b"scaling") =>
                            {
                                let (log_base, log_base_markup) = optional_markup(log_base);
                                let (orientation, orientation_markup) = orientation
                                    .map(|(value, markup)| (value, Some(markup)))
                                    .unwrap_or((Orientation::MinMax, None));
                                let (maximum, maximum_markup) = optional_markup(maximum);
                                let (minimum, minimum_markup) = optional_markup(minimum);
                                let scaling = Self {
                                    log_base,
                                    orientation,
                                    maximum,
                                    minimum,
                                    log_base_markup,
                                    orientation_markup,
                                    maximum_markup,
                                    minimum_markup,
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 4),
                                };
                                scaling.validate()?;
                                return Ok(scaling);
                            }
                            Event::Eof => return Err(missing_end("c:scaling")),
                            _ => {}
                        }
                        inner.clear();
                    }
                }
                Event::Empty(element)
                    if matches_local_name(element.name().as_ref(), b"scaling") =>
                {
                    typed_rewrite_bindings(&element, inherited)?;
                    return Ok(Self {
                        raw_attributes: capture_attributes(&element)?,
                        ..Self::default()
                    });
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("c:scaling".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.validate()?;
        let mut start = BytesStart::new("c:scaling");
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(value) = self.log_base {
            write_scalar(
                writer,
                "c:logBase",
                &value.to_string(),
                self.log_base_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        if self.orientation != Orientation::MinMax || self.orientation_markup.is_some() {
            write_scalar(
                writer,
                "c:orientation",
                self.orientation.as_str(),
                self.orientation_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(2))?;
        if let Some(value) = self.maximum {
            write_scalar(
                writer,
                "c:max",
                &value.to_string(),
                self.maximum_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(3))?;
        if let Some(value) = self.minimum {
            write_scalar(
                writer,
                "c:min",
                &value.to_string(),
                self.minimum_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(4))?;
        writer
            .write_event(Event::End(BytesEnd::new("c:scaling")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        if let Some(value) = self.log_base
            && (!value.is_finite() || !(2.0..=1000.0).contains(&value))
        {
            return Err(ChartError::InvalidValue {
                element: "c:logBase".to_owned(),
                value: value.to_string(),
            });
        }
        for (element, value) in [("c:max", self.maximum), ("c:min", self.minimum)] {
            if let Some(value) = value
                && !value.is_finite()
            {
                return Err(ChartError::InvalidValue {
                    element: element.to_owned(),
                    value: value.to_string(),
                });
            }
        }
        if let (Some(minimum), Some(maximum)) = (self.minimum, self.maximum)
            && minimum > maximum
        {
            return Err(ChartError::InvalidValue {
                element: "c:scaling".to_owned(),
                value: format!("minimum {minimum} exceeds maximum {maximum}"),
            });
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_scaling_child(
    name: &[u8],
    raw: Vec<u8>,
    log_base: &mut Option<(f64, ScalarMarkup)>,
    orientation: &mut Option<(Orientation, ScalarMarkup)>,
    maximum: &mut Option<(f64, ScalarMarkup)>,
    minimum: &mut Option<(f64, ScalarMarkup)>,
    raw_children: &mut OrderedRawChildren,
    boundary: &mut usize,
) -> Result<()> {
    match name {
        b"logBase" => {
            set_once(log_base, parse_f64_scalar(&raw, "logBase")?, "c:logBase")?;
            *boundary = (*boundary).max(1);
        }
        b"orientation" => {
            let (value, markup) = scalar_value(&raw, "orientation")?;
            let value = value
                .ok_or_else(|| invalid_attribute("orientation", "val", "<missing>".to_owned()))?;
            let parsed = Orientation::parse(&value)
                .ok_or_else(|| invalid_attribute("orientation", "val", value))?;
            set_once(orientation, (parsed, markup), "c:orientation")?;
            *boundary = (*boundary).max(2);
        }
        b"max" => {
            set_once(maximum, parse_f64_scalar(&raw, "max")?, "c:max")?;
            *boundary = (*boundary).max(3);
        }
        b"min" => {
            set_once(minimum, parse_f64_scalar(&raw, "min")?, "c:min")?;
            *boundary = (*boundary).max(4);
        }
        _ => raw_children.push(*boundary, raw),
    }
    Ok(())
}

impl ChartLines {
    fn from_xml(xml: &[u8], local: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
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
                    let mut sp_pr = None;
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
                                if name.as_deref() == Some(b"spPr") {
                                    parse_chart_shape_properties(&raw, &namespaces, &mut sp_pr)?;
                                    boundary = 1;
                                } else {
                                    raw_children.push(boundary, raw);
                                }
                            }
                            Event::Empty(child) => {
                                let name = chart_child_local(&child, &namespaces)?;
                                let raw = capture_empty(&child, reader.empty_writing())?;
                                if name.as_deref() == Some(b"spPr") {
                                    parse_chart_shape_properties(&raw, &namespaces, &mut sp_pr)?;
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
                                return Ok(Self {
                                    sp_pr,
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 1),
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
                    typed_rewrite_bindings(&element, inherited)?;
                    return Ok(Self {
                        sp_pr: None,
                        raw_attributes: capture_attributes(&element)?,
                        raw_children: OrderedRawChildren::default(),
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

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        let mut start = BytesStart::new(tag);
        push_attributes(&mut start, &self.raw_attributes);
        if self.sp_pr.is_none() && self.raw_children.is_empty() {
            writer
                .write_event(Event::Empty(start))
                .map_err(OxmlError::from)?;
            return Ok(());
        }
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(properties) = &self.sp_pr {
            properties.write_xml_as(writer, "c:spPr")?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        writer
            .write_event(Event::End(BytesEnd::new(tag)))
            .map_err(OxmlError::from)?;
        Ok(())
    }
}

pub(super) fn default_axis_id_markup(id: AxisId) -> AxisIdMarkup {
    AxisIdMarkup {
        scalar: ScalarMarkup::default(),
        parsed: id,
        lexical: id.value().to_string(),
    }
}

pub(super) fn parse_axis_id_scalar(xml: &[u8], local: &str) -> Result<(AxisId, AxisIdMarkup)> {
    let (value, scalar) = scalar_value(xml, local)?;
    let lexical = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed = lexical
        .parse::<i64>()
        .map_err(|_| invalid_attribute(local, "val", lexical.clone()))?;
    let id = AxisId::new(parsed).map_err(|_| invalid_attribute(local, "val", lexical.clone()))?;
    Ok((
        id,
        AxisIdMarkup {
            scalar,
            parsed: id,
            lexical,
        },
    ))
}

pub(super) fn write_axis_id<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    id: AxisId,
    markup: &AxisIdMarkup,
) -> Result<()> {
    AxisId::new(id.value())?;
    let normalized;
    let value = if id == markup.parsed {
        markup.lexical.as_str()
    } else {
        normalized = id.value().to_string();
        normalized.as_str()
    };
    write_scalar(writer, tag, value, Some(&markup.scalar))
}

fn parse_f64_scalar(xml: &[u8], local: &str) -> Result<(f64, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed = value
        .parse::<f64>()
        .map_err(|_| invalid_attribute(local, "val", value.clone()))?;
    if !parsed.is_finite() {
        return Err(invalid_attribute(local, "val", value));
    }
    Ok((parsed, markup))
}

fn parse_axis_position(xml: &[u8]) -> Result<(AxisPosition, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, "axPos")?;
    let value = value.ok_or_else(|| invalid_attribute("axPos", "val", "<missing>".to_owned()))?;
    let parsed =
        AxisPosition::parse(&value).ok_or_else(|| invalid_attribute("axPos", "val", value))?;
    Ok((parsed, markup))
}

fn parse_tick_mark(xml: &[u8], local: &str) -> Result<(TickMark, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed = TickMark::parse(&value).ok_or_else(|| invalid_attribute(local, "val", value))?;
    Ok((parsed, markup))
}

fn parse_tick_label_position(xml: &[u8]) -> Result<(TickLabelPosition, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, "tickLblPos")?;
    let value =
        value.ok_or_else(|| invalid_attribute("tickLblPos", "val", "<missing>".to_owned()))?;
    let parsed = TickLabelPosition::parse(&value)
        .ok_or_else(|| invalid_attribute("tickLblPos", "val", value))?;
    Ok((parsed, markup))
}

fn axis_id_markup_eq(left: &AxisIdMarkup, right: &AxisIdMarkup) -> bool {
    left.scalar == right.scalar
}
