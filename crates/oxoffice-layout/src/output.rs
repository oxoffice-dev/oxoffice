//! Output types for the layout engine: positioned page frames, glyph runs, etc.

use std::num::NonZeroU32;
use std::sync::Arc;

use crate::paint::{GradientStop, Paint, Stroke};
use crate::path::Path;
use crate::transform::Transform;

/// A point in 2D space (in typographic points from the top-left corner).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// An axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// An RGBA color with components in [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub const BLACK: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    /// Parse a hex color string like "FF0000" to Color.
    pub fn from_hex(hex: &str) -> Self {
        let hex = hex.trim_start_matches('#');
        if hex.len() >= 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f64 / 255.0;
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f64 / 255.0;
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f64 / 255.0;
            Color { r, g, b, a: 1.0 }
        } else {
            Color::BLACK
        }
    }
}

/// Opaque font identifier assigned by FontManager.
///
/// Ordered, so a backend keying a map on it can iterate in a fixed order rather
/// than a hashed one. The PDF writer does exactly that, and the order it
/// iterates in reaches the bytes it writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontId(pub u32);

/// Stable content-addressed media key for renderer-local reuse.
///
/// This compact key is not a collision-free content guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaId(pub u64);

impl MediaId {
    /// Derive a stable key from raw media bytes using 64-bit FNV-1a.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self(hash)
    }
}

/// Result-local identity of one format-specific source node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceNodeId(NonZeroU32);

impl SourceNodeId {
    /// Construct an identity from its one-based side-table index.
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Return the one-based side-table index.
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Exclusive Unicode-scalar range within one source node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub node: SourceNodeId,
    pub char_start: u32,
    pub char_end: u32,
}

/// Format-neutral identity of one field within a source node.
///
/// It is separate from [`SourceSpan`] so that attributing a field does not
/// change which runs a backend groups as contiguous source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldSource {
    pub node: SourceNodeId,
    /// Zero-based preorder position among the node's fields, including nested operands and result fields.
    pub index: u32,
}

/// Stable result-local identity of one logical structure element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StructureId(NonZeroU32);

impl StructureId {
    /// Construct an identity from its one-based node index.
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Return the one-based node index.
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Backend-neutral semantic role for one logical structure element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StructureRole {
    Document,
    Paragraph,
    Heading(u8),
    List,
    ListItem,
    Table,
    TableRow,
    TableHeaderCell,
    TableCell,
    Figure,
}

/// One node in the result-local logical structure tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureNode {
    pub id: StructureId,
    pub role: StructureRole,
    pub children: Vec<StructureId>,
    pub alternate_text: Option<String>,
}

/// Backend-neutral logical structure for a laid-out document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentStructure {
    pub root: StructureId,
    pub nodes: Vec<StructureNode>,
}

impl DocumentStructure {
    /// Resolve a result-local structure identity.
    pub fn node(&self, id: StructureId) -> Option<&StructureNode> {
        self.nodes
            .get(id.get() as usize - 1)
            .filter(|node| node.id == id)
    }
}

/// Kind of field for post-pagination substitution.
///
/// Target carriers stay format-neutral at this shared layout boundary.
///
/// ```
/// use oxoffice_layout::FieldKind;
///
/// assert_eq!(FieldKind::TargetPage(3), FieldKind::TargetPage(3));
/// assert_eq!(FieldKind::Target(3), FieldKind::Target(3));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Current page number.
    Page,
    /// Total number of pages.
    NumPages,
    /// One-based owning section number.
    Section,
    /// Physical pages occupied by the owning section.
    SectionPages,
    /// Page containing a target.
    TargetPage(usize),
    /// Zero-width target position retained until page locations are collected.
    Target(usize),
    /// Zero-width Word sequence event position in the current source snapshot.
    SequenceContext(usize),
    /// Word sequence repeat resolved from its physical source and page context.
    SequenceRepeat,
}

