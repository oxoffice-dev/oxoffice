//! The style of a cell, resolved: its `cellXfs` record with the font, fill,
//! border and number format it points to, colours resolved to RGB through
//! the theme and the indexed palette.
//!
//! The record gives everything (Excel writes complete records); its
//! `apply*` attributes are not read. A missing `s` or one out of range is
//! record 0. A font that leaves out its name, size or colour (Google's
//! fonts without `sz`) takes it from font 0.
//!
//! The theme is read by `oxoffice-drawing`, Strict ones included; a colour
//! of its scheme is its `srgbClr`, transforms applied, or the `lastClr` of
//! its `sysClr`.

use std::collections::HashMap;
use std::sync::Arc;

use oxoffice_drawing::color::{
    ColorChoice, RgbColor, ThemeColorSlot, apply_color_transforms, apply_hls_tint,
};
use oxoffice_drawing::theme::CT_OfficeStyleSheet;
use oxoffice_numfmt::{DateKind, Locale, NumberFormat, builtin_code};
use oxoffice_xlsx_oxml::Element;
use oxoffice_xlsx_oxml::styles::{StyleList, StylesXml, Xf};
use oxoffice_xlsx_oxml::xstring;

/// A colour of a style, resolved to RGB, with where it comes from.
#[derive(Clone, Debug, PartialEq)]
pub struct Color {
    /// The colour shown, `RRGGBB`; `None` for an automatic colour (the
    /// application's own) or one that cannot be resolved.
    pub rgb: Option<String>,
    pub source: ColorSource,
    /// `tint`, -1.0 (black) to 1.0 (white), already applied to `rgb`.
    pub tint: f64,
}

/// Where a colour comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorSource {
    /// `rgb`: the colour itself (its alpha byte is ignored).
    Rgb,
    /// `theme`: a colour of the workbook's theme, by Excel's index: 0
    /// `lt1`, 1 `dk1`, 2 `lt2`, 3 `dk2`, 4 to 9 `accent1` to `accent6`, 10
    /// `hlink`, 11 `folHlink`.
    Theme(u32),
    /// `indexed`: a colour of the palette, the file's `indexedColors` or
    /// the default one; 64 is the system foreground, 65 the background.
    Indexed(u32),
    /// `auto`: the application's own colour.
    Auto,
}

impl ColorSource {
    /// `rgb`, `theme`, `indexed` or `auto`.
    pub fn kind(self) -> &'static str {
        match self {
            ColorSource::Rgb => "rgb",
            ColorSource::Theme(_) => "theme",
            ColorSource::Indexed(_) => "indexed",
            ColorSource::Auto => "auto",
        }
    }

    /// The theme or palette index.
    pub fn index(self) -> Option<u32> {
        match self {
            ColorSource::Theme(index) | ColorSource::Indexed(index) => Some(index),
            _ => None,
        }
    }
}

/// The font of a style.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub name: Option<String>,
    /// In points.
    pub size: Option<f64>,
    pub bold: bool,
    pub italic: bool,
    /// `single`, `double`, `singleAccounting` or `doubleAccounting`.
    pub underline: Option<String>,
    pub strike: bool,
    /// `superscript` or `subscript`.
    pub vertical_align: Option<String>,
    pub color: Option<Color>,
}

/// A stop of a gradient fill.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientStop {
    /// 0.0 to 1.0.
    pub position: f64,
    pub color: Option<Color>,
}

/// The fill of a style.
#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    /// A pattern (`none`, `solid`, `gray125`...) in its foreground colour
    /// over its background colour.
    Pattern {
        pattern: String,
        foreground: Option<Color>,
        background: Option<Color>,
    },
    /// A gradient: `linear` at `degree`, or `path` from the rectangle the
    /// four edges (0.0 to 1.0) give.
    Gradient {
        kind: String,
        degree: f64,
        left: f64,
        right: f64,
        top: f64,
        bottom: f64,
        stops: Vec<GradientStop>,
    },
}

/// A side of a border.
#[derive(Clone, Debug, PartialEq)]
pub struct BorderSide {
    /// `none`, `thin`, `medium`, `thick`, `double`, `dashed`...
    pub style: String,
    pub color: Option<Color>,
}

