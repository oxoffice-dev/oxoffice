//! Theme parsing: extracts color scheme and font scheme from theme1.xml.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::Result;
use crate::namespace::matches_local_name;

/// Parsed theme information from `word/theme/theme1.xml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Theme {
    /// Theme color scheme (dk1, dk2, lt1, lt2, accent1-6, hlink, folHlink).
    pub colors: ThemeColors,
    /// Major font family (typically used for headings).
    pub major_font: Option<String>,
    /// Minor font family (typically used for body text).
    pub minor_font: Option<String>,
}

/// The 12 standard theme colors, stored as 6-character hex RGB strings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeColors {
    pub dk1: Option<String>,
    pub dk2: Option<String>,
    pub lt1: Option<String>,
    pub lt2: Option<String>,
    pub accent1: Option<String>,
    pub accent2: Option<String>,
    pub accent3: Option<String>,
    pub accent4: Option<String>,
    pub accent5: Option<String>,
    pub accent6: Option<String>,
    pub hlink: Option<String>,
    pub fol_hlink: Option<String>,
}

impl ThemeColors {
    /// Look up a theme color by its OOXML name (e.g., "accent1", "dark1").
    pub fn get(&self, name: &str) -> Option<&str> {
        match name {
            "dark1" | "dk1" => self.dk1.as_deref(),
            "dark2" | "dk2" => self.dk2.as_deref(),
            "light1" | "lt1" => self.lt1.as_deref(),
            "light2" | "lt2" => self.lt2.as_deref(),
            "accent1" => self.accent1.as_deref(),
            "accent2" => self.accent2.as_deref(),
            "accent3" => self.accent3.as_deref(),
            "accent4" => self.accent4.as_deref(),
            "accent5" => self.accent5.as_deref(),
            "accent6" => self.accent6.as_deref(),
            "hlink" | "hyperlink" => self.hlink.as_deref(),
            "folHlink" | "followedHyperlink" => self.fol_hlink.as_deref(),
            // Word also uses "text1" = dk1, "text2" = dk2, "background1" = lt1, "background2" = lt2
            "text1" => self.dk1.as_deref(),
            "text2" => self.dk2.as_deref(),
            "background1" | "bg1" => self.lt1.as_deref(),
            "background2" | "bg2" => self.lt2.as_deref(),
            _ => None,
        }
    }
}

impl Theme {
    /// Parse theme from XML bytes (the content of `word/theme/theme1.xml`).
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut theme = Theme::default();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    if matches_local_name(name.as_ref(), b"clrScheme") {
                        parse_color_scheme(&mut reader, &mut theme.colors)?;
                    } else if matches_local_name(name.as_ref(), b"majorFont") {
                        theme.major_font = parse_font_scheme(&mut reader, b"majorFont")?;
                    } else if matches_local_name(name.as_ref(), b"minorFont") {
                        theme.minor_font = parse_font_scheme(&mut reader, b"minorFont")?;
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(theme)
    }
}

/// Parse the `<a:clrScheme>` element to extract theme colors.
fn parse_color_scheme(reader: &mut Reader<&[u8]>, colors: &mut ThemeColors) -> Result<()> {
    let mut buf = Vec::new();
    let mut current_slot: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let local = local_name(name.as_ref());
                // Color slot elements: dk1, dk2, lt1, lt2, accent1-6, hlink, folHlink
                match local {
                    b"dk1" | b"dk2" | b"lt1" | b"lt2" | b"accent1" | b"accent2" | b"accent3"
                    | b"accent4" | b"accent5" | b"accent6" | b"hlink" | b"folHlink" => {
                        current_slot = Some(std::str::from_utf8(local).unwrap_or("").to_string());
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = e.name();
                let local = local_name(name.as_ref());
                // Inside a color slot, look for <a:srgbClr val="RRGGBB"/> or <a:sysClr lastClr="RRGGBB"/>
                if let Some(ref slot) = current_slot {
                    let color = if matches_local_name(name.as_ref(), b"srgbClr") {
                        get_attr(e, b"val")
                    } else if matches_local_name(name.as_ref(), b"sysClr") {
                        // System color: use lastClr (the resolved value) if available
                        get_attr(e, b"lastClr").or_else(|| get_attr(e, b"val"))
                    } else {
                        None
                    };

                    if let Some(hex) = color {
                        match slot.as_str() {
                            "dk1" => colors.dk1 = Some(hex),
                            "dk2" => colors.dk2 = Some(hex),
                            "lt1" => colors.lt1 = Some(hex),
                            "lt2" => colors.lt2 = Some(hex),
                            "accent1" => colors.accent1 = Some(hex),
                            "accent2" => colors.accent2 = Some(hex),
                            "accent3" => colors.accent3 = Some(hex),
                            "accent4" => colors.accent4 = Some(hex),
                            "accent5" => colors.accent5 = Some(hex),
                            "accent6" => colors.accent6 = Some(hex),
                            "hlink" => colors.hlink = Some(hex),
                            "folHlink" => colors.fol_hlink = Some(hex),
                            _ => {}
                        }
                    }
                } else {
                    // Top-level color slot with inline color (dk1, etc.)
                    match local {
                        b"dk1" | b"dk2" | b"lt1" | b"lt2" | b"accent1" | b"accent2"
                        | b"accent3" | b"accent4" | b"accent5" | b"accent6" | b"hlink"
                        | b"folHlink" => {
                            // Shouldn't happen (they're Start not Empty), but handle anyway
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.name();
                let local = local_name(name.as_ref());
                match local {
                    b"dk1" | b"dk2" | b"lt1" | b"lt2" | b"accent1" | b"accent2" | b"accent3"
                    | b"accent4" | b"accent5" | b"accent6" | b"hlink" | b"folHlink" => {
                        current_slot = None;
                    }
                    b"clrScheme" => break,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.into()),
            _ => {}
        }
        buf.clear();
    }

    Ok(())
}

/// Parse a `<a:majorFont>` or `<a:minorFont>` element to extract the latin typeface.
fn parse_font_scheme(reader: &mut Reader<&[u8]>, end_tag: &[u8]) -> Result<Option<String>> {
    let mut buf = Vec::new();
    let mut latin_font = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) => {
                if matches_local_name(e.name().as_ref(), b"latin") {
                    latin_font = get_attr(e, b"typeface");
                }
            }
            Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), end_tag) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.into()),
            _ => {}
        }
        buf.clear();
    }

    Ok(latin_font)
}

