//! Drawing elements for inline and anchor images: `CT_Drawing`, `CT_Inline`, `CT_Anchor`.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::{Namespace, PrefixDeclaration, ResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::{Reader, Writer, XmlVersion};

use crate::error::Result;
use crate::namespace::matches_local_name;
use crate::numbering::namespace_bindings;
use crate::raw_xml::capture_element;
use crate::units::Emu;

/// Namespaces used in drawing markup.
pub mod drawing_ns {
    pub const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
    pub const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    pub const PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
    pub const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
    pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
}

/// Horizontal relative-from for anchor positioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_RelativeFromH {
    Page,
    Margin,
    Column,
    Character,
    LeftMargin,
    RightMargin,
    InsideMargin,
    OutsideMargin,
}

impl ST_RelativeFromH {
    pub fn from_str(s: &str) -> Self {
        match s {
            "page" => Self::Page,
            "margin" => Self::Margin,
            "column" => Self::Column,
            "character" => Self::Character,
            "leftMargin" => Self::LeftMargin,
            "rightMargin" => Self::RightMargin,
            "insideMargin" => Self::InsideMargin,
            "outsideMargin" => Self::OutsideMargin,
            _ => Self::Page,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Margin => "margin",
            Self::Column => "column",
            Self::Character => "character",
            Self::LeftMargin => "leftMargin",
            Self::RightMargin => "rightMargin",
            Self::InsideMargin => "insideMargin",
            Self::OutsideMargin => "outsideMargin",
        }
    }
}

/// Vertical relative-from for anchor positioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_RelativeFromV {
    Page,
    Margin,
    Paragraph,
    Line,
    TopMargin,
    BottomMargin,
    InsideMargin,
    OutsideMargin,
}

impl ST_RelativeFromV {
    pub fn from_str(s: &str) -> Self {
        match s {
            "page" => Self::Page,
            "margin" => Self::Margin,
            "paragraph" => Self::Paragraph,
            "line" => Self::Line,
            "topMargin" => Self::TopMargin,
            "bottomMargin" => Self::BottomMargin,
            "insideMargin" => Self::InsideMargin,
            "outsideMargin" => Self::OutsideMargin,
            _ => Self::Page,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Margin => "margin",
            Self::Paragraph => "paragraph",
            Self::Line => "line",
            Self::TopMargin => "topMargin",
            Self::BottomMargin => "bottomMargin",
            Self::InsideMargin => "insideMargin",
            Self::OutsideMargin => "outsideMargin",
        }
    }
}

/// Wrapping type for anchored drawings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapType {
    /// Text flows over or under the drawing without reserving space.
    #[default]
    None,
    /// Text keeps clear of the drawing's frame.
    Square,
    /// Text clears the drawing above and below, using the full width.
    TopAndBottom,
    /// Text follows the drawing's outline.
    Tight,
    /// Text follows the outline and may enter its concave regions.
    Through,
}

/// Picture source rectangle in DrawingML thousandths of one percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl WrapType {
    /// The wrapping element's qualified name.
    fn element_name(self) -> &'static str {
        match self {
            WrapType::None => "wp:wrapNone",
            WrapType::Square => "wp:wrapSquare",
            WrapType::TopAndBottom => "wp:wrapTopAndBottom",
            WrapType::Tight => "wp:wrapTight",
            WrapType::Through => "wp:wrapThrough",
        }
    }
}

/// The extension URI Word writes around `adec:decorative`.
const DECORATIVE_EXTENSION_URI: &str = "{C183D7F6-B498-43B3-948B-1728B52AA6E4}";
/// The namespace of Word's decorative-drawing marker.
pub(crate) const DECORATIVE_NAMESPACE: &str =
    "http://schemas.microsoft.com/office/drawing/2017/decorative";

pub(crate) struct DecorativeChildPlan {
    pub(crate) admitted: bool,
    decorative: bool,
}

impl DecorativeChildPlan {
    pub(crate) fn read(
        element: &BytesStart<'_>,
        depth: usize,
        parent_admitted: bool,
        drawing_namespace: bool,
        decorative_namespace: bool,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<Self> {
        let local = element.local_name();
        let admitted = match depth {
            0 => drawing_namespace && local.as_ref() == b"extLst",
            1 => {
                parent_admitted
                    && drawing_namespace
                    && local.as_ref() == b"ext"
                    && element
                        .try_get_attribute("uri")?
                        .map(|attribute| {
                            attribute
                                .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                                .map(|value| value == DECORATIVE_EXTENSION_URI)
                        })
                        .transpose()?
                        .unwrap_or(false)
            }
            2 => parent_admitted && decorative_namespace && local.as_ref() == b"decorative",
            _ => false,
        };
        let mut decorative = false;
        if depth == 2 && admitted {
            for attribute in element.attributes() {
                let attribute = attribute?;
                if attribute.key.as_ref() == b"val" {
                    let value =
                        attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?;
                    decorative |= matches!(value.as_ref(), "1" | "true" | "on");
                }
            }
        }
        Ok(Self {
            admitted,
            decorative,
        })
    }
}

/// Read the rest of a `wp:docPr` element and report whether its extension
/// list marks the drawing decorative, as Word writes it.
fn read_doc_pr_decorative(reader: &mut NsReader<&[u8]>) -> Result<bool> {
    let mut path = Vec::<bool>::new();
    let mut decorative = false;
    let mut buffer = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer)?;
        let is_start = matches!(event, Event::Start(_));
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let (namespace, _) = reader.resolver().resolve_element(element.name());
                let plan = DecorativeChildPlan::read(
                    element,
                    path.len(),
                    path.last().copied().unwrap_or(true),
                    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == drawing_ns::A.as_bytes()),
                    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == DECORATIVE_NAMESPACE.as_bytes()),
                    reader.decoder(),
                )?;
                let admitted = plan.admitted;
                decorative |= plan.decorative;
                if is_start {
                    path.push(admitted);
                }
            }
            Event::End(_) if path.is_empty() => break,
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => {
                return Err(crate::OxmlError::MissingElement(
                    "wp:docPr closing element".into(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(decorative)
}

/// Write `wp:docPr` with its accessibility attributes, and the decorative
/// marker as Word writes it when the drawing is decorative.
fn write_doc_pr<W: std::io::Write>(
    writer: &mut Writer<W>,
    id: u32,
    name: Option<&str>,
    description: Option<&str>,
    title: Option<&str>,
    decorative: bool,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let mut buf = itoa::Buffer::new();
    let mut doc_pr = crate::text::FieldXmlStart::new("wp:docPr", emission)?;
    doc_pr.push("id", buf.format(id))?;
    doc_pr.push("name", name.unwrap_or("Picture"))?;
    if let Some(description) = description {
        doc_pr.push("descr", description)?;
    }
    if let Some(title) = title {
        doc_pr.push("title", title)?;
    }
    if !decorative {
        doc_pr.write(writer, true)?;
        return Ok(());
    }
    doc_pr.write(writer, false)?;
    let mut list = crate::text::FieldXmlStart::new("a:extLst", emission)?;
    list.push("xmlns:a", drawing_ns::A)?;
    list.write(writer, false)?;
    let mut extension = crate::text::FieldXmlStart::new("a:ext", emission)?;
    extension.push("uri", DECORATIVE_EXTENSION_URI)?;
    extension.write(writer, false)?;
    let mut marker = crate::text::FieldXmlStart::new("adec:decorative", emission)?;
    marker.push("xmlns:adec", DECORATIVE_NAMESPACE)?;
    marker.push("val", "1")?;
    marker.write(writer, true)?;
    writer.write_event(Event::End(BytesEnd::new("a:ext")))?;
    writer.write_event(Event::End(BytesEnd::new("a:extLst")))?;
    writer.write_event(Event::End(BytesEnd::new("wp:docPr")))?;
    Ok(())
}

fn write_wrap_element<W: std::io::Write>(
    writer: &mut Writer<W>,
    wrap: WrapType,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    match wrap {
        WrapType::None | WrapType::TopAndBottom => {
            crate::text::FieldXmlStart::new(wrap.element_name(), emission)?.write(writer, true)?;
        }
        WrapType::Square => {
            let mut element = crate::text::FieldXmlStart::new(wrap.element_name(), emission)?;
            element.push("wrapText", "bothSides")?;
            element.write(writer, true)?;
        }
        WrapType::Tight | WrapType::Through => {
            let mut element = crate::text::FieldXmlStart::new(wrap.element_name(), emission)?;
            element.push("wrapText", "bothSides")?;
            element.write(writer, false)?;
            let mut polygon = crate::text::FieldXmlStart::new("wp:wrapPolygon", emission)?;
            polygon.push("edited", "0")?;
            polygon.write(writer, false)?;
            for (name, x, y) in [
                ("wp:start", "0", "0"),
                ("wp:lineTo", "0", "21600"),
                ("wp:lineTo", "21600", "21600"),
                ("wp:lineTo", "21600", "0"),
                ("wp:lineTo", "0", "0"),
            ] {
                let mut point = crate::text::FieldXmlStart::new(name, emission)?;
                point.push("x", x)?;
                point.push("y", y)?;
                point.write(writer, true)?;
            }
            writer.write_event(Event::End(BytesEnd::new("wp:wrapPolygon")))?;
            writer.write_event(Event::End(BytesEnd::new(wrap.element_name())))?;
        }
    }
    Ok(())
}

/// The wrap mode a wrapping element names, or `None` if it is not one.
fn wrap_type_of(name: &[u8]) -> Option<WrapType> {
    if matches_local_name(name, b"wrapNone") {
        Some(WrapType::None)
    } else if matches_local_name(name, b"wrapSquare") {
        Some(WrapType::Square)
    } else if matches_local_name(name, b"wrapTopAndBottom") {
        Some(WrapType::TopAndBottom)
    } else if matches_local_name(name, b"wrapTight") {
        Some(WrapType::Tight)
    } else if matches_local_name(name, b"wrapThrough") {
        Some(WrapType::Through)
    } else {
        None
    }
}

/// Horizontal alignment for an anchored drawing.
///
/// An anchor positions itself either by an offset or by an alignment. When an
/// alignment is given the offset is not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorAlignH {
    Left,
    Center,
    Right,
    Inside,
    Outside,
}

impl AnchorAlignH {
    /// Parse an alignment. An unrecognised value reads as no alignment, which
    /// falls back to the offset rather than inventing a position.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "left" => Some(Self::Left),
            "center" => Some(Self::Center),
            "right" => Some(Self::Right),
            "inside" => Some(Self::Inside),
            "outside" => Some(Self::Outside),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
            Self::Inside => "inside",
            Self::Outside => "outside",
        }
    }
}

/// Vertical alignment for an anchored drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorAlignV {
    Top,
    Center,
    Bottom,
    Inside,
    Outside,
}

impl AnchorAlignV {
    /// Parse an alignment. An unrecognised value reads as no alignment.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "top" => Some(Self::Top),
            "center" => Some(Self::Center),
            "bottom" => Some(Self::Bottom),
            "inside" => Some(Self::Inside),
            "outside" => Some(Self::Outside),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Center => "center",
            Self::Bottom => "bottom",
            Self::Inside => "inside",
            Self::Outside => "outside",
        }
    }
}

