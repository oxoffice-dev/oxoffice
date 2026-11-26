//! Paragraph properties (`CT_PPr`) and run properties (`CT_RPr`).

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer, XmlVersion};

use crate::error::{OxmlError, Result};
use crate::namespace::{W_NS, matches_local_name};
use crate::run_properties::language_element_is_explicitly_empty;
use crate::shared::ST_OnOff;

pub use crate::paragraph_properties::{CT_FramePr, CT_PPr};
pub use crate::run_properties::{CT_EastAsianLayout, CT_FitText, CT_RPr, ST_Em, ST_TextEffect};

/// `CT_Shd` — Shading/background fill.
///
/// The six theme attributes are modeled rather than dropped. `w:themeTint`,
/// `w:themeShade`, `w:themeFillTint` and `w:themeFillShade` are
/// `ST_UcharHexNumber`, a single byte written as two hex digits, and they are
/// re-serialized in the canonical upper-case spelling Word itself writes.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Shd {
    /// Shading pattern (e.g. "clear", "solid", "horzStripe")
    pub val: String,
    /// Foreground color hex
    pub color: Option<String>,
    /// Background fill color hex
    pub fill: Option<String>,
    /// Foreground theme colour reference (`w:themeColor`).
    pub theme_color: Option<String>,
    /// Foreground theme tint, 0 to 255 (`w:themeTint`).
    pub theme_tint: Option<u8>,
    /// Foreground theme shade, 0 to 255 (`w:themeShade`).
    pub theme_shade: Option<u8>,
    /// Background theme colour reference (`w:themeFill`).
    pub theme_fill: Option<String>,
    /// Background theme tint, 0 to 255 (`w:themeFillTint`).
    pub theme_fill_tint: Option<u8>,
    /// Background theme shade, 0 to 255 (`w:themeFillShade`).
    pub theme_fill_shade: Option<u8>,
    /// Attributes this type does not model, in source order.
    ///
    /// A theme tint or shade whose value is not two hex digits reaches a
    /// serialized element through here rather than being discarded. Names and
    /// values keep their stored spelling and are written ahead of the modeled
    /// attributes, so a caller storing one itself owns the escaping.
    pub extra_attributes: Vec<(String, String)>,
}

impl Default for CT_Shd {
    /// The pattern Word omits, matching what the parser assumes for a
    /// `w:shd` that carries no `w:val`.
    fn default() -> Self {
        CT_Shd {
            val: "clear".to_owned(),
            color: None,
            fill: None,
            theme_color: None,
            theme_tint: None,
            theme_shade: None,
            theme_fill: None,
            theme_fill_tint: None,
            theme_fill_shade: None,
            extra_attributes: Vec::new(),
        }
    }
}

/// Parse an `ST_UcharHexNumber` attribute value.
pub(crate) fn parse_uchar_hex(value: &str) -> Option<u8> {
    (value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| u8::from_str_radix(value, 16).ok())
        .flatten()
}

pub(crate) fn uchar_hex_bytes(value: u8) -> [u8; 2] {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    [
        DIGITS[usize::from(value >> 4)],
        DIGITS[usize::from(value & 0x0f)],
    ]
}

impl CT_Shd {
    pub fn from_xml_attrs(e: &BytesStart) -> Result<Self> {
        let mut shd = CT_Shd::default();

        for attr in e.attributes() {
            let attr = attr?;
            let key = attr.key.as_ref();
            let v = oxoffice_core::xml::attribute_text(&attr)?;
            let v: &str = &v;
            let text =
                || oxoffice_core::xml::attribute_text(&attr).map(std::borrow::Cow::into_owned);
            if matches_local_name(key, b"val") {
                shd.val = text()?;
            } else if matches_local_name(key, b"color") {
                shd.color = Some(text()?);
            } else if matches_local_name(key, b"fill") {
                shd.fill = Some(text()?);
            } else if matches_local_name(key, b"themeColor") {
                shd.theme_color = Some(text()?);
            } else if matches_local_name(key, b"themeFill") {
                shd.theme_fill = Some(text()?);
            } else if let Some(slot) = shd.theme_byte_slot(key) {
                match parse_uchar_hex(v) {
                    Some(parsed) => *slot = Some(parsed),
                    None => shd.extra_attributes.push((
                        std::str::from_utf8(key)?.to_owned(),
                        std::str::from_utf8(&attr.value)?.to_owned(),
                    )),
                }
            } else {
                shd.extra_attributes.push((
                    std::str::from_utf8(key)?.to_owned(),
                    std::str::from_utf8(&attr.value)?.to_owned(),
                ));
            }
        }

        Ok(shd)
    }

    pub(crate) fn from_xml_attrs_with_prefixes(
        e: &BytesStart,
        word_prefixes: &[String],
    ) -> Result<Self> {
        let mut shd = CT_Shd::default();

        for attr in e.attributes() {
            let attr = attr?;
            let key = attr.key.as_ref();
            let v = oxoffice_core::xml::attribute_text(&attr)?;
            let v: &str = &v;
            let text =
                || oxoffice_core::xml::attribute_text(&attr).map(std::borrow::Cow::into_owned);
            if is_word_attribute(key, b"val", word_prefixes) {
                shd.val = text()?;
            } else if is_word_attribute(key, b"color", word_prefixes) {
                shd.color = Some(text()?);
            } else if is_word_attribute(key, b"fill", word_prefixes) {
                shd.fill = Some(text()?);
            } else if is_word_attribute(key, b"themeColor", word_prefixes) {
                shd.theme_color = Some(text()?);
            } else if is_word_attribute(key, b"themeFill", word_prefixes) {
                shd.theme_fill = Some(text()?);
            } else if let Some(slot) = shd.word_theme_byte_slot(key, word_prefixes) {
                match parse_uchar_hex(v) {
                    Some(parsed) => *slot = Some(parsed),
                    None => shd.extra_attributes.push((
                        std::str::from_utf8(key)?.to_owned(),
                        std::str::from_utf8(&attr.value)?.to_owned(),
                    )),
                }
            } else {
                shd.extra_attributes.push((
                    std::str::from_utf8(key)?.to_owned(),
                    std::str::from_utf8(&attr.value)?.to_owned(),
                ));
            }
        }

        Ok(shd)
    }

