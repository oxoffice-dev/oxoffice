//! Line breaking: converts inline items into laid-out lines.
//!
//! Uses a greedy algorithm with unicode-linebreak for break opportunities.

use crate::error::{LayoutError, Result};
use crate::font::{FontManager, MultilingualTextSegment, TextDirection, explicit_direction_levels};
use crate::output::{
    Color, FieldKind, FieldSource, FontId, GroupElement, MediaId, SourceNodeId, SourceSpan,
    StructureId,
};

/// A tab stop positioned in typographic points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabStop {
    pub pos_pt: f64,
    pub align: TabAlign,
    pub leader: Option<TabLeader>,
}

/// Paragraph alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    Justify,
    Distribute,
}

/// Alignment relative to a tab stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAlign {
    Left,
    Center,
    Right,
    Decimal,
    Bar,
}

/// Leader style used to fill a tab gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabLeader {
    None,
    Dot,
    Hyphen,
    Underscore,
    Heavy,
    MiddleDot,
}

/// Underline style applied to a text segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Underline {
    Single,
    Words,
    Double,
    Thick,
    Dotted,
    Dash,
    DotDash,
    DotDotDash,
    Wave,
}

/// Line height rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSpacing {
    Single,
    /// A multiple of the largest text point size on the line.
    Multiple(f64),
    Exact(f64),
    AtLeast(f64),
}

/// An inline item to be placed on a line.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum InlineItem {
    /// A shaped text segment.
    Text(TextSegment),
    /// A validated script, font, and bidi-level text span.
    MultilingualText(MultilingualTextSegment),
    /// A shaped text segment eligible for language-aware automatic hyphenation.
    HyphenatedText {
        segment: TextSegment,
        language: String,
    },
    /// A tab character, and the fill its run draws behind it (a highlight
    /// or shading) across the tab's width.
    Tab { fill: Option<Color> },
    /// A forced line break, and where the text after it restarts beside
    /// floating objects.
    LineBreak(LineClear),
    /// A forced page break.
    PageBreak,
    /// A forced column break.
    ColumnBreak,
    /// An inline image.
    Image {
        width: f64,
        height: f64,
        media_id: MediaId,
        /// The form field the image draws part of, if any.
        form_mark: Option<FormMark>,
    },
    /// A backend-neutral group with child-local coordinates.
    Group {
        width: f64,
        height: f64,
        /// Distance from the group top to its text baseline. `None` keeps the
        /// established top-aligned drawing behavior.
        baseline: Option<f64>,
        /// An equation: its extent sets the line's ascent and descent, but
        /// proportional line spacing scales the text of the line, not the
        /// equation.
        math: bool,
        group: GroupElement,
    },
    /// An informative drawing carried to a semantic output container.
    Figure {
        item: Box<InlineItem>,
        alternate_text: String,
        structure_id: Option<StructureId>,
    },
    /// A numbering marker (rendered before the first line).
    Marker(TextSegment),
}

/// Which stream a note reference belongs to.
///
/// A reference carries only a number in the markup, and the two streams
/// number independently, so a document can hold a footnote and an endnote
/// that share a number. Without the stream the two are indistinguishable and
/// one silently shadows the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteStream {
    /// Rendered at the foot of the page carrying the reference.
    Footnote,
    /// Rendered at the end of the document.
    Endnote,
}

/// A reference to one note, unique across both streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoteRef {
    pub stream: NoteStream,
    pub id: i32,
}

/// A shaped text segment with associated formatting.
#[derive(Debug, Clone)]
pub struct TextSegment {
    pub text: String,
    /// Requested character direction before paragraph-wide bidi resolution.
    pub direction: TextDirection,
    /// Exact source range for this segment, when directly attributable.
    pub source: Option<SourceSpan>,
    pub font_id: FontId,
    /// The face the line is measured on, when the producer measures on the
    /// face of the requested family rather than on `font_id`. A face picked
    /// only because it has the glyphs then draws the text without making its
    /// line taller or shorter. `None` measures on `font_id`.
    pub measure_font: Option<FontId>,
    pub font_size: f64,
    pub glyph_ids: Vec<u16>,
    pub advances: Vec<f64>,
    pub width: f64,
    pub ascent: f64,
    pub descent: f64,
    /// Additional font leading included in the natural line advance.
    pub line_gap: f64,
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
    /// Underline style (None = no underline).
    pub underline: Option<Underline>,
    /// Single strikethrough.
    pub strike: bool,
    /// Double strikethrough.
    pub dstrike: bool,
    /// Highlight/background color for the run.
    pub highlight: Option<Color>,
    /// Baseline offset in points (positive = raise, negative = lower).
    pub baseline_offset: f64,
    /// Hyperlink URL if this segment is inside a hyperlink.
    pub hyperlink_url: Option<String>,
    /// If this segment is a field placeholder, the kind of field.
    pub field_kind: Option<FieldKind>,
    /// Source field of a page-number placeholder, when attributable.
    pub field_source: Option<FieldSource>,
    /// If this segment is a note reference marker, which note it points at.
    pub note: Option<NoteRef>,
    /// Structural source of a note reference, independent of generated glyph text.
    pub note_reference_source: Option<SourceSpan>,
    /// The form field this segment draws part of, when the producer was
    /// asked for form fields.
    pub form_mark: Option<FormMark>,
}

/// Which form field a line item belongs to, local to its paragraph: the
/// paragraph's source node and the field's position among the paragraph's
/// fields. A producer resolves it to a [`crate::FormFieldId`] after layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FormMark {
    pub node: SourceNodeId,
    pub field: u32,
}

/// A single item positioned on a line.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum LineItem {
    Text(TextSegment),
    /// A validated script, font, and bidi-level text span.
    MultilingualText(MultilingualTextSegment),
    Tab {
        width: f64,
        /// Pre-shaped leader text to fill the tab gap (e.g., dots, hyphens).
        leader: Option<TextSegment>,
        /// The stop's alignment. Text after a right, centre or decimal stop
        /// is aligned on it up to the next tab or the end of the line.
        align: TabAlign,
        /// The width the tab would take if the text after it could start
        /// before it. It is below `width` only when that text is too wide to
        /// align on its stop.
        gap: f64,
        /// The fill drawn behind the tab's width, from its run.
        fill: Option<Color>,
    },
    Image {
        width: f64,
        height: f64,
        media_id: MediaId,
        /// The form field the image draws part of, if any.
        form_mark: Option<FormMark>,
    },
    Group {
        width: f64,
        height: f64,
        /// Distance from the group top to its text baseline.
        baseline: Option<f64>,
        /// An equation, as on [`InlineItem::Group`].
        math: bool,
        group: GroupElement,
    },
    /// An informative drawing carried to a semantic output container.
    Figure {
        item: Box<LineItem>,
        alternate_text: String,
        structure_id: Option<StructureId>,
    },
    Marker(TextSegment),
}

impl LineItem {
    pub fn width(&self) -> f64 {
        match self {
            LineItem::Text(seg) => seg.width,
            LineItem::MultilingualText(seg) => seg.width(),
            LineItem::Tab { width, .. } => *width,
            LineItem::Image { width, .. } => *width,
            LineItem::Group { width, .. } => *width,
            LineItem::Figure { item, .. } => item.width(),
            LineItem::Marker(seg) => seg.width,
        }
    }
}

/// Where the text after a forced line break restarts beside the floating
/// objects that wrap text (Word's `w:clear`). The line breaker only carries
/// it: whoever places the floats moves the next line below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineClear {
    /// The next line, as any line break.
    #[default]
    None,
    /// Below the floats on the left.
    Left,
    /// Below the floats on the right.
    Right,
    /// Below every float.
    All,
}

/// The kind of explicit break that ended a laid-out line.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForcedBreakKind {
    Line(LineClear),
    Page,
    Column,
}

/// A laid-out line within a paragraph.
#[derive(Debug, Clone)]
pub struct LayoutLine {
    pub items: Vec<LineItem>,
    /// Total content width of the line.
    ///
    /// Spaces that end a line hang past its end, so they can take this past
    /// `available_width`. See [`LayoutLine::hanging_space_counts`].
    pub width: f64,
    /// Maximum ascent on this line (above baseline).
    pub ascent: f64,
    /// Maximum descent on this line (below baseline).
    pub descent: f64,
    /// Effective leading needed to preserve the tallest run's natural advance.
    pub line_gap: f64,
    /// Total line height.
    pub height: f64,
    /// Left indent for this line.
    pub indent_left: f64,
    /// Available width this line was laid out against.
    pub available_width: f64,
    /// Whether this is the last line of the paragraph.
    pub is_last: bool,
    /// The explicit break that ended this line, when present.
    pub forced_break_after: Option<ForcedBreakKind>,
}

impl LayoutLine {
    /// Distance from the line-box top to its text baseline.
    pub fn baseline_offset(&self) -> f64 {
        let leading = self.height - self.ascent - self.descent;
        self.ascent + if leading >= 0.0 { leading / 2.0 } else { 0.0 }
    }

    /// How many items at the visual start and at the visual end of the line
    /// are spaces that end it logically.
    ///
    /// Those spaces hang past the line end, which is its visual right in
    /// left-to-right text and its visual left in right-to-left text. Alignment
    /// leaves them out of the width, and they may pass the available width.
    ///
    /// Plain text is never reordered, so its spaces hang at the visual end,
    /// after an item that is not a space. Line breaking gives the spaces that
    /// end a plain line an item of their own.
    pub fn hanging_space_counts(&self) -> (usize, usize) {
        let is_plain_space = |item: &LineItem| match item {
            LineItem::Text(segment) => is_space_run(&segment.text),
            LineItem::MultilingualText(segment) if segment.uses_legacy_shaping() => {
                is_space_run(segment.text())
            }
            _ => false,
        };
        let plain_end = self
            .items
            .iter()
            .rev()
            .take_while(|item| is_plain_space(item))
            .count();
        if plain_end > 0 && plain_end < self.items.len() {
            return (0, plain_end);
        }
        let last_content = self
            .items
            .iter()
            .filter_map(|item| match item {
                LineItem::MultilingualText(span)
                    if !span.uses_legacy_shaping() && !is_space_run(span.text()) =>
                {
                    Some(span.logical_index())
                }
                _ => None,
            })
            .max();
        let hangs = |item: &LineItem| match (item, last_content) {
            (LineItem::MultilingualText(span), Some(last)) => {
                !span.uses_legacy_shaping()
                    && is_space_run(span.text())
                    && span.logical_index() > last
            }
            _ => false,
        };
        let start = self.items.iter().take_while(|item| hangs(item)).count();
        let end = self.items[start..]
            .iter()
            .rev()
            .take_while(|item| hangs(item))
            .count();
        (start, end)
    }
}

/// Whether text is only U+0020 spaces, the one character UAX 14 classes SP,
/// which is what hangs at a line end.
fn is_space_run(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|character| character == ' ')
}

/// Parameters for line breaking.
#[derive(Debug, Clone)]
pub struct LineBreakParams {
    /// Total available width (page width minus margins).
    pub available_width: f64,
    /// Left indentation in points.
    pub ind_left: f64,
    /// Right indentation in points.
    pub ind_right: f64,
    /// First line indent in points (positive = indent, 0 if hanging).
    pub ind_first_line: f64,
    /// Hanging indent in points (positive = text lines indented relative to first).
    pub ind_hanging: f64,
    /// Whether the paragraph starts at the right edge, as a right-to-left
    /// one does: its first-line and hanging indents apply there, and its tab
    /// stops are measured from the right margin.
    pub first_line_at_right: bool,
    /// Tab stops.
    pub tab_stops: Vec<TabStop>,
    /// Line spacing rule and value.
    pub line_spacing: LineSpacing,
    /// Paragraph justification.
    pub jc: Option<Align>,
    /// Whether width overflow may create automatic line breaks.
    pub wrap: bool,
    /// Extra width kept clear at the start of individual lines, by line index.
    ///
    /// This is how a floating drawing pushes text aside. An empty vector, the
    /// default, reserves nothing and reproduces unwrapped line breaking
    /// exactly.
    pub line_prefix_widths: Vec<f64>,
    /// Extra width kept clear at the end of individual lines, by line index.
    pub line_suffix_widths: Vec<f64>,
    /// Interval between implicit tab stops, in points.
    ///
    /// Word calls this the default tab stop. `36.0`, half an inch, is the
    /// value used when a document says nothing.
    pub default_tab_interval_pt: f64,
    /// Whether a tab stop past the right margin moves to the end of the line.
    ///
    /// Word 2013 and later do this, in a document whose `w:compatibilityMode`
    /// is 15. Earlier versions keep such a stop, and the text after a stop at
    /// or past the margin then runs on past it, which `false` gives.
    pub clamp_tabs_past_margin: bool,
}