/// A positioned run of shaped glyphs.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphRun {
    /// Baseline origin of the first glyph (in points).
    pub origin: Point,
    /// Font identifier (from FontManager).
    pub font_id: FontId,
    /// Font size in points.
    pub font_size: f64,
    /// Shaped glyph IDs.
    pub glyph_ids: Vec<u16>,
    /// Per-glyph advances in points.
    pub advances: Vec<f64>,
    /// Original text (for PDF ToUnicode mapping).
    pub text: String,
    /// Exact source range for this run, when it is a direct text projection.
    pub source: Option<SourceSpan>,
    /// Text color.
    pub color: Color,
    /// Whether the font is bold.
    pub bold: bool,
    /// Whether the font is italic.
    pub italic: bool,
    /// If this glyph run is a field placeholder, the kind of field.
    pub field_kind: Option<FieldKind>,
    /// Source field of a page-number placeholder, when attributable.
    pub field_source: Option<FieldSource>,
    /// If this glyph run is a footnote/endnote reference marker, its ID.
    pub note: Option<crate::line::NoteRef>,
    /// Structural source of a note reference, independent of generated glyph text.
    pub note_reference_source: Option<SourceSpan>,
    /// Where a field placeholder sits in text aligned on a tab stop.
    pub tab_aligned: Option<TabAlignedField>,
}

/// Where a field placeholder sits in text aligned on a right, centre or
/// decimal tab stop.
///
/// Page fields are laid out with a fixed placeholder and take their value
/// after pagination, in place. A value narrower or wider than the
/// placeholder would leave the aligned text short of its stop or past it, so
/// the substitution moves the text by `shift` times the change in width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabAlignedField {
    /// Left edge of the text after the tab, in points.
    pub start: f64,
    /// Right edge of the text after the tab, in points.
    pub end: f64,
    /// Share of the field's change in width the aligned text moves back by:
    /// 1 on a right stop, one half on a centre stop, and on a decimal stop 1
    /// before the decimal point and 0 after it.
    pub shift: f64,
    /// The tab's width before it was kept from going negative. The text
    /// moves back no further than the tab allows.
    pub gap: f64,
}

/// A positioned multilingual span with complete two-axis glyph positioning.
#[derive(Debug, Clone, PartialEq)]
pub struct MultilingualGlyphRun {
    pub origin: Point,
    pub font_id: FontId,
    pub font_size: f64,
    pub glyph_ids: Vec<u16>,
    pub x_advances: Vec<f64>,
    pub y_advances: Vec<f64>,
    pub x_offsets: Vec<f64>,
    pub y_offsets: Vec<f64>,
    pub clusters: Vec<crate::font::GlyphCluster>,
    pub logical_text: String,
    pub logical_index: usize,
    pub source: Option<SourceSpan>,
    pub script: crate::font::TextScript,
    pub language: Option<String>,
    pub direction: crate::font::TextDirection,
    pub bidi_level: u8,
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
    pub field_kind: Option<FieldKind>,
    pub field_source: Option<FieldSource>,
    pub note: Option<crate::line::NoteRef>,
    /// Structural source of a note reference, independent of generated glyph text.
    pub note_reference_source: Option<SourceSpan>,
}

impl MultilingualGlyphRun {
    /// Whether all glyph-position and logical-cluster vectors form one complete run.
    pub fn is_valid(&self) -> bool {
        let glyph_count = self.glyph_ids.len();
        self.x_advances.len() == glyph_count
            && self.y_advances.len() == glyph_count
            && self.x_offsets.len() == glyph_count
            && self.y_offsets.len() == glyph_count
            && unicode_bidi::Level::new(self.bidi_level).is_ok()
            && crate::font::position_values_are_finite(
                &self.x_advances,
                &self.y_advances,
                &self.x_offsets,
                &self.y_offsets,
                self.x_advances.iter().sum(),
            )
            && crate::font::cluster_ranges_are_valid(
                &self.clusters,
                glyph_count,
                self.logical_text.chars().count(),
                self.bidi_level % 2 == 1,
            )
    }