    /// The typed slot a prefix-agnostic theme byte attribute writes into.
    fn theme_byte_slot(&mut self, key: &[u8]) -> Option<&mut Option<u8>> {
        if matches_local_name(key, b"themeTint") {
            Some(&mut self.theme_tint)
        } else if matches_local_name(key, b"themeShade") {
            Some(&mut self.theme_shade)
        } else if matches_local_name(key, b"themeFillTint") {
            Some(&mut self.theme_fill_tint)
        } else if matches_local_name(key, b"themeFillShade") {
            Some(&mut self.theme_fill_shade)
        } else {
            None
        }
    }

    /// The typed slot a Word-prefixed theme byte attribute writes into.
    fn word_theme_byte_slot(
        &mut self,
        key: &[u8],
        word_prefixes: &[String],
    ) -> Option<&mut Option<u8>> {
        if is_word_attribute(key, b"themeTint", word_prefixes) {
            Some(&mut self.theme_tint)
        } else if is_word_attribute(key, b"themeShade", word_prefixes) {
            Some(&mut self.theme_shade)
        } else if is_word_attribute(key, b"themeFillTint", word_prefixes) {
            Some(&mut self.theme_fill_tint)
        } else if is_word_attribute(key, b"themeFillShade", word_prefixes) {
            Some(&mut self.theme_fill_shade)
        } else {
            None
        }
    }

    pub fn write_xml<W: std::io::Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        self.write_xml_in(
            writer,
            tag,
            crate::text::XmlWriteRole::Publication,
            None,
            None,
        )
    }

    pub(crate) fn write_xml_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        name: &str,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if role == crate::text::XmlWriteRole::Source {
            let mut root = crate::text::FieldXmlStart::word("w", name, emission)?;
            for (name, value) in &self.extra_attributes {
                root.push_raw(name, value)?;
            }
            let prefix = root.qualify(&[], role, source_context)?;
            self.write_xml_core(writer, root, &prefix, false)
        } else {
            let root = crate::text::FieldXmlStart::new(name, emission)?;
            self.write_xml_core(writer, root, "w", true)
        }
    }

    fn write_xml_core<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        mut e: crate::text::FieldXmlStart<'_, '_>,
        word_prefix: &str,
        include_retained: bool,
    ) -> Result<()> {
        if include_retained {
            for (name, value) in &self.extra_attributes {
                e.push_raw(name, value)?;
            }
        }
        e.push_word(word_prefix, "val", self.val.as_str())?;
        if let Some(ref c) = self.color {
            e.push_word(word_prefix, "color", c.as_str())?;
        }
        if let Some(ref c) = self.theme_color {
            e.push_word(word_prefix, "themeColor", c.as_str())?;
        }
        if let Some(tint) = self.theme_tint {
            let encoded = uchar_hex_bytes(tint);
            e.push_word(word_prefix, "themeTint", std::str::from_utf8(&encoded)?)?;
        }
        if let Some(shade) = self.theme_shade {
            let encoded = uchar_hex_bytes(shade);
            e.push_word(word_prefix, "themeShade", std::str::from_utf8(&encoded)?)?;
        }
        if let Some(ref f) = self.fill {
            e.push_word(word_prefix, "fill", f.as_str())?;
        }
        if let Some(ref f) = self.theme_fill {
            e.push_word(word_prefix, "themeFill", f.as_str())?;
        }
        if let Some(tint) = self.theme_fill_tint {
            let encoded = uchar_hex_bytes(tint);
            e.push_word(word_prefix, "themeFillTint", std::str::from_utf8(&encoded)?)?;
        }
        if let Some(shade) = self.theme_fill_shade {
            let encoded = uchar_hex_bytes(shade);
            e.push_word(
                word_prefix,
                "themeFillShade",
                std::str::from_utf8(&encoded)?,
            )?;
        }
        e.write(writer, true)?;
        Ok(())
    }
}

const RAW_MODELED_ATTRIBUTES_FLAG: usize = 1 << (usize::BITS - 1);
const RAW_MODELED_DUPLICATE_FLAG: usize = 1 << (usize::BITS - 2);
const RAW_MODELED_FLAGS: usize = RAW_MODELED_ATTRIBUTES_FLAG | RAW_MODELED_DUPLICATE_FLAG;

pub(crate) fn raw_occurrence(position: (u8, usize)) -> usize {
    position.1 & !RAW_MODELED_FLAGS
}

pub(crate) fn raw_is_modeled_attribute_carrier(position: (u8, usize)) -> bool {
    position.1 & RAW_MODELED_ATTRIBUTES_FLAG != 0
}

pub(crate) fn modeled_attribute_carrier_occurrence(occurrence: usize) -> usize {
    occurrence | RAW_MODELED_ATTRIBUTES_FLAG
}

fn raw_is_modeled_duplicate(position: (u8, usize)) -> bool {
    position.1 & RAW_MODELED_DUPLICATE_FLAG != 0
}

fn modeled_duplicate_occurrence(occurrence: usize) -> usize {
    occurrence | RAW_MODELED_DUPLICATE_FLAG
}

pub(crate) fn replay_modeled_toggle_raw(
    position: (u8, usize),
    modeled_value_is_present: bool,
) -> bool {
    !raw_is_modeled_attribute_carrier(position)
        && (!raw_is_modeled_duplicate(position) || modeled_value_is_present)
}

