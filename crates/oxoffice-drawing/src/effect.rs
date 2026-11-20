use std::fmt;
use std::io::Write;

use oxoffice_core::OxmlError;
use oxoffice_core::raw_xml::{capture_element, capture_empty_element};
use oxoffice_core::units::{Angle, Percent1000};
use oxoffice_core::xml::{get_attr, local_name, matches_local_name};
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::{Reader, Writer, XmlVersion};

use crate::color::{ColorChoice, ColorError};
use crate::namespace::A_NS;
use crate::order::OrderedRawChildren;

const MAX_POSITIVE_COORDINATE: i64 = 27_273_042_316_900;
const FULL_CIRCLE_ANGLE: i32 = 21_600_000;
const QUARTER_CIRCLE_ANGLE: i32 = 5_400_000;

/// Errors produced while parsing or writing DrawingML effects.
#[derive(Debug)]
pub enum EffectError {
    Xml(OxmlError),
    Color(ColorError),
    UnexpectedElement(String),
    MissingColor,
    InvalidAttribute {
        element: String,
        attribute: String,
        value: String,
    },
}

impl fmt::Display for EffectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => error.fmt(formatter),
            Self::Color(error) => error.fmt(formatter),
            Self::UnexpectedElement(element) => {
                write!(formatter, "unexpected DrawingML effect element: {element}")
            }
            Self::MissingColor => write!(formatter, "DrawingML outerShdw requires a colour child"),
            Self::InvalidAttribute {
                element,
                attribute,
                value,
            } => write!(
                formatter,
                "DrawingML {element} has invalid @{attribute}: {value}"
            ),
        }
    }
}

impl std::error::Error for EffectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            Self::Color(error) => Some(error),
            _ => None,
        }
    }
}

impl From<OxmlError> for EffectError {
    fn from(error: OxmlError) -> Self {
        Self::Xml(error)
    }
}

impl From<ColorError> for EffectError {
    fn from(error: ColorError) -> Self {
        Self::Color(error)
    }
}

pub type Result<T> = std::result::Result<T, EffectError>;

/// DrawingML rectangle alignment used as the origin of a shadow transform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RectAlignment {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl RectAlignment {
    /// Parses an `ST_RectAlignment` token such as `tl` or `ctr`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "tl" => Some(Self::TopLeft),
            "t" => Some(Self::Top),
            "tr" => Some(Self::TopRight),
            "l" => Some(Self::Left),
            "ctr" => Some(Self::Center),
            "r" => Some(Self::Right),
            "bl" => Some(Self::BottomLeft),
            "b" => Some(Self::Bottom),
            "br" => Some(Self::BottomRight),
            _ => None,
        }
    }

    /// Returns the `ST_RectAlignment` token written for this alignment.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TopLeft => "tl",
            Self::Top => "t",
            Self::TopRight => "tr",
            Self::Left => "l",
            Self::Center => "ctr",
            Self::Right => "r",
            Self::BottomLeft => "bl",
            Self::Bottom => "b",
            Self::BottomRight => "br",
        }
    }
}

/// Modelled properties of one DrawingML `a:outerShdw` effect.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CT_OuterShadowEffect {
    pub blur_radius: Option<i64>,
    pub distance: Option<i64>,
    pub direction: Option<Angle>,
    pub scale_x: Option<Percent1000>,
    pub scale_y: Option<Percent1000>,
    pub skew_x: Option<Angle>,
    pub skew_y: Option<Angle>,
    pub alignment: Option<RectAlignment>,
    pub rotate_with_shape: Option<bool>,
    pub color: Option<ColorChoice>,
    raw_children: OrderedRawChildren,
}

impl CT_OuterShadowEffect {
    fn from_start(start: &BytesStart<'_>) -> Result<Self> {
        let shadow = Self {
            blur_radius: optional_parse(start, b"blurRad")?,
            distance: optional_parse(start, b"dist")?,
            direction: optional_parse::<i32>(start, b"dir")?.map(Angle),
            scale_x: optional_parse::<i32>(start, b"sx")?.map(Percent1000),
            scale_y: optional_parse::<i32>(start, b"sy")?.map(Percent1000),
            skew_x: optional_parse::<i32>(start, b"kx")?.map(Angle),
            skew_y: optional_parse::<i32>(start, b"ky")?.map(Angle),
            alignment: optional_enum(start, b"algn", RectAlignment::parse)?,
            rotate_with_shape: optional_bool(start, b"rotWithShape")?,
            color: None,
            raw_children: OrderedRawChildren::default(),
        };
        shadow.validate_attributes()?;
        Ok(shadow)
    }