/// Get the local name (after namespace prefix) from a qualified name.
fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().position(|&b| b == b':') {
        Some(pos) => &qname[pos + 1..],
        None => qname,
    }
}

/// Extract a named attribute value from an element.
fn get_attr(e: &quick_xml::events::BytesStart, attr_name: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        let key = attr.key.as_ref();
        let local = local_name(key);
        if local == attr_name {
            return std::str::from_utf8(&attr.value).ok().map(|s| s.to_string());
        }
    }
    None
}

/// Apply theme tint/shade modifiers to a base color.
///
/// `tint_val` is 0-255 where 255 means full tint (lightest).
/// `shade_val` is 0-255 where 255 means full shade (darkest).
pub fn apply_tint_shade(hex: &str, tint_val: Option<u8>, shade_val: Option<u8>) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return hex.to_string();
    }

    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f64 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f64 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f64 / 255.0;

    let (r, g, b) = if let Some(tint) = tint_val {
        // Tint: mix with white. tint=0 means no change, tint=255 means pure white
        let t = tint as f64 / 255.0;
        (r + (1.0 - r) * t, g + (1.0 - g) * t, b + (1.0 - b) * t)
    } else if let Some(shade) = shade_val {
        // Shade: mix with black. shade=0 means pure black, shade=255 means no change
        let s = shade as f64 / 255.0;
        (r * s, g * s, b * s)
    } else {
        (r, g, b)
    };

    format!(
        "{:02X}{:02X}{:02X}",
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// Project a shared DrawingML theme onto this crate's Word theme.
///
/// The adapter lives here rather than in `oxml-drawing` so the dependency runs
/// one way, from the format crate to the shared crate. Hosting it the other way
/// round made the two publication trains mutually dependent, since
/// `rdocx-layout` already depends on `oxml-layout`. The orphan rule allows this
/// placement because `Theme` is local here.
impl From<&oxml_drawing::theme::CT_OfficeStyleSheet> for Theme {
    fn from(theme: &oxml_drawing::theme::CT_OfficeStyleSheet) -> Self {
        let colours = &theme.theme_elements.color_scheme;
        let fonts = &theme.theme_elements.font_scheme;
        Self {
            colors: ThemeColors {
                dk1: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Dark1)),
                dk2: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Dark2)),
                lt1: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Light1)),
                lt2: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Light2)),
                accent1: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Accent1)),
                accent2: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Accent2)),
                accent3: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Accent3)),
                accent4: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Accent4)),
                accent5: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Accent5)),
                accent6: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Accent6)),
                hlink: legacy_colour(colours.color(oxml_drawing::color::ThemeColorSlot::Hyperlink)),
                fol_hlink: legacy_colour(
                    colours.color(oxml_drawing::color::ThemeColorSlot::FollowedHyperlink),
                ),
            },
            major_font: Some(fonts.major_font.latin.typeface.clone()),
            minor_font: Some(fonts.minor_font.latin.typeface.clone()),
        }
    }
}

fn legacy_colour(colour: &oxml_drawing::color::ColorChoice) -> Option<String> {
    match colour {
        oxml_drawing::color::ColorChoice::Srgb { value, .. } => Some(value.to_string()),
        oxml_drawing::color::ColorChoice::System {
            value, last_color, ..
        } => Some(
            last_color
                .map(|resolved| resolved.to_string())
                .unwrap_or_else(|| value.clone()),
        ),
        oxml_drawing::color::ColorChoice::Scheme { .. }
        | oxml_drawing::color::ColorChoice::Preset { .. } => None,
    }
}