/// The borders of a style.
#[derive(Clone, Debug, PartialEq)]
pub struct Border {
    pub left: BorderSide,
    pub right: BorderSide,
    pub top: BorderSide,
    pub bottom: BorderSide,
    pub diagonal: BorderSide,
    pub diagonal_up: bool,
    pub diagonal_down: bool,
}

/// The alignment of a style.
#[derive(Clone, Debug, PartialEq)]
pub struct Alignment {
    /// `general`, `left`, `center`, `right`, `fill`, `justify`,
    /// `centerContinuous` or `distributed`.
    pub horizontal: String,
    /// `top`, `center`, `bottom`, `justify` or `distributed`.
    pub vertical: String,
    pub wrap_text: bool,
    pub shrink_to_fit: bool,
    pub indent: u32,
    /// Degrees: 0 to 90 counterclockwise, 91 to 180 clockwise (90 less),
    /// 255 for vertical text.
    pub text_rotation: u32,
    /// 0 from the content, 1 left to right, 2 right to left.
    pub reading_order: u32,
}

/// The protection of a style, which applies when the sheet is protected.
#[derive(Clone, Debug, PartialEq)]
pub struct Protection {
    pub locked: bool,
    pub hidden: bool,
}

/// The style of a cell, resolved. See the [module](self).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedStyle {
    /// The number format code, a built-in id resolved.
    pub number_format: String,
    pub font: Font,
    pub fill: Fill,
    pub border: Border,
    pub alignment: Alignment,
    pub protection: Protection,
    /// The name of the cell style the record follows (`Normal`).
    pub style_name: Option<String>,
}

/// The cell formats of a workbook, each resolved once, and what resolves
/// the ones an edit appends.
#[derive(Clone, Debug)]
pub(crate) struct StyleTable {
    formats: Vec<CellFormat>,
    palette: Option<Palette>,
}

/// A cell format: its style and its number format read.
#[derive(Clone, Debug)]
pub(crate) struct CellFormat {
    pub style: Arc<ResolvedStyle>,
    pub format: NumberFormat,
    pub date_kind: Option<DateKind>,
}

impl StyleTable {
    /// The cell formats of `styles` (none for a workbook without a styles
    /// part), colours resolved through `theme`.
    pub(crate) fn build(styles: Option<&StylesXml>, theme: &ThemeColors) -> Self {
        let Some(styles) = styles else {
            return StyleTable {
                formats: vec![CellFormat::new(ResolvedStyle::default_style())],
                palette: None,
            };
        };
        let palette = Palette::new(styles, theme);
        let resolver = Resolver::new(styles, &palette);
        let mut formats: Vec<CellFormat> = styles
            .cell_xfs()
            .map(|xf| CellFormat::new(resolver.style(xf)))
            .collect();
        if formats.is_empty() {
            formats.push(CellFormat::new(ResolvedStyle::default_style()));
        }
        StyleTable {
            formats,
            palette: Some(palette),
        }
    }

    /// The cell format of record `s`: record 0 when out of range.
    pub(crate) fn get(&self, s: u32) -> &CellFormat {
        self.formats.get(s as usize).unwrap_or(&self.formats[0])
    }

    /// Reads the number format `id` an edit appended with `code`.
    pub(crate) fn add_number_format(&mut self, id: u32, code: &str) {
        if let Some(palette) = &mut self.palette {
            palette
                .number_formats
                .entry(id)
                .or_insert_with(|| code.to_owned());
        }
    }

    /// Resolves cell format `index`, which an edit appended to `styles`.
    pub(crate) fn add_format(&mut self, styles: &StylesXml, index: usize) {
        let Some(palette) = &self.palette else {
            return;
        };
        let Some(xf) = styles.list(StyleList::CellXfs).get(index).map(Xf) else {
            return;
        };
        let format = CellFormat::new(Resolver::new(styles, palette).style(xf));
        match self.formats.get_mut(index) {
            // A part without cell formats read as record 0 the default one.
            Some(slot) => *slot = format,
            None => self.formats.push(format),
        }
    }
}