pub(crate) fn record_modeled_toggle_candidate(
    raw_xml: &mut Vec<Vec<u8>>,
    positions: &mut Vec<(u8, usize)>,
    raw: Vec<u8>,
    slot: u8,
    occurrence: usize,
) {
    let replacing_carrier = positions
        .iter()
        .any(|position| position.0 == slot && raw_is_modeled_attribute_carrier(*position));
    if replacing_carrier {
        for position in positions
            .iter_mut()
            .filter(|position| position.0 == slot && raw_is_modeled_attribute_carrier(**position))
        {
            position.1 = modeled_duplicate_occurrence(raw_occurrence(*position));
        }
    }
    raw_xml.push(raw);
    positions.push((slot, modeled_attribute_carrier_occurrence(occurrence)));
}

pub(crate) fn remove_redundant_modeled_toggle_candidate(
    raw_xml: &mut Vec<Vec<u8>>,
    positions: &mut Vec<(u8, usize)>,
    slot: u8,
    carrier_required: bool,
) {
    if carrier_required {
        return;
    }
    if let Some(index) = positions
        .iter()
        .rposition(|position| position.0 == slot && raw_is_modeled_attribute_carrier(*position))
    {
        let carrier_occurrence = raw_occurrence(positions[index]);
        for (candidate_index, position) in positions.iter_mut().enumerate() {
            if candidate_index == index || position.0 != slot {
                continue;
            }
            let flags = position.1 & RAW_MODELED_FLAGS;
            let occurrence = raw_occurrence(*position);
            position.1 = flags
                | usize::from(
                    occurrence > carrier_occurrence
                        || (occurrence == carrier_occurrence && candidate_index > index),
                );
        }
        raw_xml.remove(index);
        positions.remove(index);
    }
}

pub(crate) fn toggle_element_is_explicitly_empty(xml: &[u8]) -> Result<bool> {
    language_element_is_explicitly_empty(xml)
}

pub(crate) fn toggle_has_unsupported_attributes(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        if !is_word_attribute(attribute.key.as_ref(), b"val", word_prefixes) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn modeled_toggle_attributes<'a>(
    raw_xml: &'a [Vec<u8>],
    positions: &[(u8, usize)],
    slot: u8,
) -> Result<Vec<(std::borrow::Cow<'a, str>, std::borrow::Cow<'a, str>)>> {
    let mut preserved = Vec::new();
    for (raw, position) in raw_xml.iter().zip(positions) {
        if position.0 != slot || !raw_is_modeled_attribute_carrier(*position) {
            continue;
        }
        let mut reader = Reader::from_reader(raw.as_slice());
        let (source, close) = match reader.read_event()? {
            Event::Start(source) => (source, 1),
            Event::Empty(source) => (source, 2),
            _ => continue,
        };
        let prefixes = carrier_word_prefixes(&source)?;
        // Actual reader positions select the borrowed physical header. The
        // UTF8 probe selects borrowing only, it never refuses a legacy input.
        let end = reader.buffer_position() as usize - close;
        let header = &raw[end - source.as_ref().len()..end];
        if let Ok(header) = std::str::from_utf8(header) {
            for attribute in
                quick_xml::events::attributes::Attributes::new(header, source.name().as_ref().len())
            {
                let attribute = attribute?;
                if is_word_attribute(attribute.key.as_ref(), b"val", &prefixes) {
                    continue;
                }
                let name = std::str::from_utf8(attribute.key.0)?;
                let value = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, source.decoder())?;
                preserved.push((std::borrow::Cow::Borrowed(name), value));
            }
        } else {
            // An invalid outer QName was not decoded by the legacy helper.
            // Keep that exact acceptance/error path instead of validating it.
            for attribute in source.attributes() {
                let attribute = attribute?;
                if is_word_attribute(attribute.key.as_ref(), b"val", &prefixes) {
                    continue;
                }
                let name = std::str::from_utf8(attribute.key.as_ref())?.to_owned();
                let value = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, source.decoder())?
                    .into_owned();
                preserved.push((
                    std::borrow::Cow::Owned(name),
                    std::borrow::Cow::Owned(value),
                ));
            }
        }
    }
    Ok(preserved)
}

/// The prefixes that name WordprocessingML on `carrier`, the retained source
/// of a modelled Word element: `w`, those it declares, and its own prefix
/// when it does not declare it, since the reader took it for a Word element
/// under the binding of its part (a document that names WordprocessingML
/// `ox0` carries `ox0:cnfStyle ox0:val="..."`).
fn carrier_word_prefixes(carrier: &BytesStart<'_>) -> Result<Vec<String>> {
    let mut prefixes = word_prefixes_at(carrier, &["w".to_owned()])?.into_owned();
    let name = carrier.name();
    if let Some(prefix) = name.prefix() {
        let prefix = std::str::from_utf8(prefix.as_ref())?;
        let declaration = format!("xmlns:{prefix}");
        let declared = carrier.attributes().any(|attribute| {
            attribute.is_ok_and(|attribute| attribute.key.as_ref() == declaration.as_bytes())
        });
        if !declared && !prefixes.iter().any(|known| known == prefix) {
            prefixes.push(prefix.to_owned());
        }
    }
    Ok(prefixes)
}

pub(crate) fn word_prefixes_at<'scope>(
    start: &BytesStart<'_>,
    inherited: &'scope [String],
) -> Result<std::borrow::Cow<'scope, [String]>> {
    let mut prefixes: std::borrow::Cow<'_, [String]> = std::borrow::Cow::Borrowed(inherited);
    for attribute in start.attributes() {
        let attribute = attribute?;
        let name = attribute.key.as_ref();
        let prefix = if name == b"xmlns" {
            b"".as_slice()
        } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
            prefix
        } else {
            continue;
        };
        let prefix = std::str::from_utf8(prefix)?.to_string();
        let prefixes = prefixes.to_mut();
        prefixes.retain(|candidate| candidate != &prefix);
        let value =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?;
        if value.as_bytes() == W_NS.as_bytes() {
            prefixes.push(prefix);
        }
    }
    Ok(prefixes)
}