    fn from_element(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>) -> Result<Self> {
        let mut shadow = Self::from_start(start)?;
        let mut boundary = 0;
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element)
                    if is_color(element.name().as_ref()) && shadow.color.is_none() =>
                {
                    shadow.color = Some(ColorChoice::from_xml(reader, &element)?);
                    boundary = 1;
                }
                Event::Empty(element)
                    if is_color(element.name().as_ref()) && shadow.color.is_none() =>
                {
                    shadow.color = Some(ColorChoice::from_empty_xml(&element)?);
                    boundary = 1;
                }
                Event::Start(element) => shadow
                    .raw_children
                    .push(boundary, capture_element(reader, &element)?),
                Event::Empty(element) => shadow
                    .raw_children
                    .push(boundary, capture_empty_element(&element)?),
                Event::End(element)
                    if matches_local_name(element.name().as_ref(), b"outerShdw") =>
                {
                    break;
                }
                Event::Eof => return Err(missing_end("outerShdw")),
                _ => {}
            }
            buffer.clear();
        }
        if shadow.color.is_none() && shadow.raw_children.is_empty() {
            return Err(EffectError::MissingColor);
        }
        Ok(shadow)
    }

    fn validate_attributes(&self) -> Result<()> {
        if let Some(value) = self.blur_radius {
            validate_coordinate("blurRad", value)?;
        }
        if let Some(value) = self.distance {
            validate_coordinate("dist", value)?;
        }
        if let Some(value) = self.direction {
            validate_direction(value.0)?;
        }
        if let Some(value) = self.skew_x {
            validate_skew("kx", value.0)?;
        }
        if let Some(value) = self.skew_y {
            validate_skew("ky", value.0)?;
        }
        Ok(())
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.validate_attributes()?;
        if self.color.is_none() && self.raw_children.is_empty() {
            return Err(EffectError::MissingColor);
        }
        let mut start = BytesStart::new("a:outerShdw");
        let blur_radius = self.blur_radius.map(|value| value.to_string());
        let distance = self.distance.map(|value| value.to_string());
        let direction = self.direction.map(|value| value.0.to_string());
        let scale_x = self.scale_x.map(|value| value.0.to_string());
        let scale_y = self.scale_y.map(|value| value.0.to_string());
        let skew_x = self.skew_x.map(|value| value.0.to_string());
        let skew_y = self.skew_y.map(|value| value.0.to_string());
        for (name, value) in [
            ("blurRad", blur_radius.as_deref()),
            ("dist", distance.as_deref()),
            ("dir", direction.as_deref()),
            ("sx", scale_x.as_deref()),
            ("sy", scale_y.as_deref()),
            ("kx", skew_x.as_deref()),
            ("ky", skew_y.as_deref()),
        ] {
            if let Some(value) = value {
                start.push_attribute((name, value));
            }
        }
        if let Some(alignment) = self.alignment {
            start.push_attribute(("algn", alignment.as_str()));
        }
        if let Some(rotate) = self.rotate_with_shape.map(bool_text) {
            start.push_attribute(("rotWithShape", rotate));
        }

        write_start(writer, start)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(color) = &self.color {
            color.to_xml(writer)?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        write_end(writer, "a:outerShdw")
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }

    /// Reports a colour choice that is kept as raw XML, such as an
    /// `a:scrgbClr` or `a:hslClr`, which `color` does not model.
    pub fn has_unmodelled_color(&self) -> bool {
        (0..=1).any(|boundary| self.raw_children.at(boundary).any(raw_is_color))
    }

    /// Replaces the shadow colour, dropping an unmodelled colour choice so
    /// that the element keeps the single colour its schema allows. The new
    /// colour takes the dropped one's place among the unmodelled siblings.
    pub fn replace_color(&mut self, color: ColorChoice) {
        let mut raw_children = OrderedRawChildren::default();
        let mut boundary = 0;
        for child in self.raw_children.at(0) {
            if raw_is_color(child) {
                boundary = 1;
            } else {
                raw_children.push(boundary, child.to_vec());
            }
        }
        for child in self.raw_children.at(1).filter(|child| !raw_is_color(child)) {
            raw_children.push(1, child.to_vec());
        }
        self.raw_children = raw_children;
        self.color = Some(color);
    }
}

/// DrawingML `a:effectLst` with an outer shadow and preserved other effects.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CT_EffectList {
    pub outer_shadow: Option<CT_OuterShadowEffect>,
    raw_attributes: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
}