impl Default for LineBreakParams {
    fn default() -> Self {
        LineBreakParams {
            available_width: 468.0, // US Letter with 1" margins
            line_prefix_widths: Vec::new(),
            line_suffix_widths: Vec::new(),
            ind_left: 0.0,
            ind_right: 0.0,
            ind_first_line: 0.0,
            ind_hanging: 0.0,
            first_line_at_right: false,
            tab_stops: Vec::new(),
            line_spacing: LineSpacing::Single,
            jc: None,
            wrap: true,
            default_tab_interval_pt: 36.0,
            clamp_tabs_past_margin: false,
        }
    }
}

/// Break inline items into lines using a greedy algorithm.
///
/// Tab stops are measured from the margin the paragraph starts at, as Word
/// measures them (the right one when `first_line_at_right`), so a tab
/// resolves against where its line starts rather than against the line's own
/// origin. A right, centre or decimal tab takes its width from the text after
/// it, up to the next tab or the end of the line.
pub fn break_into_lines(
    items: &[InlineItem],
    params: &LineBreakParams,
    fm: &FontManager,
) -> Result<Vec<LayoutLine>> {
    break_into_lines_recorded(items, params, fm, false).map(|(lines, _)| lines)
}

/// Preserve actual top-level opaque input occurrences through wrapping and bidi.
#[doc(hidden)]
// Keep the neutral bridge in existing public types, without a new model.
#[allow(clippy::type_complexity)]
pub fn break_into_lines_with_opaque_origins(
    items: &[InlineItem],
    params: &LineBreakParams,
    fm: &FontManager,
    base_direction: Option<TextDirection>,
) -> Result<(Vec<LayoutLine>, Vec<(usize, usize, usize)>)> {
    let (mut lines, mut origins) = break_into_lines_recorded(items, params, fm, true)?;
    if let Some(direction) = base_direction {
        reorder_multilingual_lines(&mut lines, direction, Some(&mut origins))?;
    }
    origins.sort_unstable_by_key(|(line, item, _)| (*line, *item));
    let mut used = Vec::new();
    used.try_reserve_exact(items.len())
        .map_err(|_| LayoutError::Layout("opaque origin validation allocation refused".into()))?;
    used.resize(items.len(), false);
    let mut previous = None;
    for &(line, item, input) in &origins {
        let output = lines.get(line).and_then(|line| line.items.get(item));
        let valid = matches!(
            (items.get(input), output),
            (Some(InlineItem::Group { .. }), Some(LineItem::Group { .. }))
                | (
                    Some(InlineItem::Figure { .. }),
                    Some(LineItem::Figure { .. })
                )
        );
        if !valid
            || used.get(input).copied().unwrap_or(true)
            || previous.is_some_and(|key| key >= (line, item))
        {
            return Err(LayoutError::Layout(
                "inconsistent opaque input origin".into(),
            ));
        }
        used[input] = true;
        previous = Some((line, item));
    }
    let output_opaque_count = lines
        .iter()
        .flat_map(|line| &line.items)
        .filter(|item| matches!(item, LineItem::Group { .. } | LineItem::Figure { .. }))
        .count();
    if output_opaque_count != origins.len() {
        return Err(LayoutError::Layout("unbound opaque output origin".into()));
    }
    if items.iter().zip(&used).any(|(item, used)| {
        matches!(item, InlineItem::Group { .. } | InlineItem::Figure { .. }) != *used
    }) {
        return Err(LayoutError::Layout("missing opaque input origin".into()));
    }
    Ok((lines, origins))
}

fn push_layout_line(
    lines: &mut Vec<LayoutLine>,
    origins: &mut Vec<(usize, usize, usize)>,
    line: &mut LineState,
    params: &LineBreakParams,
    fm: &FontManager,
    is_last: bool,
    forced: Option<ForcedBreakKind>,
) -> Result<()> {
    origins
        .try_reserve(line.opaque_origins.len())
        .map_err(|_| LayoutError::Layout("opaque origin allocation refused".into()))?;
    for (item, input) in line.opaque_origins.drain(..) {
        origins.push((lines.len(), item, input));
    }
    lines.push(line.break_line(params, fm, is_last, forced));
    Ok(())
}

// The private core shares the bridge's existing-type return shape.
#[allow(clippy::type_complexity)]
fn break_into_lines_recorded(
    items: &[InlineItem],
    params: &LineBreakParams,
    fm: &FontManager,
    capture_origins: bool,
) -> Result<(Vec<LayoutLine>, Vec<(usize, usize, usize)>)> {
    if items.is_empty() {
        // Empty paragraph still gets one empty line
        return Ok((
            vec![LayoutLine {
                items: Vec::new(),
                width: 0.0,
                ascent: 0.0,
                descent: 0.0,
                line_gap: 0.0,
                height: compute_line_height(0.0, 0.0, 0.0, 0.0, params),
                indent_left: line_indent_at(params, 0, true),
                available_width: line_width_at(params, 0, true),
                is_last: true,
                forced_break_after: None,
            }],
            Vec::new(),
        ));
    }

    let mut lines: Vec<LayoutLine> = Vec::new();
    let mut origins = Vec::new();
    if capture_origins {
        let count = items
            .iter()
            .filter(|item| matches!(item, InlineItem::Group { .. } | InlineItem::Figure { .. }))
            .count();
        count
            .checked_mul(3 * std::mem::size_of::<usize>())
            .ok_or_else(|| LayoutError::Layout("opaque origin byte overflow".into()))?;
        origins
            .try_reserve_exact(count)
            .map_err(|_| LayoutError::Layout("opaque origin allocation refused".into()))?;
    }
    let mut line = LineState::new(params, 0, true);

    // Track the most recent font context for shaping tab leaders
    let mut font_ctx: Option<(FontId, f64)> = None;
    // Initialize from the first text segment if available
    for item in items {
        if let InlineItem::Text(seg)
        | InlineItem::HyphenatedText { segment: seg, .. }
        | InlineItem::Marker(seg) = item
        {
            font_ctx = Some((seg.font_id, seg.font_size));
            break;
        }
        if let InlineItem::MultilingualText(seg) = item {
            font_ctx = Some((seg.font_id(), seg.base().font_size));
            break;
        }
    }

    // Build breakable segments from inline items
    let mut segments =
        std::collections::VecDeque::from(build_breakable_segments(items, fm, capture_origins)?);

    while let Some(seg) = segments.pop_front() {
        match seg {
            BreakableSegment::Items(seg_items, _)
                if matches!(seg_items[..], [InlineItem::Tab { .. }]) =>
            {
                let [InlineItem::Tab { fill }] = seg_items[..] else {
                    unreachable!("matched a lone tab");
                };
                line.settle_tab(fm);
                let mut tab = line.next_tab(params);
                if params.wrap && tab.wraps && !line.items.is_empty() {
                    // A tab with no stop left on its line starts the next one.
                    push_layout_line(&mut lines, &mut origins, &mut line, params, fm, false, None)?;
                    tab = line.next_tab(params);
                }
                line.push_tab(tab, fm, font_ctx, fill);
            }
            BreakableSegment::Items(seg_items, seg_origins) => {
                if params.wrap
                    && !line.items.is_empty()
                    && line.width_with(&seg_items, fm)? > line.limit + 0.01
                    && line.width_with(&seg_items, fm)? - hanging_space_width(&seg_items, fm)?
                        > line.limit + 0.01
                {
                    push_layout_line(&mut lines, &mut origins, &mut line, params, fm, false, None)?;
                }
                // A group wider than a whole line has no break opportunity
                // left: Word breaks it at the character that overflows.
                let (seg_items, seg_origins) = if params.wrap
                    && line.items.is_empty()
                    && line.beside_no_float(params)
                    && line.width_with(&seg_items, fm)? - hanging_space_width(&seg_items, fm)?
                        > line.limit + 0.01
                {
                    match cut_overflowing_group(seg_items, seg_origins, line.limit, true, fm)? {
                        Ok(cut) => {
                            for (item_index, item) in cut.head.iter().enumerate() {
                                line.push(
                                    item,
                                    fm,
                                    cut.head_origins.iter().find_map(|(index, origin)| {
                                        (*index == item_index).then_some(*origin)
                                    }),
                                )?;
                            }
                            push_layout_line(
                                &mut lines,
                                &mut origins,
                                &mut line,
                                params,
                                fm,
                                false,
                                None,
                            )?;
                            segments
                                .push_front(BreakableSegment::Items(cut.tail, cut.tail_origins));
                            continue;
                        }
                        Err(whole) => whole,
                    }
                } else {
                    (seg_items, seg_origins)
                };

                // Add segment items to current line
                for (item_index, item) in seg_items.iter().enumerate() {
                    // Update font context from text segments
                    if let InlineItem::Text(seg) | InlineItem::Marker(seg) = item {
                        font_ctx = Some((seg.font_id, seg.font_size));
                    } else if let InlineItem::MultilingualText(seg) = item {
                        font_ctx = Some((seg.font_id(), seg.base().font_size));
                    }
                    line.push(
                        item,
                        fm,
                        seg_origins
                            .iter()
                            .find_map(|(index, origin)| (*index == item_index).then_some(*origin)),
                    )?;
                }
            }
            BreakableSegment::Hyphenated(boxed) => {
                let HyphenatedSegment {
                    segment,
                    break_points,
                } = *boxed;
                let hanging = terminal_u0020_width(hyphenation_base(&segment)?, fm)?;
                let whole = segment;
                if !params.wrap
                    || line.width_with(std::slice::from_ref(&whole), fm)? <= line.limit + 0.01
                    || line.width_with(std::slice::from_ref(&whole), fm)? - hanging
                        <= line.limit + 0.01
                {
                    font_ctx = Some((segment_font_id(&whole), segment_font_size(&whole)));
                    line.push(&whole, fm, None)?;
                    continue;
                }
                let segment = whole;

                if let Some(FittingHyphenation {
                    prefix,
                    hyphen,
                    remainder,
                    remaining_points,
                }) =
                    fitting_hyphenation(&segment, &break_points, line.used_width(), line.limit, fm)?
                {
                    for item in [prefix, hyphen] {
                        font_ctx = Some((segment_font_id(&item), segment_font_size(&item)));
                        line.push(&item, fm, None)?;
                    }
                    push_layout_line(&mut lines, &mut origins, &mut line, params, fm, false, None)?;
                    segments.push_front(BreakableSegment::Hyphenated(Box::new(
                        HyphenatedSegment {
                            segment: remainder,
                            break_points: remaining_points,
                        },
                    )));
                } else if !line.items.is_empty() {
                    push_layout_line(&mut lines, &mut origins, &mut line, params, fm, false, None)?;
                    segments.push_front(BreakableSegment::Hyphenated(Box::new(
                        HyphenatedSegment {
                            segment,
                            break_points,
                        },
                    )));
                } else {
                    // No hyphenation point fits an empty line: the word is
                    // broken at the character that overflows, as above.
                    let cut = if line.beside_no_float(params) {
                        cut_overflowing_group(vec![segment], Vec::new(), line.limit, false, fm)?
                    } else {
                        Err((vec![segment], Vec::new()))
                    };
                    match cut {
                        Ok(cut) => {
                            let cut_at = cut
                                .head
                                .iter()
                                .map(|item| hyphenation_base(item).map(|base| base.text.len()))
                                .sum::<Result<usize>>()?;
                            for item in &cut.head {
                                font_ctx = Some((segment_font_id(item), segment_font_size(item)));
                                line.push(item, fm, None)?;
                            }
                            push_layout_line(
                                &mut lines,
                                &mut origins,
                                &mut line,
                                params,
                                fm,
                                false,
                                None,
                            )?;
                            let mut tail = cut.tail.into_iter();
                            let (Some(remainder), None) = (tail.next(), tail.next()) else {
                                return Err(LayoutError::Layout(
                                    "a cut word lost its remainder".into(),
                                ));
                            };
                            segments.push_front(BreakableSegment::Hyphenated(Box::new(
                                HyphenatedSegment {
                                    segment: remainder,
                                    break_points: break_points
                                        .iter()
                                        .filter(|point| **point > cut_at)
                                        .map(|point| point - cut_at)
                                        .collect(),
                                },
                            )));
                        }
                        Err((whole, _)) => {
                            for item in &whole {
                                font_ctx = Some((segment_font_id(item), segment_font_size(item)));
                                line.push(item, fm, None)?;
                            }
                        }
                    }
                }
            }
            BreakableSegment::ForcedBreak(break_kind) => {
                let is_last = matches!(break_kind, ForcedBreakKind::Page | ForcedBreakKind::Column);
                push_layout_line(
                    &mut lines,
                    &mut origins,
                    &mut line,
                    params,
                    fm,
                    is_last,
                    Some(break_kind),
                )?;
            }
        }
    }

    // Flush remaining items as the last line
    push_layout_line(&mut lines, &mut origins, &mut line, params, fm, true, None)?;

    for line in &mut lines {
        split_hanging_spaces(&mut line.items, fm)?;
    }

    Ok((lines, origins))
}