/// `CT_Anchor` — An anchored (floating) drawing element.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Anchor {
    /// Non-visual drawing properties identifier (`wp:docPr/@id`).
    pub doc_pr_id: u32,
    /// Whether the drawing is behind document text.
    pub behind_doc: bool,
    /// Horizontal position offset in EMUs.
    pub pos_h_offset: Emu,
    /// Horizontal relative-from.
    pub pos_h_relative_from: ST_RelativeFromH,
    /// Horizontal alignment, used instead of the offset when present.
    pub pos_h_align: Option<AnchorAlignH>,
    /// Vertical position offset in EMUs.
    pub pos_v_offset: Emu,
    /// Vertical relative-from.
    pub pos_v_relative_from: ST_RelativeFromV,
    /// Vertical alignment, used instead of the offset when present.
    pub pos_v_align: Option<AnchorAlignV>,
    /// Width in EMUs.
    pub extent_cx: Emu,
    /// Height in EMUs.
    pub extent_cy: Emu,
    /// Space kept between the drawing and the text wrapping around it.
    pub dist_t: Emu,
    pub dist_b: Emu,
    pub dist_l: Emu,
    pub dist_r: Emu,
    /// Wrapping type.
    pub wrap: WrapType,
    /// Relationship ID referencing the image part.
    pub embed_id: String,
    /// Relationship ID referencing an externally linked image.
    pub link_id: Option<String>,
    /// Relationship ID referencing a ChartML part.
    pub chart_rel_id: Option<String>,
    /// Z-order relative height.
    pub relative_height: u32,
    /// Optional description/alt text.
    pub description: Option<String>,
    /// Optional accessibility title.
    pub title: Option<String>,
    /// Whether the qualified decorative extension marks this drawing.
    pub decorative: bool,
    /// Optional name.
    pub name: Option<String>,
    /// Optional crop rectangle applied before scaling the picture.
    pub source_rect: Option<SourceRect>,
    /// Raw XML bytes for the entire wp:anchor element (used for round-trip preservation).
    /// When present, to_xml uses this instead of structured serialization.
    pub raw_xml: Option<Vec<u8>>,
    /// Inherited scope used only for semantic projection, never emitted over raw source.
    #[doc(hidden)]
    pub source_namespace_bindings: Vec<(String, String)>,
    /// Shape content, when the anchor holds a `wps:wsp` rather than a picture.
    pub shape: Option<CT_Shape>,
}

/// A `wps:wsp` shape: preset geometry, an optional fill, and optional text.
///
/// Word writes these inside `mc:AlternateContent`, with a VML fallback beside
/// them. Only what we can actually draw is modelled here.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CT_Shape {
    /// Preset geometry name from `a:prstGeom@prst`, e.g. "rect" or "line".
    pub preset: Option<String>,
    /// Solid fill colour as RRGGBB, from the shape's own fill rather than its
    /// outline. `None` covers both `a:noFill` and a fill we cannot resolve,
    /// such as a theme colour.
    pub solid_fill: Option<String>,
    /// Paragraphs of the shape's text box, from `wps:txbx/w:txbxContent`.
    pub text: Vec<crate::text::CT_P>,
    /// Whether the shape grows to fit its text (`a:spAutoFit` in
    /// `wps:bodyPr`). Otherwise its extent is fixed, and Word hides the
    /// lines that do not fit.
    pub grows_to_fit_text: bool,
    /// Space between the shape's edges and its text, from `wps:bodyPr`.
    pub text_insets: TextInsets,
    /// Authoritative parsed block body, including tables and modeled controls.
    /// The paragraph-only projection is used for authored legacy shapes when absent.
    pub text_body: Option<crate::document::CT_Body>,
    /// Physical selected text-box owner ordinal in the source OPC part.
    /// Assigned by the document projection, never by renderer traversal order.
    pub source_text_box_owner: Option<usize>,
}

/// Space between a shape's edges and its text (`lIns`, `tIns`, `rIns` and
/// `bIns` of `wps:bodyPr`). An absent side is Word's default: 0.1 inch left
/// and right, 0.05 inch top and bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextInsets {
    pub left: Emu,
    pub top: Emu,
    pub right: Emu,
    pub bottom: Emu,
}

impl Default for TextInsets {
    fn default() -> Self {
        Self {
            left: Emu(91440),
            top: Emu(45720),
            right: Emu(91440),
            bottom: Emu(45720),
        }
    }
}

/// Read the insets a `wps:bodyPr` states; an unreadable value keeps the
/// default.
fn read_text_insets(element: &BytesStart<'_>, insets: &mut TextInsets) {
    for attribute in element.attributes().flatten() {
        let side = match attribute.key.as_ref() {
            b"lIns" => &mut insets.left,
            b"tIns" => &mut insets.top,
            b"rIns" => &mut insets.right,
            b"bIns" => &mut insets.bottom,
            _ => continue,
        };
        if let Some(value) = std::str::from_utf8(&attribute.value)
            .ok()
            .and_then(|value| value.parse().ok())
        {
            *side = Emu(value);
        }
    }
}

/// Parse the DrawingML held inside a captured `mc:AlternateContent` block.
///
/// Word writes a shape as a compatibility block: the modern DrawingML sits in
/// `mc:Choice` and a VML fallback in `mc:Fallback`. We read the Choice and
/// ignore the Fallback.
///
/// The caller keeps the raw bytes for write back, so whatever comes out of
/// here must not be serialised again or the element ends up duplicated.
pub fn parse_alternate_content(raw: &[u8], inherited_prefixes: &[String]) -> Option<CT_Drawing> {
    crate::document::admit_word_xml(raw, inherited_prefixes, None).ok()?;
    parse_alternate_content_admitted(raw, inherited_prefixes)
}

