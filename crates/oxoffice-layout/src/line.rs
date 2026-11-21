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
    /// A tab character.
    Tab,
    /// A forced line break.
    LineBreak,
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

/// The kind of explicit break that ended a laid-out line.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForcedBreakKind {
    Line,
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
/// Tab stops are measured from the zero indent, the margin in Word, so a tab
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
            BreakableSegment::Items(seg_items, _) if matches!(seg_items[..], [InlineItem::Tab]) => {
                line.settle_tab(fm);
                let mut tab = line.next_tab(params);
                if params.wrap && tab.wraps && !line.items.is_empty() {
                    // A tab with no stop left on its line starts the next one.
                    push_layout_line(&mut lines, &mut origins, &mut line, params, fm, false, None)?;
                    tab = line.next_tab(params);
                }
                line.push_tab(tab, fm, font_ctx);
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
                    let item = segment;
                    font_ctx = Some((segment_font_id(&item), segment_font_size(&item)));
                    line.push(&item, fm, None)?;
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

    /// The width appended text starts from, once a waiting tab has given up
    /// its gap to it.
    fn used_width(&self) -> f64 {
        self.width - self.pending.map_or(0.0, |tab| tab.width())
    }

    /// Where a tab appended to this line goes.
    ///
    /// The first explicit stop past the tab wins, in the order the stops are
    /// listed as Word reads them, bar stops excepted. A hanging indent adds a
    /// left stop at the left indent, which is where the tab after a list
    /// number goes. Past the last explicit stop the tab takes the next
    /// multiple of the default interval.
    fn next_tab(&self, params: &LineBreakParams) -> ResolvedTab {
        let x = self.start + self.width;
        let line_end = self.start + self.available;
        let margin = params.available_width;
        let hanging = (params.ind_hanging > 0.0).then_some(TabStop {
            pos_pt: params.ind_left,
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
            // Word 2013 and later move a stop past the right margin to the
            // end of the line. Earlier versions keep it, and the text after a
            // stop at or past the margin then runs on past it. A stop past the
            // right indent lets the text run to the margin. Text after a
            // right, centre or decimal stop is pushed back so it ends at the
            // margin, unless an earlier version keeps the stop at or past it.
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
                stop: pos,
                align: stop.align,
                leader: leader_char(stop.leader),
                // Word 2013 moves a left tab that reaches the end of its line
                // to a line of its own, and the text after it to the next.
                wraps: params.clamp_tabs_past_margin
                    && stop.align == TabAlign::Left
                    && pos > reach - 0.01,
                reach,
                shift_limit: (params.clamp_tabs_past_margin || pos < margin)
                    .then_some(margin.max(line_end)),
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
            stop: pos,
            align: TabAlign::Left,
            leader: None,
            // A default stop at or past the end of the line is no stop, unless
            // an earlier stop took the line past its end.
            wraps: pos > self.start + self.limit - 0.01,
            reach: line_end,
            shift_limit: None,
        }
    }

    fn push_tab(&mut self, tab: ResolvedTab, fm: &FontManager, font_ctx: Option<(FontId, f64)>) {
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
                in_number: false,
                shift_limit: tab.shift_limit.map(|limit| limit - self.start),
                font_ctx,
            };
            self.width = start + pending.width();
            self.pending = Some(pending);
            // The width is settled once the text after the tab is known.
            self.items.push(LineItem::Tab {
                width: 0.0,
                leader: None,
                align: tab.align,
                gap: 0.0,
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

/// The stop a tab goes to, measured from the zero indent.
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
    /// Whether the text scanned so far ends inside a number.
    in_number: bool,
    shift_limit: Option<f64>,
    font_ctx: Option<(FontId, f64)>,
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
                if segment.uses_legacy_shaping()
                    && segment.text().chars().all(char::is_whitespace) =>
            {
                self.trailing + width
            }
            InlineItem::MultilingualText(segment) if segment.uses_legacy_shaping() => {
                let trimmed = segment.text().trim_end_matches(char::is_whitespace);
                segment.width() - fm.context_prefix_width(segment, trimmed.len())?
            }
            _ => 0.0,
        };
        if self.align == TabAlign::Decimal && !self.aligned_found {
            match decimal_alignment_offset(item, &mut self.in_number, fm)? {
                Some(offset) => {
                    self.aligned += offset;
                    self.aligned_found = true;
                }
                None => self.aligned += width,
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
        InlineItem::MultilingualText(segment) if segment.uses_legacy_shaping() => segment.base(),
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

    for (input_slot, item) in items.iter().enumerate() {
        match item {
            InlineItem::LineBreak => {
                if !current_group.is_empty() {
                    segments.push(BreakableSegment::Items(
                        std::mem::take(&mut current_group),
                        std::mem::take(&mut current_origins),
                    ));
                }
                segments.push(BreakableSegment::ForcedBreak(ForcedBreakKind::Line));
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
            InlineItem::Tab => {
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
                append_legacy_breakable_item(
                    item,
                    fm,
                    &mut segments,
                    &mut current_group,
                    &mut current_origins,
                )?;
            }
            InlineItem::MultilingualText(segment) => {
                if segment.uses_legacy_shaping() {
                    append_legacy_breakable_item(
                        item,
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

/// Preserve the former plain tokenization while a legacy carrier retains
/// its explicit shaping and hyphenation context.
fn append_legacy_breakable_item(
    item: &InlineItem,
    fm: &FontManager,
    segments: &mut Vec<BreakableSegment>,
    current_group: &mut Vec<InlineItem>,
    current_origins: &mut Vec<(usize, usize)>,
) -> Result<()> {
    let seg = match item {
        InlineItem::Text(base) | InlineItem::HyphenatedText { segment: base, .. } => base,
        InlineItem::MultilingualText(span) if span.uses_legacy_shaping() => span.base(),
        _ => {
            return Err(LayoutError::Layout(
                "plain tokenization lacks a legacy producer".into(),
            ));
        }
    };
    if seg.text.is_empty() {
        if !current_group.is_empty() {
            segments.push(BreakableSegment::Items(
                std::mem::take(current_group),
                std::mem::take(current_origins),
            ));
        }
        segments.push(BreakableSegment::Items(vec![item.clone()], Vec::new()));
        return Ok(());
    }
    // Use unicode-linebreak to find break opportunities within text
    let breaks = split_text_at_break_opportunities(seg);
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

        // If this chunk starts with whitespace, treat as a break opportunity
        if !current_group.is_empty() && chunk.starts_with(|c: char| c.is_whitespace()) {
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
        let break_points = language.map_or_else(Vec::new, |language| {
            hyphenation_opportunities(language, chunk)
        });
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

    // Flush any remaining
    if !current_group.is_empty() {
        segments.push(BreakableSegment::Items(
            std::mem::take(current_group),
            std::mem::take(current_origins),
        ));
    }
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

fn split_text_at_break_opportunities(seg: &TextSegment) -> Vec<TextBreakInfo> {
    use unicode_linebreak::{BreakOpportunity, linebreaks};

    let text = &seg.text;
    if text.is_empty() {
        return vec![];
    }

    let mut breaks = Vec::new();
    let mut last_start = 0;

    for (byte_pos, opportunity) in linebreaks(text) {
        if byte_pos == 0 {
            continue;
        }

        let is_break = matches!(
            opportunity,
            BreakOpportunity::Allowed | BreakOpportunity::Mandatory
        );

        breaks.push(TextBreakInfo {
            start: last_start,
            end: byte_pos,
            is_break,
        });
        last_start = byte_pos;
    }

    // If unicode-linebreak didn't produce any breaks, treat as one chunk
    if breaks.is_empty() {
        breaks.push(TextBreakInfo {
            start: 0,
            end: text.len(),
            is_break: true,
        });
    }

    breaks
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
        InlineItem::Tab => 36.0, // Default tab width, will be resolved
        InlineItem::Image { width, .. } => *width,
        InlineItem::Group { width, .. } => *width,
        InlineItem::Figure { item, .. } => inline_item_width(item),
        InlineItem::Marker(seg) => seg.width,
        InlineItem::LineBreak | InlineItem::PageBreak | InlineItem::ColumnBreak => 0.0,
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
        InlineItem::Tab => (36.0, 0.0, 0.0, 0.0, 0.0),
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
        InlineItem::LineBreak | InlineItem::PageBreak | InlineItem::ColumnBreak => {
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
        InlineItem::Tab => LineItem::Tab {
            width: 0.0,
            leader: None,
            align: TabAlign::Left,
            gap: 0.0,
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
        InlineItem::LineBreak | InlineItem::PageBreak | InlineItem::ColumnBreak => LineItem::Tab {
            width: 0.0,
            leader: None,
            align: TabAlign::Left,
            gap: 0.0,
        },
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
    if params.ind_hanging > 0.0 {
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