/// Give the spaces that end a plain line an item of their own while retaining
/// their glyph advances and source spans.
fn split_hanging_spaces(items: &mut Vec<LineItem>, fm: &FontManager) -> Result<()> {
    if let Some(LineItem::MultilingualText(span)) = items.last()
        && span.uses_legacy_shaping()
    {
        let Some(spaces) = trailing_spaces(span.base(), fm)? else {
            return Ok(());
        };
        if is_space_run(span.text()) {
            return Ok(());
        }
        let span = span.clone();
        let base = span.base();
        let glyph_cut = base.glyph_ids.len() - spaces;
        let char_cut = base.text.chars().count() - spaces;
        if span.clusters().iter().any(|cluster| {
            ((cluster.glyph_start as usize) < glyph_cut && cluster.glyph_end as usize > glyph_cut)
                || ((cluster.char_start as usize) < char_cut
                    && cluster.char_end as usize > char_cut)
        }) {
            return Err(LayoutError::Layout(
                "hanging-space split crosses a shaped cluster".into(),
            ));
        }
        let mut ink = base.clone();
        let mut hanging = base.clone();
        ink.text.truncate(base.text.len() - spaces);
        hanging.text = base.text[base.text.len() - spaces..].to_owned();
        ink.glyph_ids.truncate(glyph_cut);
        ink.advances.truncate(glyph_cut);
        ink.width = ink.advances.iter().sum();
        hanging.glyph_ids = base.glyph_ids[glyph_cut..].to_vec();
        hanging.advances = base.advances[glyph_cut..].to_vec();
        hanging.width = hanging.advances.iter().sum();
        if let Some(source) = base.source {
            ink.source = Some(SourceSpan {
                char_end: source.char_end - spaces as u32,
                ..source
            });
            hanging.source = Some(SourceSpan {
                char_start: source.char_end - spaces as u32,
                ..source
            });
        }
        let build = |base: TextSegment, start: usize, end: usize, char_start: usize| {
            MultilingualTextSegment::new_with_legacy_shaping(
                base,
                span.logical_index(),
                span.language().map(str::to_owned),
                span.script(),
                span.direction(),
                span.bidi_level(),
                span.x_advances()[start..end].to_vec(),
                span.y_advances()[start..end].to_vec(),
                span.x_offsets()[start..end].to_vec(),
                span.y_offsets()[start..end].to_vec(),
                span.clusters()
                    .iter()
                    .filter(|cluster| {
                        cluster.glyph_start as usize >= start && cluster.glyph_end as usize <= end
                    })
                    .map(|cluster| crate::font::GlyphCluster {
                        glyph_start: cluster.glyph_start - start as u32,
                        glyph_end: cluster.glyph_end - start as u32,
                        char_start: cluster.char_start - char_start as u32,
                        char_end: cluster.char_end - char_start as u32,
                    })
                    .collect(),
                span.break_after(),
                span.uses_legacy_shaping(),
            )
            .map(|result| result.inherit_shaping_context(&span))
        };
        let ink = build(ink, 0, glyph_cut, 0)?;
        let hanging = build(hanging, glyph_cut, base.glyph_ids.len(), char_cut)?;
        *items.last_mut().expect("retained shaped owner") = LineItem::MultilingualText(ink);
        items.push(LineItem::MultilingualText(hanging));
        return Ok(());
    }
    let Some(LineItem::Text(segment)) = items.last() else {
        return Ok(());
    };
    if segment.field_kind.is_some() || segment.note.is_some() || is_space_run(&segment.text) {
        return Ok(());
    }
    let Some(spaces) = trailing_spaces(segment, fm)? else {
        return Ok(());
    };
    let Some(LineItem::Text(ink)) = items.last_mut() else {
        unreachable!("the last item is text");
    };
    let glyphs = ink.glyph_ids.len() - spaces;
    let advances = ink.advances.split_off(glyphs);
    let width = advances.iter().sum::<f64>();
    ink.width -= width;
    let source = ink.source.as_mut().map(|source| {
        source.char_end -= spaces as u32;
        SourceSpan {
            char_start: source.char_end,
            char_end: source.char_end + spaces as u32,
            ..*source
        }
    });
    let hanging = TextSegment {
        text: ink.text.split_off(ink.text.len() - spaces),
        glyph_ids: ink.glyph_ids.split_off(glyphs),
        advances,
        width,
        source,
        hyperlink_url: ink.hyperlink_url.clone(),
        form_mark: ink.form_mark,
        ..*ink
    };
    items.push(LineItem::Text(hanging));
    Ok(())
}

/// The line `break_into_lines` is filling.
struct LineState {
    /// The line index drives the per-line reservations a floating drawing
    /// creates, so it is tracked rather than a plain first-or-not flag.
    index: usize,
    /// Where the line starts, measured from the zero indent.
    start: f64,
    /// Width the line is laid out against.
    available: f64,
    /// Width the line may fill before its next segment wraps. It is
    /// `available` unless a tab stop took the line past it.
    limit: f64,
    items: Vec<LineItem>,
    opaque_origins: Vec<(usize, usize)>,
    width: f64,
    ascent: f64,
    descent: f64,
    natural_height: f64,
    font_size: f64,
    /// A right, centre or decimal tab still waiting for the text after it.
    pending: Option<PendingTab>,
}

impl LineState {
    fn new(params: &LineBreakParams, index: usize, is_first_line: bool) -> Self {
        let available = line_width_at(params, index, is_first_line);
        LineState {
            index,
            start: line_indent_at(params, index, is_first_line),
            available,
            limit: available,
            items: Vec::new(),
            opaque_origins: Vec::new(),
            width: 0.0,
            ascent: 0.0,
            descent: 0.0,
            natural_height: 0.0,
            font_size: 0.0,
            pending: None,
        }
    }

    fn push(&mut self, item: &InlineItem, fm: &FontManager, origin: Option<usize>) -> Result<()> {
        if origin.is_some() {
            self.opaque_origins
                .try_reserve(1)
                .map_err(|_| LayoutError::Layout("line opaque origin allocation refused".into()))?;
        }
        let (width, ascent, descent, natural_height, font_size) = item_metrics(item);
        self.ascent = self.ascent.max(ascent);
        self.descent = self.descent.max(descent);
        self.natural_height = self.natural_height.max(natural_height);
        self.font_size = self.font_size.max(font_size);
        match self.pending.as_mut() {
            Some(tab) => {
                tab.add(item, width, fm)?;
                self.width = tab.start + tab.width() + tab.following;
            }
            None => self.width += width,
        }
        if let Some(input) = origin {
            if !matches!(item, InlineItem::Group { .. } | InlineItem::Figure { .. }) {
                return Err(LayoutError::Layout("opaque origin attached to text".into()));
            }
            self.opaque_origins.push((self.items.len(), input));
        }
        self.items.push(inline_to_line_item(item));
        Ok(())
    }

    /// The line's width once `items` are appended to it.
    fn width_with(&self, items: &[InlineItem], fm: &FontManager) -> Result<f64> {
        Ok(match self.pending {
            Some(mut tab) => {
                for item in items {
                    tab.add(item, inline_item_width(item), fm)?;
                }
                tab.visible_line_width()
            }
            None => self.width + items.iter().map(inline_item_width).sum::<f64>(),
        })
    }

    /// Whether no floating object narrows this line. A word too wide for a
    /// narrowed line is not cut: the text goes on past the object.
    fn beside_no_float(&self, params: &LineBreakParams) -> bool {
        line_prefix_width(params, self.index) == 0.0 && line_suffix_width(params, self.index) == 0.0
    }

    /// The width appended text starts from, once a waiting tab has given up
    /// its gap to it.
    fn used_width(&self) -> f64 {
        self.width - self.pending.map_or(0.0, |tab| tab.width())
    }

    /// Where a tab appended to this line goes.
    ///
    /// The first explicit stop past the tab wins, in the order the stops are
    /// listed as Word reads them, bar stops excepted. A hanging indent adds a
    /// left stop at the start indent, which is where the tab after a list
    /// number goes. Past the last explicit stop the tab takes the next
    /// multiple of the default interval.
    ///
    /// Word measures every stop from the margin the paragraph starts at,
    /// whatever its indents: the left one, or the right one in a paragraph
    /// that starts at the right edge. The stops are found in that measure,
    /// the distance from the start margin, and the result is given in the
    /// line's own coordinates, where the line runs from `start` for
    /// `available` points: a line that starts at the right edge is laid out
    /// from its end and then mirrored, so its start edge lies `gap` points
    /// from the right margin. On a left-to-right line both measures agree.
    fn next_tab(&self, params: &LineBreakParams) -> ResolvedTab {
        let margin = params.available_width;
        let gap = if params.first_line_at_right {
            margin - (self.start + self.available)
        } else {
            self.start
        };
        // From the distance to the start margin to the line's coordinates.
        let on_line = |distance: f64| distance - gap + self.start;
        let x = gap + self.width;
        let line_end = gap + self.available;
        // The number's stop is the start indent: the first line of a hanging
        // indent starts before it, and the lines after it start on it.
        let hanging = (params.ind_hanging > 0.0).then_some(TabStop {
            pos_pt: if params.first_line_at_right {
                params.ind_right
            } else {
                params.ind_left
            },
            align: TabAlign::Left,
            leader: None,
        });
        let stops = || {
            params
                .tab_stops
                .iter()
                .copied()
                .filter(|stop| stop.align != TabAlign::Bar)
        };
        let next = match (
            stops().find(|stop| stop.pos_pt > x),
            hanging.filter(|stop| stop.pos_pt > x),
        ) {
            (Some(listed), Some(hanging)) if hanging.pos_pt < listed.pos_pt => Some(hanging),
            (listed, hanging) => listed.or(hanging),
        };
        if let Some(stop) = next {
            // Word 2013 and later move a stop past the far margin to the end
            // of the line. Earlier versions keep it, and the text after a
            // stop at or past the margin then runs on past it. A stop past the
            // end indent lets the text run to the margin. Text after a right,
            // centre or decimal stop is pushed back so it ends at the margin,
            // unless an earlier version keeps the stop at or past it.
            let past_margin = stop.pos_pt > margin;
            let pos = if past_margin && params.clamp_tabs_past_margin {
                line_end
            } else {
                stop.pos_pt
            };
            let reach = if !params.clamp_tabs_past_margin && pos >= margin {
                f64::INFINITY
            } else if pos > line_end {
                margin.max(line_end)
            } else {
                line_end
            };
            return ResolvedTab {
                stop: on_line(pos),
                align: stop.align,
                leader: leader_char(stop.leader),
                // Word 2013 moves a left tab that reaches the end of its line
                // to a line of its own, and the text after it to the next.
                wraps: params.clamp_tabs_past_margin
                    && stop.align == TabAlign::Left
                    && pos > reach - 0.01,
                reach: on_line(reach),
                shift_limit: (params.clamp_tabs_past_margin || pos < margin)
                    .then_some(on_line(margin.max(line_end))),
                mirrored: params.first_line_at_right,
            };
        }
        let interval = if params.default_tab_interval_pt > 0.0 {
            params.default_tab_interval_pt
        } else {
            36.0
        };
        let last = stops()
            .chain(hanging)
            .map(|stop| stop.pos_pt)
            .fold(f64::NEG_INFINITY, f64::max);
        let pos = ((x.max(last) / interval).floor() + 1.0) * interval;
        ResolvedTab {
            stop: on_line(pos),
            align: TabAlign::Left,
            leader: None,
            // A default stop at or past the end of the line is no stop, unless
            // an earlier stop took the line past its end.
            wraps: pos > gap + self.limit - 0.01,
            reach: on_line(line_end),
            shift_limit: None,
            mirrored: params.first_line_at_right,
        }
    }