impl CT_EffectList {
    /// Parses one complete `a:effectLst` with any namespace prefix.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element)
                    if matches_local_name(element.name().as_ref(), b"effectLst") =>
                {
                    let mut effects = Self::from_element(&mut reader)?;
                    effects.raw_attributes = capture_raw_attributes(&element)?;
                    return Ok(effects);
                }
                Event::Empty(element)
                    if matches_local_name(element.name().as_ref(), b"effectLst") =>
                {
                    return Ok(Self {
                        raw_attributes: capture_raw_attributes(&element)?,
                        ..Self::default()
                    });
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(unexpected(&element));
                }
                Event::Eof => {
                    return Err(EffectError::Xml(OxmlError::MissingElement(
                        "DrawingML effect list".to_owned(),
                    )));
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(reader: &mut Reader<&[u8]>) -> Result<Self> {
        let mut effects = Self::default();
        let mut boundary = 0;
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element)
                    if matches_local_name(element.name().as_ref(), b"outerShdw")
                        && effects.outer_shadow.is_none() =>
                {
                    effects.outer_shadow =
                        Some(CT_OuterShadowEffect::from_element(reader, &element)?);
                    boundary = 1;
                }
                Event::Empty(element)
                    if matches_local_name(element.name().as_ref(), b"outerShdw")
                        && effects.outer_shadow.is_none() =>
                {
                    return Err(EffectError::MissingColor);
                }
                Event::Start(element) => {
                    boundary = boundary.max(raw_effect_boundary(element.name().as_ref()));
                    effects
                        .raw_children
                        .push(boundary, capture_element(reader, &element)?);
                }
                Event::Empty(element) => {
                    boundary = boundary.max(raw_effect_boundary(element.name().as_ref()));
                    effects
                        .raw_children
                        .push(boundary, capture_empty_element(&element)?);
                }
                Event::End(element)
                    if matches_local_name(element.name().as_ref(), b"effectLst") =>
                {
                    break;
                }
                Event::Eof => return Err(missing_end("effectLst")),
                _ => {}
            }
            buffer.clear();
        }
        Ok(effects)
    }

    /// Writes this list with a fixed root prefix and the shadow in schema order.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml(&mut writer)?;
        Ok(writer.into_inner())
    }

    /// Writes this effect list into an existing XML writer.
    pub fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        let mut start = BytesStart::new("a:effectLst");
        for (name, value) in &self.raw_attributes {
            start.push_attribute((name.as_str(), value.as_str()));
        }
        if self.outer_shadow.is_none() && self.raw_children.is_empty() {
            return write_empty(writer, start);
        }
        write_start(writer, start)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(shadow) = &self.outer_shadow {
            shadow.write_xml(writer)?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        write_end(writer, "a:effectLst")
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }

    /// Reports an unresolved placeholder colour inside an opaque effect child.
    pub fn has_unmodelled_placeholder_color(&self) -> bool {
        self.raw_children
            .at(0)
            .chain(self.raw_children.at(1))
            .any(raw_contains_placeholder_color)
            || self.outer_shadow.as_ref().is_some_and(|shadow| {
                shadow
                    .raw_children()
                    .at(0)
                    .chain(shadow.raw_children().at(1))
                    .any(raw_contains_placeholder_color)
            })
    }
}

/// Places an effect the schema orders after `a:outerShdw` behind the outer
/// shadow slot, so a shadow added later is still written before it.
fn raw_effect_boundary(name: &[u8]) -> usize {
    match local_name(name) {
        b"prstShdw" | b"reflection" | b"softEdge" => 1,
        _ => 0,
    }
}

pub(crate) fn raw_contains_placeholder_color(xml: &[u8]) -> bool {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_resolved_event_into(&mut buffer) {
            Ok((namespace, Event::Start(element) | Event::Empty(element)))
                if is_drawingml_namespace(&namespace)
                    && matches_local_name(element.name().as_ref(), b"schemeClr")
                    && get_attr(&element, b"val").as_deref() == Some("phClr") =>
            {
                return true;
            }
            Ok((_, Event::Eof)) | Err(_) => return false,
            _ => {}
        }
        buffer.clear();
    }
}

/// Reports whether one captured element is a DrawingML `a:effectDag`.
pub fn raw_is_effect_dag(xml: &[u8]) -> bool {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_resolved_event_into(&mut buffer) {
            Ok((namespace, Event::Start(element) | Event::Empty(element))) => {
                return is_drawingml_namespace(&namespace)
                    && matches_local_name(element.name().as_ref(), b"effectDag");
            }
            Ok((_, Event::Eof)) | Err(_) => return false,
            _ => {}
        }
        buffer.clear();
    }
}