fn is_word_name(name: &[u8], word_prefixes: &[String]) -> bool {
    let Some(separator) = name.iter().position(|byte| *byte == b':') else {
        return word_prefixes.iter().any(String::is_empty);
    };
    word_prefixes
        .iter()
        .any(|prefix| prefix.as_bytes() == &name[..separator])
}

pub(crate) fn is_word_element(name: &[u8], local: &[u8], word_prefixes: &[String]) -> bool {
    matches_local_name(name, local) && is_word_name(name, word_prefixes)
}

pub(crate) fn is_word_attribute(key: &[u8], local: &[u8], word_prefixes: &[String]) -> bool {
    let Some(separator) = key.iter().position(|byte| *byte == b':') else {
        return false;
    };
    key.get(separator + 1..) == Some(local)
        && word_prefixes
            .iter()
            .any(|prefix| prefix.as_bytes() == &key[..separator])
}

/// The `w:val` attribute of `e`, read by
/// [`oxoffice_core::xml::attribute_text`], so that a caption or a style id
/// holding `&` or `<` reads back as it was written.
pub(crate) fn get_word_val_attr(
    e: &BytesStart,
    word_prefixes: &[String],
) -> Result<Option<String>> {
    for attr in e.attributes() {
        let attr = attr?;
        if is_word_attribute(attr.key.as_ref(), b"val", word_prefixes) {
            return Ok(Some(
                oxoffice_core::xml::attribute_text(&attr)?.into_owned(),
            ));
        }
    }
    Ok(None)
}

/// Parse an integer measurement attribute, such as an `ST_TwipsMeasure`,
/// `ST_SignedTwipsMeasure` or `ST_HpsMeasure`, tolerating a decimal.
///
/// The schemas allow only an integer, but Google Docs writes a decimal part
/// (`4320.0`, `2210.0000000000005`), and LibreOffice and Word open it. A
/// decimal is read as the nearest integer, half away from zero, decided on
/// the decimal digits themselves so no float rounding intervenes. An
/// exponent, a non-finite spelling, a missing digit, or an exact value
/// outside `T` is refused with an error naming the element, the attribute
/// and the value.
pub(crate) fn parse_integer_measurement<T: TryFrom<i64>>(
    element: &[u8],
    attribute: &[u8],
    value: &str,
) -> Result<T> {
    let error = |problem: &str| {
        OxmlError::InvalidValue(format!(
            "{}/@{} {value:?} {problem}",
            String::from_utf8_lossy(element),
            String::from_utf8_lossy(attribute),
        ))
    };
    let (negative, unsigned) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, "0"));
    if whole.is_empty()
        || fraction.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(error("is not an integer measurement"));
    }

    // Eighteen digits cannot overflow an i64 and exceed every target type.
    let whole = whole.trim_start_matches('0');
    if whole.len() > 18 {
        return Err(error("is out of range"));
    }
    let whole = whole
        .bytes()
        .fold(0i64, |sum, digit| sum * 10 + i64::from(digit - b'0'));
    let signed = |magnitude: i64| if negative { -magnitude } else { magnitude };
    // The exact value must be in range, not only its rounding, so a
    // fraction counts as the next integer away from zero here.
    let exceeds = whole + i64::from(fraction.bytes().any(|digit| digit != b'0'));
    if T::try_from(signed(exceeds)).is_err() {
        return Err(error("is out of range"));
    }
    let rounded = whole + i64::from(fraction.as_bytes()[0] >= b'5');
    T::try_from(signed(rounded)).map_err(|_| error("is out of range"))
}

/// Twips per unit of an `ST_UniversalMeasure`, as numerator over
/// denominator.
pub(crate) const UNIVERSAL_UNITS: [(&str, i128, i128); 6] = [
    ("in", 1440, 1),
    ("cm", 144000, 254),
    ("mm", 14400, 254),
    ("pt", 20, 1),
    ("pc", 240, 1),
    ("pi", 240, 1),
];

/// Read an `ST_TwipsMeasure` or `ST_SignedTwipsMeasure`: twips, written as
/// an integer as [`parse_integer_measurement`] reads it or, in
/// Transitional, as a universal measure such as `"2in"` or `"1.5cm"`,
/// rounded to the nearest twip. A value outside `T` is refused with an
/// error naming the element, the attribute and the value.
pub(crate) fn parse_twips_measure<T: TryFrom<i64>>(
    element: &[u8],
    attribute: &[u8],
    value: &str,
) -> Result<T> {
    parse_universal_measure(element, attribute, value, 1)
}

/// Read an `ST_HpsMeasure` or `ST_SignedHpsMeasure`: half-points, written as
/// an integer or, in Transitional, a universal measure such as `"12pt"`,
/// rounded to the nearest half-point.
pub(crate) fn parse_half_point_measure<T: TryFrom<i64>>(
    element: &[u8],
    attribute: &[u8],
    value: &str,
) -> Result<T> {
    parse_universal_measure(element, attribute, value, 10)
}

/// An integer as [`parse_integer_measurement`] reads it, or a universal
/// measure in twips divided by `twips_per_unit`.
fn parse_universal_measure<T: TryFrom<i64>>(
    element: &[u8],
    attribute: &[u8],
    value: &str,
    twips_per_unit: i128,
) -> Result<T> {
    let Some((number, numerator, denominator)) =
        UNIVERSAL_UNITS
            .iter()
            .find_map(|(unit, numerator, denominator)| {
                value
                    .strip_suffix(unit)
                    .map(|number| (number, *numerator, *denominator))
            })
    else {
        return parse_integer_measurement(element, attribute, value);
    };
    let error = |problem: &str| {
        OxmlError::InvalidValue(format!(
            "{}/@{} {value:?} {problem}",
            String::from_utf8_lossy(element),
            String::from_utf8_lossy(attribute),
        ))
    };
    let scaled = scaled_decimal(number, numerator, denominator * twips_per_unit)
        .ok_or_else(|| error("is not a measurement or is out of range"))?;
    T::try_from(i64::from(scaled)).map_err(|_| error("is out of range"))
}