    fn push_tab(
        &mut self,
        tab: ResolvedTab,
        fm: &FontManager,
        font_ctx: Option<(FontId, f64)>,
        fill: Option<Color>,
    ) {
        self.limit = self.limit.max(tab.reach - self.start);
        let start = self.width;
        let stop = tab.stop - self.start;
        if matches!(
            tab.align,
            TabAlign::Right | TabAlign::Center | TabAlign::Decimal
        ) {
            let pending = PendingTab {
                index: self.items.len(),
                start,
                stop,
                align: tab.align,
                leader: tab.leader,
                following: 0.0,
                trailing: 0.0,
                aligned: 0.0,
                aligned_found: false,
                after_point: false,
                spaces_after_point: 0.0,
                in_number: false,
                shift_limit: tab.shift_limit.map(|limit| limit - self.start),
                font_ctx,
                fill,
                mirrored: tab.mirrored,
            };
            self.width = start + pending.width();
            self.pending = Some(pending);
            // The width is settled once the text after the tab is known.
            self.items.push(LineItem::Tab {
                width: 0.0,
                leader: None,
                align: tab.align,
                gap: 0.0,
                fill,
            });
        } else {
            let width = (stop - start).max(0.0);
            let leader = tab
                .leader
                .and_then(|ch| shape_leader(fm, font_ctx, ch, width));
            self.items.push(LineItem::Tab {
                width,
                leader,
                align: TabAlign::Left,
                gap: width,
                fill,
            });
            self.width += width;
        }
    }

    /// Fix the width of a tab that was waiting for the text after it.
    fn settle_tab(&mut self, fm: &FontManager) {
        if let Some(tab) = self.pending.take() {
            let width = tab.width();
            let leader = tab
                .leader
                .and_then(|ch| shape_leader(fm, tab.font_ctx, ch, width));
            self.items[tab.index] = LineItem::Tab {
                width,
                leader,
                align: tab.align,
                gap: tab.gap(),
                fill: tab.fill,
            };
        }
    }

    /// Close this line and start the next one in its place.
    fn break_line(
        &mut self,
        params: &LineBreakParams,
        fm: &FontManager,
        is_last: bool,
        forced_break_after: Option<ForcedBreakKind>,
    ) -> LayoutLine {
        self.settle_tab(fm);
        let next = LineState::new(params, self.index + 1, false);
        let line = std::mem::replace(self, next);
        let line_gap = effective_line_gap(line.ascent, line.descent, line.natural_height);
        LayoutLine {
            height: compute_line_height(
                line.ascent,
                line.descent,
                line_gap,
                line.font_size,
                params,
            ),
            items: line.items,
            width: line.width,
            ascent: line.ascent,
            descent: line.descent,
            line_gap,
            indent_left: line.start,
            available_width: line.available,
            is_last,
            forced_break_after,
        }
    }
}

/// The stop a tab goes to, in the line's coordinates (see
/// [`LineState::next_tab`]).
struct ResolvedTab {
    stop: f64,
    align: TabAlign,
    leader: Option<char>,
    /// Whether the tab found no stop before the end of its line, so it moves
    /// to the next one.
    wraps: bool,
    /// How far the line may reach once the tab is on it.
    reach: f64,
    /// The position text after a right, centre or decimal tab may not pass.
    shift_limit: Option<f64>,
    /// Whether the line starts at the right edge, so that it is mirrored once
    /// laid out (see [`PendingTab::mirrored`]).
    mirrored: bool,
}

/// A right, centre or decimal tab, which takes its width from the text after
/// it. Positions are measured from the start of the line.
#[derive(Debug, Clone, Copy)]
struct PendingTab {
    /// Index of the tab among the line's items.
    index: usize,
    start: f64,
    stop: f64,
    align: TabAlign,
    leader: Option<char>,
    /// Width of the items after the tab.
    following: f64,
    /// Width of the spaces that end the items after the tab. Word aligns the
    /// text without them, so text that wraps ends on its stop.
    trailing: f64,
    /// For a decimal tab, the width of the text before its alignment point.
    aligned: f64,
    aligned_found: bool,
    /// Whether the text after a decimal point found on a mirrored line still
    /// goes on, reading against the line, and lies before the stop.
    after_point: bool,
    /// Width of the items without text of their own since that text last
    /// went on: they count once it goes on again.
    spaces_after_point: f64,
    /// Whether the text scanned so far ends inside a number.
    in_number: bool,
    shift_limit: Option<f64>,
    font_ctx: Option<(FontId, f64)>,
    fill: Option<Color>,
    /// Whether the line is mirrored once laid out, as a line that starts at
    /// the right edge is. Text that reads against it keeps its own order, so
    /// of a number a decimal stop aligns, the part before the stop, on the
    /// side the line starts from, is the part after its decimal point.
    mirrored: bool,
}

impl PendingTab {
    fn add(&mut self, item: &InlineItem, width: f64, fm: &FontManager) -> Result<()> {
        self.following += width;
        self.trailing = match item {
            InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. }
                if segment.text.chars().all(char::is_whitespace) =>
            {
                self.trailing + width
            }
            InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. } => {
                trailing_space_width(segment, fm)
            }
            InlineItem::MultilingualText(segment)
                if segment.text().chars().all(char::is_whitespace) =>
            {
                self.trailing + width
            }
            InlineItem::MultilingualText(segment) => {
                let trimmed = segment.text().trim_end_matches(char::is_whitespace);
                segment.width() - fm.context_prefix_width(segment, trimmed.len())?
            }
            _ => 0.0,
        };
        if self.align == TabAlign::Decimal && !self.aligned_found {
            // A mirrored line draws text that reads against it, a number or
            // Latin words, the other way round: of such text, what lies
            // before the stop is what follows its alignment point, the rest
            // of that text in later items included, and none of a part of it
            // that holds no point (an integer, the digits of a number split
            // across runs).
            let against = self.mirrored && reads_against_a_mirrored_line(item);
            match decimal_alignment_offset(item, &mut self.in_number, fm)? {
                Some(offset) => {
                    self.aligned += if against { width - offset } else { offset };
                    self.aligned_found = true;
                    self.after_point = against;
                }
                None if against => {}
                None => self.aligned += width,
            }
        } else if self.after_point {
            // Spaces count once the text they separate goes on.
            if reads_right_to_left(item) {
                self.after_point = false;
            } else if reads_against_a_mirrored_line(item) {
                self.aligned += self.spaces_after_point + width;
                self.spaces_after_point = 0.0;
            } else {
                self.spaces_after_point += width;
            }
        }
        Ok(())
    }

    /// The tab's width, which never goes negative: text too wide to end at
    /// its stop starts where the tab does.
    fn width(&self) -> f64 {
        self.gap().max(0.0)
    }

    /// The tab's width before it is kept from going negative.
    fn gap(&self) -> f64 {
        let visible = self.following - self.trailing;
        let before_stop = match self.align {
            TabAlign::Center => visible / 2.0,
            TabAlign::Decimal if self.aligned_found => self.aligned,
            TabAlign::Decimal => self.aligned - self.trailing,
            _ => visible,
        };
        let mut width = self.stop - self.start - before_stop;
        if let Some(limit) = self.shift_limit {
            width = width.min(limit - self.start - visible);
        }
        width
    }

    /// How wide the line is with this tab, less the spaces that end it,
    /// which may run past the stop.
    fn visible_line_width(&self) -> f64 {
        self.start + self.width() + self.following - self.trailing
    }
}

/// Where a decimal tab aligns within one item, as a width from its start.
///
/// Word aligns the first full stop, or else the end of the first number, a
/// comma inside a number belonging to it whatever the document's decimal
/// symbol. `in_number` carries a number from one item to the next.
fn decimal_alignment_offset(
    item: &InlineItem,
    in_number: &mut bool,
    fm: &FontManager,
) -> Result<Option<f64>> {
    let segment = match item {
        InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. } => segment,
        InlineItem::MultilingualText(segment) => segment.base(),
        _ => return Ok(in_number.then_some(0.0)),
    };
    for (index, ch) in segment.text.char_indices() {
        if ch == '.' || (*in_number && !ch.is_ascii_digit() && ch != ',') {
            return Ok(Some(match item {
                InlineItem::MultilingualText(span) => fm.context_prefix_width(span, index)?,
                _ => text_prefix_width(segment, index, fm),
            }));
        }
        *in_number |= ch.is_ascii_digit();
    }
    Ok(None)
}

/// Whether an item's text reads right to left: its first strong character
/// is Hebrew or Arabic. Digits and other weak characters read left to right.
fn reads_right_to_left(item: &InlineItem) -> bool {
    let text = match item {
        InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. } => &segment.text,
        InlineItem::MultilingualText(segment) => segment.text(),
        _ => return false,
    };
    text.chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(|character| {
            matches!(
                character as u32,
                0x0590..=0x08ff | 0xfb1d..=0xfdff | 0xfe70..=0xfeff
            )
        })
}

/// Whether an item reads against a mirrored line: text whose first strong
/// character is not Hebrew or Arabic and that holds a letter, a digit or a
/// full stop. White space, punctuation alone and objects read with the line.
fn reads_against_a_mirrored_line(item: &InlineItem) -> bool {
    let text = match item {
        InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. } => &segment.text,
        InlineItem::MultilingualText(segment) => segment.text(),
        _ => return false,
    };
    !reads_right_to_left(item)
        && text
            .chars()
            .any(|character| character.is_alphanumeric() || character == '.')
}

/// Width of the spaces that end a segment's text.
fn trailing_space_width(segment: &TextSegment, fm: &FontManager) -> f64 {
    let trimmed = segment.text.trim_end_matches(char::is_whitespace);
    if trimmed.len() == segment.text.len() {
        return 0.0;
    }
    segment.width - text_prefix_width(segment, trimmed.len(), fm)
}

/// Width of a segment's text before a byte offset.
fn text_prefix_width(segment: &TextSegment, byte_index: usize, fm: &FontManager) -> f64 {
    if byte_index == 0 {
        return 0.0;
    }
    let prefix = &segment.text[..byte_index];
    if segment.advances.len() == segment.text.chars().count() {
        return segment.advances[..prefix.chars().count()].iter().sum();
    }
    fm.shape_text(segment.font_id, prefix, segment.font_size)
        .map_or(segment.width, |shaped| shaped.width)
}