impl CellFormat {
    fn new(style: ResolvedStyle) -> Self {
        // Excel shows a code it cannot read as General.
        let format =
            NumberFormat::parse(&style.number_format).unwrap_or_else(|_| NumberFormat::general());
        CellFormat {
            date_kind: format.date_kind(),
            format,
            style: Arc::new(style),
        }
    }
}

impl ResolvedStyle {
    /// The style of a workbook without styles.
    fn default_style() -> Self {
        ResolvedStyle {
            number_format: "General".to_owned(),
            font: Font {
                name: None,
                size: None,
                bold: false,
                italic: false,
                underline: None,
                strike: false,
                vertical_align: None,
                color: None,
            },
            fill: Fill::Pattern {
                pattern: "none".to_owned(),
                foreground: None,
                background: None,
            },
            border: Border::from_element(None, &|_| None),
            alignment: Alignment::from_element(None),
            protection: Protection {
                locked: true,
                hidden: false,
            },
            style_name: None,
        }
    }
}

/// The default indexed palette (ECMA-376 Part 1, 18.8.27), indexes 0 to 63.
const INDEXED_COLORS: [&str; 64] = [
    "000000", "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF", "000000",
    "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF", "800000", "008000",
    "000080", "808000", "800080", "008080", "C0C0C0", "808080", "9999FF", "993366", "FFFFCC",
    "CCFFFF", "660066", "FF8080", "0066CC", "CCCCFF", "000080", "FF00FF", "FFFF00", "00FFFF",
    "800080", "800000", "008080", "0000FF", "00CCFF", "CCFFFF", "CCFFCC", "FFFF99", "99CCFF",
    "FF99CC", "CC99FF", "FFCC99", "3366FF", "33CCCC", "99CC00", "FFCC00", "FF9900", "FF6600",
    "666699", "969696", "003366", "339966", "003300", "333300", "993300", "993366", "333399",
    "333333",
];

/// The colours of a theme's colour scheme, by Excel's theme index: light 1
/// first, then dark 1, light 2, dark 2, the six accents and the two links.
pub(crate) type ThemeColors = [Option<RgbColor>; 12];

const THEME_SLOTS: [ThemeColorSlot; 12] = [
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
];

/// The colour scheme of a theme part; a colour it gives otherwise than as
/// RGB or a system colour with its last value stays unresolved.
pub(crate) fn theme_colors(xml: &[u8]) -> Result<ThemeColors, String> {
    let theme = CT_OfficeStyleSheet::from_xml(xml).map_err(|error| error.to_string())?;
    let scheme = &theme.theme_elements.color_scheme;
    Ok(THEME_SLOTS.map(|slot| match scheme.color(slot) {
        ColorChoice::Srgb {
            value, transforms, ..
        } => {
            let resolved = apply_color_transforms(*value, transforms);
            Some(RgbColor::new(resolved.red, resolved.green, resolved.blue))
        }
        ColorChoice::System { last_color, .. } => *last_color,
        _ => None,
    }))
}

/// What resolves colours, style names and number format codes in one
/// styles part: read once, kept to resolve the records an edit appends.
#[derive(Clone, Debug)]
struct Palette {
    theme: ThemeColors,
    indexed: Vec<RgbColor>,
    /// The name of the first cell style of each cell style format.
    style_names: HashMap<u32, String>,
    /// The file's own number format codes, the first of each id.
    number_formats: HashMap<u32, String>,
}

impl Palette {
    fn new(styles: &StylesXml, theme: &ThemeColors) -> Self {
        let own: Vec<RgbColor> = styles
            .colors()
            .and_then(|colors| colors.child("indexedColors"))
            .map(|indexed| {
                indexed
                    .elements_named("rgbColor")
                    .filter_map(|color| parse_rgb(&color.tag.get("rgb")?))
                    .collect()
            })
            .unwrap_or_default();
        let indexed = if own.is_empty() {
            INDEXED_COLORS
                .iter()
                .map(|hex| RgbColor::parse(hex).expect("a palette colour"))
                .collect()
        } else {
            own
        };
        let mut style_names = HashMap::new();
        for style in styles.cell_styles() {
            style_names
                .entry(style.xf_id())
                .or_insert_with(|| xstring::decode(&style.name()).into_owned());
        }
        let mut number_formats = HashMap::new();
        for format in styles.num_fmts() {
            if let Some(id) = format.id() {
                number_formats
                    .entry(id)
                    .or_insert_with(|| xstring::decode(&format.code()).into_owned());
            }
        }
        Palette {
            theme: *theme,
            indexed,
            style_names,
            number_formats,
        }
    }
}