    /// Return a legacy horizontal projection for backends that cannot consume
    /// the richer positioning and cluster metadata directly.
    pub fn legacy_projection(&self) -> GlyphRun {
        GlyphRun {
            origin: self.origin,
            font_id: self.font_id,
            font_size: self.font_size,
            glyph_ids: self.glyph_ids.clone(),
            advances: self.x_advances.clone(),
            text: self.logical_text.clone(),
            source: self.source,
            color: self.color,
            bold: self.bold,
            italic: self.italic,
            field_kind: self.field_kind,
            field_source: self.field_source,
            note: self.note,
            note_reference_source: self.note_reference_source,
            tab_aligned: None,
        }
    }
}

/// A positioned element on a page.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PositionedElement {
    /// A run of shaped text glyphs.
    Text(GlyphRun),
    /// A visually positioned span that retains logical text and clusters.
    MultilingualText(MultilingualGlyphRun),
    /// A line segment (for borders, underlines, strikethrough).
    Line {
        start: Point,
        end: Point,
        width: f64,
        color: Color,
        /// Optional dash pattern (dash_on, dash_off) in points. None = solid line.
        dash_pattern: Option<(f64, f64)>,
    },
    /// A filled rectangle (for shading, highlights).
    FilledRect { rect: Rect, color: Color },
    /// An inline image.
    Image {
        rect: Rect,
        data: Vec<u8>,
        content_type: String,
        media_id: MediaId,
    },
    /// A link annotation (hyperlink). A `url` that starts with `#` is an
    /// internal link to the [`PositionedElement::Destination`] of that name.
    LinkAnnotation { rect: Rect, url: String },
    /// Where an internal link of this name lands: a point on the page that
    /// draws nothing.
    Destination { name: String, point: Point },
    /// A backend-neutral filled or stroked path.
    Path(PathElement),
    /// A nested group with one child-local transform.
    Group(GroupElement),
    /// The drawing of one form field's value on one line, and where the
    /// field's widget goes.
    ///
    /// A PDF writer moves the children into the field's appearance. Every
    /// other consumer draws them in place, like [`PositionedElement::MarkedContent`].
    FormField(FormFieldFragment),
    /// Non-drawing semantic ownership for a sequence of positioned elements.
    ///
    /// `structure` is `None` for an artifact that must not enter the logical
    /// reading order.
    MarkedContent {
        structure: Option<StructureId>,
        children: Vec<PositionedElement>,
    },
}

/// Index of a form field in [`LayoutResult::form_fields`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FormFieldId(pub u32);

/// One fillable form field. Its fragments in the pages say where it is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    /// The field name a PDF writer makes unique.
    pub name: String,
    /// A short description viewers show on hover.
    pub tooltip: Option<String>,
    pub kind: FormFieldKind,
    pub read_only: bool,
    /// Size in points of the text a user types.
    pub font_size: f64,
    /// Colour of the text a user types.
    pub color: Color,
    pub alignment: FieldAlignment,
    /// The fragments draw a placeholder, not a value: the field shows
    /// nothing until it is filled.
    pub placeholder: bool,
}

/// What a form field holds.
#[derive(Debug, Clone, PartialEq)]
pub enum FormFieldKind {
    /// Free text, `value` empty when the field is empty.
    Text {
        value: String,
        multiline: bool,
        max_len: Option<u32>,
    },
    /// A box ticked or not. The fragment's `alternate` draws the other state.
    CheckBox { checked: bool },
    /// One option of a list, or typed text when `editable`.
    Choice {
        options: Vec<ChoiceOption>,
        /// The selected option's `value`, or typed text.
        value: Option<String>,
        editable: bool,
    },
    /// An empty signature field, signed later in a PDF viewer.
    Signature,
}

/// One option of a choice field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// What the field's value becomes when the option is chosen.
    pub value: String,
    /// What the list shows.
    pub display: String,
}

/// Horizontal alignment of the text a user types.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FieldAlignment {
    #[default]
    Left,
    Center,
    Right,
}