/// `-?[0-9]+(\.[0-9]+)?` times `numerator / denominator`, rounded half away
/// from zero in decimal arithmetic from the first eighteen fraction digits,
/// if it fits an `i32`.
pub(crate) fn scaled_decimal(number: &str, numerator: i128, denominator: i128) -> Option<i32> {
    let (negative, unsigned) = match number.strip_prefix('-') {
        Some(unsigned) => (true, unsigned),
        None => (false, number),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || (unsigned.contains('.') && fraction.is_empty())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    // The smallest scale is 2 half-points a point, so eleven whole digits never
    // fit an i32; with eighteen fraction digits the products stay well
    // inside an i128, and checked arithmetic refuses anything else.
    let whole = whole.trim_start_matches('0');
    let fraction = &fraction[..fraction.len().min(18)];
    if whole.len() > 10 {
        return None;
    }
    // "0", "00" or "-0" leave no digit once the zeros are trimmed.
    let digits = format!("{whole}{fraction}");
    let digits: i128 = if digits.is_empty() {
        0
    } else {
        digits.parse().ok()?
    };
    let scale = 10i128
        .checked_pow(fraction.len() as u32)?
        .checked_mul(denominator)?;
    let product = digits.checked_mul(numerator)?;
    let rounded = product
        .checked_mul(2)?
        .checked_add(scale)?
        .checked_div(scale.checked_mul(2)?)?;
    let signed = if negative { -rounded } else { rounded };
    i32::try_from(signed).ok()
}

pub(crate) fn parse_word_toggle(e: &BytesStart, word_prefixes: &[String]) -> Result<bool> {
    let val = get_word_val_attr(e, word_prefixes)?;
    Ok(ST_OnOff::from_str_or_default(val.as_deref()).is_on())
}

/// Write a toggle element.
pub(crate) fn write_toggle<W: std::io::Write>(
    writer: &mut Writer<W>,
    tag: &str,
    value: bool,
) -> Result<()> {
    write_toggle_core(writer, BytesStart::new(tag), "w", value)
}

pub(crate) fn write_toggle_source<W: std::io::Write>(
    writer: &mut Writer<W>,
    local: &str,
    value: bool,
    source_context: Option<&[(&str, &str)]>,
) -> Result<()> {
    let (start, prefix) = crate::text::generated_word_start(
        BytesStart::new(format!("w:{local}")),
        &[],
        crate::text::XmlWriteRole::Source,
        source_context,
    )?;
    write_toggle_core(writer, start, &prefix, value)
}

fn write_toggle_core<W: std::io::Write>(
    writer: &mut Writer<W>,
    mut start: BytesStart<'_>,
    word_prefix: &str,
    value: bool,
) -> Result<()> {
    if value {
        writer.write_event(Event::Empty(start))?;
    } else {
        start.push_attribute((format!("{word_prefix}:val").as_str(), "false"));
        writer.write_event(Event::Empty(start))?;
    }
    Ok(())
}

// These sets are owned by the corresponding typed shading/border adapters.
pub(crate) fn source_shading_attributes() -> &'static [&'static [u8]] {
    &[
        b"val",
        b"color",
        b"fill",
        b"themeColor",
        b"themeTint",
        b"themeShade",
        b"themeFill",
        b"themeFillTint",
        b"themeFillShade",
    ]
}
pub(crate) fn source_border_attributes() -> &'static [&'static [u8]] {
    &[b"val", b"sz", b"space", b"color"]
}

type ResidualName = (Option<String>, String);
struct ResidualNode {
    name: ResidualName,
    known: bool,
    text: bool,
    attributes: Vec<String>,
    scope: std::collections::BTreeMap<String, String>,
    range: std::ops::Range<usize>,
    opening_end: usize,
    items: Vec<ResidualItem>,
}
enum ResidualItem {
    Node(usize),
    Event { raw: Vec<u8>, modeled: bool },
}

