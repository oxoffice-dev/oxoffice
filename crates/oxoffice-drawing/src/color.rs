use std::fmt;
use std::io::Write;

use oxoffice_core::OxmlError;
use oxoffice_core::raw_xml::{capture_element, capture_empty_element};
use oxoffice_core::units::{Angle, Percent1000};
use oxoffice_core::xml::{get_attr, local_name, matches_local_name};
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer};

use crate::namespace::reject_conflicting_a_prefix;
use crate::order::OrderedRawChildren;

/// Errors produced while parsing or writing DrawingML colours.
#[derive(Debug)]
pub enum ColorError {
    Xml(OxmlError),
    InvalidRgb(String),
    UnresolvedColor(String),
    MissingAttribute { element: String, attribute: String },
    UnexpectedElement(String),
    InvalidTransformValue { element: String, value: String },
}

impl fmt::Display for ColorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => error.fmt(formatter),
            Self::InvalidRgb(value) => write!(
                formatter,
                "DrawingML RGB colour must be exactly six hexadecimal digits: {value}"
            ),
            Self::UnresolvedColor(value) => {
                write!(formatter, "no concrete colour is available for: {value}")
            }
            Self::MissingAttribute { element, attribute } => {
                write!(formatter, "DrawingML {element} requires @{attribute}")
            }
            Self::UnexpectedElement(element) => {
                write!(formatter, "unexpected DrawingML colour element: {element}")
            }
            Self::InvalidTransformValue { element, value } => {
                write!(formatter, "DrawingML {element} has invalid @val: {value}")
            }
        }
    }
}

impl std::error::Error for ColorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            _ => None,
        }
    }
}

impl From<OxmlError> for ColorError {
    fn from(error: OxmlError) -> Self {
        Self::Xml(error)
    }
}

pub type Result<T> = std::result::Result<T, ColorError>;

/// A validated DrawingML sRGB colour.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RgbColor([u8; 3]);

impl RgbColor {
    /// Creates a colour from its red, green, and blue components.
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self([red, green, blue])
    }

    /// Parses an `RRGGBB` DrawingML colour value.
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != 6 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ColorError::InvalidRgb(value.to_owned()));
        }

        Ok(Self([
            parse_component(&value[0..2], value)?,
            parse_component(&value[2..4], value)?,
            parse_component(&value[4..6], value)?,
        ]))
    }

    /// Returns the red, green, and blue components.
    pub const fn components(self) -> [u8; 3] {
        self.0
    }
}

impl fmt::Display for RgbColor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:02X}{:02X}{:02X}",
            self.0[0], self.0[1], self.0[2]
        )
    }
}

/// One DrawingML colour transform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorTransform {
    Tint(Percent1000),
    Shade(Percent1000),
    Complement,
    Inverse,
    Gray,
    Alpha(Percent1000),
    AlphaOffset(Percent1000),
    AlphaModulation(Percent1000),
    Hue(Angle),
    HueOffset(Angle),
    HueModulation(Percent1000),
    Saturation(Percent1000),
    SaturationOffset(Percent1000),
    SaturationModulation(Percent1000),
    Luminance(Percent1000),
    LuminanceOffset(Percent1000),
    LuminanceModulation(Percent1000),
    Red(Percent1000),
    RedOffset(Percent1000),
    RedModulation(Percent1000),
    Green(Percent1000),
    GreenOffset(Percent1000),
    GreenModulation(Percent1000),
    Blue(Percent1000),
    BlueOffset(Percent1000),
    BlueModulation(Percent1000),
    Gamma,
    InverseGamma,
}