/// What resolves the records of one styles part.
struct Resolver<'a> {
    styles: &'a StylesXml,
    palette: &'a Palette,
}

impl<'a> Resolver<'a> {
    fn new(styles: &'a StylesXml, palette: &'a Palette) -> Self {
        Resolver { styles, palette }
    }

    fn record(&self, list: StyleList, index: u32) -> Option<&'a Element> {
        self.styles.list(list).get(index as usize)
    }

    /// A `CT_Color` element.
    fn color(&self, element: Option<&Element>) -> Option<Color> {
        let tag = &element?.tag;
        let tint = tag.f64("tint").unwrap_or(0.0);
        let (source, base) = if let Some(rgb) = tag.get("rgb") {
            (ColorSource::Rgb, parse_rgb(&rgb))
        } else if let Some(index) = tag.u32("theme") {
            (
                ColorSource::Theme(index),
                self.palette.theme.get(index as usize).copied().flatten(),
            )
        } else if let Some(index) = tag.u32("indexed") {
            let base = match index {
                64 => Some(RgbColor::new(0, 0, 0)),
                65 => Some(RgbColor::new(0xFF, 0xFF, 0xFF)),
                _ => self.palette.indexed.get(index as usize).copied(),
            };
            (ColorSource::Indexed(index), base)
        } else if tag.bool("auto") == Some(true) {
            (ColorSource::Auto, None)
        } else {
            return None;
        };
        Some(Color {
            rgb: base.map(|base| apply_hls_tint(base, tint).to_string()),
            source,
            tint,
        })
    }

    fn style(&self, xf: Xf<'_>) -> ResolvedStyle {
        let color = |element: Option<&Element>| self.color(element);
        ResolvedStyle {
            number_format: self.number_format(xf.num_fmt_id()),
            font: self.font(xf.font_id()),
            fill: self.fill(xf.fill_id()),
            border: Border::from_element(self.record(StyleList::Borders, xf.border_id()), &color),
            alignment: Alignment::from_element(xf.alignment()),
            protection: Protection {
                locked: flag(xf.protection(), "locked", true),
                hidden: flag(xf.protection(), "hidden", false),
            },
            style_name: xf
                .xf_id()
                .and_then(|xf_id| self.palette.style_names.get(&xf_id).cloned()),
        }
    }

    /// The code of format `id`: the file's own `numFmt` first, which may
    /// redefine a built-in id, then the built-in code, else General.
    fn number_format(&self, id: u32) -> String {
        self.palette
            .number_formats
            .get(&id)
            .cloned()
            .or_else(|| builtin_code(id, Locale::EnUs).map(str::to_owned))
            .unwrap_or_else(|| "General".to_owned())
    }

    fn font(&self, index: u32) -> Font {
        let font = self.record(StyleList::Fonts, index);
        let first = self.record(StyleList::Fonts, 0);
        let value = |element: Option<&Element>, local: &str| element?.child(local)?.tag.get("val");
        let inherited = |local: &str| value(font, local).or_else(|| value(first, local));
        let set = |local: &str| {
            font.and_then(|font| font.child(local))
                .is_some_and(|element| element.tag.bool("val").unwrap_or(true))
        };
        let color = font
            .and_then(|font| font.child("color"))
            .or_else(|| first.and_then(|first| first.child("color")));
        Font {
            name: inherited("name"),
            size: inherited("sz")
                .as_deref()
                .and_then(oxoffice_xlsx_oxml::parse_f64),
            bold: set("b"),
            italic: set("i"),
            underline: font
                .and_then(|font| font.child("u"))
                .map(|u| u.tag.get("val").unwrap_or_else(|| "single".to_owned()))
                .filter(|underline| underline != "none"),
            strike: set("strike"),
            vertical_align: value(font, "vertAlign").filter(|align| align != "baseline"),
            color: self.color(color),
        }
    }

    fn fill(&self, index: u32) -> Fill {
        let fill = self.record(StyleList::Fills, index);
        if let Some(gradient) = fill.and_then(|fill| fill.child("gradientFill")) {
            let tag = &gradient.tag;
            let edge = |name: &str| tag.f64(name).unwrap_or(0.0);
            return Fill::Gradient {
                kind: tag.get("type").unwrap_or_else(|| "linear".to_owned()),
                degree: edge("degree"),
                left: edge("left"),
                right: edge("right"),
                top: edge("top"),
                bottom: edge("bottom"),
                stops: gradient
                    .elements_named("stop")
                    .map(|stop| GradientStop {
                        position: stop.tag.f64("position").unwrap_or(0.0),
                        color: self.color(stop.child("color")),
                    })
                    .collect(),
            };
        }
        let pattern = fill.and_then(|fill| fill.child("patternFill"));
        Fill::Pattern {
            pattern: pattern
                .and_then(|pattern| pattern.tag.get("patternType"))
                .unwrap_or_else(|| "none".to_owned()),
            foreground: self.color(pattern.and_then(|pattern| pattern.child("fgColor"))),
            background: self.color(pattern.and_then(|pattern| pattern.child("bgColor"))),
        }
    }
}