/// Return the source facts omitted from the existing typed Word model.
///
/// This read-only bridge serves comparison internals. Its strings are an opaque
/// internal identity representation, not an authoring or persistent interchange
/// format. Modeled values retain their existing parser normalization. Opaque
/// attributes, payloads, events, order and effective context remain source facts,
/// except that an equation the typed reader takes reads as its model writes it
/// (`OfficeMath::canonical_xml`), in its effective context.
#[doc(hidden)]
pub fn protected_source_residual(
    xml: &[u8],
    inherited_bindings: &[(String, String)],
) -> Result<Vec<String>> {
    let mut wrapper = BytesStart::new("rdocxResidual");
    for (prefix, uri) in inherited_bindings {
        let name = if prefix == "xmlns" || prefix.starts_with("xmlns:") {
            prefix.clone()
        } else if prefix.is_empty() {
            "xmlns".to_owned()
        } else {
            format!("xmlns:{prefix}")
        };
        wrapper.push_attribute((name.as_str(), uri.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Start(wrapper))?;
    writer.get_mut().extend_from_slice(xml);
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new(
        "rdocxResidual",
    )))?;
    let source = writer.into_inner();
    let mut reader = quick_xml::NsReader::from_reader(source.as_slice());
    reader.config_mut().trim_text(false);
    let mut nodes = Vec::<ResidualNode>::new();
    // The prefixes each node's subtree names: the only bindings of its scope
    // that can change what the node says.
    let mut used = Vec::<std::collections::BTreeSet<String>>::new();
    let mut stack = Vec::<usize>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event()?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let namespace = residual_namespace(namespace)?;
                let local = std::str::from_utf8(element.local_name().as_ref())?.to_owned();
                let parent = stack.last().copied();
                let parent_local = parent
                    .filter(|index| {
                        *index != 0
                            && nodes[*index].known
                            && nodes[*index].name.0.as_deref() == Some(W_NS)
                    })
                    .map(|index| nodes[index].name.1.as_bytes())
                    .unwrap_or_default();
                let root = parent == Some(0);
                let owned = if nodes.is_empty() {
                    Some(&[][..])
                } else if namespace.as_deref() == Some(W_NS) && (root || !parent_local.is_empty()) {
                    residual_owned_attributes(parent_local, local.as_bytes(), root)
                } else {
                    None
                };
                let known = owned.is_some();
                let text = known
                    && matches!(
                        local.as_str(),
                        "t" | "delText" | "instrText" | "delInstrText"
                    );
                let mut attributes = Vec::new();
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    if attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:")
                    {
                        continue;
                    }
                    let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                    let namespace = residual_namespace(namespace)?;
                    let decoded = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
                        .into_owned();
                    let modeled = if namespace.as_deref() == Some(W_NS) {
                        owned.is_some_and(|keys| keys.contains(&name.as_ref()))
                            && residual_owned_value(
                                parent_local,
                                local.as_bytes(),
                                name.as_ref(),
                                &decoded,
                            )
                    } else if namespace.as_deref() == Some("http://www.w3.org/XML/1998/namespace") {
                        // The whitespace rule of a run's modeled text, field
                        // instructions included.
                        name.as_ref() == b"space"
                            && matches!(
                                local.as_str(),
                                "t" | "delText" | "instrText" | "delInstrText"
                            )
                            && parent_local == b"r"
                            && matches!(decoded.as_str(), "default" | "preserve")
                    } else {
                        namespace.as_deref() == Some(crate::namespace::R_NS)
                            && name.as_ref() == b"id"
                            && matches!(
                                (parent_local, local.as_str()),
                                (b"sectPr", "headerReference" | "footerReference")
                                    | (b"p", "hyperlink")
                            )
                    };
                    if !modeled || !known {
                        let identity = format!(
                            "{namespace:?}:{:?}:{:?}:{decoded:?}",
                            name.as_ref(),
                            attribute.value.as_ref()
                        );
                        attributes.push(if known {
                            identity
                        } else {
                            // The opaque root keeps every non-declaration attribute,
                            // including its lexical name and encoded value.
                            format!("{:?}:{identity}", attribute.key.as_ref())
                        });
                    }
                }
                if known {
                    attributes.sort();
                }
                let mut scope = std::collections::BTreeMap::new();
                for (prefix, uri) in reader.resolver().bindings() {
                    let prefix = match prefix {
                        quick_xml::name::PrefixDeclaration::Default => String::new(),
                        quick_xml::name::PrefixDeclaration::Named(prefix) => {
                            std::str::from_utf8(prefix)?.to_owned()
                        }
                    };
                    let uri = quick_xml::escape::unescape(std::str::from_utf8(uri.as_ref())?)
                        .map_err(|error| OxmlError::InvalidValue(error.to_string()))?
                        .into_owned();
                    if prefix != "xml" && !(prefix.is_empty() && uri.is_empty()) {
                        scope.insert(prefix, uri);
                    }
                }
                used.push(residual_used_prefixes(&element, reader.resolver())?);
                let index = nodes.len();
                let end = reader.buffer_position() as usize;
                nodes.push(ResidualNode {
                    name: (namespace, local),
                    known,
                    text,
                    attributes,
                    scope,
                    range: before..end,
                    opening_end: end,
                    items: Vec::new(),
                });
                if let Some(parent) = parent {
                    nodes[parent].items.push(ResidualItem::Node(index));
                }
                if !empty {
                    stack.push(index);
                } else if let Some(parent) = parent {
                    let named = used[index].clone();
                    used[parent].extend(named);
                }
            }
            Event::End(_) => {
                let index = stack
                    .pop()
                    .ok_or_else(|| OxmlError::InvalidValue("unbalanced residual source".into()))?;
                nodes[index].range.end = reader.buffer_position() as usize;
                if let Some(parent) = stack.last().copied() {
                    let named = used[index].clone();
                    used[parent].extend(named);
                }
            }
            Event::Eof => break,
            event => {
                if let Some(index) = stack.last().copied() {
                    let modeled = nodes[index].text
                        && matches!(
                            event,
                            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_)
                        );
                    let raw = source[before..reader.buffer_position() as usize].to_vec();
                    nodes[index]
                        .items
                        .push(ResidualItem::Event { raw, modeled });
                }
            }
        }
    }
    if !stack.is_empty() {
        return Err(OxmlError::InvalidValue("unclosed residual source".into()));
    }
    // A binding no name of the subtree uses says nothing about it: a save
    // that drops or adds an unused root declaration changes no fact.
    for (node, named) in nodes.iter_mut().zip(&used) {
        node.scope.retain(|prefix, _| named.contains(prefix));
    }
    let mut output = Vec::new();
    if !nodes.is_empty() {
        residual_emit_children(0, &nodes, &source, &[], &mut output);
    }
    Ok(output)
}