impl ColorTransform {
    pub(crate) fn from_xml(element: &BytesStart<'_>) -> Result<Option<Self>> {
        let qualified_name = element.name();
        let name = local_name(qualified_name.as_ref());
        let transform = match name {
            b"tint" => Self::Tint(parse_percent(element)?),
            b"shade" => Self::Shade(parse_percent(element)?),
            b"comp" => Self::Complement,
            b"inv" => Self::Inverse,
            b"gray" => Self::Gray,
            b"alpha" => Self::Alpha(parse_percent(element)?),
            b"alphaOff" => Self::AlphaOffset(parse_percent(element)?),
            b"alphaMod" => Self::AlphaModulation(parse_percent(element)?),
            b"hue" => Self::Hue(parse_angle(element)?),
            b"hueOff" => Self::HueOffset(parse_angle(element)?),
            b"hueMod" => Self::HueModulation(parse_percent(element)?),
            b"sat" => Self::Saturation(parse_percent(element)?),
            b"satOff" => Self::SaturationOffset(parse_percent(element)?),
            b"satMod" => Self::SaturationModulation(parse_percent(element)?),
            b"lum" => Self::Luminance(parse_percent(element)?),
            b"lumOff" => Self::LuminanceOffset(parse_percent(element)?),
            b"lumMod" => Self::LuminanceModulation(parse_percent(element)?),
            b"red" => Self::Red(parse_percent(element)?),
            b"redOff" => Self::RedOffset(parse_percent(element)?),
            b"redMod" => Self::RedModulation(parse_percent(element)?),
            b"green" => Self::Green(parse_percent(element)?),
            b"greenOff" => Self::GreenOffset(parse_percent(element)?),
            b"greenMod" => Self::GreenModulation(parse_percent(element)?),
            b"blue" => Self::Blue(parse_percent(element)?),
            b"blueOff" => Self::BlueOffset(parse_percent(element)?),
            b"blueMod" => Self::BlueModulation(parse_percent(element)?),
            b"gamma" => Self::Gamma,
            b"invGamma" => Self::InverseGamma,
            _ => return Ok(None),
        };
        Ok(Some(transform))
    }

    fn to_xml<W: Write>(self, writer: &mut Writer<W>) -> Result<()> {
        let (name, value) = match self {
            Self::Tint(value) => ("a:tint", Some(value.0)),
            Self::Shade(value) => ("a:shade", Some(value.0)),
            Self::Complement => ("a:comp", None),
            Self::Inverse => ("a:inv", None),
            Self::Gray => ("a:gray", None),
            Self::Alpha(value) => ("a:alpha", Some(value.0)),
            Self::AlphaOffset(value) => ("a:alphaOff", Some(value.0)),
            Self::AlphaModulation(value) => ("a:alphaMod", Some(value.0)),
            Self::Hue(value) => ("a:hue", Some(value.0)),
            Self::HueOffset(value) => ("a:hueOff", Some(value.0)),
            Self::HueModulation(value) => ("a:hueMod", Some(value.0)),
            Self::Saturation(value) => ("a:sat", Some(value.0)),
            Self::SaturationOffset(value) => ("a:satOff", Some(value.0)),
            Self::SaturationModulation(value) => ("a:satMod", Some(value.0)),
            Self::Luminance(value) => ("a:lum", Some(value.0)),
            Self::LuminanceOffset(value) => ("a:lumOff", Some(value.0)),
            Self::LuminanceModulation(value) => ("a:lumMod", Some(value.0)),
            Self::Red(value) => ("a:red", Some(value.0)),
            Self::RedOffset(value) => ("a:redOff", Some(value.0)),
            Self::RedModulation(value) => ("a:redMod", Some(value.0)),
            Self::Green(value) => ("a:green", Some(value.0)),
            Self::GreenOffset(value) => ("a:greenOff", Some(value.0)),
            Self::GreenModulation(value) => ("a:greenMod", Some(value.0)),
            Self::Blue(value) => ("a:blue", Some(value.0)),
            Self::BlueOffset(value) => ("a:blueOff", Some(value.0)),
            Self::BlueModulation(value) => ("a:blueMod", Some(value.0)),
            Self::Gamma => ("a:gamma", None),
            Self::InverseGamma => ("a:invGamma", None),
        };
        let mut element = BytesStart::new(name);
        let value_string = value.map(|raw| raw.to_string());
        if let Some(value) = value_string.as_deref() {
            element.push_attribute(("val", value));
        }
        writer
            .write_event(Event::Empty(element))
            .map_err(OxmlError::from)?;
        Ok(())
    }
}