pub(crate) fn parse_alternate_content_admitted(
    raw: &[u8],
    inherited_prefixes: &[String],
) -> Option<CT_Drawing> {
    let raw = crate::text::raw_with_external_bindings(raw, &namespace_bindings(inherited_prefixes))
        .ok()?;
    let mut reader = NsReader::from_reader(raw.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut in_choice = false;
    let mut choice_prefixes = Vec::new();
    loop {
        let offset = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer).ok()?;
        match event {
            Event::Start(ref element) => {
                let name = element.name();
                let local = name.local_name();
                if depth == 0
                    && !(namespace_matches(&namespace, crate::namespace::MC_NS, b"mc")
                        && local.as_ref() == b"AlternateContent")
                {
                    return None;
                }
                let qualified_drawing = namespace_matches(&namespace, crate::namespace::W_NS, b"w")
                    && local.as_ref() == b"drawing";
                if depth == 1 {
                    in_choice = namespace_matches(&namespace, crate::namespace::MC_NS, b"mc")
                        && local.as_ref() == b"Choice";
                    if in_choice {
                        // Snapshot the actual qualified Choice parent once.
                        // A later sibling End can leave a pending resolver pop,
                        // so pre-read resolver state is not parent authority.
                        choice_prefixes.clear();
                        for (prefix, namespace) in reader.resolver().bindings() {
                            let prefix = match prefix {
                                PrefixDeclaration::Default => "",
                                PrefixDeclaration::Named(prefix) => {
                                    std::str::from_utf8(prefix).ok()?
                                }
                            };
                            let namespace = quick_xml::escape::unescape(
                                std::str::from_utf8(namespace.as_ref()).ok()?,
                            )
                            .ok()?;
                            choice_prefixes
                                .push(crate::numbering::namespace_binding(prefix, &namespace));
                        }
                    }
                }
                if depth == 2 && in_choice && qualified_drawing {
                    let mut bindings = Vec::new();
                    for (prefix, namespace) in reader.resolver().bindings() {
                        let prefix = match prefix {
                            PrefixDeclaration::Default => "",
                            PrefixDeclaration::Named(prefix) => std::str::from_utf8(prefix).ok()?,
                        };
                        let namespace = quick_xml::escape::unescape(
                            std::str::from_utf8(namespace.as_ref()).ok()?,
                        )
                        .ok()?;
                        bindings.push((prefix.to_owned(), namespace.into_owned()));
                    }
                    let name = element.name().as_ref().to_vec();
                    reader
                        .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                        .ok()?;
                    let physical = &raw[offset..reader.buffer_position() as usize];
                    let scoped =
                        crate::text::raw_with_external_bindings(physical, &bindings).ok()?;
                    let mut qualified = NsReader::from_reader(scoped.as_slice());
                    let mut owner_buffer = Vec::new();
                    let mut owner_depth = 0_usize;
                    let mut owners = 0_usize;
                    loop {
                        let (owner_namespace, owner_event) =
                            qualified.read_resolved_event_into(&mut owner_buffer).ok()?;
                        match owner_event {
                            Event::Start(ref owner) | Event::Empty(ref owner)
                                if owner_depth == 1
                                    && matches!(
                                        owner.local_name().as_ref(),
                                        b"anchor" | b"inline"
                                    ) =>
                            {
                                if !namespace_matches(&owner_namespace, drawing_ns::WP, b"wp") {
                                    return None;
                                }
                                owners += 1;
                                if matches!(owner_event, Event::Start(_)) {
                                    owner_depth += 1;
                                }
                            }
                            Event::Start(_) => owner_depth += 1,
                            Event::End(_) => owner_depth = owner_depth.saturating_sub(1),
                            Event::Eof => break,
                            _ => {}
                        }
                        owner_buffer.clear();
                    }
                    if owners != 1 {
                        return None;
                    }
                    let prefixes: Vec<_> = bindings
                        .iter()
                        .map(|(prefix, namespace)| {
                            crate::numbering::namespace_binding(prefix, namespace)
                        })
                        .collect();
                    let mut projection = Reader::from_reader(physical);
                    let mut start_buffer = Vec::new();
                    let Event::Start(start) = projection.read_event_into(&mut start_buffer).ok()?
                    else {
                        return None;
                    };
                    let drawing_prefixes =
                        crate::numbering::word_prefixes_at(&start, &prefixes).ok()?;
                    let drawing = CT_Drawing::from_xml_with_prefixes(
                        &mut projection,
                        &drawing_prefixes,
                        &choice_prefixes,
                    )
                    .ok()?;
                    return (drawing.anchor.is_some() || drawing.inline.is_some())
                        .then_some(drawing);
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 1 {
                    in_choice = false;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    None
}

/// Pull the preset geometry and solid fill out of a captured `wps:spPr`.
///
/// The fill has to be told apart from the outline colour. Both are written as
/// `a:srgbClr`, and the outline sits inside `a:ln`, so anything at or below an
/// `a:ln` is skipped.
fn parse_shape_props(raw: &[u8]) -> (Option<String>, Option<String>) {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut preset = None;
    let mut fill = None;
    let mut ln_depth = 0usize;
    let mut in_solid_fill = false;

    // Only a Start can open a scope. A self-closing a:ln or a:solidFill has no
    // children, so it must not change the depth or the fill flag.
    loop {
        let event = reader.read_event_into(&mut buf);
        match event {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let local = name.as_ref();
                if matches_local_name(local, b"ln") {
                    ln_depth += 1;
                } else if matches_local_name(local, b"solidFill") {
                    in_solid_fill = true;
                } else if matches_local_name(local, b"prstGeom") {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"prst" {
                            preset = std::str::from_utf8(&attr.value).ok().map(str::to_string);
                        }
                    }
                } else if matches_local_name(local, b"srgbClr")
                    && in_solid_fill
                    && ln_depth == 0
                    && fill.is_none()
                {
                    // srgbClr can carry children such as a:alpha, so it turns
                    // up as a Start as well as an Empty.
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"val" {
                            fill = std::str::from_utf8(&attr.value).ok().map(str::to_string);
                        }
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = e.name();
                let local = name.as_ref();
                if matches_local_name(local, b"prstGeom") {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"prst" {
                            preset = std::str::from_utf8(&attr.value).ok().map(str::to_string);
                        }
                    }
                } else if matches_local_name(local, b"srgbClr")
                    && in_solid_fill
                    && ln_depth == 0
                    && fill.is_none()
                {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"val" {
                            fill = std::str::from_utf8(&attr.value).ok().map(str::to_string);
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.name();
                let local = name.as_ref();
                if matches_local_name(local, b"ln") {
                    ln_depth = ln_depth.saturating_sub(1);
                } else if matches_local_name(local, b"solidFill") {
                    in_solid_fill = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    (preset, fill)
}

impl CT_Anchor {
    /// Read only the selected qualified WPS shape transform, without guessing
    /// rotations from extension or picture payloads.
    pub fn shape_rotation_degrees(&self) -> Option<f64> {
        let scoped = crate::text::raw_with_external_bindings(
            self.raw_xml.as_deref()?,
            &self.source_namespace_bindings,
        )
        .ok()?;
        let mut reader = NsReader::from_reader(scoped.as_slice());
        let mut path = Vec::new();
        let expected = [
            (drawing_ns::WP, "anchor"),
            (drawing_ns::A, "graphic"),
            (drawing_ns::A, "graphicData"),
            (
                "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
                "wsp",
            ),
            (
                "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
                "spPr",
            ),
            (drawing_ns::A, "xfrm"),
        ];
        let mut rotation = None;
        loop {
            let (namespace, event) = reader.read_resolved_event().ok()?;
            match event {
                Event::Start(ref element) | Event::Empty(ref element) => {
                    let namespace = match namespace {
                        ResolveResult::Bound(namespace) => namespace.as_ref().to_vec(),
                        _ => Vec::new(),
                    };
                    path.push((namespace, element.local_name().as_ref().to_vec()));
                    if path.len() == expected.len()
                        && path
                            .iter()
                            .zip(expected)
                            .all(|((namespace, local), (uri, name))| {
                                namespace == uri.as_bytes() && local == name.as_bytes()
                            })
                    {
                        for attribute in element.attributes() {
                            let attribute = attribute.ok()?;
                            if attribute.key.as_ref() == b"rot" {
                                if rotation.is_some() {
                                    return None;
                                }
                                let amount: i32 = attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        element.decoder(),
                                    )
                                    .ok()?
                                    .parse()
                                    .ok()?;
                                rotation = Some(f64::from(amount) / 60_000.0);
                            }
                        }
                    }
                    if matches!(event, Event::Empty(_)) {
                        path.pop();
                    }
                }
                Event::End(_) => {
                    path.pop();
                }
                Event::Eof => break,
                _ => {}
            }
        }
        rotation
    }

    /// Resolve effects only from this anchor's qualified physical picture owner.
    /// Missing effects keep the ordinary image path unchanged.
    pub fn picture_effects(&self) -> Option<(f64, f64)> {
        let scoped = crate::text::raw_with_external_bindings(
            self.raw_xml.as_deref()?,
            &self.source_namespace_bindings,
        )
        .ok()?;
        let mut reader = NsReader::from_reader(scoped.as_slice());
        let mut path = Vec::new();
        let mut opacity = None;
        let mut rotation = None;
        loop {
            let (namespace, event) = reader.read_resolved_event().ok()?;
            match event {
                Event::Start(ref element) | Event::Empty(ref element) => {
                    let uri = match namespace {
                        ResolveResult::Bound(uri) => uri.as_ref().to_vec(),
                        _ => Vec::new(),
                    };
                    path.push((uri, element.local_name().as_ref().to_vec()));
                    let matches = |expected: &[(&str, &str)]| {
                        path.len() == expected.len()
                            && path.iter().zip(expected).all(
                                |((uri, local), (expected_uri, expected_local))| {
                                    uri == expected_uri.as_bytes()
                                        && local == expected_local.as_bytes()
                                },
                            )
                    };
                    let prefix = &[
                        (
                            "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
                            "anchor",
                        ),
                        (
                            "http://schemas.openxmlformats.org/drawingml/2006/main",
                            "graphic",
                        ),
                        (
                            "http://schemas.openxmlformats.org/drawingml/2006/main",
                            "graphicData",
                        ),
                        (
                            "http://schemas.openxmlformats.org/drawingml/2006/picture",
                            "pic",
                        ),
                    ];
                    let alpha_path = [
                        prefix[0],
                        prefix[1],
                        prefix[2],
                        prefix[3],
                        (
                            "http://schemas.openxmlformats.org/drawingml/2006/picture",
                            "blipFill",
                        ),
                        (
                            "http://schemas.openxmlformats.org/drawingml/2006/main",
                            "blip",
                        ),
                        (
                            "http://schemas.openxmlformats.org/drawingml/2006/main",
                            "alphaModFix",
                        ),
                    ];
                    let transform_path = [
                        prefix[0],
                        prefix[1],
                        prefix[2],
                        prefix[3],
                        (
                            "http://schemas.openxmlformats.org/drawingml/2006/picture",
                            "spPr",
                        ),
                        (
                            "http://schemas.openxmlformats.org/drawingml/2006/main",
                            "xfrm",
                        ),
                    ];
                    let alpha = matches(&alpha_path);
                    let transform = matches(&transform_path);
                    if alpha || transform {
                        for attribute in element.attributes() {
                            let attribute = attribute.ok()?;
                            if alpha && attribute.key.as_ref() == b"amt" {
                                if opacity.is_some() {
                                    return None;
                                }
                                let amount: u32 = attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        element.decoder(),
                                    )
                                    .ok()?
                                    .parse()
                                    .ok()?;
                                if amount > 100_000 {
                                    return None;
                                }
                                opacity = Some(f64::from(amount) / 100_000.0);
                            } else if transform && attribute.key.as_ref() == b"rot" {
                                if rotation.is_some() {
                                    return None;
                                }
                                let amount: i32 = attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        element.decoder(),
                                    )
                                    .ok()?
                                    .parse()
                                    .ok()?;
                                rotation = Some(f64::from(amount) / 60_000.0);
                            }
                        }
                    }
                    if matches!(event, Event::Empty(_)) {
                        path.pop();
                    }
                }
                Event::End(_) => {
                    path.pop();
                }
                Event::Eof => break,
                _ => {}
            }
        }
        (opacity.is_some() || rotation.is_some())
            .then_some((opacity.unwrap_or(1.0), rotation.unwrap_or(0.0)))
    }

    /// Create an anchor for a full-page background image. It carries no alt
    /// text: a caller sets the description, or marks the picture decorative.
    pub fn background(embed_id: &str, page_width_emu: i64, page_height_emu: i64) -> Self {
        CT_Anchor {
            doc_pr_id: 1,
            behind_doc: true,
            pos_h_offset: Emu(0),
            pos_h_relative_from: ST_RelativeFromH::Page,
            pos_h_align: None,
            pos_v_offset: Emu(0),
            pos_v_relative_from: ST_RelativeFromV::Page,
            pos_v_align: None,
            extent_cx: Emu(page_width_emu),
            extent_cy: Emu(page_height_emu),
            dist_t: Emu(0),
            dist_b: Emu(0),
            dist_l: Emu(0),
            dist_r: Emu(0),
            wrap: WrapType::None,
            embed_id: embed_id.to_string(),
            link_id: None,
            chart_rel_id: None,
            relative_height: 0,
            description: None,
            title: None,
            decorative: false,
            name: Some("Background".to_string()),
            source_rect: None,
            raw_xml: None,
            source_namespace_bindings: Vec::new(),
            shape: None,
        }
    }

    /// Create an anchored drawing whose payload is a native Word chart.
    pub fn new_chart(chart_rel_id: &str, width_emu: i64, height_emu: i64) -> Self {
        let mut anchor = Self::background("", width_emu, height_emu);
        anchor.behind_doc = false;
        anchor.chart_rel_id = Some(chart_rel_id.to_owned());
        anchor.description = None;
        anchor.name = Some("Chart".to_owned());
        anchor
    }

    pub fn from_xml(reader: &mut NsReader<&[u8]>, start: &BytesStart) -> Result<Self> {
        let prefixes: Vec<String> = reader
            .resolver()
            .bindings()
            .map(|(prefix, uri)| {
                let name = match prefix {
                    PrefixDeclaration::Default => "",
                    PrefixDeclaration::Named(name) => std::str::from_utf8(name).unwrap_or(""),
                };
                format!("\0{name}\0{}", String::from_utf8_lossy(uri.as_ref()))
            })
            .collect();
        crate::document::admit_word_reader(reader, &prefixes, b"anchor", Some(drawing_ns::WP))?;
        Self::from_xml_admitted(reader, start)
    }

    fn from_xml_admitted(reader: &mut NsReader<&[u8]>, start: &BytesStart) -> Result<Self> {
        Self::from_xml_admitted_in(reader, start, None)
    }

    fn from_xml_admitted_in(
        reader: &mut NsReader<&[u8]>,
        start: &BytesStart,
        mapped: Option<(&crate::document::WordFieldInput, &[u8])>,
    ) -> Result<Self> {
        let cursor = mapped.map(|(input, raw)| input.cursor(raw)).transpose()?;
        let AnchorAttributes {
            behind_doc,
            relative_height,
            dist_t,
            dist_b,
            dist_l,
            dist_r,
        } = AnchorAttributes::read(start)?;

        let mut pos_h_offset = Emu(0);
        let mut pos_h_relative_from = ST_RelativeFromH::Page;
        let mut pos_h_align = None;
        let mut pos_v_offset = Emu(0);
        let mut pos_v_relative_from = ST_RelativeFromV::Page;
        let mut pos_v_align = None;
        let mut wrap = WrapType::None;
        let mut extent_cx = Emu(0);
        let mut extent_cy = Emu(0);
        let mut shape: Option<CT_Shape> = None;
        let mut description = None;
        let mut title = None;
        let mut decorative = false;
        let mut name = None;
        let mut source_rect = None;
        let mut doc_pr_id = None;
        let mut buf = Vec::new();

        loop {
            let event_start = reader.buffer_position();
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let ename = e.name();
                    let action = DrawingChildAction::for_name(
                        DrawingContainerKind::Anchor,
                        ename.as_ref(),
                        true,
                        canonical_wp_element(reader, e, b"docPr"),
                        false,
                    );
                    if action == DrawingChildAction::Extent {
                        read_extent(e, &mut extent_cx, &mut extent_cy)?;
                    } else if action == DrawingChildAction::DocPr {
                        for attr in e.attributes() {
                            let attr = attr?;
                            match DrawingDocPrAttribute::read(&attr, e.decoder())? {
                                DrawingDocPrAttribute::Description(value) => {
                                    description = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Title(value) => {
                                    title = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Name(value) => {
                                    name = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Id(value) => doc_pr_id = Some(value),
                                DrawingDocPrAttribute::Ignored => {}
                            }
                        }
                    } else if action == DrawingChildAction::SourceRect {
                        source_rect = Some(parse_source_rect(e)?);
                    } else if matches_local_name(ename.as_ref(), b"simplePos") {
                        // Ignore simplePos
                    } else if let DrawingChildAction::Wrap(parsed) = action {
                        wrap = parsed;
                    } else if let Some(shape) = shape.as_mut() {
                        // Inside the shape's `wps:bodyPr`, or `wps:bodyPr`
                        // itself when it has no children.
                        if matches_local_name(ename.as_ref(), b"spAutoFit") {
                            shape.grows_to_fit_text = true;
                        } else if matches_local_name(ename.as_ref(), b"bodyPr") {
                            read_text_insets(e, &mut shape.text_insets);
                        }
                    }
                }
                Ok(Event::Start(ref e)) => {
                    let ename = e.name();
                    let is_word_text_box = {
                        let (namespace, local) = reader.resolver().resolve_element(e.name());
                        namespace_matches(&namespace, crate::namespace::W_NS, b"w")
                            && local.as_ref() == b"txbxContent"
                    };
                    let action = DrawingChildAction::for_name(
                        DrawingContainerKind::Anchor,
                        ename.as_ref(),
                        false,
                        canonical_wp_element(reader, e, b"docPr"),
                        is_word_text_box,
                    );
                    // A shape's `wps:bodyPr` follows its `wps:spPr`, which
                    // made the shape.
                    if matches_local_name(ename.as_ref(), b"bodyPr")
                        && let Some(shape) = shape.as_mut()
                    {
                        read_text_insets(e, &mut shape.text_insets);
                    }
                    if action == DrawingChildAction::SourceRect {
                        source_rect = Some(parse_source_rect(e)?);
                        reader.read_to_end_into(ename, &mut Vec::new())?;
                    } else if action == DrawingChildAction::PositionH {
                        if let Some(DrawingPositionReference::Horizontal(value)) =
                            DrawingPositionReference::read(e, true)?
                        {
                            pos_h_relative_from = value;
                        }
                        // Read child <wp:posOffset>
                        let mut inner_buf = Vec::new();
                        loop {
                            match reader.read_event_into(&mut inner_buf) {
                                Ok(Event::Start(ref ie))
                                    if matches_local_name(ie.name().as_ref(), b"posOffset") =>
                                {
                                    let text = reader
                                        .read_text(ie.name())
                                        .map(|t| crate::xml_text::decode_escaped(&t))
                                        .unwrap_or_default();
                                    pos_h_offset = Emu(text.trim().parse().unwrap_or(0));
                                }
                                Ok(Event::Start(ref ie))
                                    if matches_local_name(ie.name().as_ref(), b"align") =>
                                {
                                    let text = reader
                                        .read_text(ie.name())
                                        .map(|t| crate::xml_text::decode_escaped(&t))
                                        .unwrap_or_default();
                                    pos_h_align = AnchorAlignH::parse(text.trim());
                                }
                                Ok(Event::End(ref ie))
                                    if matches_local_name(ie.name().as_ref(), b"positionH") =>
                                {
                                    break;
                                }
                                Ok(Event::Eof) => break,
                                Err(e) => return Err(e.into()),
                                _ => {}
                            }
                            inner_buf.clear();
                        }
                    } else if let DrawingChildAction::Wrap(parsed) = action {
                        // Expanded spelling. wrapSquare and the outline wraps
                        // carry children we do not model, so skip the subtree.
                        wrap = parsed;
                        reader.read_to_end_into(ename, &mut Vec::new())?;
                    } else if action == DrawingChildAction::PositionV {
                        if let Some(DrawingPositionReference::Vertical(value)) =
                            DrawingPositionReference::read(e, false)?
                        {
                            pos_v_relative_from = value;
                        }
                        let mut inner_buf = Vec::new();
                        loop {
                            match reader.read_event_into(&mut inner_buf) {
                                Ok(Event::Start(ref ie))
                                    if matches_local_name(ie.name().as_ref(), b"posOffset") =>
                                {
                                    let text = reader
                                        .read_text(ie.name())
                                        .map(|t| crate::xml_text::decode_escaped(&t))
                                        .unwrap_or_default();
                                    pos_v_offset = Emu(text.trim().parse().unwrap_or(0));
                                }
                                Ok(Event::Start(ref ie))
                                    if matches_local_name(ie.name().as_ref(), b"align") =>
                                {
                                    let text = reader
                                        .read_text(ie.name())
                                        .map(|t| crate::xml_text::decode_escaped(&t))
                                        .unwrap_or_default();
                                    pos_v_align = AnchorAlignV::parse(text.trim());
                                }
                                Ok(Event::End(ref ie))
                                    if matches_local_name(ie.name().as_ref(), b"positionV") =>
                                {
                                    break;
                                }
                                Ok(Event::Eof) => break,
                                Err(e) => return Err(e.into()),
                                _ => {}
                            }
                            inner_buf.clear();
                        }
                    } else if action == DrawingChildAction::ShapeProperties {
                        // Capture the shape properties and read geometry and
                        // fill out of them separately, so the fill colour is
                        // not confused with the outline colour.
                        let raw = capture_ns_element(reader, e)?;
                        let (preset, solid_fill) = parse_shape_props(&raw);
                        let s = shape.get_or_insert_with(CT_Shape::default);
                        s.preset = preset;
                        s.solid_fill = solid_fill;
                    } else if action == DrawingChildAction::TextBox {
                        let body = text_box_body(reader, e, cursor.as_ref(), event_start)?;
                        let shape = shape.get_or_insert_with(CT_Shape::default);
                        shape.text_body = Some(body);
                    } else if action == DrawingChildAction::Blip {
                        reader.read_to_end_into(ename, &mut Vec::new())?;
                    } else if action == DrawingChildAction::DocPr {
                        for attr in e.attributes() {
                            let attr = attr?;
                            match DrawingDocPrAttribute::read(&attr, e.decoder())? {
                                DrawingDocPrAttribute::Description(value) => {
                                    description = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Title(value) => {
                                    title = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Name(value) => {
                                    name = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Id(value) => doc_pr_id = Some(value),
                                DrawingDocPrAttribute::Ignored => {}
                            }
                        }
                        decorative = read_doc_pr_decorative(reader)?;
                    } else {
                        // Continue into nested elements (graphic, graphicData, pic, etc.)
                    }
                }
                Ok(Event::End(ref e))
                    if DrawingContainerKind::Anchor.matches_end(e.name().as_ref()) =>
                {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(CT_Anchor {
            doc_pr_id: doc_pr_id
                .ok_or_else(|| crate::OxmlError::MissingElement("wp:docPr/@id".to_owned()))?,
            behind_doc,
            pos_h_offset,
            pos_h_relative_from,
            pos_h_align,
            pos_v_offset,
            pos_v_relative_from,
            pos_v_align,
            extent_cx,
            extent_cy,
            dist_t,
            dist_b,
            dist_l,
            dist_r,
            wrap,
            embed_id: String::new(),
            link_id: None,
            chart_rel_id: None,
            relative_height,
            description,
            title,
            decorative,
            name,
            source_rect,
            raw_xml: None, // Will be set by CT_Drawing::from_xml
            source_namespace_bindings: Vec::new(),
            shape,
        })
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.to_xml_in(writer, None)
    }

    fn to_xml_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        crate::document::admit_word_model(crate::document::WordOwner::Anchor(self))?;
        // If we have raw XML from parsing, use it for perfect round-trip
        if let Some(ref raw) = self.raw_xml {
            writer.write_indent()?;
            writer.get_mut().write_all(raw)?;
            return Ok(());
        }

        let payload = drawing_payload(&self.embed_id, self.chart_rel_id.as_deref())?;

        let mut buf = itoa::Buffer::new();
        let mut anchor = crate::text::FieldXmlStart::new("wp:anchor", emission)?;
        anchor.push("behindDoc", if self.behind_doc { "1" } else { "0" })?;
        anchor.push("simplePos", "0")?;
        anchor.push("relativeHeight", buf.format(self.relative_height))?;
        // Every distance is written, as Word does: an absent `distL` or
        // `distR` is an eighth of an inch to Word, not zero.
        anchor.push("distT", buf.format(self.dist_t.0))?;
        anchor.push("distB", buf.format(self.dist_b.0))?;
        anchor.push("distL", buf.format(self.dist_l.0))?;
        anchor.push("distR", buf.format(self.dist_r.0))?;
        anchor.push("locked", "0")?;
        anchor.push("layoutInCell", "1")?;
        anchor.push("allowOverlap", "1")?;
        if emission.is_some_and(crate::text::FieldXmlEmission::note_drawing_bindings) {
            anchor.push("xmlns:r", drawing_ns::R)?;
            anchor.push("xmlns:wp", drawing_ns::WP)?;
        }
        anchor.write(writer, false)?;

        // wp:simplePos
        let mut sp = crate::text::FieldXmlStart::new("wp:simplePos", emission)?;
        sp.push("x", "0")?;
        sp.push("y", "0")?;
        sp.write(writer, true)?;

        // wp:positionH
        let mut pos_h = crate::text::FieldXmlStart::new("wp:positionH", emission)?;
        pos_h.push("relativeFrom", self.pos_h_relative_from.to_str())?;
        pos_h.write(writer, false)?;
        // An anchor positions by an alignment or by an offset, never both.
        if let Some(align) = self.pos_h_align {
            crate::text::FieldXmlStart::new("wp:align", emission)?.write(writer, false)?;
            if let Some(emission) = emission {
                emission.write_text(writer, align.to_str(), None)?;
            } else {
                writer.write_event(Event::Text(BytesText::new(align.to_str())))?;
            }
            writer.write_event(Event::End(BytesEnd::new("wp:align")))?;
        } else {
            crate::text::FieldXmlStart::new("wp:posOffset", emission)?.write(writer, false)?;
            if let Some(emission) = emission {
                emission.write_text(writer, buf.format(self.pos_h_offset.0), None)?;
            } else {
                writer.write_event(Event::Text(BytesText::new(buf.format(self.pos_h_offset.0))))?;
            }
            writer.write_event(Event::End(BytesEnd::new("wp:posOffset")))?;
        }
        writer.write_event(Event::End(BytesEnd::new("wp:positionH")))?;

        // wp:positionV
        let mut pos_v = crate::text::FieldXmlStart::new("wp:positionV", emission)?;
        pos_v.push("relativeFrom", self.pos_v_relative_from.to_str())?;
        pos_v.write(writer, false)?;
        if let Some(align) = self.pos_v_align {
            crate::text::FieldXmlStart::new("wp:align", emission)?.write(writer, false)?;
            if let Some(emission) = emission {
                emission.write_text(writer, align.to_str(), None)?;
            } else {
                writer.write_event(Event::Text(BytesText::new(align.to_str())))?;
            }
            writer.write_event(Event::End(BytesEnd::new("wp:align")))?;
        } else {
            crate::text::FieldXmlStart::new("wp:posOffset", emission)?.write(writer, false)?;
            if let Some(emission) = emission {
                emission.write_text(writer, buf.format(self.pos_v_offset.0), None)?;
            } else {
                writer.write_event(Event::Text(BytesText::new(buf.format(self.pos_v_offset.0))))?;
            }
            writer.write_event(Event::End(BytesEnd::new("wp:posOffset")))?;
        }
        writer.write_event(Event::End(BytesEnd::new("wp:positionV")))?;

        // wp:extent
        let mut extent = crate::text::FieldXmlStart::new("wp:extent", emission)?;
        extent.push("cx", buf.format(self.extent_cx.0))?;
        extent.push("cy", buf.format(self.extent_cy.0))?;
        extent.write(writer, true)?;

        // The wrapping element, in the sequence position wp:wrapNone held.
        write_wrap_element(writer, self.wrap, emission)?;

        // wp:docPr
        write_doc_pr(
            writer,
            self.doc_pr_id,
            self.name.as_deref(),
            self.description.as_deref(),
            self.title.as_deref(),
            self.decorative,
            emission,
        )?;

        write_graphic_element(
            writer,
            payload,
            self.extent_cx,
            self.extent_cy,
            self.name.as_deref(),
            self.source_rect,
            emission,
        )?;

        writer.write_event(Event::End(BytesEnd::new("wp:anchor")))?;
        Ok(())
    }
}

/// `CT_Inline` — An inline drawing (image) element.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Inline {
    /// Non-visual drawing properties identifier (`wp:docPr/@id`).
    pub doc_pr_id: u32,
    /// Width in EMUs
    pub extent_cx: Emu,
    /// Height in EMUs
    pub extent_cy: Emu,
    /// Relationship ID referencing the image part
    pub embed_id: String,
    /// Relationship ID referencing an externally linked image.
    pub link_id: Option<String>,
    /// Relationship ID referencing a ChartML part.
    pub chart_rel_id: Option<String>,
    /// Optional description/alt text
    pub description: Option<String>,
    /// Optional accessibility title.
    pub title: Option<String>,
    /// Whether the qualified decorative extension marks this drawing.
    pub decorative: bool,
    /// Optional name
    pub name: Option<String>,
    /// Optional crop rectangle applied before scaling the picture.
    pub source_rect: Option<SourceRect>,
    /// Raw XML bytes for the entire wp:inline element (used for round-trip preservation).
    /// When present, to_xml uses this instead of structured serialization.
    pub raw_xml: Option<Vec<u8>>,
    /// Whether `doc_pr_id` is still to be given. A picture a run, cell or
    /// paragraph handle adds from a relationship id cannot reach the
    /// document's identifier registry, so the save gives it its id; a
    /// drawing read from a file never carries this mark, whatever its id.
    #[doc(hidden)]
    pub doc_pr_id_unallocated: bool,
}

impl CT_Inline {
    pub fn new(embed_id: &str, width_emu: i64, height_emu: i64) -> Self {
        CT_Inline {
            doc_pr_id: 1,
            extent_cx: Emu(width_emu),
            extent_cy: Emu(height_emu),
            embed_id: embed_id.to_string(),
            link_id: None,
            chart_rel_id: None,
            description: None,
            title: None,
            decorative: false,
            name: None,
            source_rect: None,
            raw_xml: None,
            doc_pr_id_unallocated: false,
        }
    }

    /// Create an inline drawing whose payload is a native Word chart.
    pub fn new_chart(chart_rel_id: &str, width_emu: i64, height_emu: i64) -> Self {
        CT_Inline {
            doc_pr_id: 1,
            extent_cx: Emu(width_emu),
            extent_cy: Emu(height_emu),
            embed_id: String::new(),
            link_id: None,
            chart_rel_id: Some(chart_rel_id.to_owned()),
            description: None,
            title: None,
            decorative: false,
            name: Some("Chart".to_owned()),
            source_rect: None,
            raw_xml: None,
            doc_pr_id_unallocated: false,
        }
    }

    pub fn from_xml(reader: &mut NsReader<&[u8]>) -> Result<Self> {
        let prefixes: Vec<String> = reader
            .resolver()
            .bindings()
            .map(|(prefix, uri)| {
                let name = match prefix {
                    PrefixDeclaration::Default => "",
                    PrefixDeclaration::Named(name) => std::str::from_utf8(name).unwrap_or(""),
                };
                format!("\0{name}\0{}", String::from_utf8_lossy(uri.as_ref()))
            })
            .collect();
        crate::document::admit_word_reader(reader, &prefixes, b"inline", Some(drawing_ns::WP))?;
        Self::from_xml_admitted(reader)
    }

    fn from_xml_admitted(reader: &mut NsReader<&[u8]>) -> Result<Self> {
        let mut cx = Emu(0);
        let mut cy = Emu(0);
        let mut description = None;
        let mut title = None;
        let mut decorative = false;
        let mut name = None;
        let mut source_rect = None;
        let mut doc_pr_id = None;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let ename = e.name();
                    let action = DrawingChildAction::for_name(
                        DrawingContainerKind::Inline,
                        ename.as_ref(),
                        true,
                        canonical_wp_element(reader, e, b"docPr"),
                        false,
                    );
                    if action == DrawingChildAction::Extent {
                        read_extent(e, &mut cx, &mut cy)?;
                    } else if action == DrawingChildAction::DocPr {
                        for attr in e.attributes() {
                            let attr = attr?;
                            match DrawingDocPrAttribute::read(&attr, e.decoder())? {
                                DrawingDocPrAttribute::Description(value) => {
                                    description = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Title(value) => {
                                    title = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Name(value) => {
                                    name = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Id(value) => doc_pr_id = Some(value),
                                DrawingDocPrAttribute::Ignored => {}
                            }
                        }
                    } else if action == DrawingChildAction::SourceRect {
                        source_rect = Some(parse_source_rect(e)?);
                    }
                }
                Ok(Event::Start(ref e)) => {
                    let ename = e.name();
                    let action = DrawingChildAction::for_name(
                        DrawingContainerKind::Inline,
                        ename.as_ref(),
                        false,
                        canonical_wp_element(reader, e, b"docPr"),
                        false,
                    );
                    if action == DrawingChildAction::SourceRect {
                        source_rect = Some(parse_source_rect(e)?);
                        reader.read_to_end_into(ename, &mut Vec::new())?;
                    } else if action == DrawingChildAction::Blip {
                        reader.read_to_end_into(ename, &mut Vec::new())?;
                    } else if action == DrawingChildAction::DocPr {
                        for attr in e.attributes() {
                            let attr = attr?;
                            match DrawingDocPrAttribute::read(&attr, e.decoder())? {
                                DrawingDocPrAttribute::Description(value) => {
                                    description = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Title(value) => {
                                    title = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Name(value) => {
                                    name = Some(value.into_owned())
                                }
                                DrawingDocPrAttribute::Id(value) => doc_pr_id = Some(value),
                                DrawingDocPrAttribute::Ignored => {}
                            }
                        }
                        decorative = read_doc_pr_decorative(reader)?;
                    } else if !matches_local_name(ename.as_ref(), b"inline") {
                        // Continue parsing nested elements (graphic, graphicData, pic, etc.)
                    }
                }
                Ok(Event::End(ref e))
                    if DrawingContainerKind::Inline.matches_end(e.name().as_ref()) =>
                {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(CT_Inline {
            doc_pr_id: doc_pr_id
                .ok_or_else(|| crate::OxmlError::MissingElement("wp:docPr/@id".to_owned()))?,
            extent_cx: cx,
            extent_cy: cy,
            embed_id: String::new(),
            link_id: None,
            chart_rel_id: None,
            description,
            title,
            decorative,
            name,
            source_rect,
            raw_xml: None, // Will be set by CT_Drawing::from_xml
            doc_pr_id_unallocated: false,
        })
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.to_xml_in(writer, None)
    }

    fn to_xml_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        // If we have raw XML from parsing, use it for perfect round-trip
        if let Some(ref raw) = self.raw_xml {
            writer.write_indent()?;
            writer.get_mut().write_all(raw)?;
            return Ok(());
        }

        let payload = drawing_payload(&self.embed_id, self.chart_rel_id.as_deref())?;

        // wp:inline
        let mut buf = itoa::Buffer::new();
        let mut inline = crate::text::FieldXmlStart::new("wp:inline", emission)?;
        inline.push("distT", "0")?;
        inline.push("distB", "0")?;
        inline.push("distL", "0")?;
        inline.push("distR", "0")?;
        if emission.is_some_and(crate::text::FieldXmlEmission::note_drawing_bindings) {
            inline.push("xmlns:r", drawing_ns::R)?;
            inline.push("xmlns:wp", drawing_ns::WP)?;
        }
        inline.write(writer, false)?;

        // wp:extent
        let mut extent = crate::text::FieldXmlStart::new("wp:extent", emission)?;
        extent.push("cx", buf.format(self.extent_cx.0))?;
        extent.push("cy", buf.format(self.extent_cy.0))?;
        extent.write(writer, true)?;

        // wp:docPr
        write_doc_pr(
            writer,
            self.doc_pr_id,
            self.name.as_deref(),
            self.description.as_deref(),
            self.title.as_deref(),
            self.decorative,
            emission,
        )?;

        // a:graphic
        write_graphic_element(
            writer,
            payload,
            self.extent_cx,
            self.extent_cy,
            self.name.as_deref(),
            self.source_rect,
            emission,
        )?;

        writer.write_event(Event::End(BytesEnd::new("wp:inline")))?;

        Ok(())
    }
}

#[derive(Clone, Copy)]
enum DrawingPayload<'a> {
    Picture(&'a str),
    Chart(&'a str),
}

fn drawing_payload<'a>(
    embed_id: &'a str,
    chart_rel_id: Option<&'a str>,
) -> Result<DrawingPayload<'a>> {
    match (
        !embed_id.is_empty(),
        chart_rel_id.filter(|id| !id.is_empty()),
    ) {
        (true, None) => Ok(DrawingPayload::Picture(embed_id)),
        (false, Some(id)) => Ok(DrawingPayload::Chart(id)),
        (true, Some(_)) => Err(crate::OxmlError::InvalidValue(
            "a Word drawing cannot contain both picture and chart relationships".to_owned(),
        )),
        (false, None) => Err(crate::OxmlError::InvalidValue(
            "a Word drawing requires exactly one picture or chart relationship".to_owned(),
        )),
    }
}

fn chart_relationship_id(xml: &[u8]) -> Result<Option<String>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut chart_graphic_data_depth = None;
    loop {
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        match event {
            Event::Start(ref element) => {
                if chart_graphic_data_depth.is_none()
                    && namespace_matches(&namespace, drawing_ns::A, b"a")
                    && matches_local_name(element.name().as_ref(), b"graphicData")
                    && graphic_data_uri_is_chart(element)?
                {
                    chart_graphic_data_depth = Some(depth);
                } else if chart_graphic_data_depth.is_some_and(|parent| depth == parent + 1)
                    && namespace_matches(&namespace, drawing_ns::C, b"c")
                    && matches_local_name(element.name().as_ref(), b"chart")
                {
                    return chart_element_relationship_id(&reader, element);
                }
                depth = depth.checked_add(1).ok_or_else(|| {
                    crate::OxmlError::InvalidValue("drawing XML depth overflow".to_owned())
                })?;
            }
            Event::Empty(ref element)
                if chart_graphic_data_depth.is_some_and(|parent| depth == parent + 1)
                    && namespace_matches(&namespace, drawing_ns::C, b"c")
                    && matches_local_name(element.name().as_ref(), b"chart") =>
            {
                return chart_element_relationship_id(&reader, element);
            }
            Event::End(ref element) => {
                depth = depth.saturating_sub(1);
                if chart_graphic_data_depth == Some(depth)
                    && namespace_matches(&namespace, drawing_ns::A, b"a")
                    && matches_local_name(element.name().as_ref(), b"graphicData")
                {
                    chart_graphic_data_depth = None;
                }
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

fn image_relationship_ids(xml: &[u8]) -> Result<(String, Option<String>)> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut graphic_depth = None;
    let mut graphic_data_depth = None;
    let mut picture_depth = None;
    let mut blip_fill_depth = None;
    let mut picture_count = 0usize;
    let mut blip_count = 0usize;
    let mut relationship_ids = None;
    let mut ambiguous = false;
    loop {
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        match event {
            Event::Start(ref element) => {
                if depth == 1
                    && namespace_matches(&namespace, drawing_ns::A, b"a")
                    && matches_local_name(element.name().as_ref(), b"graphic")
                {
                    graphic_depth = Some(depth);
                } else if graphic_depth.is_some_and(|parent| depth == parent + 1)
                    && namespace_matches(&namespace, drawing_ns::A, b"a")
                    && matches_local_name(element.name().as_ref(), b"graphicData")
                    && graphic_data_uri_is_picture(element)?
                {
                    graphic_data_depth = Some(depth);
                } else if graphic_data_depth.is_some_and(|parent| depth == parent + 1)
                    && namespace_matches(&namespace, drawing_ns::PIC, b"pic")
                    && matches_local_name(element.name().as_ref(), b"pic")
                {
                    picture_count += 1;
                    ambiguous |= picture_count > 1;
                    picture_depth = Some(depth);
                } else if picture_depth.is_some_and(|parent| depth == parent + 1)
                    && namespace_matches(&namespace, drawing_ns::PIC, b"pic")
                    && matches_local_name(element.name().as_ref(), b"blipFill")
                {
                    blip_fill_depth = Some(depth);
                } else if blip_fill_depth.is_some_and(|parent| depth == parent + 1)
                    && namespace_matches(&namespace, drawing_ns::A, b"a")
                    && matches_local_name(element.name().as_ref(), b"blip")
                {
                    blip_count += 1;
                    ambiguous |= blip_count > 1;
                    match blip_relationship_ids(&reader, element)? {
                        Some(ids) if blip_count == 1 => relationship_ids = Some(ids),
                        Some(_) | None => ambiguous = true,
                    }
                }
                depth = depth.checked_add(1).ok_or_else(|| {
                    crate::OxmlError::InvalidValue("drawing XML depth overflow".to_owned())
                })?;
            }
            Event::Empty(ref element) => {
                if graphic_data_depth.is_some_and(|parent| depth == parent + 1)
                    && namespace_matches(&namespace, drawing_ns::PIC, b"pic")
                    && matches_local_name(element.name().as_ref(), b"pic")
                {
                    picture_count += 1;
                    ambiguous |= picture_count > 1;
                } else if blip_fill_depth.is_some_and(|parent| depth == parent + 1)
                    && namespace_matches(&namespace, drawing_ns::A, b"a")
                    && matches_local_name(element.name().as_ref(), b"blip")
                {
                    blip_count += 1;
                    ambiguous |= blip_count > 1;
                    match blip_relationship_ids(&reader, element)? {
                        Some(ids) if blip_count == 1 => relationship_ids = Some(ids),
                        Some(_) | None => ambiguous = true,
                    }
                }
            }
            Event::End(ref element) => {
                depth = depth.saturating_sub(1);
                if blip_fill_depth == Some(depth)
                    && namespace_matches(&namespace, drawing_ns::PIC, b"pic")
                    && matches_local_name(element.name().as_ref(), b"blipFill")
                {
                    blip_fill_depth = None;
                }
                if picture_depth == Some(depth)
                    && namespace_matches(&namespace, drawing_ns::PIC, b"pic")
                    && matches_local_name(element.name().as_ref(), b"pic")
                {
                    picture_depth = None;
                }
                if graphic_data_depth == Some(depth)
                    && namespace_matches(&namespace, drawing_ns::A, b"a")
                    && matches_local_name(element.name().as_ref(), b"graphicData")
                {
                    graphic_data_depth = None;
                }
                if graphic_depth == Some(depth)
                    && namespace_matches(&namespace, drawing_ns::A, b"a")
                    && matches_local_name(element.name().as_ref(), b"graphic")
                {
                    graphic_depth = None;
                }
            }
            Event::Eof => {
                return Ok(if ambiguous || picture_count != 1 || blip_count != 1 {
                    (String::new(), None)
                } else {
                    relationship_ids.unwrap_or_default()
                });
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn blip_relationship_ids(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<Option<(String, Option<String>)>> {
    let mut embed_id = None;
    let mut link_id = None;
    for attribute in element.attributes() {
        let attribute = attribute?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if !namespace_matches(&namespace, drawing_ns::R, b"r") {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())?
            .into_owned();
        let duplicate = match local.as_ref() {
            b"embed" => embed_id.replace(value).is_some(),
            b"link" => link_id.replace(value).is_some(),
            _ => false,
        };
        if duplicate {
            return Ok(None);
        }
    }
    Ok(Some((embed_id.unwrap_or_default(), link_id)))
}

fn graphic_data_uri_is_chart(element: &BytesStart<'_>) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == b"uri" {
            return Ok(attribute.value.as_ref() == drawing_ns::C.as_bytes());
        }
    }
    Ok(false)
}

fn graphic_data_uri_is_picture(element: &BytesStart<'_>) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == b"uri" {
            return Ok(attribute.value.as_ref() == drawing_ns::PIC.as_bytes());
        }
    }
    Ok(false)
}

fn chart_element_relationship_id(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if namespace_matches(&namespace, drawing_ns::R, b"r") && local.as_ref() == b"id" {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn namespace_matches(namespace: &ResolveResult<'_>, expected: &str, _conventional: &[u8]) -> bool {
    match namespace {
        ResolveResult::Bound(Namespace(uri)) => *uri == expected.as_bytes(),
        ResolveResult::Unknown(_) => false,
        ResolveResult::Unbound => false,
    }
}

fn canonical_wp_element(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    expected_local: &[u8],
) -> bool {
    let (namespace, local) = reader.resolver().resolve_element(element.name());
    namespace_matches(&namespace, drawing_ns::WP, b"wp") && local.as_ref() == expected_local
}

/// Project the complete selected text-box body with its inherited namespace scope.
/// Drawing source bytes remain the serializer's authority.
fn text_box_body(
    reader: &mut NsReader<&[u8]>,
    start: &BytesStart<'_>,
    cursor: Option<&crate::document::ControlSourceCursor<'_>>,
    event_start: u64,
) -> Result<crate::document::CT_Body> {
    let mut bindings = Vec::new();
    for (prefix, namespace) in reader.resolver().bindings() {
        let prefix = match prefix {
            PrefixDeclaration::Default => "",
            PrefixDeclaration::Named(prefix) => std::str::from_utf8(prefix)?,
        };
        let namespace = quick_xml::escape::unescape(std::str::from_utf8(namespace.as_ref())?)
            .map_err(quick_xml::Error::from)?;
        bindings.push((prefix.to_owned(), namespace.into_owned()));
    }
    let captured = capture_ns_element(reader, start)?;
    let input = cursor.and_then(|cursor| {
        cursor.field_input(reader, event_start..reader.buffer_position(), &captured)
    });
    let (raw, input) = if let Some(input) = input {
        let (bytes, input) = input.close_namespaces(&captured, &bindings)?;
        (bytes, Some(input))
    } else {
        (
            crate::text::raw_with_external_bindings(&captured, &bindings)?,
            None,
        )
    };
    let mut controls = input.as_ref().map(|input| input.cursor(&raw)).transpose()?;
    let mut body_reader = Reader::from_reader(raw.as_slice());
    let mut buffer = Vec::new();
    let Event::Start(root) = body_reader.read_event_into(&mut buffer)? else {
        return Err(crate::error::OxmlError::InvalidValue(
            "missing text-box body root".into(),
        ));
    };
    let prefixes = crate::numbering::word_prefixes_at(&root, &["w".to_owned()])?.into_owned();
    crate::document::CT_Body::from_xml_with_prefixes_and_owner_bindings_until(
        &mut body_reader,
        &prefixes,
        &bindings,
        b"txbxContent",
        controls.as_mut(),
    )
}

fn capture_ns_element(reader: &mut NsReader<&[u8]>, start: &BytesStart<'_>) -> Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Start(start.to_owned()))?;
    let tag_name = start.name().as_ref().to_vec();
    let mut depth = 1u32;
    let mut buffer = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer)?;
        match event {
            Event::Start(ref element) => {
                if element.name().as_ref() == tag_name {
                    depth += 1;
                }
                writer.write_event(Event::Start(element.to_owned()))?;
            }
            Event::End(ref element) => {
                if element.name().as_ref() == tag_name {
                    depth -= 1;
                }
                writer.write_event(Event::End(element.to_owned()))?;
                if depth == 0 {
                    break;
                }
            }
            Event::Empty(ref element) => {
                writer.write_event(Event::Empty(element.to_owned()))?;
            }
            Event::Text(ref text) => {
                writer.write_event(Event::Text(text.to_owned().into_owned()))?;
            }
            Event::CData(ref text) => {
                writer.write_event(Event::CData(text.to_owned().into_owned()))?;
            }
            Event::Comment(ref text) => {
                writer.write_event(Event::Comment(text.to_owned().into_owned()))?;
            }
            Event::PI(ref text) => {
                writer.write_event(Event::PI(text.to_owned().into_owned()))?;
            }
            Event::Decl(ref declaration) => {
                writer.write_event(Event::Decl(declaration.to_owned().into_owned()))?;
            }
            Event::DocType(ref declaration) => {
                writer.write_event(Event::DocType(declaration.to_owned().into_owned()))?;
            }
            Event::GeneralRef(ref reference) => {
                writer.write_event(Event::GeneralRef(reference.to_owned().into_owned()))?;
            }
            Event::Eof => break,
        }
        buffer.clear();
    }
    Ok(writer.into_inner())
}

/// Write the `a:graphic` payload shared by inline and anchored drawings.
fn write_graphic_element<W: std::io::Write>(
    writer: &mut Writer<W>,
    payload: DrawingPayload<'_>,
    cx: Emu,
    cy: Emu,
    name: Option<&str>,
    source_rect: Option<SourceRect>,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let mut buf = itoa::Buffer::new();
    let mut graphic = crate::text::FieldXmlStart::new("a:graphic", emission)?;
    graphic.push("xmlns:a", drawing_ns::A)?;
    graphic.write(writer, false)?;

    let mut gd = crate::text::FieldXmlStart::new("a:graphicData", emission)?;
    gd.push(
        "uri",
        match payload {
            DrawingPayload::Picture(_) => drawing_ns::PIC,
            DrawingPayload::Chart(_) => drawing_ns::C,
        },
    )?;
    gd.write(writer, false)?;

    if let DrawingPayload::Chart(chart_rel_id) = payload {
        let mut chart = crate::text::FieldXmlStart::new("c:chart", emission)?;
        chart.push("xmlns:c", drawing_ns::C)?;
        chart.push("xmlns:r", drawing_ns::R)?;
        chart.push("r:id", chart_rel_id)?;
        chart.write(writer, true)?;
        writer.write_event(Event::End(BytesEnd::new("a:graphicData")))?;
        writer.write_event(Event::End(BytesEnd::new("a:graphic")))?;
        return Ok(());
    }

    let DrawingPayload::Picture(embed_id) = payload else {
        unreachable!();
    };

    let mut pic = crate::text::FieldXmlStart::new("pic:pic", emission)?;
    pic.push("xmlns:pic", drawing_ns::PIC)?;
    pic.write(writer, false)?;

    // pic:nvPicPr
    crate::text::FieldXmlStart::new("pic:nvPicPr", emission)?.write(writer, false)?;
    let mut cnvpr = crate::text::FieldXmlStart::new("pic:cNvPr", emission)?;
    cnvpr.push("id", "0")?;
    cnvpr.push("name", name.unwrap_or("Picture"))?;
    cnvpr.write(writer, true)?;
    crate::text::FieldXmlStart::new("pic:cNvPicPr", emission)?.write(writer, true)?;
    writer.write_event(Event::End(BytesEnd::new("pic:nvPicPr")))?;

    // pic:blipFill
    crate::text::FieldXmlStart::new("pic:blipFill", emission)?.write(writer, false)?;
    let mut blip = crate::text::FieldXmlStart::new("a:blip", emission)?;
    blip.push("r:embed", embed_id)?;
    blip.write(writer, true)?;
    if let Some(source_rect) = source_rect {
        let mut rect = crate::text::FieldXmlStart::new("a:srcRect", emission)?;
        rect.push("l", buf.format(source_rect.left))?;
        rect.push("t", buf.format(source_rect.top))?;
        rect.push("r", buf.format(source_rect.right))?;
        rect.push("b", buf.format(source_rect.bottom))?;
        rect.write(writer, true)?;
    }
    crate::text::FieldXmlStart::new("a:stretch", emission)?.write(writer, false)?;
    crate::text::FieldXmlStart::new("a:fillRect", emission)?.write(writer, true)?;
    writer.write_event(Event::End(BytesEnd::new("a:stretch")))?;
    writer.write_event(Event::End(BytesEnd::new("pic:blipFill")))?;

    // pic:spPr
    crate::text::FieldXmlStart::new("pic:spPr", emission)?.write(writer, false)?;
    crate::text::FieldXmlStart::new("a:xfrm", emission)?.write(writer, false)?;
    let mut off = crate::text::FieldXmlStart::new("a:off", emission)?;
    off.push("x", "0")?;
    off.push("y", "0")?;
    off.write(writer, true)?;
    let mut ext = crate::text::FieldXmlStart::new("a:ext", emission)?;
    ext.push("cx", buf.format(cx.0))?;
    ext.push("cy", buf.format(cy.0))?;
    ext.write(writer, true)?;
    writer.write_event(Event::End(BytesEnd::new("a:xfrm")))?;
    let mut prst = crate::text::FieldXmlStart::new("a:prstGeom", emission)?;
    prst.push("prst", "rect")?;
    prst.write(writer, false)?;
    crate::text::FieldXmlStart::new("a:avLst", emission)?.write(writer, true)?;
    writer.write_event(Event::End(BytesEnd::new("a:prstGeom")))?;
    writer.write_event(Event::End(BytesEnd::new("pic:spPr")))?;

    writer.write_event(Event::End(BytesEnd::new("pic:pic")))?;
    writer.write_event(Event::End(BytesEnd::new("a:graphicData")))?;
    writer.write_event(Event::End(BytesEnd::new("a:graphic")))?;

    Ok(())
}

pub(crate) fn parse_source_rect(element: &BytesStart<'_>) -> Result<SourceRect> {
    let mut rect = SourceRect::default();
    for attribute in element.attributes() {
        let attribute = attribute?;
        let value = std::str::from_utf8(&attribute.value)?.parse()?;
        match attribute.key.as_ref() {
            b"l" => rect.left = value,
            b"t" => rect.top = value,
            b"r" => rect.right = value,
            b"b" => rect.bottom = value,
            _ => {}
        }
    }
    Ok(rect)
}

/// `CT_Drawing` — A drawing element that wraps inline or anchor images.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Drawing {
    pub inline: Option<CT_Inline>,
    pub anchor: Option<CT_Anchor>,
}

/// Apply the actual Empty-extent attribute action without constructing a drawing.
/// Missing dimensions retain the caller's previous values.
pub(crate) fn read_extent(element: &BytesStart<'_>, cx: &mut Emu, cy: &mut Emu) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let value =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?;
        if key == b"cx" {
            *cx = Emu(value.parse()?);
        } else if key == b"cy" {
            *cy = Emu(value.parse()?);
        }
    }
    Ok(())
}

pub(crate) struct AnchorAttributes {
    behind_doc: bool,
    relative_height: u32,
    dist_t: Emu,
    dist_b: Emu,
    dist_l: Emu,
    dist_r: Emu,
}

/// Word's side distance of a floating object whose anchor has no `distL` or
/// `distR`: 114,300 EMU, an eighth of an inch.
const DEFAULT_ANCHOR_SIDE_DISTANCE: i64 = 114_300;

impl AnchorAttributes {
    pub(crate) fn read(start: &BytesStart<'_>) -> Result<Self> {
        let mut result = Self {
            behind_doc: false,
            relative_height: 0,
            dist_t: Emu(0),
            dist_b: Emu(0),
            // Word keeps text an eighth of an inch from the sides of a
            // floating object whose anchor gives no side distance.
            dist_l: Emu(DEFAULT_ANCHOR_SIDE_DISTANCE),
            dist_r: Emu(DEFAULT_ANCHOR_SIDE_DISTANCE),
        };
        for attribute in start.attributes() {
            let attribute = attribute?;
            let value = std::str::from_utf8(&attribute.value)?;
            match attribute.key.as_ref() {
                b"behindDoc" => result.behind_doc = value == "1" || value == "true",
                b"relativeHeight" => result.relative_height = value.parse().unwrap_or(0),
                b"distT" => result.dist_t = Emu(value.parse().unwrap_or(0)),
                b"distB" => result.dist_b = Emu(value.parse().unwrap_or(0)),
                b"distL" => result.dist_l = Emu(value.parse().unwrap_or(0)),
                b"distR" => result.dist_r = Emu(value.parse().unwrap_or(0)),
                _ => {}
            }
        }
        Ok(result)
    }
}

pub(crate) enum DrawingPositionReference {
    Horizontal(ST_RelativeFromH),
    Vertical(ST_RelativeFromV),
}

impl DrawingPositionReference {
    pub(crate) fn read(start: &BytesStart<'_>, horizontal: bool) -> Result<Option<Self>> {
        let mut selected = None;
        for attribute in start.attributes() {
            let attribute = attribute?;
            if attribute.key.as_ref() == b"relativeFrom" {
                let value = std::str::from_utf8(&attribute.value)?;
                selected = Some(if horizontal {
                    Self::Horizontal(ST_RelativeFromH::from_str(value))
                } else {
                    Self::Vertical(ST_RelativeFromV::from_str(value))
                });
            }
        }
        Ok(selected)
    }
}

/// One actual container event action. Consuming Started actions do not expose
/// their descendants to the container loop.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawingChildAction {
    Extent,
    DocPr,
    SourceRect,
    PositionH,
    PositionV,
    Wrap(WrapType),
    ShapeProperties,
    TextBox,
    Blip,
    Walk,
}

impl DrawingChildAction {
    pub(crate) fn for_name(
        kind: DrawingContainerKind,
        name: &[u8],
        empty: bool,
        doc_pr: bool,
        word_text_box: bool,
    ) -> Self {
        if empty {
            if matches_local_name(name, b"extent") {
                Self::Extent
            } else if doc_pr {
                Self::DocPr
            } else if matches_local_name(name, b"srcRect") {
                Self::SourceRect
            } else if kind == DrawingContainerKind::Anchor {
                wrap_type_of(name).map_or(Self::Walk, Self::Wrap)
            } else {
                Self::Walk
            }
        } else if matches_local_name(name, b"srcRect") {
            Self::SourceRect
        } else if kind == DrawingContainerKind::Anchor {
            if matches_local_name(name, b"positionH") {
                Self::PositionH
            } else if let Some(wrap) = wrap_type_of(name) {
                Self::Wrap(wrap)
            } else if matches_local_name(name, b"positionV") {
                Self::PositionV
            } else if matches_local_name(name, b"spPr") {
                Self::ShapeProperties
            } else if word_text_box {
                Self::TextBox
            } else if matches_local_name(name, b"blip") {
                Self::Blip
            } else if doc_pr {
                Self::DocPr
            } else {
                Self::Walk
            }
        } else if matches_local_name(name, b"blip") {
            Self::Blip
        } else if doc_pr {
            Self::DocPr
        } else {
            Self::Walk
        }
    }
}

/// The actual docPr reader decodes every attribute, including ignored ones.
pub(crate) enum DrawingDocPrAttribute<'a> {
    Description(std::borrow::Cow<'a, str>),
    Title(std::borrow::Cow<'a, str>),
    Name(std::borrow::Cow<'a, str>),
    Id(u32),
    Ignored,
}

impl<'a> DrawingDocPrAttribute<'a> {
    pub(crate) fn read(
        attribute: &'a quick_xml::events::attributes::Attribute<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<Self> {
        let value = attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?;
        Ok(match attribute.key.as_ref() {
            b"descr" => Self::Description(value),
            b"title" => Self::Title(value),
            b"name" => Self::Name(value),
            b"id" => Self::Id(value.parse()?),
            _ => Self::Ignored,
        })
    }
}

/// Direct nonempty drawing containers selected by the actual drawing reader.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DrawingContainerKind {
    Inline,
    Anchor,
}
impl DrawingContainerKind {
    pub(crate) fn matches_end(self, name: &[u8]) -> bool {
        matches_local_name(
            name,
            match self {
                Self::Inline => b"inline",
                Self::Anchor => b"anchor",
            },
        )
    }

    pub(crate) fn for_name(name: &[u8], empty: bool) -> Option<Self> {
        if empty {
            None
        } else if matches_local_name(name, b"inline") {
            Some(Self::Inline)
        } else if matches_local_name(name, b"anchor") {
            Some(Self::Anchor)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DrawingSourceMaps {
    pub(crate) inline: Option<std::sync::Arc<crate::document::WordSourceMap>>,
    pub(crate) anchor: Option<std::sync::Arc<crate::document::WordSourceMap>>,
}

impl CT_Drawing {
    pub fn inline(inline: CT_Inline) -> Self {
        CT_Drawing {
            inline: Some(inline),
            anchor: None,
        }
    }

    pub fn anchor(anchor: CT_Anchor) -> Self {
        CT_Drawing {
            inline: None,
            anchor: Some(anchor),
        }
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        crate::document::admit_word_reader(
            reader,
            &["w".to_owned()],
            b"drawing",
            Some(crate::namespace::W_NS),
        )?;
        Self::from_xml_with_prefixes(reader, &[], &[])
    }

    pub(crate) fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        prefixes: &[String],
        ancestor_prefixes: &[String],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_in(reader, prefixes, ancestor_prefixes, None, None)
    }

    pub(crate) fn from_xml_with_prefixes_in(
        reader: &mut Reader<&[u8]>,
        prefixes: &[String],
        ancestor_prefixes: &[String],
        cursor: Option<&crate::document::ControlSourceCursor<'_>>,
        read_output: Option<&mut DrawingSourceMaps>,
    ) -> Result<Self> {
        let mut bindings = namespace_bindings(prefixes);
        // Fragment callers can supply implicit canonical Word semantics.
        // An explicit foreign rebinding removes that canonical prefix and
        // therefore cannot acquire this projection-only namespace closure.
        if prefixes.iter().any(|prefix| prefix == "w")
            && !bindings.iter().any(|(prefix, _)| prefix == "w")
        {
            bindings.push(("w".to_owned(), crate::namespace::W_NS.to_owned()));
        }

        // The drawing wrapper is regenerated, so only its introduced or
        // rebound bindings disappear. Ancestor bindings remain where they were.
        let ancestors = namespace_bindings(ancestor_prefixes);
        let lost_bindings: Vec<_> = bindings
            .iter()
            .filter(|(prefix, namespace)| {
                !ancestors
                    .iter()
                    .any(|(previous_prefix, previous_namespace)| {
                        previous_prefix == prefix && previous_namespace == namespace
                    })
            })
            .cloned()
            .collect();
        let mut retained = DrawingSourceMaps::default();
        let mut inline = None;
        let mut anchor = None;
        let mut buf = Vec::new();

        loop {
            let event_start = reader.buffer_position();
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    if DrawingContainerKind::for_name(name.as_ref(), false)
                        == Some(DrawingContainerKind::Inline)
                    {
                        // Capture full raw XML, then re-parse for structured fields
                        let (raw, captured) = if read_output.is_some()
                            && let Some(cursor) = cursor
                        {
                            cursor.capture_element(reader, e, event_start)?
                        } else {
                            (capture_element(reader, e)?, None)
                        };
                        let scoped_raw = crate::text::raw_with_external_bindings(&raw, &bindings)?;
                        let mut re_reader = NsReader::from_reader(scoped_raw.as_slice());
                        re_reader.config_mut().trim_text(false);
                        // Skip to the <wp:inline> start
                        let mut rbuf = Vec::new();
                        loop {
                            match re_reader.read_event_into(&mut rbuf) {
                                Ok(Event::Start(ref ie))
                                    if matches_local_name(ie.name().as_ref(), b"inline") =>
                                {
                                    let mut inl = CT_Inline::from_xml_admitted(&mut re_reader)?;
                                    inl.doc_pr_id = non_visual_drawing_id(&scoped_raw)?;
                                    (inl.embed_id, inl.link_id) =
                                        image_relationship_ids(&scoped_raw)?;
                                    inl.chart_rel_id = chart_relationship_id(&scoped_raw)?;
                                    let (stored, map) =
                                        crate::text::raw_with_external_bindings_and_map(
                                            &raw,
                                            &lost_bindings,
                                            None,
                                            captured.as_ref().map(|input| {
                                                std::sync::Arc::new(
                                                    input.projection_parts().0.clone(),
                                                )
                                            }),
                                        )?;
                                    inl.raw_xml = Some(stored);
                                    retained.inline = map;
                                    inline = Some(inl);
                                    break;
                                }
                                Ok(Event::Eof) => break,
                                Err(e) => return Err(e.into()),
                                _ => {}
                            }
                            rbuf.clear();
                        }
                    } else if DrawingContainerKind::for_name(name.as_ref(), false)
                        == Some(DrawingContainerKind::Anchor)
                    {
                        // Capture full raw XML, then re-parse for structured fields
                        let (raw, captured) = if let Some(cursor) = cursor {
                            cursor.capture_element(reader, e, event_start)?
                        } else {
                            (capture_element(reader, e)?, None)
                        };
                        let (scoped_raw, scoped_input) = if let Some(input) = captured.as_ref() {
                            let (bytes, input) = input.close_namespaces(&raw, &bindings)?;
                            (bytes, Some(input))
                        } else {
                            (
                                crate::text::raw_with_external_bindings(&raw, &bindings)?,
                                None,
                            )
                        };
                        let mut re_reader = NsReader::from_reader(scoped_raw.as_slice());
                        re_reader.config_mut().trim_text(false);
                        let mut rbuf = Vec::new();
                        loop {
                            match re_reader.read_event_into(&mut rbuf) {
                                Ok(Event::Start(ref ie))
                                    if matches_local_name(ie.name().as_ref(), b"anchor") =>
                                {
                                    let mut anc = CT_Anchor::from_xml_admitted_in(
                                        &mut re_reader,
                                        ie,
                                        scoped_input
                                            .as_ref()
                                            .map(|input| (input, scoped_raw.as_slice())),
                                    )?;
                                    anc.doc_pr_id = non_visual_drawing_id(&scoped_raw)?;
                                    (anc.embed_id, anc.link_id) =
                                        image_relationship_ids(&scoped_raw)?;
                                    anc.chart_rel_id = chart_relationship_id(&scoped_raw)?;
                                    let (stored, map) =
                                        crate::text::raw_with_external_bindings_and_map(
                                            &raw,
                                            &lost_bindings,
                                            None,
                                            read_output.as_ref().and(captured.as_ref()).map(
                                                |input| {
                                                    std::sync::Arc::new(
                                                        input.projection_parts().0.clone(),
                                                    )
                                                },
                                            ),
                                        )?;
                                    anc.raw_xml = Some(stored);
                                    retained.anchor = map;
                                    // Projection scope excludes only prefixes absent from the
                                    // entire physical owner, including opaque values and text.
                                    // Default bindings and all referenced foreign bindings stay.
                                    let decoded_raw = anc.raw_xml.as_ref().and_then(|raw| {
                                        std::str::from_utf8(raw).ok().and_then(|xml| {
                                            quick_xml::escape::unescape(xml)
                                                .ok()
                                                .map(|xml| xml.into_owned())
                                        })
                                    });
                                    anc.source_namespace_bindings = bindings
                                        .iter()
                                        .filter(|(prefix, _)| {
                                            prefix.is_empty()
                                                || decoded_raw
                                                    .as_ref()
                                                    .is_none_or(|decoded| decoded.contains(prefix))
                                                || anc.raw_xml.as_ref().is_some_and(|raw| {
                                                    raw.windows(prefix.len())
                                                        .any(|bytes| bytes == prefix.as_bytes())
                                                })
                                        })
                                        .cloned()
                                        .collect();
                                    anchor = Some(anc);
                                    break;
                                }
                                Ok(Event::Eof) => break,
                                Err(e) => return Err(e.into()),
                                _ => {}
                            }
                            rbuf.clear();
                        }
                    } else {
                        reader.read_to_end_into(name, &mut Vec::new())?;
                    }
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"drawing") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if let Some(output) = read_output {
            *output = retained;
        }
        Ok(CT_Drawing { inline, anchor })
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role_in(writer, crate::text::XmlWriteRole::Publication, None, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        crate::document::admit_word_model(crate::document::WordOwner::Drawing(self))?;
        let mut root = crate::text::FieldXmlStart::new("w:drawing", emission)?;
        let source = role == crate::text::XmlWriteRole::Source;
        let prefix = if source {
            std::borrow::Cow::Owned(root.qualify(&[], role, source_context)?)
        } else {
            std::borrow::Cow::Borrowed("w")
        };
        root.write(writer, false)?;
        // Raw payloads keep their original consumer. Fresh payloads retain
        // their existing concrete local binding declarations.
        if let Some(inline) = &self.inline {
            inline.to_xml_in(writer, emission)?;
        }
        if let Some(anchor) = &self.anchor {
            anchor.to_xml_in(writer, emission)?;
        }
        if let Some(emission) = emission {
            emission.write_word_end(writer, &prefix, "drawing")?;
        } else if source {
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:drawing"))))?;
        } else {
            writer.write_event(Event::End(BytesEnd::new("w:drawing")))?;
        }
        Ok(())
    }
}

/// Decode only the actually selected unqualified metadata id attribute.
pub(crate) fn drawing_id_value(
    attribute: &quick_xml::events::attributes::Attribute<'_>,
    selected: bool,
    decoder: quick_xml::encoding::Decoder,
) -> Result<Option<u32>> {
    if selected {
        Ok(Some(
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?
                .parse()?,
        ))
    } else {
        Ok(None)
    }
}

fn non_visual_drawing_id(xml: &[u8]) -> Result<u32> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element)
                if namespace_matches(&namespace, drawing_ns::WP, b"wp")
                    && element.local_name().as_ref() == b"docPr" =>
            {
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    if let Some(value) = drawing_id_value(
                        &attribute,
                        matches!(namespace, ResolveResult::Unbound) && local.as_ref() == b"id",
                        element.decoder(),
                    )? {
                        return Ok(value);
                    }
                }
                return Err(crate::OxmlError::InvalidValue(
                    "wp:docPr requires an unqualified id attribute".to_owned(),
                ));
            }
            Event::Eof => {
                return Err(crate::OxmlError::MissingElement("wp:docPr".to_owned()));
            }
            _ => {}
        }
        buffer.clear();
    }
}
