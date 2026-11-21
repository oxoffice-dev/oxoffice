//! Presentation rendering inputs and package assembly helpers.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use oxoffice_drawing::theme::CT_OfficeStyleSheet;
use oxoffice_layout::{
    Color, Diagnostic, DocumentMetadata, FontFile, FontManager, GroupElement, LayoutResult,
    MediaId, PageFrame, Paint, Path, PathCommand, PathElement, Point, PositionedElement, Rect,
    Stroke, Transform,
};
use oxoffice_pptx_layout::{
    CropRect, ResolvedBackground, ResolvedContent, ResolvedGeometry, ResolvedImage,
    ResolvedImagePlacement, ResolvedLineEnd, ResolvedLineEndKind, ResolvedLineEndSize,
    ResolvedRectAlignment, ResolvedShape, ResolvedSlide, ResolvedSlideTextDirections,
    ResolvedTable, ResolvedTableBorder, ResolvedTextBody, ResolvedTileFlip, ResolvedTilePlacement,
    ScopedHyperlinkTargets,
};
use oxoffice_pptx_oxml::notes_parts::CT_NotesSlide;
use oxoffice_pptx_oxml::slide_parts::{CT_Slide, CT_SlideLayout, CT_SlideMaster};

mod text;
pub mod timeline;

/// The source part whose relationship map owns an identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelScope {
    Slide,
    Layout,
    Master,
}

impl fmt::Display for RelScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Slide => "slide",
            Self::Layout => "layout",
            Self::Master => "master",
        })
    }
}

/// A package relationship after its target has been resolved against its source part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedRel {
    pub target: String,
    pub relationship_type: String,
    pub target_mode: Option<String>,
}

/// Relationship maps kept separate by their source-part scope.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelScopes {
    pub slide: HashMap<String, ResolvedRel>,
    pub layout: HashMap<String, ResolvedRel>,
    pub master: HashMap<String, ResolvedRel>,
}

impl RelScopes {
    /// Look up a relationship only in the explicitly selected source-part scope.
    pub fn get(
        &self,
        scope: RelScope,
        relationship_id: &str,
    ) -> Result<&ResolvedRel, RenderInputError> {
        let relationships = match scope {
            RelScope::Slide => &self.slide,
            RelScope::Layout => &self.layout,
            RelScope::Master => &self.master,
        };
        relationships
            .get(relationship_id)
            .ok_or_else(|| RenderInputError::MissingRelationship {
                scope,
                relationship_id: relationship_id.to_owned(),
            })
    }

    /// Project external hyperlink relationships into layout's source-scoped map.
    pub fn external_hyperlink_targets(&self) -> ScopedHyperlinkTargets {
        fn external_targets(
            relationships: &HashMap<String, ResolvedRel>,
        ) -> HashMap<String, String> {
            relationships
                .iter()
                .filter(|(_, relationship)| {
                    relationship.relationship_type == HYPERLINK_RELATIONSHIP
                        && relationship.target_mode.as_deref() == Some("External")
                })
                .map(|(id, relationship)| (id.clone(), relationship.target.clone()))
                .collect()
        }

        ScopedHyperlinkTargets {
            slide: external_targets(&self.slide),
            layout: external_targets(&self.layout),
            master: external_targets(&self.master),
        }
    }
}

const HYPERLINK_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

/// Media bytes available to a renderer, with their package content type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaData {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// Package assembly failures that retain relationship source context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderInputError {
    MissingRelationship {
        scope: RelScope,
        relationship_id: String,
    },
    MissingMediaTarget {
        scope: RelScope,
        relationship_id: String,
        target: String,
    },
    SlideIndexOutOfBounds {
        index: usize,
        slide_count: usize,
    },
    MissingMedia {
        media: MediaId,
    },
    InvalidPicture {
        media: MediaId,
        detail: &'static str,
    },
    TileLimitExceeded {
        media: MediaId,
        requested: usize,
        limit: usize,
    },
    TextLayout {
        detail: String,
    },
}

impl fmt::Display for RenderInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRelationship {
                scope,
                relationship_id,
            } => write!(formatter, "missing {scope} relationship {relationship_id}"),
            Self::MissingMediaTarget {
                scope,
                relationship_id,
                target,
            } => write!(
                formatter,
                "missing media target {target} for {scope} relationship {relationship_id}"
            ),
            Self::SlideIndexOutOfBounds { index, slide_count } => write!(
                formatter,
                "slide index {index} is out of bounds for {slide_count} slides"
            ),
            Self::MissingMedia { media } => {
                write!(formatter, "missing picture media {}", media.0)
            }
            Self::InvalidPicture { media, detail } => {
                write!(formatter, "invalid picture media {}: {detail}", media.0)
            }
            Self::TileLimitExceeded {
                media,
                requested,
                limit,
            } => write!(
                formatter,
                "picture media {} requests {requested} tiles, above the {limit} tile limit",
                media.0
            ),
            Self::TextLayout { detail } => write!(formatter, "text layout failed: {detail}"),
        }
    }
}

impl Error for RenderInputError {}

/// Raw package parts assembled before inheritance resolution.
#[derive(Clone, Debug)]
pub struct SlideBundle {
    pub slide: CT_Slide,
    pub layout: Arc<CT_SlideLayout>,
    pub master: Arc<CT_SlideMaster>,
    pub theme: Arc<CT_OfficeStyleSheet>,
    pub notes: Option<CT_NotesSlide>,
    pub hidden: bool,
    pub relationships: RelScopes,
}

/// Frozen, format-neutral input consumed by the rendering stage.
#[derive(Clone, Debug)]
pub struct RenderInput {
    pub slides: Vec<ResolvedSlide>,
    pub media: HashMap<MediaId, MediaData>,
    pub fonts: Vec<FontFile>,
    pub metadata: Option<DocumentMetadata>,
}

/// One shape's text as the slide renderer lays it out, in points.
///
/// Rectangles are in slide coordinates for the shape's unrotated frame, which
/// moves with the shape's centre through any parent groups. Vertical text is
/// reported in its reading frame, the content box turned a quarter turn about
/// its centre, so `height` compares with `usable.height` in every direction.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeTextLayout {
    /// The shape's frame.
    pub frame: Rect,
    /// The text rectangle minus insets, scaled by the width factor, where lines wrap.
    pub usable: Rect,
    /// The normal-autofit font scale the renderer applied, or `1.0`.
    pub font_scale: f64,
    /// The height of the laid-out text, paragraph spacing included.
    pub height: f64,
    /// Whether the text misses `usable` by the test normal autofit applies.
    pub overflow: bool,
    /// Every laid-out line in paragraph order, after anchoring.
    pub lines: Vec<TextLineLayout>,
}

/// One laid-out line of shape text, in points.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLineLayout {
    /// The zero-based paragraph the line belongs to.
    pub paragraph_index: usize,
    /// The drawn run and field text, without the bullet marker.
    pub text: String,
    /// The drawn extent of the line, bullet included, and its line height.
    pub bounds: Rect,
    /// The baseline's vertical position.
    pub baseline: f64,
    /// The largest run size on the line after autofit scaling.
    pub font_size: f64,
}

/// Lower every resolved slide to one fixed-size page in presentation order.
pub fn layout_presentation(input: &RenderInput) -> Result<LayoutResult, RenderInputError> {
    let mut font_manager = FontManager::new();
    font_manager.load_additional_fonts(&input.fonts);
    layout_presentation_with_font_manager(input, font_manager)
}

/// Lower every resolved slide using only bundled and presentation-embedded fonts.
///
/// Unlike [`layout_presentation`], this entry point never discovers host system
/// fonts, so its raster output is suitable for deterministic comparison gates.
pub fn layout_presentation_deterministic(
    input: &RenderInput,
) -> Result<LayoutResult, RenderInputError> {
    let mut font_manager =
        FontManager::new_deterministic().map_err(|error| RenderInputError::TextLayout {
            detail: error.to_string(),
        })?;
    font_manager.load_additional_fonts(&input.fonts);
    layout_presentation_with_font_manager(input, font_manager)
}

/// Lowers every slide with the same manager that shaped any frozen group content.
pub fn layout_presentation_with_font_manager(
    input: &RenderInput,
    mut font_manager: FontManager,
) -> Result<LayoutResult, RenderInputError> {
    layout_presentation_with_displayed_numbers(input, &mut font_manager, None, 1)
}

/// Lowers every slide with paragraph directions from the additive resolver path.
///
/// Direction entries follow slides, shapes, text bodies, and paragraphs in that
/// order. A table shape has one text-body entry per cell in row-major order.
pub fn layout_presentation_with_font_manager_and_text_directions(
    input: &RenderInput,
    mut font_manager: FontManager,
    text_directions: &[ResolvedSlideTextDirections],
) -> Result<LayoutResult, RenderInputError> {
    layout_presentation_with_font_manager_and_text_directions_mut(
        input,
        &mut font_manager,
        text_directions,
    )
}