/// A concrete colour after its DrawingML transform stack is applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl ResolvedColor {
    pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    pub const fn rgba(self) -> [u8; 4] {
        [self.red, self.green, self.blue, self.alpha]
    }
}

/// One of the twelve semantic slots selected by a DrawingML colour map.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ColorMapSlot {
    Background1,
    Text1,
    Background2,
    Text2,
    Accent1,
    Accent2,
    Accent3,
    Accent4,
    Accent5,
    Accent6,
    Hyperlink,
    FollowedHyperlink,
}

impl ColorMapSlot {
    fn from_value(value: &str) -> Option<Self> {
        match value {
            "bg1" => Some(Self::Background1),
            "tx1" => Some(Self::Text1),
            "bg2" => Some(Self::Background2),
            "tx2" => Some(Self::Text2),
            "accent1" => Some(Self::Accent1),
            "accent2" => Some(Self::Accent2),
            "accent3" => Some(Self::Accent3),
            "accent4" => Some(Self::Accent4),
            "accent5" => Some(Self::Accent5),
            "accent6" => Some(Self::Accent6),
            "hlink" => Some(Self::Hyperlink),
            "folHlink" => Some(Self::FollowedHyperlink),
            _ => None,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Background1 => 0,
            Self::Text1 => 1,
            Self::Background2 => 2,
            Self::Text2 => 3,
            Self::Accent1 => 4,
            Self::Accent2 => 5,
            Self::Accent3 => 6,
            Self::Accent4 => 7,
            Self::Accent5 => 8,
            Self::Accent6 => 9,
            Self::Hyperlink => 10,
            Self::FollowedHyperlink => 11,
        }
    }
}

/// One of the twelve concrete slots in a DrawingML theme colour scheme.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThemeColorSlot {
    Dark1,
    Light1,
    Dark2,
    Light2,
    Accent1,
    Accent2,
    Accent3,
    Accent4,
    Accent5,
    Accent6,
    Hyperlink,
    FollowedHyperlink,
}

impl ThemeColorSlot {
    /// Returns the DrawingML theme slot name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dark1 => "dk1",
            Self::Light1 => "lt1",
            Self::Dark2 => "dk2",
            Self::Light2 => "lt2",
            Self::Accent1 => "accent1",
            Self::Accent2 => "accent2",
            Self::Accent3 => "accent3",
            Self::Accent4 => "accent4",
            Self::Accent5 => "accent5",
            Self::Accent6 => "accent6",
            Self::Hyperlink => "hlink",
            Self::FollowedHyperlink => "folHlink",
        }
    }
}

/// The twelve master-controlled mappings applied before theme colour lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColorMap {
    slots: [ThemeColorSlot; 12],
}

impl ColorMap {
    /// Creates a colour map from parsed master values in schema attribute order.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        background1: ThemeColorSlot,
        text1: ThemeColorSlot,
        background2: ThemeColorSlot,
        text2: ThemeColorSlot,
        accent1: ThemeColorSlot,
        accent2: ThemeColorSlot,
        accent3: ThemeColorSlot,
        accent4: ThemeColorSlot,
        accent5: ThemeColorSlot,
        accent6: ThemeColorSlot,
        hyperlink: ThemeColorSlot,
        followed_hyperlink: ThemeColorSlot,
    ) -> Self {
        Self {
            slots: [
                background1,
                text1,
                background2,
                text2,
                accent1,
                accent2,
                accent3,
                accent4,
                accent5,
                accent6,
                hyperlink,
                followed_hyperlink,
            ],
        }
    }

    /// Returns the concrete theme slot selected for one semantic map slot.
    pub const fn theme_slot(&self, slot: ColorMapSlot) -> ThemeColorSlot {
        self.slots[slot.index()]
    }

    /// Returns a copy with only the named layout or slide overrides replaced.
    pub fn with_overrides(&self, overrides: &[(ColorMapSlot, ThemeColorSlot)]) -> Self {
        let mut resolved = self.clone();
        for (source, destination) in overrides {
            resolved.slots[source.index()] = *destination;
        }
        resolved
    }

    fn mapped_name<'a>(&self, value: &'a str) -> &'a str {
        ColorMapSlot::from_value(value)
            .map(|slot| self.theme_slot(slot).as_str())
            .unwrap_or(value)
    }
}