fn leader_char(leader: Option<TabLeader>) -> Option<char> {
    leader.and_then(|leader| match leader {
        TabLeader::Dot => Some('.'),
        TabLeader::Hyphen => Some('-'),
        TabLeader::Underscore => Some('_'),
        TabLeader::MiddleDot => Some('\u{00B7}'),
        TabLeader::Heavy => Some('_'),
        TabLeader::None => None,
    })
}

/// Break rich text in logical order, then reorder each completed line for painting.
pub fn break_multilingual_into_lines(
    items: &[InlineItem],
    params: &LineBreakParams,
    fm: &FontManager,
    base_direction: TextDirection,
) -> Result<Vec<LayoutLine>> {
    let mut lines = break_into_lines(items, params, fm)?;
    reorder_multilingual_lines(&mut lines, base_direction, None)?;
    Ok(lines)
}

fn reorder_multilingual_lines(
    lines: &mut [LayoutLine],
    base_direction: TextDirection,
    mut origins: Option<&mut Vec<(usize, usize, usize)>>,
) -> Result<()> {
    let mut paragraph_text = String::new();
    let mut line_maps = Vec::with_capacity(lines.len());
    let mut has_text = false;
    for line in lines.iter() {
        let line_start = paragraph_text.len();
        let mut positions = Vec::new();
        for (index, item) in line.items.iter().enumerate() {
            let (text, direction, shaped_level) = match item {
                LineItem::Text(segment) | LineItem::Marker(segment) => {
                    (segment.text.as_str(), segment.direction, None)
                }
                LineItem::MultilingualText(segment) => (
                    segment.text(),
                    segment.base().direction,
                    if segment.uses_legacy_shaping() {
                        None
                    } else {
                        Some(unicode_bidi::Level::new(segment.bidi_level()).map_err(|_| {
                            LayoutError::Layout(
                                "rich text carried an invalid bidi level".to_owned(),
                            )
                        })?)
                    },
                ),
                LineItem::Tab { .. } => ("\t", TextDirection::Auto, None),
                LineItem::Image { .. } | LineItem::Group { .. } | LineItem::Figure { .. } => {
                    ("\u{fffc}", TextDirection::Auto, None)
                }
            };
            has_text |= !text.is_empty();
            let start = paragraph_text.len();
            paragraph_text.push_str(text);
            let end = paragraph_text.len();
            positions.push((
                index,
                start,
                end,
                direction,
                text.chars().all(char::is_whitespace),
                shaped_level,
            ));
        }
        line_maps.push((line_start..paragraph_text.len(), positions));
    }
    if !has_text {
        return Ok(());
    }
    let paragraph_level = match base_direction {
        TextDirection::Auto => None,
        TextDirection::LeftToRight => Some(unicode_bidi::Level::ltr()),
        TextDirection::RightToLeft => Some(unicode_bidi::Level::rtl()),
    };
    let bidi = unicode_bidi::BidiInfo::new(&paragraph_text, paragraph_level);
    let [paragraph] = bidi.paragraphs.as_slice() else {
        return Err(LayoutError::Layout(
            "multilingual line layout requires one bidi paragraph".to_owned(),
        ));
    };
    for (line_index, (line, (line_range, mapped_positions))) in
        lines.iter_mut().zip(line_maps).enumerate()
    {
        let positions = mapped_positions
            .iter()
            .map(|(index, _, _, _, _, _)| *index)
            .collect::<Vec<_>>();
        if positions.is_empty() {
            continue;
        }
        let adjusted_levels = bidi.reordered_levels(paragraph, line_range);
        let levels = mapped_positions
            .into_iter()
            .map(|(_, start, end, direction, whitespace, shaped_level)| {
                let adjusted = adjusted_levels.get(start).copied().ok_or_else(|| {
                    LayoutError::Layout(
                        "multilingual line range exceeded its bidi paragraph".to_owned(),
                    )
                })?;
                Ok(if whitespace {
                    adjusted
                } else if let Some(shaped_level) = shaped_level {
                    shaped_level
                } else if direction == TextDirection::Auto {
                    adjusted
                } else {
                    explicit_direction_levels(
                        &paragraph_text[start..end],
                        direction,
                        paragraph.level,
                    )?
                    .first()
                    .copied()
                    .unwrap_or(adjusted)
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let visual_order = unicode_bidi::BidiInfo::reorder_visual(&levels);
        if let Some(origins) = origins.as_deref_mut() {
            let mut destination_for_old = Vec::new();
            destination_for_old
                .try_reserve_exact(positions.len())
                .map_err(|_| LayoutError::Layout("bidi opaque origin allocation refused".into()))?;
            destination_for_old.resize(positions.len(), usize::MAX);
            for (destination, old) in visual_order.iter().copied().enumerate() {
                destination_for_old[old] = positions[destination];
            }
            for (origin_line, origin_item, _) in origins
                .iter_mut()
                .filter(|(origin_line, _, _)| *origin_line == line_index)
            {
                let old = positions.binary_search(origin_item).map_err(|_| {
                    LayoutError::Layout("bidi opaque origin lost its actual slot".into())
                })?;
                *origin_line = line_index;
                *origin_item = destination_for_old[old];
            }
        }
        let logical_items = positions
            .iter()
            .zip(&levels)
            .map(|(index, level)| match &line.items[*index] {
                LineItem::MultilingualText(segment) if !segment.uses_legacy_shaping() => Ok(
                    LineItem::MultilingualText(multilingual_segment_with_level(segment, *level)?),
                ),
                item => Ok(item.clone()),
            })
            .collect::<Result<Vec<_>>>()?;
        for (visual_slot, logical_index) in positions.into_iter().zip(visual_order) {
            line.items[visual_slot] = logical_items[logical_index].clone();
        }
    }
    Ok(())
}

fn multilingual_segment_with_level(
    segment: &MultilingualTextSegment,
    level: unicode_bidi::Level,
) -> Result<MultilingualTextSegment> {
    let parity_changed = segment.bidi_level() % 2 != level.number() % 2;
    let cluster_order = if parity_changed {
        (0..segment.clusters().len()).rev().collect::<Vec<_>>()
    } else {
        (0..segment.clusters().len()).collect::<Vec<_>>()
    };
    let mut glyph_ids = Vec::with_capacity(segment.glyph_ids().len());
    let mut x_advances = Vec::with_capacity(segment.x_advances().len());
    let mut y_advances = Vec::with_capacity(segment.y_advances().len());
    let mut x_offsets = Vec::with_capacity(segment.x_offsets().len());
    let mut y_offsets = Vec::with_capacity(segment.y_offsets().len());
    let mut clusters = Vec::with_capacity(segment.clusters().len());
    for index in cluster_order {
        let cluster = &segment.clusters()[index];
        let glyph_range = cluster.glyph_start as usize..cluster.glyph_end as usize;
        let glyph_start = glyph_ids.len() as u32;
        glyph_ids.extend_from_slice(&segment.glyph_ids()[glyph_range.clone()]);
        x_advances.extend_from_slice(&segment.x_advances()[glyph_range.clone()]);
        y_advances.extend_from_slice(&segment.y_advances()[glyph_range.clone()]);
        x_offsets.extend_from_slice(&segment.x_offsets()[glyph_range.clone()]);
        y_offsets.extend_from_slice(&segment.y_offsets()[glyph_range]);
        clusters.push(crate::font::GlyphCluster {
            glyph_start,
            glyph_end: glyph_ids.len() as u32,
            char_start: cluster.char_start,
            char_end: cluster.char_end,
        });
    }
    let mut base = segment.base().clone();
    base.glyph_ids = glyph_ids;
    base.advances = x_advances.clone();

    MultilingualTextSegment::new(
        base,
        segment.logical_index(),
        segment.language().map(str::to_owned),
        segment.script(),
        if level.is_rtl() {
            TextDirection::RightToLeft
        } else {
            TextDirection::LeftToRight
        },
        level.number(),
        x_advances,
        y_advances,
        x_offsets,
        y_offsets,
        clusters,
        segment.break_after(),
    )
    .map(|span| span.inherit_shaping_context(segment))
}

// ---- Internal helpers ----

#[derive(Debug)]
enum BreakableSegment {
    /// A group of items that should be kept together (word or cluster).
    Items(Vec<InlineItem>, Vec<(usize, usize)>),
    /// One language-aware text chunk and its byte-index break candidates.
    Hyphenated(Box<HyphenatedSegment>),
    /// A forced break.
    ForcedBreak(ForcedBreakKind),
}

#[derive(Debug)]
struct HyphenatedSegment {
    segment: InlineItem,
    break_points: Vec<usize>,
}

struct FittingHyphenation {
    prefix: InlineItem,
    hyphen: InlineItem,
    remainder: InlineItem,
    remaining_points: Vec<usize>,
}

/// Whether a complex-script line may break between two logical characters.
pub(crate) fn multilingual_break_allowed(before: char, after: char) -> bool {
    const OPENING: &[char] = &['(', '[', '{', '〈', '《', '「', '『', '【', '〔', '〖'];
    const CLOSING_OR_NONSTARTER: &[char] = &[
        ')', ']', '}', '〉', '》', '」', '』', '】', '〕', '〗', '、', '。', '，', '．', '！',
        '？', '：', '；', '％', '‰',
    ];
    !OPENING.contains(&before) && !CLOSING_OR_NONSTARTER.contains(&after)
}

/// Build breakable segments by finding break opportunities in text.
///
/// Text items are split at unicode line-break opportunities (word boundaries,
/// hyphens, etc.). Non-text items (tabs, images, markers) are treated as
/// atomic units with break opportunities around them.
fn build_breakable_segments(
    items: &[InlineItem],
    fm: &FontManager,
    capture_origins: bool,
) -> Result<Vec<BreakableSegment>> {
    let mut segments = Vec::new();
    let mut current_group: Vec<InlineItem> = Vec::new();
    let mut current_origins = Vec::new();
    let text_breaks = plain_text_breaks(items);

    for (input_slot, item) in items.iter().enumerate() {
        match item {
            InlineItem::LineBreak(clear) => {
                if !current_group.is_empty() {
                    segments.push(BreakableSegment::Items(
                        std::mem::take(&mut current_group),
                        std::mem::take(&mut current_origins),
                    ));
                }
                segments.push(BreakableSegment::ForcedBreak(ForcedBreakKind::Line(*clear)));
            }
            InlineItem::PageBreak => {
                if !current_group.is_empty() {
                    segments.push(BreakableSegment::Items(
                        std::mem::take(&mut current_group),
                        std::mem::take(&mut current_origins),
                    ));
                }
                segments.push(BreakableSegment::ForcedBreak(ForcedBreakKind::Page));
            }
            InlineItem::ColumnBreak => {
                if !current_group.is_empty() {
                    segments.push(BreakableSegment::Items(
                        std::mem::take(&mut current_group),
                        std::mem::take(&mut current_origins),
                    ));
                }
                segments.push(BreakableSegment::ForcedBreak(ForcedBreakKind::Column));
            }
            InlineItem::Tab { .. } => {
                // Tab is a break opportunity
                if !current_group.is_empty() {
                    segments.push(BreakableSegment::Items(
                        std::mem::take(&mut current_group),
                        std::mem::take(&mut current_origins),
                    ));
                }
                segments.push(BreakableSegment::Items(vec![item.clone()], Vec::new()));
            }
            InlineItem::Text(_) | InlineItem::HyphenatedText { .. } => {
                append_plain_text_item(
                    item,
                    text_breaks[input_slot].as_ref(),
                    fm,
                    &mut segments,
                    &mut current_group,
                    &mut current_origins,
                )?;
            }
            InlineItem::MultilingualText(segment) => {
                if segment.uses_legacy_shaping() {
                    append_plain_text_item(
                        item,
                        text_breaks[input_slot].as_ref(),
                        fm,
                        &mut segments,
                        &mut current_group,
                        &mut current_origins,
                    )?;
                    continue;
                }
                let break_points = segment
                    .hyphenation_language()
                    .map_or_else(Vec::new, |language| {
                        hyphenation_opportunities(language, segment.text())
                    });
                if break_points.is_empty() {
                    current_group.push(InlineItem::MultilingualText(segment.clone()));
                    if segment.break_after() {
                        segments.push(BreakableSegment::Items(
                            std::mem::take(&mut current_group),
                            std::mem::take(&mut current_origins),
                        ));
                    }
                } else {
                    if !current_group.is_empty() {
                        segments.push(BreakableSegment::Items(
                            std::mem::take(&mut current_group),
                            std::mem::take(&mut current_origins),
                        ));
                    }
                    segments.push(BreakableSegment::Hyphenated(Box::new(HyphenatedSegment {
                        segment: InlineItem::MultilingualText(segment.clone()),
                        break_points,
                    })));
                }
            }
            InlineItem::Marker(_)
            | InlineItem::Image { .. }
            | InlineItem::Group { .. }
            | InlineItem::Figure { .. } => {
                if capture_origins
                    && matches!(item, InlineItem::Group { .. } | InlineItem::Figure { .. })
                {
                    current_origins.try_reserve(1).map_err(|_| {
                        LayoutError::Layout("breakable opaque origin allocation refused".into())
                    })?;
                    current_origins.push((current_group.len(), input_slot));
                }
                current_group.push(item.clone());
            }
        }
    }

    if !current_group.is_empty() {
        segments.push(BreakableSegment::Items(current_group, current_origins));
    }

    Ok(segments)
}

/// Where a plain text item may break, read from the text of the stretch of
/// plain text items it belongs to.
///
/// A stretch runs up to the next tab, forced break, picture or rich span. A
/// producer splits its text into items wherever the formatting changes, and
/// Word splits a word's runs at each editing session, so the end of an item
/// is a break only when the text on both sides allows one.
struct PlainTextBreaks {
    /// Byte offsets inside the item after which a line may break, ascending.
    inner: Vec<usize>,
    /// The item starts inside a word begun by the item before it.
    glued_before: bool,
    /// A line may break after the item.
    break_after: bool,
}

fn plain_text(item: &InlineItem) -> Option<&TextSegment> {
    match item {
        InlineItem::Text(base) | InlineItem::HyphenatedText { segment: base, .. } => Some(base),
        InlineItem::MultilingualText(span) if span.uses_legacy_shaping() => Some(span.base()),
        _ => None,
    }
}

/// An item that adds no text a break is decided on: a numbering marker, an
/// empty text, or the zero-width word joiner a producer places to record a
/// field. It joins the group around it.
fn transparent_to_breaks(item: &InlineItem) -> bool {
    matches!(item, InlineItem::Marker(_))
        || plain_text(item).is_some_and(|segment| {
            segment.text.is_empty()
                || (segment.width == 0.0
                    && segment.field_kind.is_some()
                    && segment.text.chars().all(|c| c == '\u{2060}'))
        })
}

/// The breaks of every plain text item, `None` for the others and for the
/// items transparent to breaks.
fn plain_text_breaks(items: &[InlineItem]) -> Vec<Option<PlainTextBreaks>> {
    use unicode_linebreak::linebreaks;

    let mut breaks: Vec<Option<PlainTextBreaks>> = items.iter().map(|_| None).collect();
    let mut index = 0;
    while index < items.len() {
        let mut text = String::new();
        let mut members = Vec::new();
        while let Some(item) = items.get(index) {
            if transparent_to_breaks(item) {
                index += 1;
                continue;
            }
            let Some(segment) = plain_text(item) else {
                break;
            };
            let start = text.len();
            text.push_str(&segment.text);
            members.push((index, start, text.len()));
            index += 1;
        }
        if members.is_empty() {
            index += 1;
            continue;
        }
        let opportunities = linebreaks(&text)
            .map(|(offset, _)| offset)
            .filter(|offset| *offset > 0 && *offset < text.len())
            .collect::<Vec<_>>();
        let opportunity = |offset: usize| opportunities.binary_search(&offset).is_ok();
        // Each item reads its own slice of the ascending opportunities, so a
        // stretch costs its length, not its items times its words.
        for (item, start, end) in members {
            let first = opportunities.partition_point(|offset| *offset <= start);
            let last = opportunities.partition_point(|offset| *offset < end);
            breaks[item] = Some(PlainTextBreaks {
                inner: opportunities[first..last]
                    .iter()
                    .map(|offset| offset - start)
                    .collect(),
                glued_before: start > 0 && !opportunity(start),
                break_after: end == text.len() || opportunity(end),
            });
        }
    }
    breaks
}

/// Split one plain text item at its break opportunities, keeping its
/// explicit shaping and hyphenation context. An item without breaks (one
/// transparent to them) joins the current group whole.
fn append_plain_text_item(
    item: &InlineItem,
    breaks: Option<&PlainTextBreaks>,
    fm: &FontManager,
    segments: &mut Vec<BreakableSegment>,
    current_group: &mut Vec<InlineItem>,
    current_origins: &mut Vec<(usize, usize)>,
) -> Result<()> {
    let seg = plain_text(item)
        .ok_or_else(|| LayoutError::Layout("plain tokenization lacks a legacy producer".into()))?;
    let Some(text_breaks) = breaks else {
        current_group.push(item.clone());
        return Ok(());
    };
    let breaks = text_break_chunks(seg.text.len(), text_breaks);
    let spacing = if breaks.len() == 1 && breaks[0].start == 0 && breaks[0].end == seg.text.len() {
        0.0
    } else if let InlineItem::MultilingualText(span) = item {
        fm.context_character_spacing(span)?
    } else {
        let original = fm.shape_text(seg.font_id, &seg.text, seg.font_size)?;
        if original.advances.len() == seg.advances.len() && !original.advances.is_empty() {
            (seg.width - original.width) / original.advances.len() as f64
        } else {
            0.0
        }
    };

    for tb in &breaks {
        let chunk = &seg.text[tb.start..tb.end];
        if chunk.is_empty() {
            continue;
        }

        // Text glued to the item before it continues that item's word.
        let glued_before = tb.start == 0 && text_breaks.glued_before;
        // If this chunk starts with whitespace, treat as a break opportunity
        if !glued_before
            && !current_group.is_empty()
            && chunk.starts_with(|c: char| c.is_whitespace())
        {
            segments.push(BreakableSegment::Items(
                std::mem::take(current_group),
                std::mem::take(current_origins),
            ));
        }

        // Create a sub-segment for just this chunk (not the entire text)
        let sub_item = if matches!(item, InlineItem::MultilingualText(_)) {
            split_hyphenation_item(item, tb.start, tb.end, spacing, fm)?
        } else {
            split_text_subsegment(seg, tb.start, tb.end, spacing, fm)?
        };
        let language = match item {
            InlineItem::HyphenatedText { language, .. } => Some(language.as_str()),
            InlineItem::MultilingualText(span) => span.hyphenation_language(),
            _ => None,
        };
        // A chunk that is only part of a word is not hyphenated on its own: a
        // hyphenated segment stands alone, which would open a break where
        // the word's items meet.
        let break_points = match language {
            Some(language) if !glued_before && tb.is_break => {
                hyphenation_opportunities(language, chunk)
            }
            _ => Vec::new(),
        };
        if break_points.is_empty() {
            current_group.push(sub_item);
        } else {
            if !current_group.is_empty() {
                segments.push(BreakableSegment::Items(
                    std::mem::take(current_group),
                    std::mem::take(current_origins),
                ));
            }
            segments.push(BreakableSegment::Hyphenated(Box::new(HyphenatedSegment {
                segment: sub_item,
                break_points,
            })));
        }

        if tb.is_break {
            segments.push(BreakableSegment::Items(
                std::mem::take(current_group),
                std::mem::take(current_origins),
            ));
        }
    }
    // Without a break after it, the group stays open for the next item.
    Ok(())
}

/// Create a sub-segment InlineItem from a byte range within a TextSegment.
///
/// Reshapes the selected text and preserves formatting from the parent segment.
fn split_text_subsegment(
    seg: &TextSegment,
    byte_start: usize,
    byte_end: usize,
    spacing: f64,
    fm: &FontManager,
) -> Result<InlineItem> {
    // If this is the full segment, just clone it
    if byte_start == 0 && byte_end == seg.text.len() {
        return Ok(InlineItem::Text(seg.clone()));
    }

    let sub_text = seg.text[byte_start..byte_end].to_string();
    let mut shaped = fm.shape_text(seg.font_id, &sub_text, seg.font_size)?;
    for advance in &mut shaped.advances {
        *advance += spacing;
    }
    shaped.width += spacing * shaped.advances.len() as f64;

    let source = seg.source.map(|source| {
        let start = seg.text[..byte_start].chars().count() as u32;
        let end = seg.text[..byte_end].chars().count() as u32;
        SourceSpan {
            node: source.node,
            char_start: source.char_start + start,
            char_end: source.char_start + end,
        }
    });

    Ok(InlineItem::Text(TextSegment {
        text: sub_text,
        direction: seg.direction,
        source,
        font_id: seg.font_id,
        measure_font: seg.measure_font,
        font_size: seg.font_size,
        glyph_ids: shaped.glyph_ids,
        advances: shaped.advances,
        width: shaped.width,
        ascent: seg.ascent,
        descent: seg.descent,
        line_gap: seg.line_gap,
        color: seg.color,
        bold: seg.bold,
        italic: seg.italic,
        underline: seg.underline,
        strike: seg.strike,
        dstrike: seg.dstrike,
        highlight: seg.highlight,
        baseline_offset: seg.baseline_offset,
        hyperlink_url: seg.hyperlink_url.clone(),
        form_mark: seg.form_mark,
        field_kind: seg.field_kind,
        field_source: seg.field_source,
        note: seg.note,
        note_reference_source: seg.note_reference_source,
    }))
}

fn supported_hyphenation_language(language: &str) -> Option<hypher::Lang> {
    let primary = language.split('-').next()?;
    if primary.eq_ignore_ascii_case("en") {
        Some(hypher::Lang::English)
    } else if primary.eq_ignore_ascii_case("fr") {
        Some(hypher::Lang::French)
    } else if primary.eq_ignore_ascii_case("de") {
        Some(hypher::Lang::German)
    } else if primary.eq_ignore_ascii_case("es") {
        Some(hypher::Lang::Spanish)
    } else {
        None
    }
}

fn hyphenation_opportunities(language: &str, text: &str) -> Vec<usize> {
    let Some(language) = supported_hyphenation_language(language) else {
        return Vec::new();
    };
    let word_end = text
        .char_indices()
        .take_while(|(_, character)| character.is_alphabetic())
        .map(|(offset, character)| offset + character.len_utf8())
        .last()
        .unwrap_or(0);
    if word_end == 0 || text[word_end..].chars().any(char::is_alphabetic) {
        return Vec::new();
    }
    let word = &text[..word_end];
    let syllables = hypher::hyphenate(word, language).collect::<Vec<_>>();
    let mut offset = 0usize;
    syllables
        .iter()
        .take(syllables.len().saturating_sub(1))
        .map(|syllable| {
            offset += syllable.len();
            offset
        })
        .collect()
}

fn hyphenation_base(item: &InlineItem) -> Result<&TextSegment> {
    match item {
        InlineItem::Text(base) => Ok(base),
        InlineItem::MultilingualText(span) => Ok(span.base()),
        _ => Err(LayoutError::Layout(
            "hyphenation lost its text owner".into(),
        )),
    }
}

fn hyphenation_spacing(item: &InlineItem, fm: &FontManager) -> Result<f64> {
    let base = hyphenation_base(item)?;
    match item {
        InlineItem::MultilingualText(span) => fm.context_character_spacing(span),
        _ => text_segment_spacing(base, fm),
    }
}

fn split_hyphenation_item(
    item: &InlineItem,
    start: usize,
    end: usize,
    spacing: f64,
    fm: &FontManager,
) -> Result<InlineItem> {
    let base = hyphenation_base(item)?;
    if start == 0 && end == base.text.len() {
        return Ok(item.clone());
    }
    if let InlineItem::MultilingualText(span) = item {
        let mut slice = base.clone();
        slice.text = base.text[start..end].to_owned();
        slice.source = base.source.map(|source| SourceSpan {
            node: source.node,
            char_start: source.char_start + base.text[..start].chars().count() as u32,
            char_end: source.char_start + base.text[..end].chars().count() as u32,
        });
        return fm
            .reshape_context_segment(span, slice, spacing)
            .map(InlineItem::MultilingualText);
    }
    split_text_subsegment(base, start, end, spacing, fm)
}

fn fitting_hyphenation(
    segment: &InlineItem,
    break_points: &[usize],
    current_width: f64,
    available_width: f64,
    fm: &FontManager,
) -> Result<Option<FittingHyphenation>> {
    let spacing = hyphenation_spacing(segment, fm)?;
    let base = hyphenation_base(segment)?;
    let hyphen = match segment {
        InlineItem::MultilingualText(span) => {
            let mut generated = base.clone();
            generated.text = "-".into();
            generated.source = None;
            InlineItem::MultilingualText(fm.reshape_context_segment(span, generated, spacing)?)
        }
        _ => InlineItem::Text(generated_hyphen(base, spacing, fm)?),
    };
    for &break_point in break_points.iter().rev() {
        let prefix = split_hyphenation_item(segment, 0, break_point, spacing, fm)?;
        if current_width + inline_item_width(&prefix) + inline_item_width(&hyphen)
            > available_width + 0.01
        {
            continue;
        }
        let remainder = split_hyphenation_item(segment, break_point, base.text.len(), spacing, fm)?;
        let remaining_points = break_points
            .iter()
            .copied()
            .filter(|point| *point > break_point)
            .map(|point| point - break_point)
            .collect();
        return Ok(Some(FittingHyphenation {
            prefix,
            hyphen,
            remainder,
            remaining_points,
        }));
    }
    Ok(None)
}

/// A group cut where it overflows a line: what fits, and the rest.
struct OverflowCut {
    head: Vec<InlineItem>,
    head_origins: Vec<(usize, usize)>,
    tail: Vec<InlineItem>,
    tail_origins: Vec<(usize, usize)>,
}

/// Cut a group that has no break opportunity at the character that
/// overflows `available`, keeping at least one character on the line so a
/// line always advances. The cut falls between two items when the item that
/// overflows is not text, or when none of its characters fits after the
/// items before it. The group comes back whole when it cannot be cut: it
/// starts with a picture or drawing wider than the line, holds one
/// character, or would leave nothing visible on the line.
///
/// With `split_rest`, a rest much wider than the line is cut into pieces of
/// about one line in the same pass, so a long string is shaped a bounded
/// number of times per character, not once per line.
#[allow(clippy::type_complexity)]
fn cut_overflowing_group(
    mut items: Vec<InlineItem>,
    origins: Vec<(usize, usize)>,
    available: f64,
    split_rest: bool,
    fm: &FontManager,
) -> Result<std::result::Result<OverflowCut, (Vec<InlineItem>, Vec<(usize, usize)>)>> {
    let mut used = 0.0;
    let Some(index) = items.iter().position(|item| {
        let width = inline_item_width(item);
        let overflows = used + width > available + 0.01;
        used += width;
        overflows
    }) else {
        return Ok(Err((items, origins)));
    };
    let used = items[..index].iter().map(inline_item_width).sum::<f64>();
    let rest = items.split_off(index + 1);
    let Some(item) = items.pop() else {
        return Err(LayoutError::Layout("a cut lost its item".into()));
    };
    let mut head = items;
    let cut = cut_text_item(
        &item,
        available - used,
        head.is_empty(),
        split_rest.then_some(available),
        fm,
    )?;
    let mut tail = Vec::new();
    match cut {
        Some((prefix, pieces)) => {
            head.push(prefix);
            tail.extend(pieces);
        }
        None => tail.push(item),
    }
    let pieces = tail.len();
    if head.iter().map(inline_item_width).sum::<f64>() <= 0.0 {
        head.extend(tail);
        head.extend(rest);
        return Ok(Err((head, origins)));
    }
    tail.extend(rest);
    let (head_origins, tail_origins): (Vec<_>, Vec<_>) =
        origins.into_iter().partition(|(at, _)| *at < index);
    let tail_origins = tail_origins
        .into_iter()
        .map(|(at, origin): (usize, usize)| {
            // An uncut item keeps its place at the front of the rest.
            (
                if at == index {
                    0
                } else {
                    at - index - 1 + pieces
                },
                origin,
            )
        })
        .collect();
    Ok(Ok(OverflowCut {
        head,
        head_origins,
        tail,
        tail_origins,
    }))
}

/// Cut a text item after the longest prefix that fits `room`, at a grapheme
/// boundary; on an empty line at least one grapheme goes. The rest comes back
/// as one piece, or as pieces about `piece_width` wide when it is more than
/// twice that. `None` for an item that is not text or has no prefix to give.
fn cut_text_item(
    item: &InlineItem,
    room: f64,
    line_empty: bool,
    piece_width: Option<f64>,
    fm: &FontManager,
) -> Result<Option<(InlineItem, Vec<InlineItem>)>> {
    let segment = match item {
        InlineItem::Text(segment) => segment,
        InlineItem::MultilingualText(span) => span.base(),
        _ => return Ok(None),
    };
    let split = |start: usize, end: usize, spacing: f64| match item {
        InlineItem::Text(segment) => split_text_subsegment(segment, start, end, spacing, fm),
        _ => split_hyphenation_item(item, start, end, spacing, fm),
    };
    let bounds = icu_segmenter::GraphemeClusterSegmenter::new()
        .segment_str(&segment.text)
        .filter(|at| *at > 0 && *at < segment.text.len())
        .collect::<Vec<_>>();
    // Each grapheme's width is read from the shaped advances when they map one
    // to one onto the characters, else estimated from the mean advance.
    let chars = segment.text.chars().count().max(1);
    let one_to_one = segment.advances.len() == chars;
    let mean = segment.width / chars as f64;
    let mut estimated = Vec::with_capacity(bounds.len() + 1);
    let (mut previous, mut char_index, mut total) = (0, 0, 0.0);
    for end in bounds
        .iter()
        .copied()
        .chain(std::iter::once(segment.text.len()))
    {
        let count = segment.text[previous..end].chars().count();
        total += if one_to_one {
            segment.advances[char_index..char_index + count]
                .iter()
                .sum::<f64>()
        } else {
            mean * count as f64
        };
        estimated.push(total);
        char_index += count;
        previous = end;
    }
    // The character spacing the item's advances carry. Shaping a long run
    // whole costs more than the cut it serves, so a long one is read from its
    // first characters, shaped without spacing.
    const WINDOW: usize = 256;
    let spacing = if one_to_one && chars > 2 * WINDOW {
        let end = segment
            .text
            .char_indices()
            .nth(WINDOW)
            .map_or(segment.text.len(), |(at, _)| at);
        let natural = inline_item_width(&split(0, end, 0.0)?);
        (segment.advances[..WINDOW].iter().sum::<f64>() - natural) / WINDOW as f64
    } else {
        match item {
            InlineItem::MultilingualText(span) => fm.context_character_spacing(span)?,
            _ => text_segment_spacing(segment, fm)?,
        }
    };
    let piece = |start: usize, end: usize| split(start, end, spacing);
    // The longest prefix that fits, found by halving: a prefix only grows as
    // graphemes are added. The estimate bounds the search, so only prefixes of
    // about a line are shaped.
    let estimate = estimated.partition_point(|width| *width <= room + 0.01);
    let (mut low, mut high) = (0, bounds.len().min(2 * estimate + 8));
    while low < high {
        let middle = (low + high).div_ceil(2);
        if inline_item_width(&piece(0, bounds[middle - 1])?) <= room + 0.01 {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let cut = match (low, line_empty) {
        (0, true) => match bounds.first() {
            Some(first) => *first,
            None => return Ok(None),
        },
        (0, false) => return Ok(None),
        (fitting, _) => bounds[fitting - 1],
    };
    let prefix = piece(0, cut)?;
    let rest_width = segment.width - inline_item_width(&prefix);
    let Some(width) = piece_width.filter(|width| *width > 0.0 && rest_width > 2.0 * width) else {
        return Ok(Some((prefix, vec![piece(cut, segment.text.len())?])));
    };
    // Pieces about one line wide, from the estimated widths, shaped once each.
    let first = bounds.partition_point(|at| *at <= cut);
    let (mut start, mut line_start) = (cut, estimated[first.saturating_sub(1)]);
    if first == 0 {
        line_start = 0.0;
    }
    let mut pieces = Vec::new();
    for (index, end) in bounds.iter().copied().enumerate().skip(first) {
        if estimated[index + 1] - line_start > width && end > start {
            pieces.push(piece(start, end)?);
            start = end;
            line_start = estimated[index];
        }
    }
    pieces.push(piece(start, segment.text.len())?);
    Ok(Some((prefix, pieces)))
}

fn text_segment_spacing(segment: &TextSegment, fm: &FontManager) -> Result<f64> {
    let original = fm.shape_text(segment.font_id, &segment.text, segment.font_size)?;
    Ok(
        if original.advances.len() == segment.advances.len() && !original.advances.is_empty() {
            (segment.width - original.width) / original.advances.len() as f64
        } else {
            0.0
        },
    )
}

fn generated_hyphen(segment: &TextSegment, spacing: f64, fm: &FontManager) -> Result<TextSegment> {
    let mut shaped = fm.shape_text(segment.font_id, "-", segment.font_size)?;
    for advance in &mut shaped.advances {
        *advance += spacing;
    }
    shaped.width += spacing * shaped.advances.len() as f64;
    Ok(TextSegment {
        text: "-".to_owned(),
        direction: segment.direction,
        source: None,
        font_id: segment.font_id,
        measure_font: segment.measure_font,
        font_size: segment.font_size,
        glyph_ids: shaped.glyph_ids,
        advances: shaped.advances,
        width: shaped.width,
        ascent: segment.ascent,
        descent: segment.descent,
        line_gap: segment.line_gap,
        color: segment.color,
        bold: segment.bold,
        italic: segment.italic,
        underline: segment.underline,
        strike: segment.strike,
        dstrike: segment.dstrike,
        highlight: segment.highlight,
        baseline_offset: segment.baseline_offset,
        hyperlink_url: segment.hyperlink_url.clone(),
        form_mark: segment.form_mark,
        field_kind: segment.field_kind,
        field_source: segment.field_source,
        note: segment.note,
        note_reference_source: segment.note_reference_source,
    })
}

fn segment_font_id(item: &InlineItem) -> FontId {
    match item {
        InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. } => segment.font_id,
        InlineItem::MultilingualText(segment) => segment.font_id(),
        _ => unreachable!("called only for text"),
    }
}

fn segment_font_size(item: &InlineItem) -> f64 {
    match item {
        InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. } => segment.font_size,
        InlineItem::MultilingualText(segment) => segment.base().font_size,
        _ => unreachable!("called only for text"),
    }
}

struct TextBreakInfo {
    /// Byte range within the original text.
    start: usize,
    end: usize,
    /// Whether a line break is allowed after this segment.
    is_break: bool,
}

/// The chunks of a text of `len` bytes, cut at its inner breaks; the last
/// one breaks only when the text after the item allows it.
fn text_break_chunks(len: usize, breaks: &PlainTextBreaks) -> Vec<TextBreakInfo> {
    let mut chunks = Vec::with_capacity(breaks.inner.len() + 1);
    let mut start = 0;
    for end in breaks.inner.iter().copied().chain(std::iter::once(len)) {
        chunks.push(TextBreakInfo {
            start,
            end,
            is_break: end < len || breaks.break_after,
        });
        start = end;
    }
    chunks
}

/// Width of the spaces that end a group of text items.
///
/// They hang past the end of a line instead of wrapping the group, as Word and
/// PowerPoint let them.
fn hanging_space_width(items: &[InlineItem], fm: &FontManager) -> Result<f64> {
    let mut width = 0.0;
    for item in items.iter().rev() {
        match item {
            InlineItem::MultilingualText(segment) => {
                let trimmed = segment.text().trim_end_matches(' ');
                width += segment.width() - fm.context_prefix_width(segment, trimmed.len())?;
                if !is_space_run(segment.text()) {
                    break;
                }
            }
            InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. } => {
                width += terminal_u0020_width(segment, fm)?;
                if !is_space_run(&segment.text) {
                    break;
                }
            }
            _ => break,
        }
    }
    Ok(width)
}

/// Width of the spaces that end a plain text segment.
fn terminal_u0020_width(segment: &TextSegment, fm: &FontManager) -> Result<f64> {
    Ok(trailing_spaces(segment, fm)?.map_or(0.0, |spaces| {
        segment.advances[segment.advances.len() - spaces..]
            .iter()
            .sum()
    }))
}

/// How many U+0020 spaces end a plain text segment, which are then as many
/// glyphs at its end.
///
/// Plain text is shaped left to right, so its last glyphs are its last
/// characters, whatever ligatures come before them. `None` when it ends with
/// no space, holds a right-to-left character that would reverse its glyphs, or
/// does not end with that many of the font's space glyph.
fn trailing_spaces(segment: &TextSegment, fm: &FontManager) -> Result<Option<usize>> {
    if segment.field_kind.is_some() || segment.note.is_some() {
        return Ok(None);
    }
    let spaces = segment.text.len() - segment.text.trim_end_matches(' ').len();
    let glyphs = segment.glyph_ids.len();
    if spaces == 0
        || glyphs < spaces
        || segment.advances.len() != glyphs
        || segment.text.chars().any(|character| {
            matches!(
                unicode_bidi::bidi_class(character),
                unicode_bidi::BidiClass::R | unicode_bidi::BidiClass::AL
            )
        })
    {
        return Ok(None);
    }
    let space = fm.shape_text(segment.font_id, " ", segment.font_size)?;
    let [space_glyph] = space.glyph_ids[..] else {
        return Ok(None);
    };
    Ok(segment.glyph_ids[glyphs - spaces..]
        .iter()
        .all(|glyph| *glyph == space_glyph)
        .then_some(spaces))
}

fn inline_item_width(item: &InlineItem) -> f64 {
    match item {
        InlineItem::Text(seg) | InlineItem::HyphenatedText { segment: seg, .. } => seg.width,
        InlineItem::MultilingualText(seg) => seg.width(),
        InlineItem::Tab { .. } => 36.0, // Default tab width, will be resolved
        InlineItem::Image { width, .. } => *width,
        InlineItem::Group { width, .. } => *width,
        InlineItem::Figure { item, .. } => inline_item_width(item),
        InlineItem::Marker(seg) => seg.width,
        InlineItem::LineBreak(_) | InlineItem::PageBreak | InlineItem::ColumnBreak => 0.0,
    }
}

fn item_metrics(item: &InlineItem) -> (f64, f64, f64, f64, f64) {
    // Returns (width, ascent, descent, natural height, text font size)
    match item {
        InlineItem::Text(seg) | InlineItem::HyphenatedText { segment: seg, .. } => (
            seg.width,
            seg.ascent,
            seg.descent,
            seg.ascent + seg.descent + seg.line_gap,
            seg.font_size,
        ),
        InlineItem::MultilingualText(seg) => (
            seg.width(),
            seg.base().ascent,
            seg.base().descent,
            seg.base().ascent + seg.base().descent + seg.base().line_gap,
            seg.base().font_size,
        ),
        InlineItem::Marker(seg) => (
            seg.width,
            seg.ascent,
            seg.descent,
            seg.ascent + seg.descent + seg.line_gap,
            0.0,
        ),
        InlineItem::Tab { .. } => (36.0, 0.0, 0.0, 0.0, 0.0),
        InlineItem::Image { width, height, .. } => (*width, *height, 0.0, *height, 0.0),
        InlineItem::Group {
            width,
            height,
            baseline,
            ..
        } => {
            let baseline = normalized_group_baseline(*height, *baseline).unwrap_or(*height);
            (*width, baseline, *height - baseline, *height, 0.0)
        }
        InlineItem::Figure { item, .. } => item_metrics(item),
        InlineItem::LineBreak(_) | InlineItem::PageBreak | InlineItem::ColumnBreak => {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        }
    }
}

fn normalized_group_baseline(height: f64, baseline: Option<f64>) -> Option<f64> {
    if !height.is_finite() {
        return None;
    }
    let upper = height.max(0.0);
    baseline
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, upper))
}

fn inline_to_line_item(item: &InlineItem) -> LineItem {
    match item {
        InlineItem::Text(seg) | InlineItem::HyphenatedText { segment: seg, .. } => {
            LineItem::Text(seg.clone())
        }
        InlineItem::MultilingualText(seg) => LineItem::MultilingualText(seg.clone()),
        InlineItem::Marker(seg) => LineItem::Marker(seg.clone()),
        // `LineState::push_tab` places tabs, so this is only a placeholder.
        InlineItem::Tab { fill } => LineItem::Tab {
            width: 0.0,
            leader: None,
            align: TabAlign::Left,
            gap: 0.0,
            fill: *fill,
        },
        InlineItem::Image {
            width,
            height,
            media_id,
            form_mark,
        } => LineItem::Image {
            width: *width,
            height: *height,
            media_id: *media_id,
            form_mark: *form_mark,
        },
        InlineItem::Group {
            width,
            height,
            baseline,
            math,
            group,
        } => LineItem::Group {
            width: *width,
            height: *height,
            baseline: normalized_group_baseline(*height, *baseline),
            math: *math,
            group: group.clone(),
        },
        InlineItem::Figure {
            item,
            alternate_text,
            structure_id,
        } => LineItem::Figure {
            item: Box::new(inline_to_line_item(item)),
            alternate_text: alternate_text.clone(),
            structure_id: *structure_id,
        },
        InlineItem::LineBreak(_) | InlineItem::PageBreak | InlineItem::ColumnBreak => {
            LineItem::Tab {
                width: 0.0,
                leader: None,
                align: TabAlign::Left,
                gap: 0.0,
                fill: None,
            }
        }
    }
}

/// Shape a leader character repeated to fill the given width.
fn shape_leader(
    fm: &FontManager,
    font_ctx: Option<(FontId, f64)>,
    leader_char: char,
    tab_width: f64,
) -> Option<TextSegment> {
    let (font_id, font_size) = font_ctx?;
    if tab_width < 1.0 {
        return None;
    }

    // Shape a single leader character to get its advance width
    let single = String::from(leader_char);
    let shaped = fm.shape_text(font_id, &single, font_size).ok()?;
    if shaped.glyph_ids.is_empty() {
        return None;
    }
    let char_advance = shaped.advances[0];
    if char_advance < 0.5 {
        return None;
    }

    // Add a small gap between leader chars (about 50% of char width for dots, less for others)
    let spacing = match leader_char {
        '.' | '\u{00B7}' => char_advance * 0.5,
        _ => char_advance * 0.15,
    };
    let step = char_advance + spacing;
    let count = ((tab_width - spacing) / step).floor() as usize;
    if count == 0 {
        return None;
    }

    // Build the repeated leader text and glyph arrays
    let leader_text: String = std::iter::repeat_n(leader_char, count).collect();
    let mut glyph_ids = Vec::with_capacity(count);
    let mut advances = Vec::with_capacity(count);
    for i in 0..count {
        glyph_ids.push(shaped.glyph_ids[0]);
        if i + 1 < count {
            advances.push(char_advance + spacing);
        } else {
            advances.push(char_advance);
        }
    }

    let metrics = fm.metrics(font_id, font_size).ok()?;

    Some(TextSegment {
        text: leader_text,
        direction: TextDirection::Auto,
        source: None,
        font_id,
        measure_font: None,
        font_size,
        glyph_ids,
        advances,
        width: tab_width, // fill the entire tab gap
        ascent: metrics.ascent,
        descent: metrics.descent,
        line_gap: metrics.line_gap,
        color: Color::BLACK,
        bold: false,
        italic: false,
        underline: None,
        strike: false,
        dstrike: false,
        highlight: None,
        baseline_offset: 0.0,
        hyperlink_url: None,
        field_kind: None,
        field_source: None,
        note: None,
        note_reference_source: None,
        form_mark: None,
    })
}

fn compute_first_line_width(params: &LineBreakParams) -> f64 {
    if params.ind_hanging > 0.0 {
        // Hanging indent: first line has MORE width (extends left)
        params.available_width - params.ind_left - params.ind_right + params.ind_hanging
    } else {
        params.available_width - params.ind_left - params.ind_right - params.ind_first_line
    }
}

fn compute_subsequent_line_width(params: &LineBreakParams) -> f64 {
    params.available_width - params.ind_left - params.ind_right
}

/// Width kept clear at the start of a given line.
fn line_prefix_width(params: &LineBreakParams, line_index: usize) -> f64 {
    params
        .line_prefix_widths
        .get(line_index)
        .copied()
        .unwrap_or(0.0)
}

/// Width kept clear at the end of a given line.
fn line_suffix_width(params: &LineBreakParams, line_index: usize) -> f64 {
    params
        .line_suffix_widths
        .get(line_index)
        .copied()
        .unwrap_or(0.0)
}

/// Usable width of a line, once anything floating beside it is taken out.
fn line_width_at(params: &LineBreakParams, line_index: usize, is_first_line: bool) -> f64 {
    let base = if is_first_line {
        compute_first_line_width(params)
    } else {
        compute_subsequent_line_width(params)
    };
    (base - line_prefix_width(params, line_index) - line_suffix_width(params, line_index)).max(0.0)
}

/// Where a line starts, once anything floating to its left is taken out.
fn line_indent_at(params: &LineBreakParams, line_index: usize, is_first_line: bool) -> f64 {
    let base = if is_first_line {
        first_line_indent(params)
    } else {
        subsequent_line_indent(params)
    };
    base + line_prefix_width(params, line_index)
}

fn first_line_indent(params: &LineBreakParams) -> f64 {
    // At the right edge, the first line's indent only narrows it: the line
    // still starts at the left indent.
    if params.first_line_at_right {
        params.ind_left
    } else if params.ind_hanging > 0.0 {
        params.ind_left - params.ind_hanging
    } else {
        params.ind_left + params.ind_first_line
    }
}

fn subsequent_line_indent(params: &LineBreakParams) -> f64 {
    params.ind_left
}

/// Compute line height based on spacing rules.
fn effective_line_gap(ascent: f64, descent: f64, natural_height: f64) -> f64 {
    (natural_height - ascent - descent).max(0.0)
}

fn compute_line_height(
    ascent: f64,
    descent: f64,
    line_gap: f64,
    font_size: f64,
    params: &LineBreakParams,
) -> f64 {
    let natural = ascent + descent + line_gap;
    let natural = if natural < 1.0 { 12.0 } else { natural }; // minimum for empty lines
    let font_size = if font_size < 1.0 { 12.0 } else { font_size };

    match params.line_spacing {
        LineSpacing::Single => natural,
        LineSpacing::Multiple(factor) => font_size * factor,
        LineSpacing::Exact(points) => points,
        LineSpacing::AtLeast(points) => natural.max(points),
    }
}