/// The prefixes one element names: its own, its attributes' and those the
/// values of markup-compatibility and `xsi:type` attributes name.
fn residual_used_prefixes(
    element: &BytesStart<'_>,
    resolver: &quick_xml::name::NamespaceResolver,
) -> Result<std::collections::BTreeSet<String>> {
    let prefix_of = |name: &[u8]| -> Result<String> {
        Ok(name
            .iter()
            .position(|byte| *byte == b':')
            .map(|at| std::str::from_utf8(&name[..at]))
            .transpose()?
            .unwrap_or_default()
            .to_owned())
    };
    let mut named = std::collections::BTreeSet::from([prefix_of(element.name().as_ref())?]);
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            continue;
        }
        let prefix = prefix_of(key)?;
        if prefix.is_empty() {
            // `mc:Choice` names the prefixes it requires in an unprefixed
            // attribute.
            if key == b"Requires" {
                named.extend(
                    oxoffice_core::xml::attribute_text(&attribute)?
                        .split_whitespace()
                        .map(str::to_owned),
                );
            }
            continue;
        }
        let (namespace, local) = resolver.resolve_attribute(attribute.key);
        let namespace = residual_namespace(namespace)?;
        let value =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?;
        named.extend(
            crate::text::root_attribute_value_prefixes(
                namespace.as_deref(),
                local.as_ref(),
                &value,
            )
            .into_iter()
            .map(str::to_owned),
        );
        named.insert(prefix);
    }
    Ok(named)
}

/// The canonical writing of the equation an opaque node holds, read in the
/// node's scope, when the typed reader takes it.
fn residual_equation(node: &ResidualNode, source: &[u8]) -> Option<Vec<u8>> {
    if node.name.0.as_deref() != Some(crate::namespace::M_NS)
        || !matches!(node.name.1.as_str(), "oMath" | "oMathPara")
    {
        return None;
    }
    let scope = node
        .scope
        .iter()
        .map(|(prefix, uri)| (prefix.clone(), uri.clone()))
        .collect::<Vec<_>>();
    crate::math::OfficeMath::from_raw(source.get(node.range.clone())?, &scope)
        .ok()
        .flatten()?
        .canonical_xml()
        .ok()
}

fn residual_namespace(namespace: quick_xml::name::ResolveResult<'_>) -> Result<Option<String>> {
    match namespace {
        quick_xml::name::ResolveResult::Unbound => Ok(None),
        quick_xml::name::ResolveResult::Bound(uri) => Ok(Some(
            quick_xml::escape::unescape(std::str::from_utf8(uri.as_ref())?)
                .map_err(|error| OxmlError::InvalidValue(error.to_string()))?
                .into_owned(),
        )),
        quick_xml::name::ResolveResult::Unknown(prefix) => Ok(Some(format!("unbound:{prefix:?}"))),
    }
}

fn residual_revision_root(local: &[u8]) -> bool {
    matches!(
        local,
        b"ins"
            | b"del"
            | b"moveFrom"
            | b"moveTo"
            | b"pPrChange"
            | b"rPrChange"
            | b"tblPrChange"
            | b"sectPrChange"
            | b"tblGridChange"
            | b"numberingChange"
    )
}

fn residual_owned_attributes(
    parent: &[u8],
    local: &[u8],
    root: bool,
) -> Option<&'static [&'static [u8]]> {
    // Revision subtrees remain exact source, independently of modeled formatting.
    if residual_revision_root(local) {
        return None;
    }
    if root
        && matches!(
            local,
            b"p" | b"r"
                | b"tbl"
                | b"tr"
                | b"tc"
                | b"sdt"
                | b"pPr"
                | b"rPr"
                | b"tblPr"
                | b"trPr"
                | b"tcPr"
                | b"sdtPr"
                | b"sectPr"
        )
    {
        return Some(&[]);
    }
    crate::paragraph_properties::source_modeled_attributes(parent, local)
        .or_else(|| crate::run_properties::source_modeled_attributes(parent, local))
        .or_else(|| crate::table::source_modeled_attributes(parent, local))
        .or_else(|| crate::document::source_modeled_attributes(parent, local))
        .or_else(|| crate::content_control::source_modeled_attributes(parent, local))
        .or_else(|| crate::text::source_modeled_attributes(parent, local))
        .or_else(|| match (parent, local) {
            (b"p", b"pPr" | b"r" | b"sdt")
            | (b"r", b"rPr")
            | (b"tbl", b"tblPr" | b"tblGrid" | b"tr" | b"sdt")
            | (b"tr", b"tblPrEx" | b"trPr" | b"tc" | b"sdt")
            | (b"tc", b"tcPr" | b"p" | b"tbl" | b"sdt")
            | (b"sdt", b"sdtPr" | b"sdtContent")
            | (b"sdtContent", b"p" | b"r" | b"tbl" | b"tr" | b"tc" | b"sdt")
            | (b"hyperlink", b"r" | b"sdt") => Some(&[]),
            (b"numPr", b"ilvl" | b"numId") => Some(&[b"val"]),
            (b"tabs", b"tab") => Some(crate::numbering::property_attributes(
                crate::numbering::PropertyKind::Paragraph,
                local,
            )),
            (
                b"pBdr" | b"pgBorders",
                b"top" | b"left" | b"start" | b"bottom" | b"right" | b"end" | b"between" | b"bar",
            ) => Some(source_border_attributes()),
            _ => None,
        })
}

fn residual_owned_value(parent: &[u8], local: &[u8], key: &[u8], value: &str) -> bool {
    if local == b"shd"
        && matches!(
            key,
            b"themeTint" | b"themeShade" | b"themeFillTint" | b"themeFillShade"
        )
        && parse_uchar_hex(value).is_none()
    {
        return false;
    }
    if key == b"val"
        && (matches!((parent, local), (b"rPr", b"bdr" | b"border"))
            || matches!(
                parent,
                b"pBdr" | b"pgBorders" | b"tblBorders" | b"tcBorders"
            ))
        && crate::shared::ST_Border::from_str(value).is_err()
    {
        return false;
    }
    crate::paragraph_properties::source_value_is_modeled(parent, local, key, value)
        && crate::run_properties::source_value_is_modeled(parent, local, key, value)
        && crate::table::source_value_is_modeled(parent, local, key, value)
        && crate::document::source_value_is_modeled(parent, local, key, value)
        && crate::text::source_value_is_modeled(parent, local, key, value)
}