impl Default for ColorMap {
    fn default() -> Self {
        Self::new(
            ThemeColorSlot::Light1,
            ThemeColorSlot::Dark1,
            ThemeColorSlot::Light2,
            ThemeColorSlot::Dark2,
            ThemeColorSlot::Accent1,
            ThemeColorSlot::Accent2,
            ThemeColorSlot::Accent3,
            ThemeColorSlot::Accent4,
            ThemeColorSlot::Accent5,
            ThemeColorSlot::Accent6,
            ThemeColorSlot::Hyperlink,
            ThemeColorSlot::FollowedHyperlink,
        )
    }
}

/// One of the four DrawingML colour choice elements.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ColorChoice {
    Srgb {
        value: RgbColor,
        transforms: Vec<ColorTransform>,
        raw_children: OrderedRawChildren,
    },
    Scheme {
        value: String,
        transforms: Vec<ColorTransform>,
        raw_children: OrderedRawChildren,
    },
    System {
        value: String,
        last_color: Option<RgbColor>,
        transforms: Vec<ColorTransform>,
        raw_children: OrderedRawChildren,
    },
    Preset {
        value: String,
        transforms: Vec<ColorTransform>,
        raw_children: OrderedRawChildren,
    },
}

impl ColorChoice {
    /// Creates an sRGB colour without transforms.
    pub fn srgb(value: RgbColor) -> Self {
        Self::Srgb {
            value,
            transforms: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }

    /// Parses a colour after the caller has consumed its start event.
    pub fn from_xml(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>) -> Result<Self> {
        reject_conflicting_a_prefix(start)?;
        let qualified_name = start.name();
        let element_name = local_name(qualified_name.as_ref());
        let (transforms, raw_children) = capture_children(reader, element_name)?;
        Self::from_parts(start, transforms, raw_children)
    }

    /// Parses a colour from a self-closing element.
    pub fn from_empty_xml(start: &BytesStart<'_>) -> Result<Self> {
        reject_conflicting_a_prefix(start)?;
        Self::from_parts(start, Vec::new(), OrderedRawChildren::default())
    }

    pub(crate) fn from_parts(
        start: &BytesStart<'_>,
        transforms: Vec<ColorTransform>,
        raw_children: OrderedRawChildren,
    ) -> Result<Self> {
        let qualified_name = start.name();
        let element_name = local_name(qualified_name.as_ref());
        let value = required_attr(start, b"val")?;
        match element_name {
            b"srgbClr" => Ok(Self::Srgb {
                value: RgbColor::parse(&value)?,
                transforms,
                raw_children,
            }),
            b"schemeClr" => Ok(Self::Scheme {
                value,
                transforms,
                raw_children,
            }),
            b"sysClr" => Ok(Self::System {
                value,
                last_color: get_attr(start, b"lastClr")
                    .map(|last| RgbColor::parse(&last))
                    .transpose()?,
                transforms,
                raw_children,
            }),
            b"prstClr" => Ok(Self::Preset {
                value,
                transforms,
                raw_children,
            }),
            _ => Err(ColorError::UnexpectedElement(
                String::from_utf8_lossy(start.name().as_ref()).into_owned(),
            )),
        }
    }

    /// Writes this nested colour element with the canonical `a:` prefix.
    pub fn to_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        let (tag, value, last_color, transforms, raw_children) = match self {
            Self::Srgb {
                value,
                transforms,
                raw_children,
            } => (
                "a:srgbClr",
                value.to_string(),
                None,
                transforms,
                raw_children,
            ),
            Self::Scheme {
                value,
                transforms,
                raw_children,
            } => ("a:schemeClr", value.clone(), None, transforms, raw_children),
            Self::System {
                value,
                last_color,
                transforms,
                raw_children,
            } => (
                "a:sysClr",
                value.clone(),
                last_color.map(|colour| colour.to_string()),
                transforms,
                raw_children,
            ),
            Self::Preset {
                value,
                transforms,
                raw_children,
            } => ("a:prstClr", value.clone(), None, transforms, raw_children),
        };

        let mut start = BytesStart::new(tag);
        start.push_attribute(("val", value.as_str()));
        if let Some(last_color) = last_color.as_deref() {
            start.push_attribute(("lastClr", last_color));
        }

        if raw_children.is_empty() && transforms.is_empty() {
            writer
                .write_event(Event::Empty(start))
                .map_err(OxmlError::from)?;
            return Ok(());
        }

        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        for boundary in 0..=transforms.len() {
            for raw in raw_children.at(boundary) {
                writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
            }
            if let Some(transform) = transforms.get(boundary) {
                transform.to_xml(writer)?;
            }
        }
        writer
            .write_event(Event::End(BytesEnd::new(tag)))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    /// Returns raw, not-yet-modelled children in document order.
    pub fn raw_children(&self) -> &OrderedRawChildren {
        match self {
            Self::Srgb { raw_children, .. }
            | Self::Scheme { raw_children, .. }
            | Self::System { raw_children, .. }
            | Self::Preset { raw_children, .. } => raw_children,
        }
    }

    /// Returns modelled transforms in document order.
    pub fn transforms(&self) -> &[ColorTransform] {
        match self {
            Self::Srgb { transforms, .. }
            | Self::Scheme { transforms, .. }
            | Self::System { transforms, .. }
            | Self::Preset { transforms, .. } => transforms,
        }
    }
}

/// Resolves a colour through the master map, lookup table, and transform stack.
///
/// Theme slots, system colour names, and preset colour names share the concrete
/// lookup table. Only scheme colours pass through `color_map`. A system colour
/// falls back to its `lastClr` value when its name is absent from the lookup.
pub fn resolve_color(
    choice: &ColorChoice,
    color_map: &ColorMap,
    lookup: &[(&str, RgbColor)],
) -> Result<ResolvedColor> {
    let find = |name: &str| {
        lookup
            .iter()
            .find_map(|(candidate, colour)| (*candidate == name).then_some(*colour))
    };
    let (base, transforms) = match choice {
        ColorChoice::Srgb {
            value, transforms, ..
        } => (*value, transforms.as_slice()),
        ColorChoice::Scheme {
            value, transforms, ..
        } => {
            let mapped = color_map.mapped_name(value);
            (
                find(mapped).ok_or_else(|| ColorError::UnresolvedColor(mapped.to_owned()))?,
                transforms.as_slice(),
            )
        }
        ColorChoice::System {
            value,
            last_color,
            transforms,
            ..
        } => (
            find(value)
                .or(*last_color)
                .ok_or_else(|| ColorError::UnresolvedColor(value.clone()))?,
            transforms.as_slice(),
        ),
        ColorChoice::Preset {
            value, transforms, ..
        } => (
            find(value).ok_or_else(|| ColorError::UnresolvedColor(value.clone()))?,
            transforms.as_slice(),
        ),
    };
    Ok(apply_color_transforms(base, transforms))
}

fn required_attr(element: &BytesStart<'_>, name: &[u8]) -> Result<String> {
    get_attr(element, name).ok_or_else(|| ColorError::MissingAttribute {
        element: String::from_utf8_lossy(local_name(element.name().as_ref())).into_owned(),
        attribute: String::from_utf8_lossy(name).into_owned(),
    })
}

fn parse_component(component: &str, full_value: &str) -> Result<u8> {
    u8::from_str_radix(component, 16).map_err(|_| ColorError::InvalidRgb(full_value.to_owned()))
}

fn parse_percent(element: &BytesStart<'_>) -> Result<Percent1000> {
    parse_transform_i32(element).map(Percent1000)
}

fn parse_angle(element: &BytesStart<'_>) -> Result<Angle> {
    parse_transform_i32(element).map(Angle)
}

fn parse_transform_i32(element: &BytesStart<'_>) -> Result<i32> {
    let value = required_attr(element, b"val")?;
    value
        .parse()
        .map_err(|_| ColorError::InvalidTransformValue {
            element: String::from_utf8_lossy(local_name(element.name().as_ref())).into_owned(),
            value,
        })
}

fn capture_children(
    reader: &mut Reader<&[u8]>,
    end_name: &[u8],
) -> Result<(Vec<ColorTransform>, OrderedRawChildren)> {
    let mut transforms = Vec::new();
    let mut raw_children = OrderedRawChildren::default();
    let mut buffer = Vec::new();

    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) => {
                let transform = ColorTransform::from_xml(&element)?;
                let raw = capture_element(reader, &element)?;
                if let Some(transform) = transform.filter(|_| is_explicit_empty_element(&raw)) {
                    reject_conflicting_a_prefix(&element)?;
                    transforms.push(transform);
                } else {
                    raw_children.push(transforms.len(), raw);
                }
            }
            Event::Empty(element) => {
                if let Some(transform) = ColorTransform::from_xml(&element)? {
                    reject_conflicting_a_prefix(&element)?;
                    transforms.push(transform);
                } else {
                    raw_children.push(transforms.len(), capture_empty_element(&element)?);
                }
            }
            Event::End(element) if matches_local_name(element.name().as_ref(), end_name) => break,
            Event::Eof => {
                return Err(ColorError::Xml(OxmlError::MissingElement(format!(
                    "closing {} colour element",
                    String::from_utf8_lossy(end_name)
                ))));
            }
            _ => {}
        }
        buffer.clear();
    }

    Ok((transforms, raw_children))
}