/// The part of a form field drawn on one line.
#[derive(Debug, Clone, PartialEq)]
pub struct FormFieldFragment {
    pub field: FormFieldId,
    /// Where the widget goes, in the coordinates of the children. The
    /// fragments of one field on a page make one widget over their union.
    pub rect: Rect,
    /// The static drawing of the field's current value or placeholder.
    pub children: Vec<PositionedElement>,
    /// For a check box, the drawing of the other state. Text only.
    pub alternate: Vec<PositionedElement>,
    /// Producer bookkeeping while the layout is built: the marker a
    /// fragment without children still is. A finished layout leaves it
    /// `None`, and the PDF writer never sees one.
    #[doc(hidden)]
    pub mark: Option<FormMarker>,
}

/// Where a paginator drew a marked line item, before the producer gathers
/// what it drew into a fragment. Producer bookkeeping, not layout output.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormMarker {
    pub mark: crate::line::FormMark,
    /// The widget's rectangle when the line draws this field only: its
    /// whole text band, so there is room to type. `None` keeps the
    /// fragment's own rectangle, the item's box.
    pub band: Option<Rect>,
    /// The left edge of the column the item drew in: a field drawn in two
    /// columns stays static.
    pub track: f64,
}

/// One path with optional fill and stroke paints.
#[derive(Debug, Clone, PartialEq)]
pub struct PathElement {
    pub path: Path,
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
}

/// A rendering approximation or fallback message.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub message: String,
}

/// An effect applied to a group.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Effect {
    /// A child-local linear alpha mask over the complete painted group.
    /// Outside bounds is transparent. Stops use only color alpha and pad at
    /// both ends. Invalid masks contribute no visible group paint.
    LinearOpacityMask {
        bounds: Rect,
        start: Point,
        end: Point,
        stops: Vec<GradientStop>,
    },
    OuterShadow {
        dx: f64,
        dy: f64,
        blur: f64,
        color: Color,
    },
}

impl Effect {
    /// Shared validation for the linear opacity mask consumers. Other effects
    /// return true without making any assertion about their own validity.
    #[doc(hidden)]
    pub fn linear_opacity_mask_is_valid(&self) -> bool {
        let Self::LinearOpacityMask {
            bounds,
            start,
            end,
            stops,
        } = self
        else {
            return true;
        };
        [
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            start.x,
            start.y,
            end.x,
            end.y,
        ]
        .into_iter()
        .all(f64::is_finite)
            && bounds.width > 0.0
            && bounds.height > 0.0
            && (bounds.x + bounds.width).is_finite()
            && (bounds.y + bounds.height).is_finite()
            && start != end
            && stops.len() >= 2
            && stops.iter().all(|stop| {
                stop.offset.is_finite()
                    && (0.0..=1.0).contains(&stop.offset)
                    && stop.color.a.is_finite()
                    && (0.0..=1.0).contains(&stop.color.a)
            })
            && stops
                .windows(2)
                .all(|pair| pair[0].offset <= pair[1].offset)
    }
}

/// A group of positioned children in one local coordinate system.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupElement {
    /// Maps child-local coordinates into the parent coordinate system.
    pub transform: Transform,
    pub clip: Option<Path>,
    pub opacity: f64,
    pub effects: Vec<Effect>,
    pub children: Vec<PositionedElement>,
}

/// Visit every non-group element in depth-first document order.
///
/// A form field fragment is visited through its children, not its
/// `alternate`.
pub fn walk(elements: &[PositionedElement], f: &mut impl FnMut(&PositionedElement, &Transform)) {
    fn visit(
        elements: &[PositionedElement],
        accumulated: Transform,
        f: &mut dyn FnMut(&PositionedElement, &Transform),
    ) {
        for element in elements {
            match element {
                PositionedElement::Group(group) => {
                    let child_to_page = group.transform.then(accumulated);
                    visit(&group.children, child_to_page, f);
                }
                PositionedElement::MarkedContent { children, .. }
                | PositionedElement::FormField(FormFieldFragment { children, .. }) => {
                    visit(children, accumulated, f);
                }
                leaf => f(leaf, &accumulated),
            }
        }
    }

    visit(elements, Transform::IDENTITY, f);
}