fn is_drawingml_namespace(namespace: &ResolveResult<'_>) -> bool {
    match namespace {
        ResolveResult::Bound(Namespace(uri)) => *uri == A_NS.as_bytes(),
        ResolveResult::Unknown(prefix) => prefix == b"a",
        ResolveResult::Unbound => false,
    }
}

fn raw_is_color(xml: &[u8]) -> bool {
    let name_end = xml
        .iter()
        .position(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
        .unwrap_or(xml.len());
    matches!(
        local_name(xml.get(1..name_end).unwrap_or_default()),
        b"scrgbClr" | b"srgbClr" | b"hslClr" | b"sysClr" | b"schemeClr" | b"prstClr"
    )
}

fn capture_raw_attributes(start: &BytesStart<'_>) -> Result<Vec<(String, String)>> {
    let mut raw = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        raw.push((name, value));
    }
    Ok(raw)
}

fn is_color(name: &[u8]) -> bool {
    matches!(
        local_name(name),
        b"srgbClr" | b"schemeClr" | b"sysClr" | b"prstClr"
    )
}

fn validate_coordinate(attribute: &str, value: i64) -> Result<()> {
    if (0..=MAX_POSITIVE_COORDINATE).contains(&value) {
        Ok(())
    } else {
        Err(invalid_value(attribute, value))
    }
}

fn validate_direction(value: i32) -> Result<()> {
    if (0..FULL_CIRCLE_ANGLE).contains(&value) {
        Ok(())
    } else {
        Err(invalid_value("dir", value))
    }
}

fn validate_skew(attribute: &str, value: i32) -> Result<()> {
    if (-QUARTER_CIRCLE_ANGLE..QUARTER_CIRCLE_ANGLE).contains(&value) {
        Ok(())
    } else {
        Err(invalid_value(attribute, value))
    }
}

fn invalid_value(attribute: &str, value: impl ToString) -> EffectError {
    EffectError::InvalidAttribute {
        element: "outerShdw".to_owned(),
        attribute: attribute.to_owned(),
        value: value.to_string(),
    }
}

fn optional_parse<T: std::str::FromStr>(start: &BytesStart<'_>, name: &[u8]) -> Result<Option<T>> {
    get_attr(start, name)
        .map(|value| {
            value
                .parse()
                .map_err(|_| invalid(start, name, value.to_owned()))
        })
        .transpose()
}

fn optional_bool(start: &BytesStart<'_>, name: &[u8]) -> Result<Option<bool>> {
    get_attr(start, name)
        .map(|value| match value.as_str() {
            "1" | "true" => Ok(true),
            "0" | "false" => Ok(false),
            _ => Err(invalid(start, name, value)),
        })
        .transpose()
}

fn optional_enum<T>(
    start: &BytesStart<'_>,
    name: &[u8],
    parse: impl FnOnce(&str) -> Option<T>,
) -> Result<Option<T>> {
    get_attr(start, name)
        .map(|value| parse(&value).ok_or_else(|| invalid(start, name, value)))
        .transpose()
}

fn invalid(start: &BytesStart<'_>, attribute: &[u8], value: String) -> EffectError {
    EffectError::InvalidAttribute {
        element: String::from_utf8_lossy(local_name(start.name().as_ref())).into_owned(),
        attribute: String::from_utf8_lossy(attribute).into_owned(),
        value,
    }
}

fn unexpected(start: &BytesStart<'_>) -> EffectError {
    EffectError::UnexpectedElement(String::from_utf8_lossy(start.name().as_ref()).into_owned())
}

fn missing_end(name: &str) -> EffectError {
    EffectError::Xml(OxmlError::MissingElement(format!("closing a:{name}")))
}

const fn bool_text(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

fn emit_raw<'a, W: Write>(
    writer: &mut Writer<W>,
    children: impl Iterator<Item = &'a [u8]>,
) -> Result<()> {
    for child in children {
        writer.get_mut().write_all(child).map_err(OxmlError::from)?;
    }
    Ok(())
}

fn write_start<W: Write>(writer: &mut Writer<W>, start: BytesStart<'_>) -> Result<()> {
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn write_empty<W: Write>(writer: &mut Writer<W>, start: BytesStart<'_>) -> Result<()> {
    writer
        .write_event(Event::Empty(start))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn write_end<W: Write>(writer: &mut Writer<W>, name: &str) -> Result<()> {
    writer
        .write_event(Event::End(BytesEnd::new(name)))
        .map_err(OxmlError::from)?;
    Ok(())
}