fn is_explicit_empty_element(xml: &[u8]) -> bool {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    if !matches!(reader.read_event_into(&mut buffer), Ok(Event::Start(_))) {
        return false;
    }
    loop {
        buffer.clear();
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Text(text)) if is_xml_whitespace(text.as_ref()) => {}
            Ok(Event::CData(text)) if is_xml_whitespace(text.as_ref()) => {}
            Ok(Event::Comment(_) | Event::PI(_)) => {}
            Ok(Event::End(_)) => {
                buffer.clear();
                return matches!(reader.read_event_into(&mut buffer), Ok(Event::Eof));
            }
            _ => return false,
        }
    }
}

fn is_xml_whitespace(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
}

/// Applies a DrawingML transform stack from left to right.
pub fn apply_color_transforms(colour: RgbColor, transforms: &[ColorTransform]) -> ResolvedColor {
    let [red, green, blue] = colour.components();
    let mut current = WorkingColor {
        red: red as f64 / 255.0,
        green: green as f64 / 255.0,
        blue: blue as f64 / 255.0,
        alpha: 1.0,
    };
    for transform in transforms {
        current.apply(*transform);
    }
    current.resolved()
}

/// Applies spec-correct linear-gamma tint and shade percentages.
pub fn apply_tint_shade_pct(
    hex: &str,
    tint: Option<Percent1000>,
    shade: Option<Percent1000>,
) -> String {
    let Ok(colour) = RgbColor::parse(hex) else {
        return hex.to_owned();
    };
    let mut transforms = Vec::with_capacity(2);
    if let Some(value) = tint {
        transforms.push(ColorTransform::Tint(value));
    }
    if let Some(value) = shade {
        transforms.push(ColorTransform::Shade(value));
    }
    let resolved = apply_color_transforms(colour, &transforms);
    RgbColor::new(resolved.red, resolved.green, resolved.blue).to_string()
}