impl Border {
    fn from_element(
        border: Option<&Element>,
        color: &dyn Fn(Option<&Element>) -> Option<Color>,
    ) -> Self {
        // Strict and later Transitional writers say `start` and `end` for
        // left and right.
        let side = |names: &[&str]| {
            let element = names
                .iter()
                .find_map(|name| border.and_then(|border| border.child(name)));
            BorderSide {
                style: element
                    .and_then(|element| element.tag.get("style"))
                    .unwrap_or_else(|| "none".to_owned()),
                color: color(element.and_then(|element| element.child("color"))),
            }
        };
        Border {
            left: side(&["left", "start"]),
            right: side(&["right", "end"]),
            top: side(&["top"]),
            bottom: side(&["bottom"]),
            diagonal: side(&["diagonal"]),
            diagonal_up: flag(border, "diagonalUp", false),
            diagonal_down: flag(border, "diagonalDown", false),
        }
    }
}

impl Alignment {
    fn from_element(alignment: Option<&Element>) -> Self {
        let text = |name: &str, default: &str| {
            alignment
                .and_then(|alignment| alignment.tag.get(name))
                .unwrap_or_else(|| default.to_owned())
        };
        let number = |name: &str| {
            alignment
                .and_then(|alignment| alignment.tag.u32(name))
                .unwrap_or(0)
        };
        Alignment {
            horizontal: text("horizontal", "general"),
            vertical: text("vertical", "bottom"),
            wrap_text: flag(alignment, "wrapText", false),
            shrink_to_fit: flag(alignment, "shrinkToFit", false),
            indent: number("indent"),
            text_rotation: number("textRotation"),
            reading_order: number("readingOrder"),
        }
    }
}

fn flag(element: Option<&Element>, name: &str, default: bool) -> bool {
    element
        .and_then(|element| element.tag.bool(name))
        .unwrap_or(default)
}