fn residual_default_container(index: usize, nodes: &[ResidualNode]) -> bool {
    let node = &nodes[index];
    node.known && node.attributes.is_empty()
        && matches!(node.name.1.as_str(), "pPr" | "rPr" | "tblPr" | "tblPrEx" | "trPr" | "tcPr" | "numPr" | "tabs" | "pBdr" | "tblBorders" | "tcBorders" | "tcMar" | "tblCellMar" | "sdtPr" | "tblGrid")
        && node.items.iter().all(|item| matches!(item, ResidualItem::Node(child) if residual_default_container(*child, nodes)))
}

fn residual_emit_children(
    index: usize,
    nodes: &[ResidualNode],
    source: &[u8],
    path: &[String],
    output: &mut Vec<String>,
) {
    let mut occurrences = std::collections::BTreeMap::<ResidualName, usize>::new();
    let mut previous = "start".to_owned();
    let mut event_ordinal = 0usize;
    for item in &nodes[index].items {
        match item {
            ResidualItem::Node(child) => {
                let node = &nodes[*child];
                let default = residual_default_container(*child, nodes);
                let occurrence = occurrences.entry(node.name.clone()).or_default();
                let slot = format!("{:?}#{occurrence}", node.name);
                if !default {
                    *occurrence += 1;
                }
                let mut child_path = path.to_vec();
                child_path.push(slot.clone());
                if !node.known {
                    if node.name.0.as_deref() == Some(W_NS)
                        && residual_revision_root(node.name.1.as_bytes())
                    {
                        output.push(format!(
                            "revision:{child_path:?}:after:{previous}:{:?}:{:?}",
                            &source[node.range.clone()],
                            node.scope
                        ));
                    } else if let Some(equation) = residual_equation(node, source) {
                        // An equation reads as its model writes it: a save that
                        // writes it again, its declarations elsewhere or a
                        // default spelled out, changes no fact. The bindings its
                        // names resolve to stay one (those `mc:Choice` requires
                        // included, which its model's writing does not declare).
                        output.push(format!(
                            "equation:{child_path:?}:after:{previous}:{equation:?}:{:?}",
                            node.scope
                        ));
                    } else {
                        // Only the opaque root opening admits namespace declaration
                        // plumbing. Its actual expanded identity, all other attributes,
                        // effective context and shape remain checked. The remainder,
                        // including descendant-local declarations, is untouched source.
                        output.push(format!(
                            "opaque:{child_path:?}:after:{previous}:{:?}:{:?}:empty={}:{:?}",
                            node.attributes,
                            node.scope,
                            node.opening_end == node.range.end,
                            &source[node.opening_end..node.range.end]
                        ));
                    }
                } else {
                    if !node.attributes.is_empty() {
                        output.push(format!(
                            "attributes:{child_path:?}:{:?}:{:?}",
                            node.attributes, node.scope
                        ));
                    }
                    residual_emit_children(*child, nodes, source, &child_path, output);
                }
                if !default {
                    previous = slot;
                }
            }
            ResidualItem::Event { raw, modeled } if !modeled => {
                output.push(format!(
                    "event:{path:?}:after:{previous}:#{event_ordinal}:{raw:?}:{:?}",
                    nodes[index].scope
                ));
                event_ordinal += 1;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The residual of a paragraph whose root binds `x` to `x_uri` and `y`
    /// to `y_uri`, with an alternate content requiring `x`.
    fn residual(x_uri: &str, y_uri: &str) -> Vec<String> {
        let xml = format!(
            concat!(
                r#"<w:p xmlns:w="{w}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:x="{x}" xmlns:y="{y}">"#,
                r#"<w:r><mc:AlternateContent><mc:Choice Requires="x"><w:t>a</w:t></mc:Choice>"#,
                r#"<mc:Fallback><w:t>b</w:t></mc:Fallback></mc:AlternateContent></w:r></w:p>"#,
            ),
            w = W_NS,
            x = x_uri,
            y = y_uri,
        );
        protected_source_residual(xml.as_bytes(), &[]).unwrap()
    }

    /// The residual of a paragraph whose equation holds an alternate content
    /// requiring `x`, bound to `x_uri` on the paragraph root, the equation
    /// declaring `again` on itself.
    fn equation_residual(x_uri: &str, again: &str) -> Vec<String> {
        let xml = format!(
            concat!(
                r#"<w:p xmlns:w="{w}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:m="{m}" xmlns:x="{x}">"#,
                r#"<m:oMath{again}><m:r><m:t>a</m:t></m:r><mc:AlternateContent><mc:Choice Requires="x">"#,
                r#"<m:r><m:t>b</m:t></m:r></mc:Choice></mc:AlternateContent></m:oMath></w:p>"#,
            ),
            w = W_NS,
            m = crate::namespace::M_NS,
            x = x_uri,
            again = again,
        );
        protected_source_residual(xml.as_bytes(), &[]).unwrap()
    }

    #[test]
    fn an_equation_residual_keeps_what_its_names_and_requirements_resolve_to() {
        let again = format!(r#" xmlns:m="{}" xmlns:w="{W_NS}""#, crate::namespace::M_NS);
        let residual = equation_residual("urn:x", "");
        assert!(residual.iter().any(|fact| fact.starts_with("equation:")));
        assert_eq!(residual, equation_residual("urn:x", &again));
        assert_ne!(
            equation_residual("urn:a", ""),
            equation_residual("urn:b", "")
        );
    }

    #[test]
    fn a_residual_keeps_the_bindings_its_names_and_requirements_use() {
        // A binding nothing names is the part's: another root may drop it.
        assert_eq!(residual("urn:x", "urn:a"), residual("urn:x", "urn:b"));
        // A binding `Requires` names is read.
        assert_ne!(residual("urn:a", "urn:y"), residual("urn:b", "urn:y"));
    }
}