/// Applies DrawingML HSL luminance modulation followed by offset.
pub fn apply_lum_mod_off(
    hex: &str,
    lum_mod: Option<Percent1000>,
    lum_off: Option<Percent1000>,
) -> String {
    let Ok(colour) = RgbColor::parse(hex) else {
        return hex.to_owned();
    };
    let mut transforms = Vec::with_capacity(2);
    if let Some(value) = lum_mod {
        transforms.push(ColorTransform::LuminanceModulation(value));
    }
    if let Some(value) = lum_off {
        transforms.push(ColorTransform::LuminanceOffset(value));
    }
    let resolved = apply_color_transforms(colour, &transforms);
    RgbColor::new(resolved.red, resolved.green, resolved.blue).to_string()
}

/// Converts one sRGB channel to linear light.
pub fn srgb_to_linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// Converts one linear-light channel to sRGB.
pub fn linear_to_srgb(channel: f64) -> f64 {
    if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

#[derive(Clone, Copy)]
struct WorkingColor {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

impl WorkingColor {
    fn apply(&mut self, transform: ColorTransform) {
        match transform {
            ColorTransform::Tint(value) => {
                let amount = percent(value);
                self.map_linear(|channel| channel * amount + 1.0 - amount);
            }
            ColorTransform::Shade(value) => {
                let amount = percent(value);
                self.map_linear(|channel| channel * amount);
            }
            ColorTransform::Complement => {
                self.map_hsl(|hue, sat, lum| ((hue + 0.5).rem_euclid(1.0), sat, lum))
            }
            ColorTransform::Inverse => {
                self.map_linear(|channel| 1.0 - channel);
            }
            ColorTransform::Gray => {
                let luminance = 0.2126 * self.red + 0.7152 * self.green + 0.0722 * self.blue;
                self.red = luminance;
                self.green = luminance;
                self.blue = luminance;
            }
            ColorTransform::Alpha(value) => self.alpha = percent(value),
            ColorTransform::AlphaOffset(value) => self.alpha += percent(value),
            ColorTransform::AlphaModulation(value) => self.alpha *= percent(value),
            ColorTransform::Hue(value) => {
                let hue = angle_turns(value);
                self.map_hsl(|_, sat, lum| (hue, sat, lum));
            }
            ColorTransform::HueOffset(value) => {
                let offset = angle_turns(value);
                self.map_hsl(|hue, sat, lum| (hue + offset, sat, lum));
            }
            ColorTransform::HueModulation(value) => {
                let amount = percent(value);
                self.map_hsl(|hue, sat, lum| (hue * amount, sat, lum));
            }
            ColorTransform::Saturation(value) => {
                let value = percent(value);
                self.map_hsl(|hue, _, lum| (hue, value, lum));
            }
            ColorTransform::SaturationOffset(value) => {
                let offset = percent(value);
                self.map_hsl(|hue, sat, lum| (hue, sat + offset, lum));
            }
            ColorTransform::SaturationModulation(value) => {
                let amount = percent(value);
                self.map_hsl(|hue, sat, lum| (hue, sat * amount, lum));
            }
            ColorTransform::Luminance(value) => {
                let value = percent(value);
                self.map_hsl(|hue, sat, _| (hue, sat, value));
            }
            ColorTransform::LuminanceOffset(value) => {
                let offset = percent(value);
                self.map_hsl(|hue, sat, lum| (hue, sat, lum + offset));
            }
            ColorTransform::LuminanceModulation(value) => {
                let amount = percent(value);
                self.map_hsl(|hue, sat, lum| (hue, sat, lum * amount));
            }
            ColorTransform::Red(value) => self.red = linear_to_srgb(percent(value)),
            ColorTransform::RedOffset(value) => {
                self.red = linear_to_srgb(srgb_to_linear(self.red) + percent(value));
            }
            ColorTransform::RedModulation(value) => {
                self.red = linear_to_srgb(srgb_to_linear(self.red) * percent(value));
            }
            ColorTransform::Green(value) => self.green = linear_to_srgb(percent(value)),
            ColorTransform::GreenOffset(value) => {
                self.green = linear_to_srgb(srgb_to_linear(self.green) + percent(value));
            }
            ColorTransform::GreenModulation(value) => {
                self.green = linear_to_srgb(srgb_to_linear(self.green) * percent(value));
            }
            ColorTransform::Blue(value) => self.blue = linear_to_srgb(percent(value)),
            ColorTransform::BlueOffset(value) => {
                self.blue = linear_to_srgb(srgb_to_linear(self.blue) + percent(value));
            }
            ColorTransform::BlueModulation(value) => {
                self.blue = linear_to_srgb(srgb_to_linear(self.blue) * percent(value));
            }
            ColorTransform::Gamma => {
                self.red = linear_to_srgb(self.red);
                self.green = linear_to_srgb(self.green);
                self.blue = linear_to_srgb(self.blue);
            }
            ColorTransform::InverseGamma => {
                self.red = srgb_to_linear(self.red);
                self.green = srgb_to_linear(self.green);
                self.blue = srgb_to_linear(self.blue);
            }
        }
        self.clamp();
    }

    fn map_linear(&mut self, transform: impl Fn(f64) -> f64) {
        self.red = linear_to_srgb(transform(srgb_to_linear(self.red)));
        self.green = linear_to_srgb(transform(srgb_to_linear(self.green)));
        self.blue = linear_to_srgb(transform(srgb_to_linear(self.blue)));
    }

    fn map_hsl(&mut self, transform: impl Fn(f64, f64, f64) -> (f64, f64, f64)) {
        let (hue, sat, lum) = rgb_to_hsl(self.red, self.green, self.blue);
        let (hue, sat, lum) = transform(hue, sat, lum);
        (self.red, self.green, self.blue) = hsl_to_rgb(
            hue.rem_euclid(1.0),
            sat.clamp(0.0, 1.0),
            lum.clamp(0.0, 1.0),
        );
    }

    fn clamp(&mut self) {
        self.red = self.red.clamp(0.0, 1.0);
        self.green = self.green.clamp(0.0, 1.0);
        self.blue = self.blue.clamp(0.0, 1.0);
        self.alpha = self.alpha.clamp(0.0, 1.0);
    }

    fn resolved(self) -> ResolvedColor {
        let alpha = channel_byte(self.alpha);
        if alpha == 0 {
            return ResolvedColor::new(0, 0, 0, 0);
        }
        let red = alpha_quantized_channel(channel_byte(self.red), alpha);
        let green = alpha_quantized_channel(channel_byte(self.green), alpha);
        let blue = alpha_quantized_channel(channel_byte(self.blue), alpha);
        ResolvedColor::new(red, green, blue, alpha)
    }
}

fn percent(value: Percent1000) -> f64 {
    value.to_fraction()
}

fn angle_turns(value: Angle) -> f64 {
    value.to_degrees() / 360.0
}

fn channel_byte(channel: f64) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0 + 1e-9).round() as u8
}