/// Internal facade hook for `oxoffice-pptx` prepared rendering.
///
/// `font_manager` must be the same instance that assigned every
/// [`oxoffice_layout::FontId`]
/// stored in `input`. Rasterization must use the font data retained by that
/// manager so each identifier resolves to the face that shaped it.
#[doc(hidden)]
pub fn layout_presentation_with_font_manager_and_text_directions_mut(
    input: &RenderInput,
    font_manager: &mut FontManager,
    text_directions: &[ResolvedSlideTextDirections],
) -> Result<LayoutResult, RenderInputError> {
    layout_presentation_with_displayed_numbers(input, font_manager, Some(text_directions), 1)
}

/// Layout with explicit displayed slide numbering and unchanged physical page IDs.
/// The facade and timeline assembly share this checked number context.
pub fn layout_presentation_with_displayed_numbers(
    input: &RenderInput,
    font_manager: &mut FontManager,
    text_directions: Option<&[ResolvedSlideTextDirections]>,
    first_slide_number: usize,
) -> Result<LayoutResult, RenderInputError> {
    font_manager.begin_layout();
    let mut shaping_diagnostics = Vec::new();
    let pages = (0..input.slides.len())
        .map(|index| {
            layout_resolved_slide_with_diagnostics(
                input,
                &input.slides[index],
                first_slide_number.checked_add(index).ok_or_else(|| {
                    RenderInputError::TextLayout {
                        detail: "displayed slide number overflow".into(),
                    }
                })?,
                font_manager,
                text_directions
                    .and_then(|directions| directions.get(index))
                    .map(Vec::as_slice),
                None,
                &mut shaping_diagnostics,
            )
            .map_err(|error| text_layout_context(error, &format!("slide {}, ", index + 1)))
            .map(|mut page| {
                page.page_number = index + 1;
                Arc::new(page)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut diagnostics: Vec<Diagnostic> = input
        .slides
        .iter()
        .enumerate()
        .flat_map(|(index, slide)| {
            let summary = (slide.unresolved_member_count > 0).then(|| Diagnostic {
                message: format!(
                    "slide {} incomplete: {} unresolved members",
                    index + 1,
                    slide.unresolved_member_count
                ),
            });
            summary.into_iter().chain(slide.diagnostics.iter().cloned())
        })
        .collect();
    diagnostics.extend(
        shaping_diagnostics
            .into_iter()
            .map(|message| Diagnostic { message }),
    );
    let mut layout = LayoutResult::new(
        pages,
        font_manager.all_font_data(),
        input.metadata.clone(),
        Vec::new(),
    );
    // Font facts stay last: exporters split them off by count.
    diagnostics.extend(oxoffice_layout::missing_glyph_diagnostics(
        &layout.pages,
        &layout.fonts,
        MISSING_GLYPH_WAY_OUT,
    ));
    diagnostics.extend(font_manager.current_font_diagnostics().iter().cloned());
    layout.diagnostics = diagnostics;
    Ok(layout)
}

/// How a deck gets a font for text drawn as empty boxes: presentations take
/// font files, not system fonts.
pub const MISSING_GLYPH_WAY_OUT: &str = "pass a font file for it (CLI --font-dir, Python fonts= or \
     font_dir=, Rust to_pdf_with_diagnostics with font files)";
/// Names where a text layout failure happened, outermost first, so a deck
/// that cannot be laid out points at the slide and shape to fix.
fn text_layout_context(error: RenderInputError, prefix: &str) -> RenderInputError {
    match error {
        RenderInputError::TextLayout { detail } => RenderInputError::TextLayout {
            detail: format!("{prefix}{detail}"),
        },
        error => error,
    }
}

/// Lower one zero-based resolved slide to a fixed-size page.
pub fn layout_slide(input: &RenderInput, index: usize) -> Result<PageFrame, RenderInputError> {
    let mut font_manager = FontManager::new();
    font_manager.load_additional_fonts(&input.fonts);
    layout_slide_with_fonts(input, index, &mut font_manager)
}

/// Lays out one resolved shape's text exactly as slide lowering draws it.
///
/// `text` is the shape's own `ResolvedContent::Text` body and `text_directions`
/// the shape's entry in the resolver's paragraph directions. `page_number` is
/// the one-based number that slide-number fields show. `width_factor` scales
/// the usable width before line breaking and keeps its left edge. Lowering
/// passes `1.0`, so that factor reproduces the drawn lines.
pub fn layout_shape_text(
    shape: &ResolvedShape,
    text: &ResolvedTextBody,
    font_manager: &mut FontManager,
    page_number: usize,
    text_directions: &[Vec<oxoffice_layout::TextDirection>],
    width_factor: f64,
) -> Result<ShapeTextLayout, RenderInputError> {
    let (usable, _, stacked) = stack_shape_text(
        shape,
        text,
        font_manager,
        page_number,
        text_directions,
        width_factor,
    )?;
    let centre = Point {
        x: shape.bounds.x + shape.bounds.width / 2.0,
        y: shape.bounds.y + shape.bounds.height / 2.0,
    };
    let placed = shape.group_transform.apply(centre);
    let origin = Point {
        x: shape.bounds.x + (placed.x - centre.x),
        y: shape.bounds.y + (placed.y - centre.y),
    };
    let on_slide = |rect: Rect| Rect {
        x: origin.x + rect.x,
        y: origin.y + rect.y,
        ..rect
    };
    Ok(ShapeTextLayout {
        frame: Rect {
            x: origin.x,
            y: origin.y,
            width: shape.bounds.width,
            height: shape.bounds.height,
        },
        usable: on_slide(usable),
        font_scale: stacked.font_scale,
        height: stacked.height,
        overflow: !stacked.fits(usable),
        lines: stacked
            .lines
            .into_iter()
            .map(|line| TextLineLayout {
                bounds: on_slide(line.bounds),
                baseline: origin.y + line.baseline,
                ..line
            })
            .collect(),
    })
}

/// Ordered, bounded native-informed authoring candidates, in OOXML units.
/// Ordinary render-time autofit does not use this authoring policy.
pub const AUTOFIT_CANDIDATES: [(u32, u32); 14] = [
    (100000, 0),
    (100000, 10000),
    (92500, 0),
    (92500, 10000),
    (92500, 20000),
    (85000, 20000),
    (77500, 20000),
    (70000, 20000),
    (62500, 20000),
    (55000, 20000),
    (47500, 20000),
    (40000, 20000),
    (32500, 20000),
    (25000, 20000),
];

/// One real authoring layout trial before vertical anchoring.
#[derive(Clone, Debug, PartialEq)]
pub struct AutofitMeasurement {
    pub usable: Rect,
    pub physical_height: f64,
    pub line_advance_height: f64,
    pub font_scale: f64,
    /// Whether actual shaped lines fit the fixed available width.
    pub width_fits: bool,
    pub fits: bool,
}

/// Measures actual selected-face segments with the shared line breaker.
/// This is used by explicit mutation and its read-only report, not by ordinary
/// rendering. The provided body may already own stored Normal values.
pub fn measure_authoring_text(
    shape: &ResolvedShape,
    text: &ResolvedTextBody,
    fonts: &mut FontManager,
    page_number: usize,
    directions: &[Vec<oxoffice_layout::TextDirection>],
) -> Result<AutofitMeasurement, RenderInputError> {
    let mut unanchored = text.clone();
    unanchored.anchor = oxoffice_pptx_layout::TextAnchor::Top;
    let (usable, _) =
        text::oriented_content_box(text::content_box(shape, &unanchored), unanchored.vertical);
    if !usable.x.is_finite()
        || !usable.y.is_finite()
        || !usable.width.is_finite()
        || !usable.height.is_finite()
        || usable.width <= 0.0
        || usable.height <= 0.0
    {
        return Err(RenderInputError::TextLayout {
            detail: "autofit usable bounds must be finite and positive".to_owned(),
        });
    }
    // Extent is relative to the reading frame. Accumulating absolute vertical
    // origins makes changing the fitted extent change floating cancellation.
    // Keep public usable bounds, but shape and measure at one stable origin.
    let measurement_box = Rect {
        x: 0.0,
        y: 0.0,
        ..usable
    };
    let stacked = text::stack_text_for_page_with_directions(
        fonts,
        measurement_box,
        &unanchored,
        page_number,
        directions.first().map(Vec::as_slice).unwrap_or(&[]),
    )
    .map_err(|error| RenderInputError::TextLayout {
        detail: error.to_string(),
    })?;
    let (physical_height, fits) = stacked
        .authoring_extent(fonts, &unanchored, measurement_box)
        .map_err(|error| RenderInputError::TextLayout {
            detail: error.to_string(),
        })?;
    Ok(AutofitMeasurement {
        usable,
        physical_height,
        line_advance_height: stacked.height,
        font_scale: stacked.font_scale,
        width_fits: stacked.width_fits,
        fits,
    })
}

/// Checks a proposed frame using its actual persisted anchor and provider.
/// Unlike the unanchored sizing trial, this validates the published placement.
pub fn authoring_text_fits_at_anchor(
    shape: &ResolvedShape,
    text: &ResolvedTextBody,
    fonts: &mut FontManager,
    page_number: usize,
    directions: &[Vec<oxoffice_layout::TextDirection>],
) -> Result<bool, RenderInputError> {
    let (usable, _, stacked) = stack_shape_text(shape, text, fonts, page_number, directions, 1.0)?;
    let (_, fits) = stacked
        .authoring_extent(fonts, text, usable)
        .map_err(|error| RenderInputError::TextLayout {
            detail: error.to_string(),
        })?;
    Ok(usable.width.is_finite()
        && usable.height.is_finite()
        && usable.width > 0.0
        && usable.height > 0.0
        && fits)
}

/// Projects the exact merged cell box used by table lowering for authoring.
/// Continuation cells have no independently sized text owner.
pub fn authoring_table_cell(
    shape: &ResolvedShape,
    table: &ResolvedTable,
    row_index: usize,
    column_index: usize,
) -> Result<(ResolvedShape, ResolvedTextBody), RenderInputError> {
    let fail = || RenderInputError::TextLayout {
        detail: "autofit table cell is missing or a merge continuation".to_owned(),
    };
    let row = table.rows.get(row_index).ok_or_else(fail)?;
    let cell = row.cells.get(column_index).ok_or_else(fail)?;
    if cell.horizontal_merge || cell.vertical_merge || column_index >= table.column_widths.len() {
        return Err(fail());
    }
    let rows = usize::try_from(cell.row_span)
        .unwrap_or(usize::MAX)
        .max(1)
        .min(table.rows.len() - row_index);
    let columns = usize::try_from(cell.grid_span)
        .unwrap_or(usize::MAX)
        .max(1)
        .min(table.column_widths.len() - column_index);
    let mut widths = table.column_widths.clone();
    if table.right_to_left {
        widths.reverse();
    }
    let column_offsets = cumulative_offsets(&widths);
    let row_offsets =
        cumulative_offsets(&table.rows.iter().map(|row| row.height).collect::<Vec<_>>());
    let visual_column = if table.right_to_left {
        table.column_widths.len() - column_index - columns
    } else {
        column_index
    };
    let mut text = cell.text.clone().ok_or_else(fail)?;
    text.insets = cell.margins;
    let mut cell_shape = shape.clone();
    cell_shape.bounds = Rect {
        x: column_offsets[visual_column],
        y: row_offsets[row_index],
        width: column_offsets[visual_column + columns] - column_offsets[visual_column],
        height: row_offsets[row_index + rows] - row_offsets[row_index],
    };
    cell_shape.geometry = oxoffice_pptx_layout::ResolvedGeometry::Rectangle;
    cell_shape.content = ResolvedContent::Text(text.clone());
    Ok((cell_shape, text))
}

/// The height of `table` as rendering draws it, each row grown to fit its cells' text.
/// `directions` holds one entry per cell in row order, as rendering takes them.
pub fn authoring_table_height(
    table: &ResolvedTable,
    fonts: &mut FontManager,
    page_number: usize,
    directions: &[Vec<oxoffice_layout::TextDirection>],
) -> Result<f64, RenderInputError> {
    Ok(grown_row_heights(table, fonts, page_number, directions)?
        .iter()
        .sum())
}

/// Runs independent Q native-informed and S persisted-render trials for one
/// candidate. Every invocation re-shapes its own source clone. Q's rounding
/// never changes the original nominal values or ordinary fractional rendering.
pub fn measure_autofit_candidate(
    shape: &ResolvedShape,
    text: &ResolvedTextBody,
    fonts: &mut FontManager,
    page_number: usize,
    directions: &[Vec<oxoffice_layout::TextDirection>],
    pair: (u32, u32),
) -> Result<(AutofitMeasurement, AutofitMeasurement), RenderInputError> {
    if !AUTOFIT_CANDIDATES.contains(&pair) {
        return Err(RenderInputError::TextLayout {
            detail: "autofit pair is outside the admitted policy".to_owned(),
        });
    }
    let scale = f64::from(pair.0) / 100000.0;
    let reduction = f64::from(pair.1) / 100000.0;
    let round = |size: Option<f64>| -> Result<Option<f64>, RenderInputError> {
        let nominal = size.unwrap_or(18.0);
        let rounded = (nominal * scale + 0.5).floor();
        if !nominal.is_finite() || nominal <= 0.0 || rounded <= 0.0 {
            return Err(RenderInputError::TextLayout {
                detail: "autofit quantized point size is not positive and representable".to_owned(),
            });
        }
        Ok(Some(rounded))
    };
    let mut quantized = text.clone();
    quantized.autofit = oxoffice_pptx_layout::ResolvedAutofit::Normal {
        font_scale: Some(1.0),
        line_spacing_reduction: Some(reduction),
    };
    for paragraph in &mut quantized.paragraphs {
        paragraph.end_style.font_size = round(paragraph.end_style.font_size)?;
        for run in &mut paragraph.runs {
            if let oxoffice_pptx_layout::ResolvedTextRun::Text { style, .. }
            | oxoffice_pptx_layout::ResolvedTextRun::Field { style, .. } = run
            {
                style.font_size = round(style.font_size)?;
            }
        }
        if let Some(
            oxoffice_pptx_layout::ResolvedBullet::Character { size, .. }
            | oxoffice_pptx_layout::ResolvedBullet::AutoNumber { size, .. },
        ) = &mut paragraph.bullet
            && let Some(oxoffice_pptx_layout::ResolvedBulletSize::Points(points)) = size
        {
            *points = round(Some(*points))?.expect("validated positive point size");
        }
    }
    let q = measure_authoring_text(shape, &quantized, fonts, page_number, directions)?;
    let mut stored = text.clone();
    stored.autofit = oxoffice_pptx_layout::ResolvedAutofit::Normal {
        font_scale: Some(scale),
        line_spacing_reduction: Some(reduction),
    };
    let s = measure_authoring_text(shape, &stored, fonts, page_number, directions)?;
    Ok((q, s))
}

fn layout_slide_with_fonts(
    input: &RenderInput,
    index: usize,
    font_manager: &mut FontManager,
) -> Result<PageFrame, RenderInputError> {
    layout_slide_with_fonts_and_text_directions(input, index, font_manager, None)
}

fn layout_slide_with_fonts_and_text_directions(
    input: &RenderInput,
    index: usize,
    font_manager: &mut FontManager,
    text_directions: Option<&[Vec<Vec<oxoffice_layout::TextDirection>>]>,
) -> Result<PageFrame, RenderInputError> {
    layout_slide_with_fonts_text_directions_and_states(
        input,
        index,
        font_manager,
        text_directions,
        None,
    )
}

pub(crate) fn layout_slide_with_fonts_text_directions_and_states(
    input: &RenderInput,
    index: usize,
    font_manager: &mut FontManager,
    text_directions: Option<&[Vec<Vec<oxoffice_layout::TextDirection>>]>,
    shape_states: Option<&[oxoffice_pptx_layout::timeline::EvaluatedShapeState]>,
) -> Result<PageFrame, RenderInputError> {
    let slide = input
        .slides
        .get(index)
        .ok_or(RenderInputError::SlideIndexOutOfBounds {
            index,
            slide_count: input.slides.len(),
        })?;
    layout_resolved_slide_with_fonts_text_directions_and_states(
        input,
        slide,
        index + 1,
        font_manager,
        text_directions,
        shape_states,
    )
}

pub(crate) fn layout_resolved_slide_with_fonts_text_directions_and_states(
    input: &RenderInput,
    slide: &ResolvedSlide,
    page_number: usize,
    font_manager: &mut FontManager,
    text_directions: Option<&[Vec<Vec<oxoffice_layout::TextDirection>>]>,
    shape_states: Option<&[oxoffice_pptx_layout::timeline::EvaluatedShapeState]>,
) -> Result<PageFrame, RenderInputError> {
    layout_resolved_slide_with_diagnostics(
        input,
        slide,
        page_number,
        font_manager,
        text_directions,
        shape_states,
        &mut Vec::new(),
    )
}

pub(crate) fn layout_resolved_slide_with_diagnostics(
    input: &RenderInput,
    slide: &ResolvedSlide,
    page_number: usize,
    font_manager: &mut FontManager,
    text_directions: Option<&[Vec<Vec<oxoffice_layout::TextDirection>>]>,
    shape_states: Option<&[oxoffice_pptx_layout::timeline::EvaluatedShapeState]>,
    diagnostics: &mut Vec<String>,
) -> Result<PageFrame, RenderInputError> {
    // Resolved content can also be supplied by a native caller. Validate its
    // concrete group graph before cloning or lowering recursively.
    for shape in &slide.shapes {
        if let ResolvedContent::Group(root) = &shape.content {
            let mut pending = vec![(root, 1usize)];
            while let Some((group, depth)) = pending.pop() {
                if depth > oxoffice_pptx_oxml::MAX_PRESENTATION_NESTING {
                    return Err(RenderInputError::TextLayout {
                        detail: format!(
                            "render resolved group nesting exceeds limit {}, offending depth {depth}",
                            oxoffice_pptx_oxml::MAX_PRESENTATION_NESTING
                        ),
                    });
                }
                pending.extend(group.children.iter().filter_map(|child| {
                    if let PositionedElement::Group(group) = child {
                        Some((group, depth + 1))
                    } else {
                        None
                    }
                }));
            }
        }
    }
    let mut elements = Vec::new();
    let mut background_paint = None;
    match slide.background.as_ref() {
        Some(ResolvedBackground::Paint(paint)) => background_paint = Some(paint.clone()),
        Some(ResolvedBackground::Image(image)) => {
            let shape = ResolvedShape {
                group_transform: Transform::IDENTITY,
                bounds: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: slide.size.0,
                    height: slide.size.1,
                },
                rotation_deg: 0.0,
                flip_h: false,
                flip_v: false,
                geometry: ResolvedGeometry::Rectangle,
                fill: None,
                image_fill: None,
                line: None,
                head_end: None,
                tail_end: None,
                shadow: None,
                content: ResolvedContent::None,
                unsupported: None,
            };
            let paths = [Path::rect(Rect {
                x: 0.0,
                y: 0.0,
                width: slide.size.0,
                height: slide.size.1,
            })];
            elements.extend(lower_picture(input, &shape, &paths, image)?);
        }
        None => {}
    }
    elements.extend(
        slide
            .shapes
            .iter()
            .enumerate()
            .map(|(shape_index, shape)| {
                let state = shape_states.and_then(|states| states.get(shape_index));
                lower_shape(
                    input,
                    shape,
                    font_manager,
                    page_number,
                    text_directions
                        .and_then(|directions| directions.get(shape_index))
                        .map(Vec::as_slice),
                    diagnostics,
                )
                .map_err(|error| {
                    text_layout_context(
                        error,
                        &format!("shape {} in draw order: ", shape_index + 1),
                    )
                })
                .map(|mut element| {
                    let Some(state) = state else {
                        return element;
                    };
                    let opacity = if state.visible {
                        f64::from(state.opacity.clamp(0.0, 1.0))
                    } else {
                        0.0
                    };
                    if let PositionedElement::Group(group) = &mut element {
                        group.opacity *= opacity;
                        if let Some(mask) =
                            state.local_opacity_mask((shape.bounds.width, shape.bounds.height))
                        {
                            group.effects.push(mask);
                        }
                        for clip in
                            state.local_clip_paths((shape.bounds.width, shape.bounds.height))
                        {
                            if group.clip.is_none() {
                                group.clip = Some(clip);
                            } else {
                                group.children = vec![PositionedElement::Group(GroupElement {
                                    transform: Transform::IDENTITY,
                                    clip: Some(clip),
                                    opacity: 1.0,
                                    effects: Vec::new(),
                                    children: std::mem::take(&mut group.children),
                                })];
                            }
                        }
                    }
                    element
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut page = PageFrame::new(page_number, slide.size.0, slide.size.1, elements);
    page.background = background_paint;
    Ok(page)
}

// The owned renderer graph is finalized in document order without recursive
// Clone calls. Admission above has checked the source graph before this work.
fn copy_resolved_group(root: &GroupElement) -> GroupElement {
    let shallow = |group: &GroupElement| GroupElement {
        transform: group.transform,
        clip: group.clip.clone(),
        opacity: group.opacity,
        effects: group.effects.clone(),
        children: Vec::with_capacity(group.children.len()),
    };
    let mut pending = vec![(root, 0usize, shallow(root))];
    loop {
        let (source, next, output) = pending.last_mut().expect("open renderer group");
        if let Some(child) = source.children.get(*next) {
            *next += 1;
            if let PositionedElement::Group(group) = child {
                pending.push((group, 0, shallow(group)));
            } else {
                output.children.push(child.clone());
            }
        } else {
            let (_, _, group) = pending.pop().expect("completed renderer group");
            if let Some((_, _, output)) = pending.last_mut() {
                output.children.push(PositionedElement::Group(group));
            } else {
                return group;
            }
        }
    }
}

fn lower_shape(
    input: &RenderInput,
    shape: &ResolvedShape,
    font_manager: &mut FontManager,
    page_number: usize,
    text_directions: Option<&[Vec<oxoffice_layout::TextDirection>]>,
    diagnostics: &mut Vec<String>,
) -> Result<PositionedElement, RenderInputError> {
    let paths = if matches!(shape.content, ResolvedContent::Table(_)) {
        Vec::new()
    } else {
        match &shape.geometry {
            ResolvedGeometry::Rectangle | ResolvedGeometry::BoundsFallback => {
                vec![Path::rect(Rect {
                    x: 0.0,
                    y: 0.0,
                    width: shape.bounds.width,
                    height: shape.bounds.height,
                })]
            }
            ResolvedGeometry::Custom { paths, .. } => paths.clone(),
        }
    };
    let stroke = match (&shape.geometry, &shape.fill, &shape.image_fill, &shape.line) {
        (ResolvedGeometry::BoundsFallback, None, None, None) => {
            Some(Stroke::new(Paint::Solid(Color::BLACK), 1.0))
        }
        _ => shape.line.clone(),
    };
    let mut text_children = Vec::new();
    let mut children = shape
        .image_fill
        .as_ref()
        .map(|image| lower_picture(input, shape, &paths, image))
        .transpose()?
        .unwrap_or_default();
    match &shape.content {
        ResolvedContent::Image(image) => {
            children.extend(lower_picture(input, shape, &paths, image)?)
        }
        ResolvedContent::Text(text_body) => {
            let (_, text_transform, stacked) = stack_shape_text(
                shape,
                text_body,
                font_manager,
                page_number,
                text_directions.unwrap_or(&[]),
                1.0,
            )?;
            diagnostics.extend(stacked.diagnostics.iter().cloned());
            debug_assert!(stacked.width.is_finite() && stacked.height.is_finite());
            text_children = if let Some(transform) = text_transform {
                vec![PositionedElement::Group(GroupElement {
                    transform,
                    clip: None,
                    opacity: 1.0,
                    effects: Vec::new(),
                    children: stacked.elements,
                })]
            } else {
                stacked.elements
            };
        }
        ResolvedContent::Table(table) => {
            children.extend(lower_table(
                table,
                font_manager,
                page_number,
                text_directions.unwrap_or(&[]),
                diagnostics,
            )?);
        }
        ResolvedContent::Group(group) => {
            children.push(PositionedElement::Group(copy_resolved_group(group)));
        }
        _ => {}
    }
    children.extend(
        paths
            .iter()
            .cloned()
            .map(|path| {
                PositionedElement::Path(PathElement {
                    path,
                    fill: shape.fill.clone(),
                    stroke: stroke.clone(),
                })
            })
            .collect::<Vec<_>>(),
    );
    if let Some(line) = &shape.line {
        let (head_tangent, tail_tangent) = endpoint_tangents(&paths);
        if let Some(path) = shape
            .head_end
            .as_ref()
            .zip(head_tangent)
            .and_then(|(end, tangent)| line_end_path(end, tangent, line.width))
        {
            children.push(filled_line_end(path, &line.paint));
        }
        if let Some(path) = shape
            .tail_end
            .as_ref()
            .zip(tail_tangent)
            .and_then(|(end, tangent)| line_end_path(end, tangent, line.width))
        {
            children.push(filled_line_end(path, &line.paint));
        }
    }
    children.extend(text_children);
    Ok(PositionedElement::Group(GroupElement {
        transform: shape_transform(shape),
        clip: None,
        opacity: 1.0,
        effects: shape.shadow.iter().cloned().collect(),
        children,
    }))
}

/// Stacks a shape's text in its oriented content box, the one path that both
/// lowering and [`layout_shape_text`] take.
fn stack_shape_text(
    shape: &ResolvedShape,
    text_body: &ResolvedTextBody,
    font_manager: &mut FontManager,
    page_number: usize,
    text_directions: &[Vec<oxoffice_layout::TextDirection>],
    width_factor: f64,
) -> Result<(Rect, Option<Transform>, text::StackedText), RenderInputError> {
    let (mut content_box, text_transform) =
        text::oriented_content_box(text::content_box(shape, text_body), text_body.vertical);
    content_box.width *= width_factor;
    let paragraph_directions = text_directions.first().map(Vec::as_slice).unwrap_or(&[]);
    let stacked = text::stack_text_for_page_with_directions(
        font_manager,
        content_box,
        text_body,
        page_number,
        paragraph_directions,
    )
    .map_err(|error| RenderInputError::TextLayout {
        detail: error.to_string(),
    })?;
    Ok((content_box, text_transform, stacked))
}

fn table_cell_span(
    table: &ResolvedTable,
    row_index: usize,
    column_index: usize,
    cell: &oxoffice_pptx_layout::ResolvedTableCell,
) -> Option<(usize, usize, usize)> {
    if cell.horizontal_merge || cell.vertical_merge || column_index >= table.column_widths.len() {
        return None;
    }
    let row_span = usize::try_from(cell.row_span)
        .unwrap_or(usize::MAX)
        .max(1)
        .min(table.rows.len().saturating_sub(row_index));
    let column_span = usize::try_from(cell.grid_span)
        .unwrap_or(usize::MAX)
        .max(1)
        .min(table.column_widths.len().saturating_sub(column_index));
    let visual_column = if table.right_to_left {
        table
            .column_widths
            .len()
            .saturating_sub(column_index + column_span)
    } else {
        column_index
    };
    Some((row_span, column_span, visual_column))
}

/// The painted height one cell needs, measured against its current span.
///
/// Horizontal text uses the stacked line advance. Any other direction uses
/// the longest oriented line, which is the visual vertical size after
/// rotation. The measure pass discards its diagnostics. Stored `a:tr/@h`
/// is not an input this function writes.
fn painted_row_text_extent(
    font_manager: &mut FontManager,
    width: f64,
    height: f64,
    cell: &oxoffice_pptx_layout::ResolvedTableCell,
    page_number: usize,
    paragraph_directions: &[oxoffice_layout::TextDirection],
) -> Result<f64, RenderInputError> {
    let Some(source) = &cell.text else {
        return Ok(0.0);
    };
    let mut text_body = source.clone();
    text_body.insets = cell.margins;
    let content = Rect {
        x: text_body.insets.left,
        y: text_body.insets.top,
        width: (width - text_body.insets.left - text_body.insets.right).max(0.0),
        height: (height - text_body.insets.top - text_body.insets.bottom).max(0.0),
    };
    let (content, _) = text::oriented_content_box(content, text_body.vertical);
    let stacked = text::stack_text_for_page_with_directions(
        font_manager,
        content,
        &text_body,
        page_number,
        paragraph_directions,
    )
    .map_err(|error| RenderInputError::TextLayout {
        detail: error.to_string(),
    })?;
    let content_extent = if text_body.vertical == oxoffice_pptx_layout::TextDirection::Horizontal {
        stacked.height
    } else {
        stacked.width
    };
    Ok(text_body.insets.top + content_extent + text_body.insets.bottom)
}

/// The height of each table row once it has grown to fit its cells' text, as PowerPoint grows a row
/// whose stored `a:tr/@h` is too short. A cell spanning rows grows its last row.
fn grown_row_heights(
    table: &ResolvedTable,
    font_manager: &mut FontManager,
    page_number: usize,
    text_directions: &[Vec<oxoffice_layout::TextDirection>],
) -> Result<Vec<f64>, RenderInputError> {
    let mut physical_column_widths = table.column_widths.clone();
    if table.right_to_left {
        physical_column_widths.reverse();
    }
    let column_offsets = cumulative_offsets(&physical_column_widths);
    let mut required_heights = table.rows.iter().map(|row| row.height).collect::<Vec<_>>();
    let mut measure_index = 0usize;
    for (row_index, row) in table.rows.iter().enumerate() {
        for (column_index, cell) in row.cells.iter().enumerate() {
            let paragraph_directions = text_directions
                .get(measure_index)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            measure_index += 1;
            let Some((row_span, column_span, visual_column)) =
                table_cell_span(table, row_index, column_index, cell)
            else {
                continue;
            };
            if cell.text.is_none() {
                continue;
            }
            let width = column_offsets[visual_column + column_span] - column_offsets[visual_column];
            let height = required_heights[row_index..row_index + row_span]
                .iter()
                .sum::<f64>();
            let needed = painted_row_text_extent(
                font_manager,
                width,
                height,
                cell,
                page_number,
                paragraph_directions,
            )?;
            if needed > height + 0.01 {
                required_heights[row_index + row_span - 1] += needed - height;
            }
        }
    }
    Ok(required_heights)
}

fn lower_table(
    table: &ResolvedTable,
    font_manager: &mut FontManager,
    page_number: usize,
    text_directions: &[Vec<oxoffice_layout::TextDirection>],
    diagnostics: &mut Vec<String>,
) -> Result<Vec<PositionedElement>, RenderInputError> {
    let mut physical_column_widths = table.column_widths.clone();
    if table.right_to_left {
        physical_column_widths.reverse();
    }
    let column_offsets = cumulative_offsets(&physical_column_widths);
    let required_heights = grown_row_heights(table, font_manager, page_number, text_directions)?;
    let row_offsets = cumulative_offsets(&required_heights);
    let mut fills = Vec::new();
    if let Some(background) = &table.background {
        fills.push(PositionedElement::Path(PathElement {
            path: Path::rect(Rect {
                x: 0.0,
                y: 0.0,
                width: column_offsets.last().copied().unwrap_or(0.0),
                height: row_offsets.last().copied().unwrap_or(0.0),
            }),
            fill: Some(background.clone()),
            stroke: None,
        }));
    }
    let mut texts = Vec::new();
    let mut borders: HashMap<(bool, usize, usize), TableBorderCandidate> = HashMap::new();
    let mut cell_index = 0usize;

    for (row_index, row) in table.rows.iter().enumerate() {
        for (column_index, cell) in row.cells.iter().enumerate() {
            let paragraph_directions = text_directions
                .get(cell_index)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            cell_index += 1;
            if cell.horizontal_merge
                || cell.vertical_merge
                || column_index >= table.column_widths.len()
            {
                continue;
            }
            let row_span = usize::try_from(cell.row_span)
                .unwrap_or(usize::MAX)
                .max(1)
                .min(table.rows.len().saturating_sub(row_index));
            let column_span = usize::try_from(cell.grid_span)
                .unwrap_or(usize::MAX)
                .max(1)
                .min(table.column_widths.len().saturating_sub(column_index));
            let visual_column = if table.right_to_left {
                table
                    .column_widths
                    .len()
                    .saturating_sub(column_index + column_span)
            } else {
                column_index
            };
            let rectangle = Rect {
                x: column_offsets[visual_column],
                y: row_offsets[row_index],
                width: column_offsets[visual_column + column_span] - column_offsets[visual_column],
                height: row_offsets[row_index + row_span] - row_offsets[row_index],
            };
            if let Some(fill) = &cell.fill {
                fills.push(PositionedElement::Path(PathElement {
                    path: Path::rect(rectangle),
                    fill: Some(fill.clone()),
                    stroke: None,
                }));
            }
            if let Some(text_body) = &cell.text {
                let mut text_body = text_body.clone();
                text_body.insets = cell.margins;
                let content = Rect {
                    x: rectangle.x + text_body.insets.left,
                    y: rectangle.y + text_body.insets.top,
                    width: (rectangle.width - text_body.insets.left - text_body.insets.right)
                        .max(0.0),
                    height: (rectangle.height - text_body.insets.top - text_body.insets.bottom)
                        .max(0.0),
                };
                let (content, transform) = text::oriented_content_box(content, text_body.vertical);
                let stacked = text::stack_text_for_page_with_directions(
                    font_manager,
                    content,
                    &text_body,
                    page_number,
                    paragraph_directions,
                )
                .map_err(|error| RenderInputError::TextLayout {
                    detail: error.to_string(),
                })?;
                diagnostics.extend(stacked.diagnostics.iter().cloned());
                if let Some(transform) = transform {
                    texts.push(PositionedElement::Group(GroupElement {
                        transform,
                        clip: None,
                        opacity: 1.0,
                        effects: Vec::new(),
                        children: stacked.elements,
                    }));
                } else {
                    texts.extend(stacked.elements);
                }
            }

            let last_row = row_index + row_span - 1;
            let last_column = column_index + column_span - 1;
            for logical_column in column_index..=last_column {
                let physical_column = if table.right_to_left {
                    table.column_widths.len() - logical_column - 1
                } else {
                    logical_column
                };
                let top = table.rows[row_index]
                    .cells
                    .get(logical_column)
                    .and_then(|covered| covered.top.as_ref())
                    .or(cell.top.as_ref());
                let bottom = table.rows[last_row]
                    .cells
                    .get(logical_column)
                    .and_then(|covered| covered.bottom.as_ref())
                    .or(cell.bottom.as_ref());
                insert_table_border(&mut borders, (true, row_index, physical_column), top, 4);
                insert_table_border(
                    &mut borders,
                    (true, row_index + row_span, physical_column),
                    bottom,
                    2,
                );
            }
            let left_column = if table.right_to_left {
                last_column
            } else {
                column_index
            };
            let right_column = if table.right_to_left {
                column_index
            } else {
                last_column
            };
            for covered_row in row_index..=last_row {
                let left = table.rows[covered_row]
                    .cells
                    .get(left_column)
                    .and_then(|covered| covered.left.as_ref())
                    .or(cell.left.as_ref());
                let right = table.rows[covered_row]
                    .cells
                    .get(right_column)
                    .and_then(|covered| covered.right.as_ref())
                    .or(cell.right.as_ref());
                insert_table_border(&mut borders, (false, visual_column, covered_row), left, 3);
                insert_table_border(
                    &mut borders,
                    (false, visual_column + column_span, covered_row),
                    right,
                    1,
                );
            }
        }
    }

    let mut ordered_borders = borders.into_iter().collect::<Vec<_>>();
    ordered_borders.sort_by_key(|(key, _)| *key);
    let mut border_elements = Vec::new();
    for ((horizontal, boundary, segment), candidate) in ordered_borders {
        let Some(stroke) = candidate.border.stroke else {
            continue;
        };
        let path = if horizontal {
            open_path(
                Point {
                    x: column_offsets[segment],
                    y: row_offsets[boundary],
                },
                Point {
                    x: column_offsets[segment + 1],
                    y: row_offsets[boundary],
                },
            )
        } else {
            open_path(
                Point {
                    x: column_offsets[boundary],
                    y: row_offsets[segment],
                },
                Point {
                    x: column_offsets[boundary],
                    y: row_offsets[segment + 1],
                },
            )
        };
        border_elements.push(PositionedElement::Path(PathElement {
            path,
            fill: None,
            stroke: Some(stroke),
        }));
    }
    fills.extend(texts);
    fills.extend(border_elements);
    Ok(fills)
}

struct TableBorderCandidate {
    border: ResolvedTableBorder,
    side_rank: u8,
}

fn insert_table_border(
    borders: &mut HashMap<(bool, usize, usize), TableBorderCandidate>,
    key: (bool, usize, usize),
    border: Option<&ResolvedTableBorder>,
    side_rank: u8,
) {
    let Some(border) = border else {
        return;
    };
    let candidate = TableBorderCandidate {
        border: border.clone(),
        side_rank,
    };
    let replace = borders.get(&key).is_none_or(|current| {
        let candidate_width = candidate
            .border
            .stroke
            .as_ref()
            .map_or(0.0, |stroke| stroke.width);
        let current_width = current
            .border
            .stroke
            .as_ref()
            .map_or(0.0, |stroke| stroke.width);
        (
            candidate.border.priority,
            ordered_width(candidate_width),
            candidate.side_rank,
        ) > (
            current.border.priority,
            ordered_width(current_width),
            current.side_rank,
        )
    });
    if replace {
        borders.insert(key, candidate);
    }
}

fn ordered_width(width: f64) -> u64 {
    if width.is_finite() && width >= 0.0 {
        width.to_bits()
    } else {
        0
    }
}

fn cumulative_offsets(lengths: &[f64]) -> Vec<f64> {
    let mut offsets = Vec::with_capacity(lengths.len() + 1);
    offsets.push(0.0);
    for length in lengths {
        offsets.push(offsets.last().copied().unwrap_or(0.0) + length.max(0.0));
    }
    offsets
}

fn open_path(start: Point, end: Point) -> Path {
    Path {
        commands: vec![PathCommand::MoveTo(start), PathCommand::LineTo(end)],
        fill_rule: oxoffice_layout::FillRule::NonZero,
    }
}

const MAX_TILE_ELEMENTS: usize = 4_096;
const MAX_TILED_IMAGE_BYTES: usize = 64 * 1024 * 1024;

fn lower_picture(
    input: &RenderInput,
    shape: &ResolvedShape,
    paths: &[Path],
    resolved_image: &ResolvedImage,
) -> Result<Vec<PositionedElement>, RenderInputError> {
    let media_id = resolved_image.media;
    let media = input
        .media
        .get(&media_id)
        .ok_or(RenderInputError::MissingMedia { media: media_id })?;
    let crop = normalized_insets(resolved_image.src_rect, media_id, "source crop")?;
    let elements = match &resolved_image.placement {
        ResolvedImagePlacement::Stretch { fill_rect } => {
            let fill_rect = normalized_insets(*fill_rect, media_id, "stretch fill rectangle")?;
            let destination = inset_rect(
                picture_coverage_rect(shape, resolved_image.rotate_with_shape),
                fill_rect,
            );
            let image = picture_image(media_id, media, expanded_crop_rect(destination, crop));
            let image = counter_rotate_image(image, shape, resolved_image.rotate_with_shape);
            let mut image = if crop.is_some() {
                clipped_group(Path::rect(destination), vec![image])
            } else {
                image
            };
            if !matches!(shape.geometry, ResolvedGeometry::Rectangle)
                || (!resolved_image.rotate_with_shape && shape.rotation_deg != 0.0)
            {
                image = clip_to_picture_shape(shape, paths, vec![image]);
            }
            vec![image]
        }
        ResolvedImagePlacement::Tile(tile) => lower_tiled_picture(
            shape,
            paths,
            media_id,
            media,
            crop,
            tile,
            resolved_image.dpi,
            resolved_image.rotate_with_shape,
        )?,
    };
    if resolved_image.opacity < 1.0 {
        return Ok(vec![PositionedElement::Group(GroupElement {
            transform: Transform::IDENTITY,
            clip: None,
            opacity: resolved_image.opacity,
            effects: Vec::new(),
            children: elements,
        })]);
    }
    Ok(elements)
}

#[allow(clippy::too_many_arguments)]
fn lower_tiled_picture(
    shape: &ResolvedShape,
    paths: &[Path],
    media_id: MediaId,
    media: &MediaData,
    crop: Option<CropRect>,
    tile: &ResolvedTilePlacement,
    declared_dpi: Option<f64>,
    rotate_with_shape: bool,
) -> Result<Vec<PositionedElement>, RenderInputError> {
    let info = oxoffice_media::probe(&media.bytes).ok_or(RenderInputError::InvalidPicture {
        media: media_id,
        detail: "tile image metadata is unavailable",
    })?;
    let (native_width, native_height) = tile_native_size_points(info, declared_dpi, media_id)?;
    let tile_width = native_width * tile.scale_x;
    let tile_height = native_height * tile.scale_y;
    if !tile_width.is_finite()
        || !tile_height.is_finite()
        || tile_width <= 0.0
        || tile_height <= 0.0
        || !tile.translation.x.is_finite()
        || !tile.translation.y.is_finite()
    {
        return Err(RenderInputError::InvalidPicture {
            media: media_id,
            detail: "tile size or translation is not finite and positive",
        });
    }
    let shape_rect = local_shape_rect(shape);
    let coverage_rect = picture_coverage_rect(shape, rotate_with_shape);
    let anchor = tile_alignment_origin(shape_rect, tile_width, tile_height, tile.alignment);
    let translated_anchor = Point {
        x: anchor.x + tile.translation.x,
        y: anchor.y + tile.translation.y,
    };
    let origin_x = repeated_origin(translated_anchor.x, coverage_rect.x, tile_width);
    let origin_y = repeated_origin(translated_anchor.y, coverage_rect.y, tile_height);
    if !origin_x.is_finite() || !origin_y.is_finite() {
        return Err(RenderInputError::InvalidPicture {
            media: media_id,
            detail: "tile origin is not finite",
        });
    }
    let first_column = repeated_tile_index(origin_x, translated_anchor.x, tile_width, media_id)?;
    let first_row = repeated_tile_index(origin_y, translated_anchor.y, tile_height, media_id)?;
    let columns = repeat_count(
        origin_x,
        coverage_rect.x + coverage_rect.width,
        tile_width,
        media_id,
    )?;
    let rows = repeat_count(
        origin_y,
        coverage_rect.y + coverage_rect.height,
        tile_height,
        media_id,
    )?;
    let requested = columns
        .checked_mul(rows)
        .ok_or(RenderInputError::TileLimitExceeded {
            media: media_id,
            requested: usize::MAX,
            limit: MAX_TILE_ELEMENTS,
        })?;
    let byte_limit = MAX_TILED_IMAGE_BYTES / media.bytes.len().max(1);
    let limit = MAX_TILE_ELEMENTS.min(byte_limit.max(1));
    if requested > limit {
        return Err(RenderInputError::TileLimitExceeded {
            media: media_id,
            requested,
            limit,
        });
    }

    let mut tiles = Vec::with_capacity(requested);
    for row in 0..rows {
        for column in 0..columns {
            let rect = Rect {
                x: origin_x + column as f64 * tile_width,
                y: origin_y + row as f64 * tile_height,
                width: tile_width,
                height: tile_height,
            };
            let image = picture_image(media_id, media, expanded_crop_rect(rect, crop));
            let image = if crop.is_some() {
                clipped_group(Path::rect(rect), vec![image])
            } else {
                image
            };
            tiles.push(flip_tile(
                image,
                rect,
                tile.flip,
                (first_column.rem_euclid(2) == 1) != (column % 2 == 1),
                (first_row.rem_euclid(2) == 1) != (row % 2 == 1),
            ));
        }
    }
    let tiles = if rotate_with_shape || shape.rotation_deg == 0.0 {
        tiles
    } else {
        vec![PositionedElement::Group(GroupElement {
            transform: Transform::rotate_about(
                -shape.rotation_deg,
                shape.bounds.width / 2.0,
                shape.bounds.height / 2.0,
            ),
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children: tiles,
        })]
    };
    Ok(vec![clip_to_picture_shape(shape, paths, tiles)])
}

fn normalized_insets(
    crop: Option<CropRect>,
    media: MediaId,
    detail: &'static str,
) -> Result<Option<CropRect>, RenderInputError> {
    let Some(crop) = crop else {
        return Ok(None);
    };
    let values = [crop.left, crop.top, crop.right, crop.bottom];
    if values.iter().any(|value| !value.is_finite()) {
        return Err(RenderInputError::InvalidPicture { media, detail });
    }
    let crop = CropRect {
        left: crop.left.clamp(0.0, 1.0),
        top: crop.top.clamp(0.0, 1.0),
        right: crop.right.clamp(0.0, 1.0),
        bottom: crop.bottom.clamp(0.0, 1.0),
    };
    if crop.left + crop.right >= 1.0 || crop.top + crop.bottom >= 1.0 {
        return Err(RenderInputError::InvalidPicture { media, detail });
    }
    Ok((crop != CropRect::default()).then_some(crop))
}

fn local_shape_rect(shape: &ResolvedShape) -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        width: shape.bounds.width,
        height: shape.bounds.height,
    }
}

fn picture_coverage_rect(shape: &ResolvedShape, rotate_with_shape: bool) -> Rect {
    let rect = local_shape_rect(shape);
    if rotate_with_shape || shape.rotation_deg == 0.0 {
        return rect;
    }
    Transform::rotate_about(
        shape.rotation_deg,
        shape.bounds.width / 2.0,
        shape.bounds.height / 2.0,
    )
    .transform_rect_bbox(rect)
}

fn inset_rect(rect: Rect, insets: Option<CropRect>) -> Rect {
    let Some(insets) = insets else {
        return rect;
    };
    Rect {
        x: rect.x + rect.width * insets.left,
        y: rect.y + rect.height * insets.top,
        width: rect.width * (1.0 - insets.left - insets.right),
        height: rect.height * (1.0 - insets.top - insets.bottom),
    }
}

fn expanded_crop_rect(destination: Rect, crop: Option<CropRect>) -> Rect {
    let Some(crop) = crop else {
        return destination;
    };
    let retained_width = 1.0 - crop.left - crop.right;
    let retained_height = 1.0 - crop.top - crop.bottom;
    let width = destination.width / retained_width;
    let height = destination.height / retained_height;
    Rect {
        x: destination.x - width * crop.left,
        y: destination.y - height * crop.top,
        width,
        height,
    }
}

fn picture_image(media_id: MediaId, media: &MediaData, rect: Rect) -> PositionedElement {
    PositionedElement::Image {
        rect,
        data: media.bytes.clone(),
        content_type: media.content_type.clone(),
        media_id,
    }
}

fn counter_rotate_image(
    image: PositionedElement,
    shape: &ResolvedShape,
    rotate_with_shape: bool,
) -> PositionedElement {
    if rotate_with_shape || shape.rotation_deg == 0.0 {
        return image;
    }
    PositionedElement::Group(GroupElement {
        transform: Transform::rotate_about(
            -shape.rotation_deg,
            shape.bounds.width / 2.0,
            shape.bounds.height / 2.0,
        ),
        clip: None,
        opacity: 1.0,
        effects: Vec::new(),
        children: vec![image],
    })
}

fn clip_to_picture_shape(
    shape: &ResolvedShape,
    paths: &[Path],
    children: Vec<PositionedElement>,
) -> PositionedElement {
    if matches!(shape.geometry, ResolvedGeometry::Rectangle) {
        return clipped_group(Path::rect(local_shape_rect(shape)), children);
    }
    clipped_group(combined_clip_path(paths), children)
}

fn combined_clip_path(paths: &[Path]) -> Path {
    Path {
        commands: paths
            .iter()
            .flat_map(|path| path.commands.iter().cloned())
            .collect(),
        fill_rule: paths
            .first()
            .map_or(oxoffice_layout::FillRule::NonZero, |path| path.fill_rule),
    }
}

fn clipped_group(clip: Path, children: Vec<PositionedElement>) -> PositionedElement {
    PositionedElement::Group(GroupElement {
        transform: Transform::IDENTITY,
        clip: Some(clip),
        opacity: 1.0,
        effects: Vec::new(),
        children,
    })
}

fn tile_native_size_points(
    mut info: oxoffice_media::ImageInfo,
    declared_dpi: Option<f64>,
    media: MediaId,
) -> Result<(f64, f64), RenderInputError> {
    if let Some(dpi) = declared_dpi {
        if !dpi.is_finite() || dpi <= 0.0 {
            return Err(RenderInputError::InvalidPicture {
                media,
                detail: "declared picture DPI is not finite and positive",
            });
        }
        info.dpi_x = Some(dpi);
        info.dpi_y = Some(dpi);
    }
    let size = info
        .native_size(96.0)
        .ok_or(RenderInputError::InvalidPicture {
            media,
            detail: "picture DPI cannot produce a native size",
        })?;
    Ok((
        size.width_emu as f64 / 12_700.0,
        size.height_emu as f64 / 12_700.0,
    ))
}

fn tile_alignment_origin(
    rect: Rect,
    tile_width: f64,
    tile_height: f64,
    alignment: ResolvedRectAlignment,
) -> Point {
    let center_x = rect.x + (rect.width - tile_width) / 2.0;
    let right = rect.x + rect.width - tile_width;
    let center_y = rect.y + (rect.height - tile_height) / 2.0;
    let bottom = rect.y + rect.height - tile_height;
    match alignment {
        ResolvedRectAlignment::TopLeft => Point {
            x: rect.x,
            y: rect.y,
        },
        ResolvedRectAlignment::Top => Point {
            x: center_x,
            y: rect.y,
        },
        ResolvedRectAlignment::TopRight => Point {
            x: right,
            y: rect.y,
        },
        ResolvedRectAlignment::Left => Point {
            x: rect.x,
            y: center_y,
        },
        ResolvedRectAlignment::Center => Point {
            x: center_x,
            y: center_y,
        },
        ResolvedRectAlignment::Right => Point {
            x: right,
            y: center_y,
        },
        ResolvedRectAlignment::BottomLeft => Point {
            x: rect.x,
            y: bottom,
        },
        ResolvedRectAlignment::Bottom => Point {
            x: center_x,
            y: bottom,
        },
        ResolvedRectAlignment::BottomRight => Point {
            x: right,
            y: bottom,
        },
    }
}

fn repeated_origin(anchor: f64, coverage_start: f64, tile_size: f64) -> f64 {
    coverage_start - (coverage_start - anchor).rem_euclid(tile_size)
}

fn repeated_tile_index(
    origin: f64,
    anchor: f64,
    tile_size: f64,
    media: MediaId,
) -> Result<isize, RenderInputError> {
    let index = ((origin - anchor) / tile_size).round();
    if !index.is_finite() || index < isize::MIN as f64 || index > isize::MAX as f64 {
        return Err(RenderInputError::InvalidPicture {
            media,
            detail: "tile translation cannot preserve flip phase",
        });
    }
    Ok(index as isize)
}

fn repeat_count(
    origin: f64,
    coverage_end: f64,
    tile_size: f64,
    media: MediaId,
) -> Result<usize, RenderInputError> {
    let count = ((coverage_end - origin) / tile_size).ceil().max(0.0);
    if !count.is_finite() || count > usize::MAX as f64 {
        return Err(RenderInputError::TileLimitExceeded {
            media,
            requested: usize::MAX,
            limit: MAX_TILE_ELEMENTS,
        });
    }
    Ok(count as usize)
}

fn flip_tile(
    tile: PositionedElement,
    rect: Rect,
    flip: ResolvedTileFlip,
    odd_column: bool,
    odd_row: bool,
) -> PositionedElement {
    let flip_h =
        matches!(flip, ResolvedTileFlip::Horizontal | ResolvedTileFlip::Both) && odd_column;
    let flip_v = matches!(flip, ResolvedTileFlip::Vertical | ResolvedTileFlip::Both) && odd_row;
    if !flip_h && !flip_v {
        return tile;
    }
    PositionedElement::Group(GroupElement {
        transform: Transform {
            a: if flip_h { -1.0 } else { 1.0 },
            b: 0.0,
            c: 0.0,
            d: if flip_v { -1.0 } else { 1.0 },
            e: if flip_h {
                2.0 * rect.x + rect.width
            } else {
                0.0
            },
            f: if flip_v {
                2.0 * rect.y + rect.height
            } else {
                0.0
            },
        },
        clip: None,
        opacity: 1.0,
        effects: Vec::new(),
        children: vec![tile],
    })
}

#[derive(Clone, Copy)]
struct EndpointTangent {
    point: Point,
    outward: Point,
}

fn endpoint_tangents(paths: &[Path]) -> (Option<EndpointTangent>, Option<EndpointTangent>) {
    let mut head = None;
    let mut tail = None;
    for path in paths {
        let mut current = None;
        let mut subpath_start = None;
        for command in &path.commands {
            match *command {
                PathCommand::MoveTo(point) => {
                    current = Some(point);
                    subpath_start = Some(point);
                }
                PathCommand::LineTo(to) => {
                    if let Some(from) = current
                        && let Some(direction) = unit_direction(from, to)
                    {
                        head.get_or_insert(EndpointTangent {
                            point: from,
                            outward: Point {
                                x: -direction.x,
                                y: -direction.y,
                            },
                        });
                        tail = Some(EndpointTangent {
                            point: to,
                            outward: direction,
                        });
                    }
                    current = Some(to);
                }
                PathCommand::CurveTo { c1, c2, to } => {
                    if let Some(from) = current {
                        let start_direction = [c1, c2, to]
                            .into_iter()
                            .find_map(|candidate| unit_direction(from, candidate));
                        let end_direction = [c2, c1, from]
                            .into_iter()
                            .find_map(|candidate| unit_direction(candidate, to));
                        if let Some(direction) = start_direction {
                            head.get_or_insert(EndpointTangent {
                                point: from,
                                outward: Point {
                                    x: -direction.x,
                                    y: -direction.y,
                                },
                            });
                        }
                        if let Some(direction) = end_direction {
                            tail = Some(EndpointTangent {
                                point: to,
                                outward: direction,
                            });
                        }
                    }
                    current = Some(to);
                }
                PathCommand::Close => {
                    if let (Some(from), Some(to)) = (current, subpath_start)
                        && let Some(direction) = unit_direction(from, to)
                    {
                        head.get_or_insert(EndpointTangent {
                            point: from,
                            outward: Point {
                                x: -direction.x,
                                y: -direction.y,
                            },
                        });
                        tail = Some(EndpointTangent {
                            point: to,
                            outward: direction,
                        });
                    }
                    current = subpath_start;
                }
            }
        }
    }
    (head, tail)
}

fn unit_direction(from: Point, to: Point) -> Option<Point> {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let length = dx.hypot(dy);
    (length.is_finite() && length > 1.0e-10).then_some(Point {
        x: dx / length,
        y: dy / length,
    })
}

fn line_end_path(
    end: &ResolvedLineEnd,
    tangent: EndpointTangent,
    stroke_width: f64,
) -> Option<Path> {
    if !stroke_width.is_finite() || stroke_width <= 0.0 {
        return None;
    }
    let width = line_end_factor(end.width) * stroke_width;
    let length = line_end_factor(end.length) * stroke_width;
    let point = |along: f64, across: f64| Point {
        x: tangent.point.x + tangent.outward.x * along - tangent.outward.y * across,
        y: tangent.point.y + tangent.outward.y * along + tangent.outward.x * across,
    };
    let path = match end.kind {
        ResolvedLineEndKind::Triangle => closed_polygon(vec![
            point(0.0, 0.0),
            point(-length, width / 2.0),
            point(-length, -width / 2.0),
        ]),
        ResolvedLineEndKind::Stealth => closed_polygon(vec![
            point(0.0, 0.0),
            point(-length, width / 2.0),
            point(-length / 2.0, 0.0),
            point(-length, -width / 2.0),
        ]),
        ResolvedLineEndKind::Diamond => closed_polygon(vec![
            point(0.0, 0.0),
            point(-length / 2.0, width / 2.0),
            point(-length, 0.0),
            point(-length / 2.0, -width / 2.0),
        ]),
        ResolvedLineEndKind::Oval => oval_line_end(&point, width, length),
        ResolvedLineEndKind::Arrow => {
            let arm = stroke_width.min(width / 2.0);
            closed_polygon(vec![
                point(0.0, 0.0),
                point(-length, width / 2.0),
                point(-length, width / 2.0 - arm),
                point(-arm, 0.0),
                point(-length, -width / 2.0 + arm),
                point(-length, -width / 2.0),
            ])
        }
    };
    path_is_finite(&path).then_some(path)
}

fn line_end_factor(size: ResolvedLineEndSize) -> f64 {
    match size {
        ResolvedLineEndSize::Small => 2.0,
        ResolvedLineEndSize::Medium => 3.0,
        ResolvedLineEndSize::Large => 5.0,
    }
}

fn closed_polygon(points: Vec<Point>) -> Path {
    let mut points = points.into_iter();
    let mut commands = points
        .next()
        .map(PathCommand::MoveTo)
        .into_iter()
        .collect::<Vec<_>>();
    commands.extend(points.map(PathCommand::LineTo));
    commands.push(PathCommand::Close);
    Path {
        commands,
        fill_rule: oxoffice_layout::FillRule::NonZero,
    }
}

fn oval_line_end(point: &impl Fn(f64, f64) -> Point, width: f64, length: f64) -> Path {
    const KAPPA: f64 = 0.552_284_749_830_793_6;
    let rx = length / 2.0;
    let ry = width / 2.0;
    let center = -rx;
    Path {
        commands: vec![
            PathCommand::MoveTo(point(0.0, 0.0)),
            PathCommand::CurveTo {
                c1: point(0.0, KAPPA * ry),
                c2: point(center + KAPPA * rx, ry),
                to: point(center, ry),
            },
            PathCommand::CurveTo {
                c1: point(center - KAPPA * rx, ry),
                c2: point(-length, KAPPA * ry),
                to: point(-length, 0.0),
            },
            PathCommand::CurveTo {
                c1: point(-length, -KAPPA * ry),
                c2: point(center - KAPPA * rx, -ry),
                to: point(center, -ry),
            },
            PathCommand::CurveTo {
                c1: point(center + KAPPA * rx, -ry),
                c2: point(0.0, -KAPPA * ry),
                to: point(0.0, 0.0),
            },
            PathCommand::Close,
        ],
        fill_rule: oxoffice_layout::FillRule::NonZero,
    }
}

fn path_is_finite(path: &Path) -> bool {
    path.commands.iter().all(|command| match command {
        PathCommand::MoveTo(point) | PathCommand::LineTo(point) => point_is_finite(*point),
        PathCommand::CurveTo { c1, c2, to } => {
            point_is_finite(*c1) && point_is_finite(*c2) && point_is_finite(*to)
        }
        PathCommand::Close => true,
    })
}

fn point_is_finite(point: Point) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

fn filled_line_end(path: Path, paint: &Paint) -> PositionedElement {
    PositionedElement::Path(PathElement {
        path,
        fill: Some(paint.clone()),
        stroke: None,
    })
}

/// The complete painter transform used by explicit authoring to preserve a
/// resized shape's physical anchor, including parent-group contributions.
pub fn shape_transform(shape: &ResolvedShape) -> Transform {
    let center_x = shape.bounds.width / 2.0;
    let center_y = shape.bounds.height / 2.0;
    let rotation = Transform::rotate_about(shape.rotation_deg, center_x, center_y);
    let flip = Transform {
        a: if shape.flip_h { -1.0 } else { 1.0 },
        b: 0.0,
        c: 0.0,
        d: if shape.flip_v { -1.0 } else { 1.0 },
        e: if shape.flip_h {
            shape.bounds.width
        } else {
            0.0
        },
        f: if shape.flip_v {
            shape.bounds.height
        } else {
            0.0
        },
    };
    let translation = Transform {
        e: shape.bounds.x,
        f: shape.bounds.y,
        ..Transform::IDENTITY
    };
    flip.then(rotation)
        .then(translation)
        .then(shape.group_transform)
}

/// Resolve one scoped media relationship into the deck's content-addressed store.
pub fn resolve_media_relationship(
    relationships: &RelScopes,
    scope: RelScope,
    relationship_id: &str,
    package_media: &HashMap<String, MediaData>,
    deck_media: &mut HashMap<MediaId, MediaData>,
) -> Result<MediaId, RenderInputError> {
    let relationship = relationships.get(scope, relationship_id)?;
    let media = package_media.get(&relationship.target).ok_or_else(|| {
        RenderInputError::MissingMediaTarget {
            scope,
            relationship_id: relationship_id.to_owned(),
            target: relationship.target.clone(),
        }
    })?;
    let media_id = MediaId::from_bytes(&media.bytes);
    deck_media.entry(media_id).or_insert_with(|| media.clone());
    Ok(media_id)
}