/// `AARRGGBB` or `RRGGBB`, the alpha byte ignored.
fn parse_rgb(text: &str) -> Option<RgbColor> {
    if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let hex = match text.len() {
        8 => text.get(2..)?,
        6 => text,
        _ => return None,
    };
    RgbColor::parse(hex).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_resolve_through_the_theme_and_the_palette() {
        let styles = |colors: &str| {
            let xml = format!(
                r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">{colors}</styleSheet>"#
            );
            StylesXml::parse(Arc::new(xml.into_bytes())).unwrap()
        };
        // dk1 and lt1 system colours, dk2 0E2841, lt2 E8E8E8, accent1 156082.
        let theme = theme_colors(oxoffice_drawing::theme::OFFICE_DEFAULT_XML.as_bytes()).unwrap();
        let default = styles("");
        let own = styles(
            r#"<colors><indexedColors><rgbColor rgb="FF112233"/><rgbColor rgb="FF445566"/></indexedColors></colors>"#,
        );
        for (palette, color, rgb, source) in [
            (
                &default,
                r#"<color theme="0"/>"#,
                Some("FFFFFF"),
                ColorSource::Theme(0),
            ),
            (
                &default,
                r#"<color theme="1"/>"#,
                Some("000000"),
                ColorSource::Theme(1),
            ),
            (
                &default,
                r#"<color theme="2"/>"#,
                Some("E8E8E8"),
                ColorSource::Theme(2),
            ),
            (
                &default,
                r#"<color theme="3"/>"#,
                Some("0E2841"),
                ColorSource::Theme(3),
            ),
            (
                &default,
                r#"<color theme="4" tint="0.5"/>"#,
                Some("64BEE6"),
                ColorSource::Theme(4),
            ),
            (
                &default,
                r#"<color theme="12"/>"#,
                None,
                ColorSource::Theme(12),
            ),
            (
                &default,
                r#"<color rgb="800F766E"/>"#,
                Some("0F766E"),
                ColorSource::Rgb,
            ),
            (
                &default,
                r#"<color indexed="10"/>"#,
                Some("FF0000"),
                ColorSource::Indexed(10),
            ),
            (
                &default,
                r#"<color indexed="64"/>"#,
                Some("000000"),
                ColorSource::Indexed(64),
            ),
            (
                &default,
                r#"<color indexed="65"/>"#,
                Some("FFFFFF"),
                ColorSource::Indexed(65),
            ),
            (
                &default,
                r#"<color indexed="81"/>"#,
                None,
                ColorSource::Indexed(81),
            ),
            (
                &own,
                r#"<color indexed="1"/>"#,
                Some("445566"),
                ColorSource::Indexed(1),
            ),
            (&default, r#"<color auto="1"/>"#, None, ColorSource::Auto),
        ] {
            let palette = Palette::new(palette, &theme);
            let resolver = Resolver::new(&default, &palette);
            let element = Element::parse(color.as_bytes()).unwrap();
            let resolved = resolver.color(Some(&element)).unwrap();
            assert_eq!(
                (resolved.rgb.as_deref(), resolved.source),
                (rgb, source),
                "{color}"
            );
        }
        let palette = Palette::new(&default, &theme);
        let resolver = Resolver::new(&default, &palette);
        assert_eq!(
            resolver.color(Some(&Element::parse(b"<color/>").unwrap())),
            None
        );
    }

    #[test]
    fn an_rgb_value_reads_without_its_alpha_and_refuses_what_is_not_hex() {
        for (text, rgb) in [
            ("FF0F766E", Some("0F766E")),
            ("0F766E", Some("0F766E")),
            ("zz0F766E", None),
            ("\u{20ac}FFFFF", None),
            ("\u{e9}\u{e9}0F766E", None),
            ("FFF", None),
        ] {
            assert_eq!(
                parse_rgb(text).map(|rgb| rgb.to_string()).as_deref(),
                rgb,
                "{text}"
            );
        }
    }

    #[test]
    fn a_file_number_format_overrides_the_built_in_id() {
        let styles = StylesXml::parse(Arc::new(
            concat!(
                r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
                r#"<numFmts count="2"><numFmt numFmtId="14" formatCode="dd/mm/yyyy"/>"#,
                r#"<numFmt numFmtId="164" formatCode="0.0&quot; kg&quot;"/></numFmts></styleSheet>"#
            )
            .as_bytes()
            .to_vec(),
        ))
        .unwrap();
        let palette = Palette::new(&styles, &[None; 12]);
        let resolver = Resolver::new(&styles, &palette);
        for (id, code) in [
            (14, "dd/mm/yyyy"),
            (164, "0.0\" kg\""),
            (4, "#,##0.00"),
            (23, "General"),
            (300, "General"),
        ] {
            assert_eq!(resolver.number_format(id), code, "{id}");
        }
    }
}