fn alpha_quantized_channel(channel: u8, alpha: u8) -> u8 {
    if alpha == u8::MAX {
        return channel;
    }
    let channel = u16::from(channel);
    let alpha = u16::from(alpha);
    let premultiplied = (channel * alpha + 127) / 255;
    ((premultiplied * 255 + alpha / 2) / alpha) as u8
}

fn rgb_to_hsl(red: f64, green: f64, blue: f64) -> (f64, f64, f64) {
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lum = (max + min) / 2.0;
    if (max - min).abs() <= f64::EPSILON {
        return (0.0, 0.0, lum);
    }

    let delta = max - min;
    let sat = if lum > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if (max - red).abs() <= f64::EPSILON {
        (green - blue) / delta + if green < blue { 6.0 } else { 0.0 }
    } else if (max - green).abs() <= f64::EPSILON {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    } / 6.0;
    (hue, sat, lum)
}

fn hsl_to_rgb(hue: f64, sat: f64, lum: f64) -> (f64, f64, f64) {
    if sat <= f64::EPSILON {
        return (lum, lum, lum);
    }
    let q = if lum < 0.5 {
        lum * (1.0 + sat)
    } else {
        lum + sat - lum * sat
    };
    let p = 2.0 * lum - q;
    (
        hue_to_rgb(p, q, hue + 1.0 / 3.0),
        hue_to_rgb(p, q, hue),
        hue_to_rgb(p, q, hue - 1.0 / 3.0),
    )
}

fn hue_to_rgb(p: f64, q: f64, hue: f64) -> f64 {
    let hue = hue.rem_euclid(1.0);
    if hue < 1.0 / 6.0 {
        p + (q - p) * 6.0 * hue
    } else if hue < 0.5 {
        q
    } else if hue < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - hue) * 6.0
    } else {
        p
    }
}