/// A single page of laid-out content.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PageFrame {
    /// 1-based page number.
    pub page_number: usize,
    /// 1-based displayed page number after a section restart.
    pub displayed_page_number: usize,
    /// Page width in points.
    pub width: f64,
    /// Page height in points.
    pub height: f64,
    /// All positioned elements on this page.
    pub elements: Vec<PositionedElement>,
    /// Optional paint behind every page element.
    pub background: Option<Paint>,
}

impl PageFrame {
    /// Construct a page with no background paint.
    ///
    /// ```
    /// use oxoffice_layout::PageFrame;
    ///
    /// let page = PageFrame::new(1, 612.0, 792.0, Vec::new());
    /// assert!(page.background.is_none());
    /// ```
    pub fn new(
        page_number: usize,
        width: f64,
        height: f64,
        elements: Vec<PositionedElement>,
    ) -> Self {
        Self {
            page_number,
            displayed_page_number: page_number,
            width,
            height,
            elements,
            background: None,
        }
    }
}

/// Font data for embedding in PDF output.
#[derive(Debug, Clone)]
pub struct FontData {
    /// Font identifier.
    pub id: FontId,
    /// Font family name.
    pub family: String,
    /// Raw TTF/OTF bytes for PDF embedding.
    pub data: Arc<[u8]>,
    /// Face index within a font collection.
    pub face_index: u32,
    /// Whether this is a bold variant.
    pub bold: bool,
    /// Whether this is an italic variant.
    pub italic: bool,
    /// Exact selected nondefault OpenType axis coordinates.
    pub variations: Vec<([u8; 4], f32)>,
    /// Paint selected outlines with deterministic1/24em centered stroke.
    pub synthetic_bold: bool,
    /// Paint selected outlines with deterministic12degree baseline shear.
    pub synthetic_italic: bool,
}

/// Document metadata to pass through to PDF output.
#[derive(Debug, Clone, Default)]
pub struct DocumentMetadata {
    /// Document title.
    pub title: Option<String>,
    /// Document author.
    pub author: Option<String>,
    /// Document subject.
    pub subject: Option<String>,
    /// Document keywords.
    pub keywords: Option<String>,
    /// Creator application.
    pub creator: Option<String>,
}

/// An outline/bookmark entry for PDF generation.
#[derive(Debug, Clone)]
pub struct OutlineEntry {
    /// The heading text.
    pub title: String,
    /// Heading level (1 for Heading1, 2 for Heading2, etc.).
    pub level: u32,
    /// 0-based page index this heading appears on.
    pub page_index: usize,
    /// Y position on the page (in points from top).
    pub y_position: f64,
}

/// The complete result of laying out a document.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LayoutResult {
    /// Laid-out pages.
    pub pages: Vec<Arc<PageFrame>>,
    /// Font data for all fonts used.
    pub fonts: Vec<FontData>,
    /// Optional document metadata for PDF output.
    pub metadata: Option<DocumentMetadata>,
    /// Outline/bookmark entries from headings.
    pub outlines: Vec<OutlineEntry>,
    /// Rendering approximations and fallbacks collected during layout.
    pub diagnostics: Vec<Diagnostic>,
    /// Logical structure for tagged PDF output, when the producer has source
    /// semantics to preserve.
    pub structure: Option<DocumentStructure>,
    /// Fillable form fields, indexed by [`FormFieldId`]. Empty unless the
    /// producer was asked for fields.
    pub form_fields: Vec<FormField>,
}

impl LayoutResult {
    /// Construct a result with no diagnostics.
    ///
    /// ```
    /// use oxoffice_layout::LayoutResult;
    ///
    /// let result = LayoutResult::new(Vec::new(), Vec::new(), None, Vec::new());
    /// assert!(result.diagnostics.is_empty());
    /// ```
    pub fn new(
        pages: Vec<Arc<PageFrame>>,
        fonts: Vec<FontData>,
        metadata: Option<DocumentMetadata>,
        outlines: Vec<OutlineEntry>,
    ) -> Self {
        Self {
            pages,
            fonts,
            metadata,
            outlines,
            diagnostics: Vec::new(),
            structure: None,
            form_fields: Vec::new(),
        }
    }
}
