use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};

use oxoffice_chart::{render_chart, render_chart_placeholder};
use oxoffice_drawing::color::{ColorChoice, ColorMap, ColorMapSlot, RgbColor, resolve_color};
use oxoffice_drawing::effect::CT_OuterShadowEffect;
use oxoffice_drawing::fill::{
    BlipFill, BlipMode, Fill, GradientGeometry, PathGradientKind, RelativeRect, SolidFill,
};
use oxoffice_drawing::geometry::EvaluatedPathCommand;
use oxoffice_drawing::line::{
    CT_LineProperties, LineCap as DrawingLineCap, LineDash, LineEnd, LineEndSize, LineEndType,
    LineJoin as DrawingLineJoin,
};
use oxoffice_drawing::style_ref::{FontCollectionIndex, StyleReference};
use oxoffice_drawing::table::{
    CT_TableBackgroundStyle, CT_TableBorders, CT_TableCellStyle, CT_TablePartStyle, CT_TableStyle,
    CT_TableStyleList, CT_TableTextStyle,
};
use oxoffice_drawing::text::{
    CT_TextBody, CT_TextBodyProperties, CT_TextCharacterProperties, CT_TextListStyle,
    Coordinate32Value, TextAlignment, TextAnchor as DrawingTextAnchor, TextAutofit,
    TextBulletChoice, TextBulletSizeValue, TextFont, TextPointValue, TextRun, TextSpacing,
    TextVertical as DrawingTextVertical, TextWrap,
};
use oxoffice_drawing::theme::CT_OfficeStyleSheet;
use oxoffice_drawing::xfrm::CT_Transform2D;
use oxoffice_layout::{
    Color, Diagnostic, Effect, FillRule, FontManager, GradientStop, LineCap, LineJoin, Paint, Path,
    PathCommand, Point, Rect, Stroke, Transform,
};
use oxoffice_pptx_oxml::graphic_frame::GraphicDataPayload;
pub use oxoffice_pptx_oxml::inheritance::inherited_xfrm;
use oxoffice_pptx_oxml::inheritance::{is_latent, placeholder_chain, placeholder_keys_match};
use oxoffice_pptx_oxml::picture::CT_Picture;
use oxoffice_pptx_oxml::placeholder::{CT_Placeholder, PhType, PlaceholderKey};
use oxoffice_pptx_oxml::shape_tree::{CT_Shape, ShapeTreeChild};
use oxoffice_pptx_oxml::slide_parts::{
    BackgroundRendering, CT_Slide, CT_SlideLayout, CT_SlideMaster,
};

use crate::ResolveError;
use crate::style::{referenced_fill, substitute_fill};
use crate::text::EffectiveListStyle;
use crate::timeline::{
    EvaluatedFrameState, EvaluatedShapeState, ResolvedShapeIdentity, ResolvedTimelineSlide,
    TimelinePosition, evaluate_timeline,
};
use crate::{
    ChartResource, ParagraphAlignment, ResolvedAutofit, ResolvedBackground, ResolvedBullet,
    ResolvedBulletSize, ResolvedContent, ResolvedGeometry, ResolvedImage, ResolvedImagePlacement,
    ResolvedLineEnd, ResolvedLineEndKind, ResolvedLineEndSize, ResolvedParagraph,
    ResolvedRectAlignment, ResolvedRunStyle, ResolvedShape, ResolvedSlide,
    ResolvedSlideTextDirections, ResolvedTable, ResolvedTableBorder, ResolvedTableCell,
    ResolvedTableRow, ResolvedTextBody, ResolvedTextRun, ResolvedTextSpacing, ResolvedTileFlip,
    ResolvedTilePlacement, ScopedChartResources, ScopedHyperlinkTargets, ScopedMediaFailures,
    ScopedMediaIds, TextAnchor, TextDirection, TextInsets,
};

/// The producer part that supplied the effective background.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackgroundSource {
    Slide,
    Layout,
    Master,
}

/// The borrowed background payload selected for one slide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackgroundContent<'a> {
    Model(&'a BackgroundRendering),
}

/// The effective background and the per-master colour map used to resolve it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectiveBackground<'a> {
    pub source: BackgroundSource,
    pub content: BackgroundContent<'a>,
    pub color_map: &'a ColorMap,
}

/// The four ordered sources in a flattened slide view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlattenedSource {
    Background,
    Master,
    Layout,
    Slide,
}

/// One borrowed entry in final draw order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlattenedItem<'a> {
    Background(EffectiveBackground<'a>),
    Shape {
        source: FlattenedSource,
        child: &'a ShapeTreeChild,
        /// Coordinate scale accumulated from every non-shearing parent group.
        group_scale: (f64, f64),
        /// Rigid mapping accumulated from every non-shearing parent group.
        group_transform: Transform,
        /// Stable diagnostic flags accumulated from parent groups.
        group_issues: u8,
    },
}

const GROUP_ZERO_X: u8 = 1;
const GROUP_ZERO_Y: u8 = 2;
const GROUP_SHEAR: u8 = 4;

#[derive(Clone, Copy)]
struct ShapePlacement {
    source: FlattenedSource,
    group_scale: (f64, f64),
    group_transform: Transform,
}

impl FlattenedItem<'_> {
    pub const fn source(&self) -> FlattenedSource {
        match self {
            Self::Background(_) => FlattenedSource::Background,
            Self::Shape { source, .. } => *source,
        }
    }
}

/// The fixed presentation hierarchy and theme inputs for resolving one slide.
pub struct ResolveCtx<'a> {
    pub theme: &'a CT_OfficeStyleSheet,
    pub color_map: ColorMap,
    pub master: &'a CT_SlideMaster,
    pub layout: &'a CT_SlideLayout,
    pub slide: &'a CT_Slide,
    pub default_text_style: &'a CT_TextListStyle,
    pub table_styles: Option<&'a CT_TableStyleList>,
    pub(crate) list_style_cache: RefCell<HashMap<Option<PlaceholderKey>, EffectiveListStyle>>,
    media_poster_diagnostic_identity: bool,
    media_failures: Option<&'a ScopedMediaFailures>,
}

impl<'a> ResolveCtx<'a> {
    pub fn new(
        theme: &'a CT_OfficeStyleSheet,
        color_map: ColorMap,
        master: &'a CT_SlideMaster,
        layout: &'a CT_SlideLayout,
        slide: &'a CT_Slide,
        default_text_style: &'a CT_TextListStyle,
    ) -> Self {
        Self {
            theme,
            color_map,
            master,
            layout,
            slide,
            default_text_style,
            table_styles: None,
            list_style_cache: RefCell::new(HashMap::new()),
            media_poster_diagnostic_identity: false,
            media_failures: None,
        }
    }

    /// Adds the optional `ppt/tableStyles.xml` projection used by table frames.
    pub fn with_table_styles(mut self, table_styles: &'a CT_TableStyleList) -> Self {
        self.table_styles = Some(table_styles);
        self
    }

    /// Enables private source identity used while applying media fallback policy.
    #[doc(hidden)]
    pub fn with_media_poster_diagnostic_identity(mut self) -> Self {
        self.media_poster_diagnostic_identity = true;
        self
    }

    /// Adds renderer-owned failures for media rejected before layout.
    #[doc(hidden)]
    pub fn with_media_failures(mut self, failures: &'a ScopedMediaFailures) -> Self {
        self.media_failures = Some(failures);
        self
    }

    pub(crate) fn placeholder_chain<'ctx>(
        &'ctx self,
        shape: &CT_Shape,
    ) -> (Option<&'ctx CT_Shape>, Option<&'ctx CT_Shape>) {
        self.placeholder_chain_for(shape.placeholder.as_ref())
    }

    fn placeholder_chain_for<'ctx>(
        &'ctx self,
        placeholder: Option<&CT_Placeholder>,
    ) -> (Option<&'ctx CT_Shape>, Option<&'ctx CT_Shape>) {
        placeholder.map_or((None, None), |placeholder| {
            placeholder_chain(placeholder, self.layout, self.master)
        })
    }

    fn is_slide_number_placeholder(&self, shape: &CT_Shape) -> bool {
        if shape
            .placeholder
            .as_ref()
            .is_some_and(|placeholder| placeholder.ph_type == Some(PhType::SlideNumber))
        {
            return true;
        }
        let (layout, master) = self.placeholder_chain(shape);
        [layout, master].into_iter().flatten().any(|shape| {
            shape
                .placeholder
                .as_ref()
                .is_some_and(|placeholder| placeholder.ph_type == Some(PhType::SlideNumber))
        })
    }

    /// Resolves an owned transform from the slide, layout, then master shape.
    pub fn effective_xfrm(&self, shape: &CT_Shape) -> Option<CT_Transform2D> {
        shape.shape_properties.transform.clone().or_else(|| {
            inherited_xfrm(shape.placeholder.as_ref()?, self.layout, self.master).cloned()
        })
    }

    /// Resolves picture bounds from the slide picture, layout placeholder, then master placeholder.
    pub fn effective_picture_xfrm(
        &self,
        picture: &oxoffice_pptx_oxml::picture::CT_Picture,
    ) -> Option<CT_Transform2D> {
        picture.shape_properties.transform.clone().or_else(|| {
            inherited_xfrm(picture.placeholder.as_ref()?, self.layout, self.master).cloned()
        })
    }

    /// Resolves body properties per field over defaults, master, layout, and slide.
    pub fn effective_body_pr(&self, shape: &CT_Shape) -> CT_TextBodyProperties {
        let (layout, master) = self.placeholder_chain(shape);
        let mut effective = default_body_properties();
        for source in [master, layout, Some(shape)].into_iter().flatten() {
            if let Some(text_body) = &source.text_body {
                merge_body_properties(&mut effective, &text_body.body_properties);
            }
        }
        effective
    }

    /// Selects the first background in slide, layout, and master order.
    pub fn effective_background(&self) -> Option<EffectiveBackground<'_>> {
        for (source, background) in [
            (
                BackgroundSource::Slide,
                self.slide.common_slide_data.background.as_ref(),
            ),
            (
                BackgroundSource::Layout,
                self.layout.common_slide_data.background.as_ref(),
            ),
            (
                BackgroundSource::Master,
                self.master.common_slide_data.background.as_ref(),
            ),
        ] {
            if let Some(background) = background {
                return Some(EffectiveBackground {
                    source,
                    content: BackgroundContent::Model(background.rendering()),
                    color_map: &self.color_map,
                });
            }
        }
        None
    }

    /// Returns background and shape-tree leaves in final draw order.
    pub fn flatten(&self) -> Vec<FlattenedItem<'_>> {
        let slide_children = &self.slide.common_slide_data.shape_tree.children;
        let layout_children = &self.layout.common_slide_data.shape_tree.children;
        let master_children = &self.master.common_slide_data.shape_tree.children;
        let mut flattened = Vec::new();
        if let Some(background) = self.effective_background() {
            flattened.push(FlattenedItem::Background(background));
        }
        emit_tree(
            master_children,
            PassRules {
                source: FlattenedSource::Master,
                emit_non_placeholders: self.slide.show_master_shapes.unwrap_or(true)
                    && self.layout.show_master_shapes.unwrap_or(true),
            },
            &mut flattened,
        );
        emit_tree(
            layout_children,
            PassRules {
                source: FlattenedSource::Layout,
                emit_non_placeholders: self.slide.show_master_shapes.unwrap_or(true),
            },
            &mut flattened,
        );
        emit_tree(
            slide_children,
            PassRules {
                source: FlattenedSource::Slide,
                emit_non_placeholders: true,
            },
            &mut flattened,
        );
        flattened
    }

    /// Enumerates physical leaves without visibility or placeholder filtering.
    /// Child paths retain source ordinals, including nested groups. Opaque
    /// alternate content stays an explicit leaf rather than a writable fallback.
    pub fn owned_items(&self, source: FlattenedSource) -> Vec<(Vec<usize>, FlattenedItem<'_>)> {
        let children = match source {
            FlattenedSource::Slide => &self.slide.common_slide_data.shape_tree.children,
            FlattenedSource::Layout => &self.layout.common_slide_data.shape_tree.children,
            FlattenedSource::Master => &self.master.common_slide_data.shape_tree.children,
            FlattenedSource::Background => return Vec::new(),
        };
        let mut output = Vec::new();
        collect_owned_items(children, source, Transform::IDENTITY, 0, &[], &mut output);
        output
    }

    /// Resolves one owned renderer-facing slide at the supplied point size.
    pub fn resolve_slide(&self, size: (f64, f64)) -> Result<ResolvedSlide, ResolveError> {
        self.resolve_slide_inner(size, None, None, None, None)
            .map(|(slide, _)| slide)
    }

    /// Resolves one slide through the additive slide-local timeline path.
    pub fn resolve_slide_at(
        &self,
        size: (f64, f64),
        position: TimelinePosition,
    ) -> Result<ResolvedTimelineSlide, ResolveError> {
        let (slide, _, identities) =
            self.resolve_slide_inner_with_identities(size, None, None, None, None)?;
        let state = evaluate_timeline(
            self.slide.timing.as_ref(),
            self.slide.transition.as_ref(),
            position,
        )?;
        Ok(apply_timeline_state(
            slide,
            identities,
            state,
            timeline_group_geometries(&self.slide.common_slide_data.shape_tree.children),
        ))
    }

    /// Resolves one slide using embedded media identifiers scoped to source parts.
    pub fn resolve_slide_with_media(
        &self,
        size: (f64, f64),
        media: &ScopedMediaIds,
    ) -> Result<ResolvedSlide, ResolveError> {
        self.resolve_slide_inner(size, Some(media), None, None, None)
            .map(|(slide, _)| slide)
    }

    /// Resolves one slide with source-scoped media and external hyperlink targets.
    pub fn resolve_slide_with_resources(
        &self,
        size: (f64, f64),
        media: &ScopedMediaIds,
        hyperlinks: &ScopedHyperlinkTargets,
    ) -> Result<ResolvedSlide, ResolveError> {
        self.resolve_slide_inner(size, Some(media), Some(hyperlinks), None, None)
            .map(|(slide, _)| slide)
    }

    /// Resolves one slide with source-scoped charts and the caller's font manager.
    pub fn resolve_slide_with_chart_resources(
        &self,
        size: (f64, f64),
        media: &ScopedMediaIds,
        hyperlinks: &ScopedHyperlinkTargets,
        charts: &ScopedChartResources,
        fonts: &mut FontManager,
    ) -> Result<ResolvedSlide, ResolveError> {
        self.resolve_slide_inner(
            size,
            Some(media),
            Some(hyperlinks),
            Some(charts),
            Some(fonts),
        )
        .map(|(slide, _)| slide)
    }

    /// Resolves one slide and returns paragraph directions in shape and text-body order.
    ///
    /// The outer vector follows `ResolvedSlide::shapes`. Each shape entry contains
    /// one paragraph-direction vector for a text shape, or one entry per table cell
    /// in row-major order. Non-text shapes have an empty entry.
    pub fn resolve_slide_with_chart_resources_and_text_directions(
        &self,
        size: (f64, f64),
        media: &ScopedMediaIds,
        hyperlinks: &ScopedHyperlinkTargets,
        charts: &ScopedChartResources,
        fonts: &mut FontManager,
    ) -> Result<(ResolvedSlide, ResolvedSlideTextDirections), ResolveError> {
        self.resolve_slide_inner(
            size,
            Some(media),
            Some(hyperlinks),
            Some(charts),
            Some(fonts),
        )
    }

    /// Resolve a physically owned text shape even when draw flattening hides
    /// its placeholder. Explicit autofit inventory and its read-only report
    /// need the same producing scope and inheritance as visible rendering.
    pub fn resolve_owned_text_shape(
        &self,
        item: FlattenedItem<'_>,
        size: (f64, f64),
        hyperlinks: &ScopedHyperlinkTargets,
    ) -> Result<(ResolvedSlide, ResolvedSlideTextDirections), ResolveError> {
        let FlattenedItem::Shape {
            source,
            child,
            group_scale,
            group_transform,
            group_issues,
        } = item
        else {
            return Err(ResolveError::ConcreteValue {
                kind: "autofit owner",
                detail: "not a physically owned shape".to_owned(),
            });
        };
        let mut resolved = ResolvedSlide {
            unresolved_member_count: 0,
            size,
            background: None,
            shapes: Vec::new(),
            diagnostics: Vec::new(),
        };
        push_group_diagnostics(group_issues, &mut resolved.diagnostics);
        let mut directions = Vec::new();
        let shape = self.resolve_flattened_shape(
            ShapePlacement {
                source,
                group_scale,
                group_transform,
            },
            child,
            None,
            (Some(hyperlinks), None),
            None,
            (&mut resolved.diagnostics, &mut directions),
        )?;
        if let Some(shape) = shape {
            resolved.shapes.push(shape);
        }
        Ok((resolved, vec![directions]))
    }

    /// Resolves one timestamped slide while retaining text directions and target identity.
    pub fn resolve_slide_with_chart_resources_and_text_directions_at(
        &self,
        size: (f64, f64),
        media: &ScopedMediaIds,
        hyperlinks: &ScopedHyperlinkTargets,
        charts: &ScopedChartResources,
        fonts: &mut FontManager,
        position: TimelinePosition,
    ) -> Result<(ResolvedTimelineSlide, ResolvedSlideTextDirections), ResolveError> {
        let (slide, directions, identities) = self.resolve_slide_inner_with_identities(
            size,
            Some(media),
            Some(hyperlinks),
            Some(charts),
            Some(fonts),
        )?;
        let state = evaluate_timeline(
            self.slide.timing.as_ref(),
            self.slide.transition.as_ref(),
            position,
        )?;
        Ok((
            apply_timeline_state(
                slide,
                identities,
                state,
                timeline_group_geometries(&self.slide.common_slide_data.shape_tree.children),
            ),
            directions,
        ))
    }

    fn resolve_slide_inner(
        &self,
        size: (f64, f64),
        media: Option<&ScopedMediaIds>,
        hyperlinks: Option<&ScopedHyperlinkTargets>,
        charts: Option<&ScopedChartResources>,
        fonts: Option<&mut FontManager>,
    ) -> Result<(ResolvedSlide, ResolvedSlideTextDirections), ResolveError> {
        self.resolve_slide_inner_common(size, media, hyperlinks, charts, fonts, None)
    }

    fn resolve_slide_inner_with_identities(
        &self,
        size: (f64, f64),
        media: Option<&ScopedMediaIds>,
        hyperlinks: Option<&ScopedHyperlinkTargets>,
        charts: Option<&ScopedChartResources>,
        fonts: Option<&mut FontManager>,
    ) -> Result<
        (
            ResolvedSlide,
            ResolvedSlideTextDirections,
            Vec<ResolvedShapeIdentity>,
        ),
        ResolveError,
    > {
        let mut identities = Vec::new();
        let (slide, directions) = self.resolve_slide_inner_common(
            size,
            media,
            hyperlinks,
            charts,
            fonts,
            Some(&mut identities),
        )?;
        Ok((slide, directions, identities))
    }

    fn resolve_slide_inner_common(
        &self,
        size: (f64, f64),
        media: Option<&ScopedMediaIds>,
        hyperlinks: Option<&ScopedHyperlinkTargets>,
        charts: Option<&ScopedChartResources>,
        mut fonts: Option<&mut FontManager>,
        mut identities: Option<&mut Vec<ResolvedShapeIdentity>>,
    ) -> Result<(ResolvedSlide, ResolvedSlideTextDirections), ResolveError> {
        for (kind, tree) in [
            ("slide shape tree", &self.slide.common_slide_data.shape_tree),
            (
                "layout shape tree",
                &self.layout.common_slide_data.shape_tree,
            ),
            (
                "master shape tree",
                &self.master.common_slide_data.shape_tree,
            ),
        ] {
            oxoffice_pptx_oxml::shape_tree::validate_shape_nesting(&tree.children).map_err(
                |error| ResolveError::ConcreteValue {
                    kind,
                    detail: error.to_string(),
                },
            )?;
        }
        let mut slide = ResolvedSlide {
            unresolved_member_count: 0,
            size,
            background: None,
            shapes: Vec::new(),
            diagnostics: Vec::new(),
        };
        let mut text_directions = Vec::new();
        for item in self.flatten() {
            match item {
                FlattenedItem::Background(background) => {
                    let source = match background.source {
                        BackgroundSource::Slide => FlattenedSource::Slide,
                        BackgroundSource::Layout => FlattenedSource::Layout,
                        BackgroundSource::Master => FlattenedSource::Master,
                    };
                    let (resolved_background, unsupported) = match background.content {
                        BackgroundContent::Model(BackgroundRendering::Properties(fill)) => {
                            match fill {
                                Some(fill) => self.concrete_background_fill(
                                    fill,
                                    size,
                                    source,
                                    media,
                                    &mut slide.diagnostics,
                                )?,
                                None => (None, None),
                            }
                        }
                        BackgroundContent::Model(BackgroundRendering::Reference {
                            index,
                            color,
                        }) => self.concrete_background_reference(
                            *index,
                            color.as_ref(),
                            size,
                            &mut slide.diagnostics,
                        )?,
                        BackgroundContent::Model(BackgroundRendering::Unsupported(detail)) => {
                            (None, Some(*detail))
                        }
                    };
                    slide.background = resolved_background;
                    if let Some(unsupported) = unsupported {
                        slide.diagnostics.push(Diagnostic {
                            message: format!("unsupported background {unsupported}"),
                        });
                    }
                }
                FlattenedItem::Shape {
                    source,
                    child,
                    group_scale,
                    group_transform,
                    group_issues,
                } => {
                    push_group_diagnostics(group_issues, &mut slide.diagnostics);
                    let mut shape_text_directions = Vec::new();
                    let mut member_diagnostics = Vec::new();
                    let result = self
                        .resolve_flattened_shape(
                            ShapePlacement {
                                source,
                                group_scale,
                                group_transform,
                            },
                            child,
                            media,
                            (hyperlinks, charts),
                            fonts.as_deref_mut(),
                            (&mut member_diagnostics, &mut shape_text_directions),
                        )
                        .and_then(|resolved| {
                            if resolved.is_none() {
                                self.validate_unplaced_member(child, source, hyperlinks)?;
                            }
                            Ok(resolved)
                        });
                    let resolved = match result {
                        Ok(resolved) => {
                            slide.diagnostics.extend(member_diagnostics);
                            resolved
                        }
                        Err(
                            error @ (ResolveError::StyleIndexOutOfRange { .. }
                            | ResolveError::UnresolvedPlaceholderColor { .. }
                            | ResolveError::ConcreteValue { .. }),
                        ) => {
                            // Count the failed owner before deciding whether it can draw.
                            slide.unresolved_member_count += 1;
                            shape_text_directions.clear();
                            self.unresolved_shape_fallback(
                                ShapePlacement {
                                    source,
                                    group_scale,
                                    group_transform,
                                },
                                child,
                                &error,
                                &mut slide.diagnostics,
                            )
                        }
                    };
                    if let Some(shape) = resolved {
                        if let Some(identities) = identities.as_deref_mut() {
                            identities.push(self.shape_identity(source, child));
                        }
                        slide.shapes.push(shape);
                        text_directions.push(shape_text_directions);
                    }
                }
            }
        }
        Ok((slide, text_directions))
    }

    // Missing placement must not hide an error in the owned semantic inputs.
    // Validate only dimension-independent values already consumed by this resolver.
    // No replacement geometry or guessed dimensions are manufactured.
    fn validate_unplaced_member(
        &self,
        child: &ShapeTreeChild,
        source: FlattenedSource,
        hyperlinks: Option<&ScopedHyperlinkTargets>,
    ) -> Result<(), ResolveError> {
        let (fill, line, effects) = match child {
            ShapeTreeChild::Shape(shape) => {
                let (mut effective, _) = self.shape_style_for_placement(shape, true)?;
                effective.effects =
                    self.effective_effects(shape.style(), &shape.shape_properties)?;
                if let Some(body) = shape.text_body.as_ref() {
                    self.resolve_text_body(shape, body, source, hyperlinks, &mut Vec::new())?;
                }
                (effective.fill, effective.line, effective.effects)
            }
            ShapeTreeChild::Picture(picture) => (
                None,
                picture.shape_properties.line.clone(),
                picture.shape_properties.effects.clone(),
            ),
            ShapeTreeChild::Connector(connector) => (
                connector.shape_properties.fill.clone(),
                self.effective_connector_line(connector, true)?,
                self.effective_effects(connector.style(), &connector.shape_properties)?,
            ),
            ShapeTreeChild::GraphicFrame(frame) => {
                if let GraphicDataPayload::Table(table) = frame.graphic_data.payload() {
                    self.resolve_table(
                        table,
                        source,
                        hyperlinks,
                        &mut Vec::new(),
                        &mut Vec::new(),
                    )?;
                }
                return Ok(());
            }
            ShapeTreeChild::AlternateContent(_) | ShapeTreeChild::GroupShape(_) => return Ok(()),
        };
        for fill in [
            fill.as_ref(),
            line.as_ref().and_then(|line| line.fill.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            match fill {
                Fill::Solid(solid) => {
                    if let Some(color) = solid.color.as_ref() {
                        self.concrete_color(color)?;
                    }
                }
                Fill::Gradient(gradient)
                    if gradient.rotate_with_shape != Some(false)
                        && matches!(gradient.flip.as_deref(), None | Some("none"))
                        && gradient.tile_rect.is_none()
                        && !matches!(
                            gradient.geometry.as_ref(),
                            Some(GradientGeometry::Path(_))
                        ) =>
                {
                    // These stop colors precede dimension-dependent paint construction.
                    // Keep concrete_fill's early unsupported boundary for a missing color.
                    for stop in &gradient.stops {
                        let Some(color) = stop.color.as_ref() else {
                            break;
                        };
                        self.concrete_color(color)?;
                    }
                }
                _ => {}
            }
        }
        if let Some(shadow) = effects
            .as_ref()
            .and_then(|effects| effects.outer_shadow.as_ref())
        {
            self.concrete_shadow(shadow)?;
        }
        Ok(())
    }

    fn unresolved_shape_fallback(
        &self,
        placement: ShapePlacement,
        child: &ShapeTreeChild,
        error: &ResolveError,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<ResolvedShape> {
        let id = child
            .non_visual_id()
            .map_or_else(|| "without an id".to_owned(), |id| id.to_string());
        let name = child.non_visual_name().unwrap_or_default();
        let source = flattened_source_name(placement.source);
        let values = match child {
            ShapeTreeChild::Shape(shape) => transform_values(self.effective_xfrm(shape).as_ref()),
            ShapeTreeChild::Picture(picture) => {
                transform_values(self.effective_picture_xfrm(picture).as_ref())
            }
            ShapeTreeChild::GraphicFrame(frame) => transform_values(Some(&frame.transform)),
            ShapeTreeChild::Connector(connector) => {
                connector_transform_values(connector.shape_properties.transform.as_ref())
            }
            ShapeTreeChild::AlternateContent(alternate) => alternate
                .chart_choice()
                .and_then(|frame| transform_values(Some(&frame.transform))),
            ShapeTreeChild::GroupShape(_) => None,
        };
        let values = values.filter(|(bounds, rotation, flip_h, flip_v)| {
            let bounds = scaled_group_bounds(*bounds, placement.group_scale);
            let matrix = resolved_shape_page_transform(&ResolvedShape {
                group_transform: placement.group_transform,
                bounds,
                rotation_deg: *rotation,
                flip_h: *flip_h,
                flip_v: *flip_v,
                geometry: ResolvedGeometry::BoundsFallback,
                fill: None,
                image_fill: None,
                line: None,
                head_end: None,
                tail_end: None,
                shadow: None,
                content: ResolvedContent::None,
                unsupported: Some("unresolved shape"),
            });
            [
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                *rotation,
                matrix.a,
                matrix.b,
                matrix.c,
                matrix.d,
                matrix.e,
                matrix.f,
            ]
            .into_iter()
            .all(f64::is_finite)
                && bounds.width >= 0.0
                && bounds.height >= 0.0
                && (bounds.width > 0.0 || bounds.height > 0.0)
                && [
                    Point { x: 0.0, y: 0.0 },
                    Point {
                        x: bounds.width,
                        y: 0.0,
                    },
                    Point {
                        x: 0.0,
                        y: bounds.height,
                    },
                    Point {
                        x: bounds.width,
                        y: bounds.height,
                    },
                ]
                .into_iter()
                .all(|point| {
                    let point = matrix.apply(point);
                    point.x.is_finite() && point.y.is_finite()
                })
        });
        let retained = if values.is_some() {
            "retained as bounds"
        } else {
            "omitted without trustworthy finite bounds"
        };
        diagnostics.push(Diagnostic {
            message: format!(
                "unresolved member {source} shape {id} \"{name}\" {retained}: {error}"
            ),
        });
        let (bounds, rotation_deg, flip_h, flip_v) = values?;
        Some(ResolvedShape {
            group_transform: placement.group_transform,
            bounds: scaled_group_bounds(bounds, placement.group_scale),
            rotation_deg,
            flip_h,
            flip_v,
            geometry: ResolvedGeometry::BoundsFallback,
            fill: None,
            image_fill: None,
            line: None,
            head_end: None,
            tail_end: None,
            shadow: None,
            content: ResolvedContent::None,
            unsupported: Some("unresolved shape"),
        })
    }

    fn shape_identity(
        &self,
        source: FlattenedSource,
        child: &ShapeTreeChild,
    ) -> ResolvedShapeIdentity {
        let children: &[ShapeTreeChild] = match source {
            FlattenedSource::Slide => &self.slide.common_slide_data.shape_tree.children,
            FlattenedSource::Layout => &self.layout.common_slide_data.shape_tree.children,
            FlattenedSource::Master => &self.master.common_slide_data.shape_tree.children,
            FlattenedSource::Background => &[],
        };
        let mut containing_group_ids = Vec::new();
        find_group_lineage(children, child, &[], &mut containing_group_ids);
        ResolvedShapeIdentity {
            source,
            shape_id: child.non_visual_id(),
            containing_group_ids,
            name: child.non_visual_name(),
        }
    }

    fn resolve_flattened_shape(
        &self,
        placement: ShapePlacement,
        child: &ShapeTreeChild,
        media: Option<&ScopedMediaIds>,
        scoped_resources: (
            Option<&ScopedHyperlinkTargets>,
            Option<&ScopedChartResources>,
        ),
        fonts: Option<&mut FontManager>,
        output: (
            &mut Vec<Diagnostic>,
            &mut Vec<Vec<oxoffice_layout::TextDirection>>,
        ),
    ) -> Result<Option<ResolvedShape>, ResolveError> {
        let (diagnostics, text_directions) = output;
        let (hyperlinks, charts) = scoped_resources;
        let ShapePlacement {
            source,
            group_scale,
            group_transform,
        } = placement;
        match child {
            ShapeTreeChild::Shape(shape) => self.resolve_ordinary_shape(
                shape,
                placement,
                media,
                hyperlinks,
                diagnostics,
                text_directions,
            ),
            ShapeTreeChild::Picture(picture) => {
                if picture.hidden() {
                    return Ok(None);
                }
                let Some((bounds, rotation_deg, flip_h, flip_v)) =
                    transform_values(self.effective_picture_xfrm(picture).as_ref())
                else {
                    return Ok(None);
                };
                let bounds = scaled_group_bounds(bounds, group_scale);
                let line_properties = picture.shape_properties.line.as_ref();
                let line = line_properties
                    .map(|line| self.concrete_line(line, (bounds.width, bounds.height)))
                    .transpose()?
                    .flatten();
                let (head_end, tail_end) = line_properties
                    .map(resolved_line_ends)
                    .unwrap_or((None, None));
                let shadow = picture
                    .shape_properties
                    .effects
                    .as_ref()
                    .and_then(|effects| effects.outer_shadow.as_ref())
                    .map(|shadow| self.concrete_shadow(shadow))
                    .transpose()?;
                let (mut geometry, geometry_unsupported) =
                    self.concrete_picture_geometry(picture, (bounds.width, bounds.height));
                if let Some(category) = geometry_unsupported {
                    diagnostics.push(Diagnostic {
                        message: format!("unsupported {category} retained as picture bounds"),
                    });
                }
                let diagnostic_start = diagnostics.len();
                let (content, media_unsupported) = resolve_picture_content(
                    picture,
                    source,
                    media,
                    self.media_failures,
                    diagnostics,
                );
                if media_unsupported.is_some() {
                    geometry = ResolvedGeometry::BoundsFallback;
                }
                if self.media_poster_diagnostic_identity
                    && media_unsupported.is_some()
                    && picture.media.is_some()
                    && let Some(shape_id) = child.non_visual_id()
                    && let Some(diagnostic) = diagnostics.get_mut(diagnostic_start)
                {
                    diagnostic.message = format!(
                        "unresolved {} media poster for shape {shape_id}: {}",
                        flattened_source_name(source),
                        diagnostic.message
                    );
                }
                Ok(Some(ResolvedShape {
                    group_transform,
                    bounds,
                    rotation_deg,
                    flip_h,
                    flip_v,
                    geometry,
                    fill: None,
                    image_fill: None,
                    line,
                    head_end,
                    tail_end,
                    shadow,
                    content,
                    unsupported: media_unsupported.or(geometry_unsupported),
                }))
            }
            ShapeTreeChild::GraphicFrame(frame) => {
                let Some((bounds, rotation_deg, flip_h, flip_v)) =
                    transform_values(Some(&frame.transform))
                else {
                    return Ok(None);
                };
                let bounds = scaled_group_bounds(bounds, group_scale);
                let (content, unsupported, bounds_fallback) = match frame.graphic_data.payload() {
                    GraphicDataPayload::Table(table) => (
                        ResolvedContent::Table(self.resolve_table(
                            table,
                            source,
                            hyperlinks,
                            diagnostics,
                            text_directions,
                        )?),
                        None,
                        false,
                    ),
                    GraphicDataPayload::Chart(_) => self.resolve_chart_content(
                        frame,
                        None,
                        source,
                        bounds,
                        media,
                        charts,
                        fonts,
                        diagnostics,
                    )?,
                    GraphicDataPayload::SmartArt(_) => {
                        (ResolvedContent::None, Some("SmartArt"), true)
                    }
                    GraphicDataPayload::Ole { preview, .. } => {
                        if let Some(image) = resolve_ole_preview(
                            preview.as_deref(),
                            source,
                            media,
                            self.media_failures,
                        ) {
                            diagnostics.push(Diagnostic {
                                message: "OLE object rendered as a static PNG preview. Embedded OLE interactivity is not rendered".to_owned(),
                            });
                            (
                                ResolvedContent::Image(image),
                                Some("embedded OLE interactivity"),
                                false,
                            )
                        } else {
                            (ResolvedContent::None, Some("OLE"), true)
                        }
                    }
                    GraphicDataPayload::Other(_) => {
                        (ResolvedContent::None, Some("unknown graphic frame"), true)
                    }
                };
                if bounds_fallback {
                    let category = unsupported.expect("bounds fallback has a category");
                    diagnostics.push(Diagnostic {
                        message: format!("unsupported {category} content retained as bounds"),
                    });
                }
                Ok(Some(ResolvedShape {
                    group_transform,
                    bounds,
                    rotation_deg,
                    flip_h,
                    flip_v,
                    geometry: if bounds_fallback {
                        ResolvedGeometry::BoundsFallback
                    } else {
                        ResolvedGeometry::Rectangle
                    },
                    fill: None,
                    image_fill: None,
                    line: None,
                    head_end: None,
                    tail_end: None,
                    shadow: None,
                    content,
                    unsupported,
                }))
            }
            ShapeTreeChild::Connector(connector) => {
                let Some((bounds, rotation_deg, flip_h, flip_v)) =
                    connector_transform_values(connector.shape_properties.transform.as_ref())
                else {
                    return Ok(None);
                };
                let bounds = scaled_group_bounds(bounds, group_scale);
                let (fill, fill_unsupported) = connector
                    .shape_properties
                    .fill
                    .as_ref()
                    .map(|fill| self.concrete_fill(fill, (bounds.width, bounds.height)))
                    .transpose()?
                    .unwrap_or((None, None));
                let has_line_source =
                    connector.shape_properties.line.is_some() || connector.style().is_some();
                let effective_line = self.effective_connector_line(connector, false)?;
                let mut line = effective_line
                    .as_ref()
                    .map(|line| self.concrete_line(line, (bounds.width, bounds.height)))
                    .transpose()?
                    .flatten();
                let (head_end, tail_end) = effective_line
                    .as_ref()
                    .map(resolved_line_ends)
                    .unwrap_or((None, None));
                let shadow = self
                    .effective_effects(connector.style(), &connector.shape_properties)
                    .and_then(|effects| {
                        effects
                            .as_ref()
                            .and_then(|effects| effects.outer_shadow.as_ref())
                            .map(|shadow| self.concrete_shadow(shadow))
                            .transpose()
                    })?;
                let (geometry, geometry_unsupported, geometry_diagnostic) = if let Some(custom) =
                    &connector.shape_properties.custom_geometry
                {
                    match self.concrete_custom_geometry(custom, (bounds.width, bounds.height)) {
                        Ok(geometry) => (geometry, None, None),
                        Err(error) => (
                            ResolvedGeometry::BoundsFallback,
                            Some("connector custom geometry evaluation"),
                            Some(format!(
                                "connector custom geometry evaluation failed: {error}; retained as shape bounds"
                            )),
                        ),
                    }
                } else if let Some(preset) = &connector.shape_properties.preset_geometry {
                    {
                        match self.concrete_preset_geometry(preset, (bounds.width, bounds.height)) {
                            Ok(Some(geometry)) => (geometry, None, None),
                            Ok(None) => (
                                ResolvedGeometry::BoundsFallback,
                                Some("unknown connector preset geometry"),
                                Some(format!(
                                    "unknown connector preset geometry `{}` retained as shape bounds",
                                    preset.preset
                                )),
                            ),
                            Err(error) => (
                                ResolvedGeometry::BoundsFallback,
                                Some("connector preset geometry evaluation"),
                                Some(format!(
                                    "connector preset geometry `{}` evaluation failed: {error}; retained as shape bounds",
                                    preset.preset
                                )),
                            ),
                        }
                    }
                } else {
                    (
                        ResolvedGeometry::BoundsFallback,
                        Some("connector geometry"),
                        Some("unsupported connector geometry retained as shape bounds".to_owned()),
                    )
                };
                if let Some(category) = fill_unsupported {
                    diagnostics.push(Diagnostic {
                        message: format!(
                            "unsupported connector {category} retained as shape bounds"
                        ),
                    });
                }
                if let Some(message) = geometry_diagnostic {
                    diagnostics.push(Diagnostic { message });
                }
                let line_unsupported = (!has_line_source).then_some("connector line style");
                if line_unsupported.is_some() {
                    line = Some(Stroke::new(Paint::Solid(Color::BLACK), 1.0));
                    diagnostics.push(Diagnostic {
                        message: "unsupported connector line style retained as visible default"
                            .to_owned(),
                    });
                }
                let unsupported = fill_unsupported
                    .or(geometry_unsupported)
                    .or(line_unsupported);
                Ok(Some(ResolvedShape {
                    group_transform,
                    bounds,
                    rotation_deg,
                    flip_h,
                    flip_v,
                    geometry,
                    fill,
                    image_fill: None,
                    line,
                    head_end,
                    tail_end,
                    shadow,
                    content: ResolvedContent::None,
                    unsupported,
                }))
            }
            ShapeTreeChild::AlternateContent(alternate) => {
                let Some(frame) = alternate.chart_choice() else {
                    return Ok(None);
                };
                let Some((bounds, rotation_deg, flip_h, flip_v)) =
                    transform_values(Some(&frame.transform))
                else {
                    return Ok(None);
                };
                let bounds = scaled_group_bounds(bounds, group_scale);
                let (content, unsupported, bounds_fallback) = if charts.is_some() && fonts.is_some()
                {
                    self.resolve_chart_content(
                        frame,
                        alternate.picture_fallback(),
                        source,
                        bounds,
                        media,
                        charts,
                        fonts,
                        diagnostics,
                    )?
                } else if let Some(picture) = alternate.picture_fallback() {
                    let (content, _) = resolve_picture_content(
                        picture,
                        source,
                        media,
                        self.media_failures,
                        diagnostics,
                    );
                    let has_image = matches!(content, ResolvedContent::Image(_));
                    (content, Some("chart"), !has_image)
                } else {
                    (ResolvedContent::None, Some("chart"), true)
                };
                if bounds_fallback {
                    diagnostics.push(Diagnostic {
                        message: "unsupported chart content retained as bounds".to_owned(),
                    });
                }
                Ok(Some(ResolvedShape {
                    group_transform,
                    bounds,
                    rotation_deg,
                    flip_h,
                    flip_v,
                    geometry: if bounds_fallback {
                        ResolvedGeometry::BoundsFallback
                    } else {
                        ResolvedGeometry::Rectangle
                    },
                    fill: None,
                    image_fill: None,
                    line: None,
                    head_end: None,
                    tail_end: None,
                    shadow: None,
                    content,
                    unsupported,
                }))
            }
            ShapeTreeChild::GroupShape(_) => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_chart_content(
        &self,
        frame: &oxoffice_pptx_oxml::graphic_frame::CT_GraphicFrame,
        fallback: Option<&CT_Picture>,
        source: FlattenedSource,
        bounds: Rect,
        media: Option<&ScopedMediaIds>,
        charts: Option<&ScopedChartResources>,
        fonts: Option<&mut FontManager>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(ResolvedContent, Option<&'static str>, bool), ResolveError> {
        let Some(charts) = charts else {
            return Ok((ResolvedContent::None, Some("chart"), true));
        };
        let Some(fonts) = fonts else {
            return Ok((ResolvedContent::None, Some("chart"), true));
        };
        let relationship_id = frame.chart_relationship_id();
        let resource = relationship_id.and_then(|id| charts.get(source, id));
        let failure = match (relationship_id, resource) {
            (None, _) => "chart payload has no relationship identifier".to_owned(),
            (Some(id), None) => format!(
                "missing {} chart relationship `{id}`",
                flattened_source_name(source)
            ),
            (Some(_), Some(ChartResource::External(target))) => format!(
                "external {} chart relationship `{}` targets `{target}`",
                flattened_source_name(source),
                relationship_id.expect("matched a relationship id")
            ),
            (Some(_), Some(ChartResource::MissingTarget(target))) => format!(
                "missing {} chart target `{target}` for relationship `{}`",
                flattened_source_name(source),
                relationship_id.expect("matched a relationship id")
            ),
            (Some(_), Some(ChartResource::Invalid(detail))) => format!(
                "invalid {} chart relationship `{}`: {detail}",
                flattened_source_name(source),
                relationship_id.expect("matched a relationship id")
            ),
            (Some(_), Some(ChartResource::Parsed(chart_space))) => {
                let local_bounds = Rect {
                    x: 0.0,
                    y: 0.0,
                    width: bounds.width,
                    height: bounds.height,
                };
                match render_chart(
                    &chart_space.chart,
                    local_bounds,
                    self.theme,
                    &self.color_map,
                    fonts,
                ) {
                    Ok(group) => {
                        for code in oxoffice_chart::unsupported_number_formats(&chart_space.chart)
                            .map_err(|error| ResolveError::ConcreteValue {
                            kind: "chart number format",
                            detail: error.to_string(),
                        })? {
                            let message = format!(
                                "chart number format `{code}` is not supported and its labels use General"
                            );
                            if !diagnostics
                                .iter()
                                .any(|diagnostic| diagnostic.message == message)
                            {
                                diagnostics.push(Diagnostic { message });
                            }
                        }
                        return Ok((ResolvedContent::Group(group), None, false));
                    }
                    Err(error) => error.to_string(),
                }
            }
        };

        if let Some(picture) = fallback {
            let (content, _) =
                resolve_picture_content(picture, source, media, self.media_failures, diagnostics);
            let relationship_id = picture
                .blip_fill
                .as_ref()
                .and_then(|fill| fill.blip.as_ref())
                .and_then(|blip| blip.embed.as_deref());
            let renderer_compatible = relationship_id
                .and_then(|id| media.and_then(|media| media.content_type(source, id)))
                .is_some_and(renderer_compatible_image_content_type);
            if matches!(content, ResolvedContent::Image(_)) && renderer_compatible {
                diagnostics.push(Diagnostic {
                    message: format!("unsupported chart rendered as cached image: {failure}"),
                });
                return Ok((content, Some("chart"), false));
            }
            if matches!(content, ResolvedContent::Image(_)) {
                diagnostics.push(Diagnostic {
                    message: format!(
                        "unsupported chart cached image is not renderer-compatible: {failure}"
                    ),
                });
            }
        }

        let placeholder = render_chart_placeholder(
            Rect {
                x: 0.0,
                y: 0.0,
                width: bounds.width,
                height: bounds.height,
            },
            fonts,
        )
        .map_err(|error| ResolveError::ConcreteValue {
            kind: "chart placeholder",
            detail: error.to_string(),
        })?;
        diagnostics.push(Diagnostic {
            message: format!("unsupported chart rendered as labelled placeholder: {failure}"),
        });
        Ok((ResolvedContent::Group(placeholder), Some("chart"), true))
    }

    fn resolve_ordinary_shape(
        &self,
        shape: &CT_Shape,
        placement: ShapePlacement,
        media: Option<&ScopedMediaIds>,
        hyperlinks: Option<&ScopedHyperlinkTargets>,
        diagnostics: &mut Vec<Diagnostic>,
        text_directions: &mut Vec<Vec<oxoffice_layout::TextDirection>>,
    ) -> Result<Option<ResolvedShape>, ResolveError> {
        let ShapePlacement {
            source,
            group_scale,
            group_transform,
        } = placement;
        let transform = self.effective_xfrm(shape);
        let Some((unscaled_bounds, rotation_deg, flip_h, flip_v)) =
            transform_values(transform.as_ref())
        else {
            return Ok(None);
        };
        let mut bounds = scaled_group_bounds(unscaled_bounds, group_scale);
        let mut group_transform = group_transform;
        let effective = self.effective_shape_style(shape)?;
        let (fill, image_fill, fill_unsupported, fill_diagnostic) = match effective.fill.as_ref() {
            Some(Fill::Blip(fill))
                if matches!(shape.shape_properties.fill, Some(Fill::Blip(_))) =>
            {
                match resolve_image_fill(fill, source, media, self.media_failures, "shape") {
                    Ok(image) => (None, Some(image), None, None),
                    Err((category, message)) => (None, None, Some(category), Some(message)),
                }
            }
            Some(Fill::Blip(_)) => (
                None,
                None,
                Some("theme-referenced shape picture fill"),
                Some(
                    "theme-referenced shape picture fill requires theme relationship scope"
                        .to_owned(),
                ),
            ),
            Some(fill) => {
                let normalized = match fill {
                    Fill::Gradient(gradient)
                        if gradient.rotate_with_shape == Some(false)
                            && rotation_deg.rem_euclid(360.0) == 0.0
                            && !flip_h
                            && !flip_v
                            && group_transform.a > 0.0
                            && group_transform.b.abs() < 1.0e-10
                            && group_transform.c.abs() < 1.0e-10
                            && group_transform.d > 0.0 =>
                    {
                        let mut normalized = fill.clone();
                        let Fill::Gradient(gradient) = &mut normalized else {
                            unreachable!("matched a gradient fill")
                        };
                        gradient.rotate_with_shape = None;
                        Some(normalized)
                    }
                    _ => None,
                };
                let selected = normalized.as_ref().unwrap_or(fill);
                let shape_path = matches!(selected, Fill::Gradient(g) if matches!(&g.geometry, Some(GradientGeometry::Path(p)) if p.kind == PathGradientKind::Shape));
                let (fill, unsupported) = if shape_path {
                    let preset = shape
                        .shape_properties
                        .preset_geometry
                        .as_ref()
                        .map(|p| p.preset.as_str())
                        .unwrap_or("rect");
                    if shape.shape_properties.custom_geometry.is_some()
                        || !matches!(preset, "rect" | "ellipse")
                    {
                        (None, Some("shape path gradient on unsupported contour"))
                    } else {
                        let mut selected = selected.clone();
                        if preset == "ellipse"
                            && let Fill::Gradient(g) = &mut selected
                            && let Some(GradientGeometry::Path(p)) = &mut g.geometry
                        {
                            p.kind = PathGradientKind::Circle;
                        }
                        // A circle path expands to the corner. An ellipse shape stops at its own contour.
                        let (mut paint, unsupported) =
                            self.concrete_fill(&selected, (bounds.width, bounds.height))?;
                        if preset == "ellipse"
                            && let Some(Paint::Grid { colors, .. }) = &mut paint
                            && let Fill::Gradient(g) = &selected
                        {
                            let mut stops = g
                                .stops
                                .iter()
                                .filter_map(|stop| {
                                    stop.color.as_ref().map(|c| {
                                        self.concrete_color(c).map(|color| GradientStop {
                                            offset: f64::from(stop.position.0) / 100_000.0,
                                            color,
                                        })
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                            stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
                            for (index, color) in colors.iter_mut().enumerate() {
                                let x = ((index % 128) as f64 + 0.5) / 64.0 - 1.0;
                                let y = ((index / 128) as f64 + 0.5) / 64.0 - 1.0;
                                *color = path_gradient_color(&stops, x.hypot(y));
                            }
                        }
                        (paint, unsupported)
                    }
                } else {
                    self.concrete_fill(selected, (bounds.width, bounds.height))?
                };
                (fill, None, unsupported, None)
            }
            None => (None, None, None, None),
        };
        let line = effective
            .line
            .as_ref()
            .map(|line| self.concrete_line(line, (bounds.width, bounds.height)))
            .transpose()?
            .flatten();
        let (head_end, tail_end) = effective
            .line
            .as_ref()
            .map(resolved_line_ends)
            .unwrap_or((None, None));
        let shadow = effective
            .effects
            .as_ref()
            .and_then(|effects| effects.outer_shadow.as_ref())
            .map(|shadow| self.concrete_shadow(shadow))
            .transpose()?;
        let transparent_text_only = shape.text_body.is_some()
            && fill.is_none()
            && image_fill.is_none()
            && fill_unsupported.is_none()
            && line.is_none()
            && shadow.is_none();
        if transparent_text_only {
            bounds = unscaled_bounds;
            group_transform = Transform {
                a: group_scale.0,
                d: group_scale.1,
                ..Transform::IDENTITY
            }
            .then(group_transform);
        }
        let (geometry, geometry_unsupported, geometry_diagnostic) = if let Some(custom) =
            &shape.shape_properties.custom_geometry
        {
            match self.concrete_custom_geometry(custom, (bounds.width, bounds.height)) {
                Ok(geometry) => (geometry, None, None),
                Err(error) => (
                    ResolvedGeometry::BoundsFallback,
                    Some("custom geometry evaluation"),
                    Some(format!(
                        "custom geometry evaluation failed: {error}; retained as shape bounds"
                    )),
                ),
            }
        } else if let Some(preset) = &shape.shape_properties.preset_geometry {
            match self.concrete_preset_geometry(preset, (bounds.width, bounds.height)) {
                Ok(Some(geometry)) => (geometry, None, None),
                Ok(None) => (
                    ResolvedGeometry::BoundsFallback,
                    Some("unknown preset geometry"),
                    Some(format!(
                        "unknown preset geometry `{}` retained as shape bounds",
                        preset.preset
                    )),
                ),
                Err(error) => (
                    ResolvedGeometry::BoundsFallback,
                    Some("preset geometry evaluation"),
                    Some(format!(
                        "preset geometry `{}` evaluation failed: {error}; retained as shape bounds",
                        preset.preset
                    )),
                ),
            }
        } else if shape.text_body.is_some() {
            (ResolvedGeometry::Rectangle, None, None)
        } else {
            (
                ResolvedGeometry::BoundsFallback,
                Some("preset geometry pending evaluation"),
                None,
            )
        };
        let content = if let Some(body) = shape.text_body.as_ref() {
            let (body, directions) =
                self.resolve_text_body(shape, body, source, hyperlinks, diagnostics)?;
            text_directions.push(directions);
            ResolvedContent::Text(body)
        } else {
            ResolvedContent::None
        };
        if let ResolvedContent::Text(text) = &content
            && let Some(message) = vertical_text_diagnostic(text.vertical)
        {
            diagnostics.push(Diagnostic {
                message: message.to_owned(),
            });
        }
        if let Some(category) = fill_unsupported {
            diagnostics.push(Diagnostic {
                message: fill_diagnostic
                    .unwrap_or_else(|| format!("unsupported {category} retained as shape bounds")),
            });
        }
        if let Some(category) = geometry_unsupported {
            diagnostics.push(Diagnostic {
                message: geometry_diagnostic
                    .unwrap_or_else(|| format!("unsupported {category} retained as shape bounds")),
            });
        }
        Ok(Some(ResolvedShape {
            group_transform,
            bounds,
            rotation_deg,
            flip_h,
            flip_v,
            geometry,
            fill,
            image_fill,
            line,
            head_end,
            tail_end,
            shadow,
            content,
            unsupported: fill_unsupported.or(geometry_unsupported),
        }))
    }

    fn concrete_picture_geometry(
        &self,
        picture: &oxoffice_pptx_oxml::picture::CT_Picture,
        size: (f64, f64),
    ) -> (ResolvedGeometry, Option<&'static str>) {
        if let Some(custom) = &picture.shape_properties.custom_geometry {
            return self
                .concrete_custom_geometry(custom, size)
                .map(|geometry| (geometry, None))
                .unwrap_or((ResolvedGeometry::Rectangle, Some("picture custom geometry")));
        }
        if let Some(preset) = &picture.shape_properties.preset_geometry {
            return match self.concrete_preset_geometry(preset, size) {
                Ok(Some(geometry)) => (geometry, None),
                Ok(None) => (
                    ResolvedGeometry::Rectangle,
                    Some("unknown picture preset geometry"),
                ),
                Err(_) => (
                    ResolvedGeometry::Rectangle,
                    Some("picture preset geometry evaluation"),
                ),
            };
        }
        (ResolvedGeometry::Rectangle, None)
    }
}

fn renderer_compatible_image_content_type(content_type: &str) -> bool {
    content_type.eq_ignore_ascii_case("image/png")
        || content_type.eq_ignore_ascii_case("image/jpeg")
        || content_type.eq_ignore_ascii_case("image/jpg")
}

fn resolve_picture_content(
    picture: &oxoffice_pptx_oxml::picture::CT_Picture,
    source: FlattenedSource,
    media: Option<&ScopedMediaIds>,
    failures: Option<&ScopedMediaFailures>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (ResolvedContent, Option<&'static str>) {
    let Some(fill) = picture.blip_fill.as_ref() else {
        return unsupported_picture(
            "alternate picture media",
            "alternate picture media is not resolved",
            diagnostics,
        );
    };
    match resolve_image_fill(fill, source, media, failures, "picture") {
        Ok(image) => (ResolvedContent::Image(image), None),
        Err((category, message)) => unsupported_picture(category, &message, diagnostics),
    }
}

fn resolve_ole_preview(
    preview: Option<&oxoffice_pptx_oxml::picture::CT_Picture>,
    source: FlattenedSource,
    media: Option<&ScopedMediaIds>,
    failures: Option<&ScopedMediaFailures>,
) -> Option<ResolvedImage> {
    let fill = preview?.blip_fill.as_ref()?;
    let relationship_id = fill.blip.as_ref()?.embed.as_deref()?;
    let media = media?;
    if !media
        .content_type(source, relationship_id)
        .is_some_and(|content_type| content_type.eq_ignore_ascii_case("image/png"))
    {
        return None;
    }
    resolve_image_fill(fill, source, Some(media), failures, "picture").ok()
}

fn resolve_image_fill(
    fill: &BlipFill,
    source: FlattenedSource,
    media: Option<&ScopedMediaIds>,
    failures: Option<&ScopedMediaFailures>,
    role: &str,
) -> Result<ResolvedImage, (&'static str, String)> {
    let missing_blip_category = match role {
        "picture" => "missing picture blip",
        "shape" => "missing shape picture blip",
        _ => "missing background blip",
    };
    let external_media_category = match role {
        "picture" => "external picture media",
        "shape" => "external shape picture media",
        _ => "external background media",
    };
    let Some(blip) = fill.blip.as_ref() else {
        return Err((
            missing_blip_category,
            format!("{role} fill has no modelled embedded blip"),
        ));
    };
    let Some(relationship_id) = blip.embed.as_deref() else {
        let message = if let Some(link) = blip.link.as_deref() {
            format!("external {role} relationship `{link}` is unsupported")
        } else {
            format!("{role} blip has no embedded relationship")
        };
        return Err((external_media_category, message));
    };
    let Some(media) = media else {
        return Err((
            "image media pending relationship resolution",
            format!("{role} image media pending relationship resolution"),
        ));
    };
    if let Some(failure) = failures.and_then(|failures| failures.get(source, relationship_id)) {
        return Err((
            "image render failure",
            format!("{role} image relationship `{relationship_id}` {failure}"),
        ));
    }
    let Some(media_id) = media.get(source, relationship_id) else {
        return Err((
            "missing image relationship",
            format!(
                "missing {} {role} image relationship `{relationship_id}`",
                flattened_source_name(source)
            ),
        ));
    };
    Ok(ResolvedImage {
        media: media_id,
        src_rect: fill.source_rect.as_ref().map(resolved_crop_rect),
        placement: match fill.mode.as_ref() {
            Some(BlipMode::Tile(tile)) => ResolvedImagePlacement::Tile(ResolvedTilePlacement {
                translation: Point {
                    x: emu_to_points(tile.translation_x.unwrap_or(0)),
                    y: emu_to_points(tile.translation_y.unwrap_or(0)),
                },
                scale_x: tile
                    .scale_x
                    .map_or(1.0, |value| f64::from(value.0) / 100_000.0),
                scale_y: tile
                    .scale_y
                    .map_or(1.0, |value| f64::from(value.0) / 100_000.0),
                flip: resolved_tile_flip(tile.flip.as_deref()),
                alignment: resolved_rect_alignment(tile.alignment.as_deref()),
            }),
            Some(BlipMode::Stretch { fill_rect, .. }) => ResolvedImagePlacement::Stretch {
                fill_rect: fill_rect.as_ref().map(resolved_crop_rect),
            },
            None => ResolvedImagePlacement::default(),
        },
        dpi: fill.dpi.filter(|dpi| *dpi > 0).map(f64::from),
        rotate_with_shape: fill.rotate_with_shape.unwrap_or(true),
        opacity: blip
            .alpha_modulation_fix
            .map_or(1.0, |amount| f64::from(amount.0) / 100_000.0),
    })
}

fn unsupported_picture(
    category: &'static str,
    message: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> (ResolvedContent, Option<&'static str>) {
    diagnostics.push(Diagnostic {
        message: message.to_owned(),
    });
    (ResolvedContent::None, Some(category))
}

fn flattened_source_name(source: FlattenedSource) -> &'static str {
    match source {
        FlattenedSource::Background => "background",
        FlattenedSource::Master => "master",
        FlattenedSource::Layout => "layout",
        FlattenedSource::Slide => "slide",
    }
}

fn resolve_direct_hyperlink(
    properties: Option<&CT_TextCharacterProperties>,
    source: FlattenedSource,
    hyperlinks: Option<&ScopedHyperlinkTargets>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    let hyperlink = properties?.hyperlink_click.as_ref()?;
    if let Some(action) = hyperlink.action.as_deref() {
        diagnostics.push(Diagnostic {
            message: format!(
                "unsupported {} hyperlink action `{action}`",
                flattened_source_name(source)
            ),
        });
        return None;
    }
    let Some(relationship_id) = hyperlink.relationship_id.as_deref() else {
        diagnostics.push(Diagnostic {
            message: format!(
                "unsupported {} hyperlink without an external relationship",
                flattened_source_name(source)
            ),
        });
        return None;
    };
    let Some(target) = hyperlinks.and_then(|targets| targets.get(source, relationship_id)) else {
        diagnostics.push(Diagnostic {
            message: format!(
                "missing {} hyperlink relationship `{relationship_id}`",
                flattened_source_name(source)
            ),
        });
        return None;
    };
    Some(target.to_owned())
}

fn resolved_crop_rect(rect: &RelativeRect) -> crate::CropRect {
    crate::CropRect {
        left: rect
            .left
            .map_or(0.0, |value| f64::from(value.0) / 100_000.0),
        top: rect.top.map_or(0.0, |value| f64::from(value.0) / 100_000.0),
        right: rect
            .right
            .map_or(0.0, |value| f64::from(value.0) / 100_000.0),
        bottom: rect
            .bottom
            .map_or(0.0, |value| f64::from(value.0) / 100_000.0),
    }
}

fn resolved_tile_flip(value: Option<&str>) -> ResolvedTileFlip {
    match value {
        Some("x") => ResolvedTileFlip::Horizontal,
        Some("y") => ResolvedTileFlip::Vertical,
        Some("xy") => ResolvedTileFlip::Both,
        Some("none") | None => ResolvedTileFlip::None,
        Some(_) => unreachable!("DrawingML tile flip is validated while parsing"),
    }
}

fn resolved_rect_alignment(value: Option<&str>) -> ResolvedRectAlignment {
    match value {
        Some("t") => ResolvedRectAlignment::Top,
        Some("tr") => ResolvedRectAlignment::TopRight,
        Some("l") => ResolvedRectAlignment::Left,
        Some("ctr") => ResolvedRectAlignment::Center,
        Some("r") => ResolvedRectAlignment::Right,
        Some("bl") => ResolvedRectAlignment::BottomLeft,
        Some("b") => ResolvedRectAlignment::Bottom,
        Some("br") => ResolvedRectAlignment::BottomRight,
        Some("tl") | None => ResolvedRectAlignment::TopLeft,
        Some(_) => unreachable!("DrawingML rectangle alignment is validated while parsing"),
    }
}

fn resolved_line_ends(
    line: &CT_LineProperties,
) -> (Option<ResolvedLineEnd>, Option<ResolvedLineEnd>) {
    (
        line.head_end.as_ref().and_then(resolved_line_end),
        line.tail_end.as_ref().and_then(resolved_line_end),
    )
}

fn resolved_line_end(end: &LineEnd) -> Option<ResolvedLineEnd> {
    let kind = match end.kind? {
        LineEndType::None => return None,
        LineEndType::Triangle => ResolvedLineEndKind::Triangle,
        LineEndType::Stealth => ResolvedLineEndKind::Stealth,
        LineEndType::Diamond => ResolvedLineEndKind::Diamond,
        LineEndType::Oval => ResolvedLineEndKind::Oval,
        LineEndType::Arrow => ResolvedLineEndKind::Arrow,
    };
    Some(ResolvedLineEnd {
        kind,
        width: resolved_line_end_size(end.width),
        length: resolved_line_end_size(end.length),
    })
}

fn resolved_line_end_size(size: Option<LineEndSize>) -> ResolvedLineEndSize {
    match size.unwrap_or(LineEndSize::Medium) {
        LineEndSize::Small => ResolvedLineEndSize::Small,
        LineEndSize::Medium => ResolvedLineEndSize::Medium,
        LineEndSize::Large => ResolvedLineEndSize::Large,
    }
}

impl ResolveCtx<'_> {
    fn concrete_background_fill(
        &self,
        fill: &Fill,
        size: (f64, f64),
        source: FlattenedSource,
        media: Option<&ScopedMediaIds>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(Option<ResolvedBackground>, Option<&'static str>), ResolveError> {
        let mut fill = fill.clone();
        let slot = self.color_map.theme_slot(ColorMapSlot::Background1);
        let reference = self.theme.theme_elements.color_scheme.color(slot);
        substitute_fill(&mut fill, Some(reference), "background")?;
        self.concrete_background_value(&fill, size, source, media, diagnostics)
    }

    fn concrete_background_reference(
        &self,
        index: u32,
        color: Option<&ColorChoice>,
        size: (f64, f64),
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(Option<ResolvedBackground>, Option<&'static str>), ResolveError> {
        let matrix = &self.theme.theme_elements.format_scheme;
        let Some(mut fill) =
            referenced_fill(index, &matrix.fill_styles, &matrix.background_fill_styles)?
        else {
            return Ok((None, None));
        };
        substitute_fill(&mut fill, color, "background")?;
        if matches!(fill, Fill::Blip(_)) {
            diagnostics.push(Diagnostic {
                message: "theme-referenced background picture requires theme relationship scope"
                    .to_owned(),
            });
            return Ok((None, None));
        }
        let (paint, unsupported) = self.concrete_background_paint(&fill, size)?;
        Ok((paint.map(ResolvedBackground::Paint), unsupported))
    }

    fn concrete_background_value(
        &self,
        fill: &Fill,
        size: (f64, f64),
        source: FlattenedSource,
        media: Option<&ScopedMediaIds>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(Option<ResolvedBackground>, Option<&'static str>), ResolveError> {
        if let Fill::Blip(fill) = fill {
            return Ok(
                match resolve_image_fill(fill, source, media, self.media_failures, "background") {
                    Ok(image) => (Some(ResolvedBackground::Image(image)), None),
                    Err((_category, message)) => {
                        diagnostics.push(Diagnostic { message });
                        (None, None)
                    }
                },
            );
        }
        let (paint, unsupported) = self.concrete_background_paint(fill, size)?;
        Ok((paint.map(ResolvedBackground::Paint), unsupported))
    }

    fn concrete_background_paint(
        &self,
        fill: &Fill,
        size: (f64, f64),
    ) -> Result<(Option<Paint>, Option<&'static str>), ResolveError> {
        let mut fill = fill.clone();
        if let Fill::Gradient(gradient) = &mut fill {
            gradient.rotate_with_shape = None;
        }
        self.concrete_fill(&fill, size)
    }

    fn concrete_fill(
        &self,
        fill: &Fill,
        size: (f64, f64),
    ) -> Result<(Option<Paint>, Option<&'static str>), ResolveError> {
        match fill {
            Fill::NoFill(_) => Ok((None, None)),
            Fill::Solid(solid) => {
                let Some(choice) = solid.color.as_ref() else {
                    return Ok((None, Some("solid fill without colour")));
                };
                Ok((Some(Paint::Solid(self.concrete_color(choice)?)), None))
            }
            Fill::Gradient(gradient) => {
                if gradient.rotate_with_shape == Some(false) {
                    return Ok((None, Some("gradient independent of shape rotation")));
                }
                match gradient.flip.as_deref() {
                    Some("x") => return Ok((None, Some("horizontal gradient flip"))),
                    Some("y") => return Ok((None, Some("vertical gradient flip"))),
                    Some("xy") => return Ok((None, Some("horizontal and vertical gradient flip"))),
                    Some("none") | None => {}
                    Some(_) => unreachable!("DrawingML parser validates gradient flip tokens"),
                }
                if gradient.tile_rect.is_some() {
                    return Ok((None, Some("gradient tile rectangle")));
                }
                if let Some(GradientGeometry::Path(path)) = &gradient.geometry
                    && !path.fill_to_rect.as_ref().is_some_and(|rect| {
                        [rect.left, rect.top, rect.right, rect.bottom]
                            .into_iter()
                            .all(|value| value.is_some_and(|value| value.0 == 50_000))
                    })
                {
                    return Ok((None, Some("path gradient with non-centred fill rectangle")));
                }
                let mut stops = Vec::with_capacity(gradient.stops.len());
                for stop in &gradient.stops {
                    let Some(choice) = stop.color.as_ref() else {
                        return Ok((None, Some("gradient stop without colour")));
                    };
                    stops.push(GradientStop {
                        offset: f64::from(stop.position.0) / 100_000.0,
                        color: self.concrete_color(choice)?,
                    });
                }
                if stops.is_empty() {
                    return Ok((None, Some("gradient without concrete stops")));
                }
                let paint = match &gradient.geometry {
                    Some(GradientGeometry::Linear(linear)) => {
                        let radians = f64::from(linear.angle.0) / 60_000.0_f64;
                        let radians = radians.to_radians();
                        let center = Point {
                            x: size.0 / 2.0,
                            y: size.1 / 2.0,
                        };
                        let mut direction = Point {
                            x: radians.cos(),
                            y: radians.sin(),
                        };
                        if linear.scaled == Some(true) {
                            direction.x *= size.0;
                            direction.y *= size.1;
                        }
                        let length = direction.x.hypot(direction.y);
                        if length > 0.0 {
                            direction.x /= length;
                            direction.y /= length;
                        }
                        let extent =
                            (direction.x.abs() * size.0 + direction.y.abs() * size.1) / 2.0;
                        let delta = Point {
                            x: direction.x * extent,
                            y: direction.y * extent,
                        };
                        Paint::linear(
                            Point {
                                x: center.x - delta.x,
                                y: center.y - delta.y,
                            },
                            Point {
                                x: center.x + delta.x,
                                y: center.y + delta.y,
                            },
                            powerpoint_gradient_stops(&stops),
                            (true, true),
                        )
                    }
                    None => Paint::linear(
                        Point { x: 0.0, y: 0.0 },
                        Point { x: size.0, y: 0.0 },
                        powerpoint_gradient_stops(&stops),
                        (true, true),
                    ),
                    Some(GradientGeometry::Path(path)) => {
                        let centered = path.fill_to_rect.as_ref().is_some_and(|r| {
                            [r.left, r.top, r.right, r.bottom]
                                .into_iter()
                                .all(|v| v.is_some_and(|v| v.0 == 50_000))
                        });
                        if !centered {
                            return Ok((
                                None,
                                Some("path gradient with non-centred fill rectangle"),
                            ));
                        }
                        let mut ordered = stops;
                        ordered.sort_by(|a, b| a.offset.total_cmp(&b.offset));
                        let mut colors = Vec::with_capacity(128 * 128);
                        for y in 0..128 {
                            for x in 0..128 {
                                let dx = ((f64::from(x) + 0.5) / 128.0 - 0.5).abs() * 2.0;
                                let dy = ((f64::from(y) + 0.5) / 128.0 - 0.5).abs() * 2.0;
                                let distance = match path.kind {
                                    PathGradientKind::Circle => {
                                        dx.hypot(dy) / std::f64::consts::SQRT_2
                                    }
                                    PathGradientKind::Rectangle | PathGradientKind::Shape => {
                                        dx.max(dy)
                                    }
                                };
                                colors.push(path_gradient_color(&ordered, distance));
                            }
                        }
                        let Some(paint) = Paint::grid(
                            Rect {
                                x: 0.0,
                                y: 0.0,
                                width: size.0,
                                height: size.1,
                            },
                            128,
                            128,
                            colors,
                        ) else {
                            return Ok((
                                None,
                                Some("path gradient without positive finite extent"),
                            ));
                        };
                        paint
                    }
                };
                Ok((Some(paint), None))
            }
            Fill::Pattern(pattern) => {
                let Some(rows) = pattern.preset.as_deref().and_then(native_pattern_rows) else {
                    return Ok((None, Some("pattern without supported preset")));
                };
                let foreground = pattern
                    .foreground
                    .as_ref()
                    .map(|c| self.concrete_color(c))
                    .transpose()?
                    .unwrap_or(Color::BLACK);
                let background = pattern
                    .background
                    .as_ref()
                    .map(|c| self.concrete_color(c))
                    .transpose()?
                    .unwrap_or(Color::WHITE);
                let mut colors = Vec::with_capacity(4096);
                for &row in rows {
                    let bits = NATIVE_PATTERN_ROW_WEIGHTS[usize::from(row)];
                    for x in 0..64 {
                        let weight = ((bits >> (126 - x * 2)) & 3) as u8;
                        colors.push(blend_fill_colors(
                            background,
                            foreground,
                            f64::from(weight) / 2.0,
                        ));
                    }
                }
                Ok((
                    Paint::grid(
                        Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 8.0,
                            height: 8.0,
                        },
                        64,
                        64,
                        colors,
                    ),
                    None,
                ))
            }
            Fill::Blip(_) => Ok((None, Some("picture fill media"))),
        }
    }

    fn concrete_line(
        &self,
        line: &CT_LineProperties,
        size: (f64, f64),
    ) -> Result<Option<Stroke>, ResolveError> {
        let Some(fill) = line.fill.as_ref() else {
            return Ok(None);
        };
        let (Some(paint), _) = self.concrete_fill(fill, size)? else {
            return Ok(None);
        };
        let width = emu_to_points(i64::from(line.width.unwrap_or(12_700)));
        let cap = match line.cap.unwrap_or(DrawingLineCap::Flat) {
            DrawingLineCap::Round => LineCap::Round,
            DrawingLineCap::Square => LineCap::Square,
            DrawingLineCap::Flat => LineCap::Butt,
        };
        let join = match line.join.as_ref() {
            Some(DrawingLineJoin::Round { .. }) => LineJoin::Round,
            Some(DrawingLineJoin::Bevel { .. }) => LineJoin::Bevel,
            Some(DrawingLineJoin::Miter { .. }) | None => LineJoin::Miter,
        };
        let dash = line.dash.as_ref().and_then(|dash| match dash {
            LineDash::Preset(preset) => {
                let values = preset
                    .value
                    .dash_array()
                    .iter()
                    .map(|value| f64::from(*value) * width)
                    .collect::<Vec<_>>();
                (!values.is_empty()).then_some(values)
            }
            LineDash::Custom(custom) => {
                let values = custom
                    .stops
                    .iter()
                    .flat_map(|stop| [stop.dash, stop.space])
                    .map(|value| f64::from(value) / 100_000.0 * width)
                    .collect::<Vec<_>>();
                (!values.is_empty()).then_some(values)
            }
        });
        Ok(Some(Stroke {
            paint,
            width,
            cap,
            join,
            dash,
        }))
    }

    fn concrete_shadow(&self, shadow: &CT_OuterShadowEffect) -> Result<Effect, ResolveError> {
        let color = shadow
            .color
            .as_ref()
            .map(|choice| self.concrete_color(choice))
            .transpose()?
            .unwrap_or(Color::BLACK);
        let distance = emu_to_points(shadow.distance.unwrap_or(0));
        let angle = f64::from(shadow.direction.unwrap_or_default().0) / 60_000.0;
        let radians = angle.to_radians();
        Ok(Effect::OuterShadow {
            dx: radians.cos() * distance,
            dy: radians.sin() * distance,
            blur: emu_to_points(shadow.blur_radius.unwrap_or(0)),
            color,
        })
    }

    pub(crate) fn concrete_color(&self, choice: &ColorChoice) -> Result<Color, ResolveError> {
        let owned_lookup = self.theme_lookup();
        let lookup = owned_lookup
            .iter()
            .map(|(name, color)| (name.as_str(), *color))
            .collect::<Vec<_>>();
        let color = resolve_color(choice, &self.color_map, &lookup).map_err(|error| {
            ResolveError::ConcreteValue {
                kind: "colour",
                detail: error.to_string(),
            }
        })?;
        Ok(Color {
            r: f64::from(color.red) / 255.0,
            g: f64::from(color.green) / 255.0,
            b: f64::from(color.blue) / 255.0,
            a: f64::from(color.alpha) / 255.0,
        })
    }

    fn theme_lookup(&self) -> Vec<(String, RgbColor)> {
        let mut lookup = self
            .theme
            .theme_elements
            .color_scheme
            .iter()
            .filter_map(|(slot, choice)| {
                base_rgb(choice).map(|color| (slot.as_str().to_owned(), color))
            })
            .collect::<Vec<_>>();
        lookup.push(("black".to_owned(), RgbColor::new(0, 0, 0)));
        lookup.push(("white".to_owned(), RgbColor::new(255, 255, 255)));
        lookup
    }

    fn concrete_custom_geometry(
        &self,
        geometry: &oxoffice_drawing::geometry::CT_CustomGeometry2D,
        size: (f64, f64),
    ) -> Result<ResolvedGeometry, ResolveError> {
        let evaluated = geometry
            .evaluate_with_size(&BTreeMap::new(), size)
            .map_err(|error| ResolveError::ConcreteValue {
                kind: "custom geometry",
                detail: error.to_string(),
            })?;
        let paths = evaluated
            .paths
            .into_iter()
            .zip(geometry.paths())
            .map(|(commands, source)| {
                let scale_x = source.width.map_or(1.0, |width| size.0 / width);
                let scale_y = source.height.map_or(1.0, |height| size.1 / height);
                Path {
                    commands: commands
                        .into_iter()
                        .map(|command| concrete_path_command(command, scale_x, scale_y))
                        .collect(),
                    fill_rule: FillRule::NonZero,
                }
            })
            .collect();
        let first_path = geometry.paths().first();
        let text_scale_x = first_path
            .and_then(|path| path.width)
            .map_or(1.0, |width| size.0 / width);
        let text_scale_y = first_path
            .and_then(|path| path.height)
            .map_or(1.0, |height| size.1 / height);
        let text_rect = evaluated.text_rectangle.map(|rectangle| Rect {
            x: rectangle.left * text_scale_x,
            y: rectangle.top * text_scale_y,
            width: (rectangle.right - rectangle.left) * text_scale_x,
            height: (rectangle.bottom - rectangle.top) * text_scale_y,
        });
        Ok(ResolvedGeometry::Custom { paths, text_rect })
    }

    fn concrete_preset_geometry(
        &self,
        geometry: &oxoffice_drawing::geometry::CT_PresetGeometry2D,
        size: (f64, f64),
    ) -> Result<Option<ResolvedGeometry>, ResolveError> {
        let evaluated = geometry
            .evaluate(size)
            .map_err(|error| ResolveError::ConcreteValue {
                kind: "preset geometry",
                detail: error.to_string(),
            })?;
        Ok(evaluated.map(|evaluated| {
            let paths = evaluated
                .paths
                .into_iter()
                .map(|commands| Path {
                    commands: commands
                        .into_iter()
                        .map(|command| concrete_path_command(command, 1.0, 1.0))
                        .collect(),
                    fill_rule: FillRule::NonZero,
                })
                .collect();
            let text_rect = evaluated.text_rectangle.map(|rectangle| Rect {
                x: rectangle.left,
                y: rectangle.top,
                width: rectangle.right - rectangle.left,
                height: rectangle.bottom - rectangle.top,
            });
            ResolvedGeometry::Custom { paths, text_rect }
        }))
    }

    fn resolve_text_body(
        &self,
        shape: &CT_Shape,
        body: &CT_TextBody,
        source: FlattenedSource,
        hyperlinks: Option<&ScopedHyperlinkTargets>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(ResolvedTextBody, Vec<oxoffice_layout::TextDirection>), ResolveError> {
        let properties = self.effective_body_pr(shape);
        let slide_number_placeholder = self.is_slide_number_placeholder(shape);
        let resolved = body
            .paragraphs()
            .iter()
            .map(|paragraph| {
                self.resolve_paragraph(
                    shape,
                    paragraph,
                    source,
                    hyperlinks,
                    slide_number_placeholder,
                    diagnostics,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (paragraphs, directions) = resolved.into_iter().unzip();
        Ok((resolved_text_body(&properties, paragraphs)?, directions))
    }

    fn resolve_paragraph(
        &self,
        shape: &CT_Shape,
        paragraph: &oxoffice_drawing::text::CT_TextParagraph,
        source: FlattenedSource,
        hyperlinks: Option<&ScopedHyperlinkTargets>,
        slide_number_placeholder: bool,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(ResolvedParagraph, oxoffice_layout::TextDirection), ResolveError> {
        let effective = self.effective_text_properties(shape, paragraph.properties.as_ref(), None);
        let paragraph_properties = &effective.paragraph;
        let mut runs = Vec::new();
        for run in &paragraph.runs {
            match run {
                TextRun::Run(run) => {
                    let mut style = self.resolve_run_style(
                        shape,
                        paragraph.properties.as_ref(),
                        run.properties.as_ref(),
                    )?;
                    style.hyperlink_url = resolve_direct_hyperlink(
                        run.properties.as_ref(),
                        source,
                        hyperlinks,
                        diagnostics,
                    );
                    runs.push(ResolvedTextRun::Text {
                        text: run.text.value.clone(),
                        style,
                    });
                }
                TextRun::Break(_) => runs.push(ResolvedTextRun::Break),
                TextRun::Field(field) => {
                    if field.text.is_none()
                        && field.field_type.as_deref().is_some_and(|kind| {
                            kind == "datetime"
                                || (1..=13).any(|index| kind == format!("datetime{index}"))
                        })
                    {
                        diagnostics.push(Diagnostic { message: "uncached automatic date field has no computed date for static rendering".to_owned() });
                    }
                    let mut style = self.resolve_run_style(
                        shape,
                        paragraph.properties.as_ref(),
                        field.run_properties.as_ref(),
                    )?;
                    style.hyperlink_url = resolve_direct_hyperlink(
                        field.run_properties.as_ref(),
                        source,
                        hyperlinks,
                        diagnostics,
                    );
                    runs.push(ResolvedTextRun::Field {
                        text: field
                            .text
                            .as_ref()
                            .map(|text| text.value.clone())
                            .unwrap_or_default(),
                        field_type: field
                            .field_type
                            .clone()
                            .or_else(|| slide_number_placeholder.then(|| "slidenum".to_owned())),
                        style,
                    });
                }
            }
        }
        let direction = paragraph_base_direction(paragraph_properties.right_to_left);
        Ok((
            ResolvedParagraph {
                level: paragraph_properties.level.unwrap_or(0),
                left_margin: emu_to_points(i64::from(
                    paragraph_properties.left_margin.unwrap_or(0),
                )),
                right_margin: emu_to_points(i64::from(
                    paragraph_properties.right_margin.unwrap_or(0),
                )),
                indent: emu_to_points(i64::from(paragraph_properties.indent.unwrap_or(0))),
                alignment: paragraph_alignment(paragraph_properties.alignment),
                line_spacing: resolved_text_spacing(paragraph_properties.line_spacing.as_ref()),
                space_before: resolved_text_spacing(paragraph_properties.space_before.as_ref()),
                space_after: resolved_text_spacing(paragraph_properties.space_after.as_ref()),
                bullet: paragraph_properties
                    .bullet
                    .as_ref()
                    .and_then(|bullet| self.resolve_bullet(bullet).transpose())
                    .transpose()?,
                end_style: self.resolve_run_style(
                    shape,
                    paragraph.properties.as_ref(),
                    paragraph.end_properties.as_ref(),
                )?,
                runs,
            },
            direction,
        ))
    }

    fn resolve_run_style(
        &self,
        shape: &CT_Shape,
        paragraph: Option<&oxoffice_drawing::text::CT_TextParagraphProperties>,
        run: Option<&CT_TextCharacterProperties>,
    ) -> Result<ResolvedRunStyle, ResolveError> {
        let effective = self.effective_text_properties(shape, paragraph, run);
        self.resolved_run_style(effective.run, effective.language)
    }

    fn resolve_table_run_style(
        &self,
        table_style: &CT_TextCharacterProperties,
        paragraph: Option<&oxoffice_drawing::text::CT_TextParagraphProperties>,
        run: Option<&CT_TextCharacterProperties>,
    ) -> Result<ResolvedRunStyle, ResolveError> {
        let effective = self.effective_table_text_properties(Some(table_style), paragraph, run);
        self.resolved_run_style(effective.run, effective.language)
    }

    fn resolved_run_style(
        &self,
        effective: CT_TextCharacterProperties,
        language: Option<String>,
    ) -> Result<ResolvedRunStyle, ResolveError> {
        let fill = effective
            .fill
            .as_ref()
            .map(|fill| self.concrete_fill(fill, (1.0, 1.0)))
            .transpose()?
            .and_then(|(paint, _)| paint);
        Ok(ResolvedRunStyle {
            font_size: effective.font_size.map(|size| f64::from(size) / 100.0),
            bold: effective.bold.unwrap_or(false),
            italic: effective.italic.unwrap_or(false),
            all_caps: effective.all_caps.unwrap_or(false),
            small_caps: effective.small_caps.unwrap_or(false),
            language,
            highlight: effective
                .highlight
                .as_ref()
                .map(|colour| self.concrete_color(colour))
                .transpose()?,
            underline: effective
                .underline
                .is_some_and(|value| value != oxoffice_drawing::text::TextUnderline::None),
            strike: effective
                .strike
                .is_some_and(|value| value != oxoffice_drawing::text::TextStrike::None),
            spacing: effective.spacing.as_ref().and_then(text_point_value),
            baseline: effective.baseline.as_deref().and_then(parse_percent),
            fill,
            latin_typeface: effective
                .latin
                .as_ref()
                .map(|font| self.resolve_typeface(&font.typeface, None)),
            east_asian_typeface: effective
                .east_asian
                .as_ref()
                .map(|font| self.resolve_typeface(&font.typeface, None)),
            complex_script_typeface: effective
                .complex_script
                .as_ref()
                .map(|font| self.resolve_typeface(&font.typeface, None)),
            symbol_typeface: effective
                .symbol
                .as_ref()
                .map(|font| self.resolve_typeface(&font.typeface, None)),
            hyperlink_url: None,
        })
    }

    fn resolve_bullet(
        &self,
        bullet: &oxoffice_drawing::text::TextBullet,
    ) -> Result<Option<ResolvedBullet>, ResolveError> {
        let color = bullet
            .color
            .as_ref()
            .map(|color| self.concrete_color(&color.color))
            .transpose()?;
        let font = bullet
            .font
            .as_ref()
            .map(|font| self.resolve_typeface(&font.typeface, None));
        let size = bullet.size.as_ref().and_then(|size| match &size.value {
            TextBulletSizeValue::Percent(value) => {
                parse_percent(value).map(ResolvedBulletSize::Percent)
            }
            TextBulletSizeValue::Points(value) => {
                Some(ResolvedBulletSize::Points(f64::from(*value) / 100.0))
            }
        });
        Ok(match bullet.choice.as_ref() {
            Some(TextBulletChoice::Character(character)) => Some(ResolvedBullet::Character {
                character: character.character.clone(),
                font,
                color,
                size,
            }),
            Some(TextBulletChoice::AutoNumber(number)) => Some(ResolvedBullet::AutoNumber {
                scheme: number.scheme.as_str().to_owned(),
                start_at: u32::from(number.start_at.unwrap_or(1)),
                font,
                color,
                size,
            }),
            Some(TextBulletChoice::None(_)) | None => None,
        })
    }

    fn resolve_table(
        &self,
        table: &oxoffice_drawing::table::CT_Table,
        source: FlattenedSource,
        hyperlinks: Option<&ScopedHyperlinkTargets>,
        diagnostics: &mut Vec<Diagnostic>,
        text_directions: &mut Vec<Vec<oxoffice_layout::TextDirection>>,
    ) -> Result<ResolvedTable, ResolveError> {
        let column_widths = table
            .grid
            .columns
            .iter()
            .map(|width| emu_to_points(width.0))
            .collect::<Vec<_>>();
        let selected_id = table
            .properties
            .as_ref()
            .and_then(|properties| properties.style_id.as_deref());
        let explicit_style = self.table_styles.and_then(|styles| {
            selected_id.and_then(|id| styles.styles.iter().find(|style| style.style_id == id))
        });
        let default_id = self
            .table_styles
            .and_then(|styles| styles.default_style_id.as_deref());
        let default_style = self.table_styles.and_then(|styles| {
            default_id.and_then(|id| styles.styles.iter().find(|style| style.style_id == id))
        });
        let builtin_style = if explicit_style.is_none() {
            selected_id.and_then(CT_TableStyle::builtin)
        } else {
            None
        };
        let builtin_default = if selected_id.is_none() && default_style.is_none() {
            default_id.and_then(CT_TableStyle::builtin)
        } else {
            None
        };
        let style = explicit_style
            .or(builtin_style.as_ref())
            .or(default_style)
            .or(builtin_default.as_ref());
        let row_count = table.rows.len();
        let column_count = table.grid.columns.len();
        let table_properties = table.properties.as_ref();
        let rows = table
            .rows
            .iter()
            .enumerate()
            .map(|(row_index, row)| {
                let cells = row
                    .cells
                    .iter()
                    .enumerate()
                    .map(|(column_index, cell)| {
                        let mut cascade = TableCellCascade::default();
                        let position = TableCellPosition {
                            row: row_index,
                            column: column_index,
                            row_count,
                            column_count,
                        };
                        if let Some(style) = style {
                            let mut priority = 1u8;
                            cascade.apply_region(
                                style.whole_table.as_deref(),
                                true,
                                position,
                                priority,
                            );
                            priority += 1;
                            if table_properties.is_some_and(|properties| properties.band_rows) {
                                let offset = usize::from(
                                    table_properties.is_some_and(|properties| properties.first_row),
                                );
                                let region = if row_index.saturating_sub(offset) % 2 == 0 {
                                    style.band1_horizontal.as_deref()
                                } else {
                                    style.band2_horizontal.as_deref()
                                };
                                cascade.apply_region(region, false, position, priority);
                                priority += 1;
                            }
                            if table_properties.is_some_and(|properties| properties.band_columns) {
                                let offset = usize::from(
                                    table_properties
                                        .is_some_and(|properties| properties.first_column),
                                );
                                let region = if column_index.saturating_sub(offset) % 2 == 0 {
                                    style.band1_vertical.as_deref()
                                } else {
                                    style.band2_vertical.as_deref()
                                };
                                cascade.apply_region(region, false, position, priority);
                                priority += 1;
                            }
                            if column_index == 0
                                && table_properties
                                    .is_some_and(|properties| properties.first_column)
                            {
                                cascade.apply_region(
                                    style.first_column.as_deref(),
                                    false,
                                    position,
                                    priority,
                                );
                                priority += 1;
                            }
                            if column_index + 1 == column_count
                                && table_properties.is_some_and(|properties| properties.last_column)
                            {
                                cascade.apply_region(
                                    style.last_column.as_deref(),
                                    false,
                                    position,
                                    priority,
                                );
                                priority += 1;
                            }
                            if row_index == 0
                                && table_properties.is_some_and(|properties| properties.first_row)
                            {
                                cascade.apply_region(
                                    style.first_row.as_deref(),
                                    false,
                                    position,
                                    priority,
                                );
                                priority += 1;
                            }
                            if row_index + 1 == row_count
                                && table_properties.is_some_and(|properties| properties.last_row)
                            {
                                cascade.apply_region(
                                    style.last_row.as_deref(),
                                    false,
                                    position,
                                    priority,
                                );
                                priority += 1;
                            }
                            let first_row =
                                table_properties.is_some_and(|properties| properties.first_row);
                            let last_row =
                                table_properties.is_some_and(|properties| properties.last_row);
                            let first_column =
                                table_properties.is_some_and(|properties| properties.first_column);
                            let last_column =
                                table_properties.is_some_and(|properties| properties.last_column);
                            let corner = match (row_index, column_index) {
                                (0, 0) if first_row && first_column => {
                                    style.north_west_cell.as_deref()
                                }
                                (0, column)
                                    if column + 1 == column_count && first_row && last_column =>
                                {
                                    style.north_east_cell.as_deref()
                                }
                                (row, 0) if row + 1 == row_count && last_row && first_column => {
                                    style.south_west_cell.as_deref()
                                }
                                (row, column)
                                    if row + 1 == row_count
                                        && column + 1 == column_count
                                        && last_row
                                        && last_column =>
                                {
                                    style.south_east_cell.as_deref()
                                }
                                _ => None,
                            };
                            cascade.apply_region(corner, false, position, priority);
                        }
                        if let Some(properties) = &cell.properties {
                            cascade.apply_direct(properties);
                            for unsupported in &properties.unsupported {
                                push_table_diagnostic(diagnostics, unsupported);
                            }
                        }
                        for unsupported in &cascade.unsupported {
                            push_table_diagnostic(diagnostics, unsupported);
                        }
                        let referenced_fill = cascade
                            .fill_reference
                            .as_ref()
                            .map(|reference| self.table_referenced_fill(reference))
                            .transpose()?
                            .flatten();
                        let (fill, fill_unsupported) = referenced_fill
                            .as_ref()
                            .or(cascade.fill.as_ref())
                            .map(|fill| self.concrete_fill(fill, (1.0, 1.0)))
                            .transpose()?
                            .unwrap_or((None, None));
                        if let Some(unsupported) = fill_unsupported {
                            push_table_diagnostic(diagnostics, unsupported);
                        }
                        let table_text_style = table_character_properties(&cascade.text_style)?;
                        let mut text = if let Some(body) = cell.text_body.as_ref() {
                            let (body, directions) = resolve_standalone_text_body(
                                self,
                                body,
                                &table_text_style,
                                source,
                                hyperlinks,
                                diagnostics,
                            )?;
                            text_directions.push(directions);
                            Some(body)
                        } else {
                            text_directions.push(Vec::new());
                            None
                        };
                        if let Some(body) = text.as_mut()
                            && let Some(anchor) = cell
                                .properties
                                .as_ref()
                                .and_then(|properties| properties.anchor)
                        {
                            body.anchor = resolved_anchor(anchor);
                        }
                        if let Some(body) = text.as_mut()
                            && body.autofit == ResolvedAutofit::Shape
                        {
                            body.autofit = ResolvedAutofit::None;
                            let message = "table cell autofit is unsupported and was ignored";
                            if !diagnostics
                                .iter()
                                .any(|diagnostic| diagnostic.message == message)
                            {
                                diagnostics.push(Diagnostic {
                                    message: message.to_owned(),
                                });
                            }
                        }
                        Ok(ResolvedTableCell {
                            text,
                            fill,
                            margins: TextInsets {
                                left: emu_to_points(cascade.margin_left.unwrap_or(91_440)),
                                top: emu_to_points(cascade.margin_top.unwrap_or(45_720)),
                                right: emu_to_points(cascade.margin_right.unwrap_or(91_440)),
                                bottom: emu_to_points(cascade.margin_bottom.unwrap_or(45_720)),
                            },
                            left: self.resolve_table_border(cascade.left, diagnostics)?,
                            right: self.resolve_table_border(cascade.right, diagnostics)?,
                            top: self.resolve_table_border(cascade.top, diagnostics)?,
                            bottom: self.resolve_table_border(cascade.bottom, diagnostics)?,
                            row_span: cell.row_span,
                            grid_span: cell.grid_span,
                            horizontal_merge: cell.horizontal_merge,
                            vertical_merge: cell.vertical_merge,
                        })
                    })
                    .collect::<Result<Vec<_>, ResolveError>>()?;
                Ok(ResolvedTableRow {
                    height: emu_to_points(row.height.0),
                    cells,
                })
            })
            .collect::<Result<Vec<_>, ResolveError>>()?;
        let background = match style.and_then(|style| style.background.as_deref()) {
            Some(background) => {
                let size = (
                    column_widths.iter().sum(),
                    rows.iter().map(|row| row.height).sum(),
                );
                self.resolve_table_background(background, size, diagnostics)?
            }
            None => None,
        };
        Ok(ResolvedTable {
            right_to_left: table_properties.is_some_and(|properties| properties.right_to_left),
            column_widths,
            rows,
            background,
        })
    }

    /// Resolves a table style's `a:tblBg` fill over the whole table box.
    fn resolve_table_background(
        &self,
        background: &CT_TableBackgroundStyle,
        size: (f64, f64),
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<Option<Paint>, ResolveError> {
        let referenced = background
            .fill_reference
            .as_ref()
            .map(|reference| self.table_referenced_fill(reference))
            .transpose()?
            .flatten();
        let (paint, unsupported) = referenced
            .as_ref()
            .or(background.fill.as_ref())
            .map(|fill| self.concrete_fill(fill, size))
            .transpose()?
            .unwrap_or((None, None));
        for unsupported in background
            .unsupported
            .iter()
            .map(String::as_str)
            .chain(unsupported)
        {
            let message = format!("unsupported table background {unsupported} was ignored");
            if !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == message)
            {
                diagnostics.push(Diagnostic { message });
            }
        }
        Ok(paint)
    }

    fn resolve_table_border(
        &self,
        border: Option<(CT_LineProperties, u8)>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<Option<ResolvedTableBorder>, ResolveError> {
        border
            .map(|(line, priority)| {
                let stroke = self.concrete_line(&line, (1.0, 1.0))?;
                if stroke.is_none()
                    && let Some(fill) = &line.fill
                {
                    let (_, unsupported) = self.concrete_fill(fill, (1.0, 1.0))?;
                    if let Some(unsupported) = unsupported {
                        push_table_diagnostic(diagnostics, unsupported);
                    }
                }
                Ok(ResolvedTableBorder { stroke, priority })
            })
            .transpose()
    }

    fn table_referenced_fill(
        &self,
        reference: &StyleReference,
    ) -> Result<Option<Fill>, ResolveError> {
        let StyleReference::Fill(reference) = reference else {
            return Ok(None);
        };
        let matrix = &self.theme.theme_elements.format_scheme;
        let Some(mut fill) = referenced_fill(
            reference.index,
            &matrix.fill_styles,
            &matrix.background_fill_styles,
        )?
        else {
            return Ok(None);
        };
        substitute_fill(&mut fill, reference.color.as_ref(), "table cell")?;
        Ok(Some(fill))
    }
}

#[derive(Default)]
struct TableCellCascade {
    fill: Option<Fill>,
    fill_reference: Option<StyleReference>,
    left: Option<(CT_LineProperties, u8)>,
    right: Option<(CT_LineProperties, u8)>,
    top: Option<(CT_LineProperties, u8)>,
    bottom: Option<(CT_LineProperties, u8)>,
    margin_left: Option<i64>,
    margin_right: Option<i64>,
    margin_top: Option<i64>,
    margin_bottom: Option<i64>,
    text_style: TableTextCascade,
    unsupported: Vec<String>,
}

#[derive(Default)]
struct TableTextCascade {
    bold: Option<bool>,
    italic: Option<bool>,
    font_collection: Option<FontCollectionIndex>,
    color: Option<ColorChoice>,
}

#[derive(Clone, Copy)]
struct TableCellPosition {
    row: usize,
    column: usize,
    row_count: usize,
    column_count: usize,
}

fn table_character_properties(
    style: &TableTextCascade,
) -> Result<CT_TextCharacterProperties, ResolveError> {
    let mut properties = CT_TextCharacterProperties::default();
    properties.bold = style.bold;
    properties.italic = style.italic;
    if let Some(color) = &style.color {
        let mut solid = SolidFill::default();
        solid.color = Some(color.clone());
        properties.fill = Some(Fill::Solid(solid));
    }
    let font_tokens = match style.font_collection {
        Some(FontCollectionIndex::Major) => Some(("+mj-lt", "+mj-ea", "+mj-cs")),
        Some(FontCollectionIndex::Minor) => Some(("+mn-lt", "+mn-ea", "+mn-cs")),
        Some(FontCollectionIndex::None) | None => None,
    };
    if let Some((latin, east_asian, complex_script)) = font_tokens {
        let font = |typeface| {
            TextFont::new(typeface).map_err(|error| ResolveError::ConcreteValue {
                kind: "table font",
                detail: error.to_string(),
            })
        };
        properties.latin = Some(font(latin)?);
        properties.east_asian = Some(font(east_asian)?);
        properties.complex_script = Some(font(complex_script)?);
    }
    Ok(properties)
}

impl TableCellCascade {
    fn apply_region(
        &mut self,
        region: Option<&CT_TablePartStyle>,
        whole_table: bool,
        position: TableCellPosition,
        priority: u8,
    ) {
        let Some(region) = region else {
            return;
        };
        if let Some(cell_style) = &region.cell_style {
            self.apply_cell_style(cell_style, whole_table, position, priority);
        }
        if let Some(text_style) = &region.text_style {
            self.apply_text_style(text_style);
        }
    }

    fn apply_cell_style(
        &mut self,
        style: &CT_TableCellStyle,
        whole_table: bool,
        position: TableCellPosition,
        priority: u8,
    ) {
        if let Some(fill) = &style.fill {
            self.fill = Some(fill.clone());
            self.fill_reference = None;
        }
        if let Some(reference) = &style.fill_reference {
            self.fill_reference = Some(reference.clone());
            self.fill = None;
        }
        if let Some(borders) = &style.borders {
            self.apply_borders(borders, whole_table, position, priority);
            self.unsupported.extend(borders.unsupported.iter().cloned());
        }
        self.unsupported.extend(style.unsupported.iter().cloned());
    }

    fn apply_borders(
        &mut self,
        borders: &CT_TableBorders,
        whole_table: bool,
        position: TableCellPosition,
        priority: u8,
    ) {
        let left = if whole_table {
            if position.column > 0 {
                borders.inside_vertical.as_ref()
            } else {
                borders.left.as_ref()
            }
        } else {
            borders.left.as_ref().or(borders.inside_vertical.as_ref())
        };
        let right = if whole_table {
            if position.column + 1 < position.column_count {
                borders.inside_vertical.as_ref()
            } else {
                borders.right.as_ref()
            }
        } else {
            borders.right.as_ref().or(borders.inside_vertical.as_ref())
        };
        let top = if whole_table {
            if position.row > 0 {
                borders.inside_horizontal.as_ref()
            } else {
                borders.top.as_ref()
            }
        } else {
            borders.top.as_ref().or(borders.inside_horizontal.as_ref())
        };
        let bottom = if whole_table {
            if position.row + 1 < position.row_count {
                borders.inside_horizontal.as_ref()
            } else {
                borders.bottom.as_ref()
            }
        } else {
            borders
                .bottom
                .as_ref()
                .or(borders.inside_horizontal.as_ref())
        };
        apply_table_edge(&mut self.left, left, priority);
        apply_table_edge(&mut self.right, right, priority);
        apply_table_edge(&mut self.top, top, priority);
        apply_table_edge(&mut self.bottom, bottom, priority);
    }

    fn apply_text_style(&mut self, style: &CT_TableTextStyle) {
        if style.bold.is_some() {
            self.text_style.bold = style.bold;
        }
        if style.italic.is_some() {
            self.text_style.italic = style.italic;
        }
        if let Some(StyleReference::Font(reference)) = &style.font_reference {
            self.text_style.font_collection = Some(reference.index);
            if reference.color.is_some() {
                self.text_style.color = reference.color.clone();
            }
        }
        if style.color.is_some() {
            self.text_style.color = style.color.clone();
        }
    }

    fn apply_direct(&mut self, properties: &oxoffice_drawing::table::CT_TableCellProperties) {
        if let Some(fill) = &properties.fill {
            self.fill = Some(fill.clone());
            self.fill_reference = None;
        }
        if let Some(value) = properties.margin_left {
            self.margin_left = Some(value.0);
        }
        if let Some(value) = properties.margin_right {
            self.margin_right = Some(value.0);
        }
        if let Some(value) = properties.margin_top {
            self.margin_top = Some(value.0);
        }
        if let Some(value) = properties.margin_bottom {
            self.margin_bottom = Some(value.0);
        }
        apply_table_edge(&mut self.left, properties.left.as_ref(), u8::MAX);
        apply_table_edge(&mut self.right, properties.right.as_ref(), u8::MAX);
        apply_table_edge(&mut self.top, properties.top.as_ref(), u8::MAX);
        apply_table_edge(&mut self.bottom, properties.bottom.as_ref(), u8::MAX);
    }
}

fn apply_table_edge(
    target: &mut Option<(CT_LineProperties, u8)>,
    source: Option<&CT_LineProperties>,
    priority: u8,
) {
    if let Some(source) = source {
        *target = Some((source.clone(), priority));
    }
}

fn push_table_diagnostic(diagnostics: &mut Vec<Diagnostic>, unsupported: &str) {
    let message = format!("unsupported table cell {unsupported} was ignored");
    if !diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message == message)
    {
        diagnostics.push(Diagnostic { message });
    }
}

fn push_group_diagnostics(issues: u8, diagnostics: &mut Vec<Diagnostic>) {
    let messages = [
        (
            GROUP_ZERO_X,
            "zero horizontal group child extent used finite unit scale",
        ),
        (
            GROUP_ZERO_Y,
            "zero vertical group child extent used finite unit scale",
        ),
        (
            GROUP_SHEAR,
            "unsupported sheared or singular group transform retained as affine fallback",
        ),
    ];
    for (flag, message) in messages {
        if issues & flag != 0
            && !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == message)
        {
            diagnostics.push(Diagnostic {
                message: message.to_owned(),
            });
        }
    }
}

fn scaled_group_bounds(bounds: Rect, scale: (f64, f64)) -> Rect {
    Rect {
        x: bounds.x * scale.0,
        y: bounds.y * scale.1,
        width: bounds.width * scale.0,
        height: bounds.height * scale.1,
    }
}

fn transform_values(transform: Option<&CT_Transform2D>) -> Option<(Rect, f64, bool, bool)> {
    transform_values_with_degenerate_line(transform, false)
}

fn connector_transform_values(
    transform: Option<&CT_Transform2D>,
) -> Option<(Rect, f64, bool, bool)> {
    transform_values_with_degenerate_line(transform, true)
}

fn transform_values_with_degenerate_line(
    transform: Option<&CT_Transform2D>,
    allow_degenerate_line: bool,
) -> Option<(Rect, f64, bool, bool)> {
    let transform = transform?;
    let extent = transform.extent?;
    let invalid = if allow_degenerate_line {
        extent.cx.0 < 0 || extent.cy.0 < 0 || (extent.cx.0 == 0 && extent.cy.0 == 0)
    } else {
        extent.cx.0 <= 0 || extent.cy.0 <= 0
    };
    if invalid {
        return None;
    }
    let offset = transform.offset.unwrap_or_default();
    Some((
        Rect {
            x: emu_to_points(offset.x.0),
            y: emu_to_points(offset.y.0),
            width: emu_to_points(extent.cx.0),
            height: emu_to_points(extent.cy.0),
        },
        f64::from(transform.rotation.0) / 60_000.0,
        transform.flip_horizontal,
        transform.flip_vertical,
    ))
}

fn emu_to_points(value: i64) -> f64 {
    value as f64 / 12_700.0
}

fn base_rgb(choice: &ColorChoice) -> Option<RgbColor> {
    let resolved = resolve_color(choice, &ColorMap::default(), &[]).ok()?;
    Some(RgbColor::new(resolved.red, resolved.green, resolved.blue))
}

fn concrete_path_command(command: EvaluatedPathCommand, scale_x: f64, scale_y: f64) -> PathCommand {
    match command {
        EvaluatedPathCommand::MoveTo { x, y } => PathCommand::MoveTo(Point {
            x: x * scale_x,
            y: y * scale_y,
        }),
        EvaluatedPathCommand::LineTo { x, y } => PathCommand::LineTo(Point {
            x: x * scale_x,
            y: y * scale_y,
        }),
        EvaluatedPathCommand::CubicTo {
            x1,
            y1,
            x2,
            y2,
            x,
            y,
        } => PathCommand::CurveTo {
            c1: Point {
                x: x1 * scale_x,
                y: y1 * scale_y,
            },
            c2: Point {
                x: x2 * scale_x,
                y: y2 * scale_y,
            },
            to: Point {
                x: x * scale_x,
                y: y * scale_y,
            },
        },
        EvaluatedPathCommand::Close => PathCommand::Close,
    }
}

fn coordinate_points(value: Option<&Coordinate32Value>) -> Result<f64, ResolveError> {
    match value {
        Some(Coordinate32Value::Emu(value)) => Ok(emu_to_points(i64::from(*value))),
        Some(Coordinate32Value::UniversalMeasure(value)) => universal_measure_points(value),
        None => Ok(0.0),
    }
}

fn universal_measure_points(value: &str) -> Result<f64, ResolveError> {
    let split = value
        .find(|character: char| character.is_ascii_alphabetic())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number = number
        .parse::<f64>()
        .map_err(|error| ResolveError::ConcreteValue {
            kind: "universal measure",
            detail: error.to_string(),
        })?;
    match unit {
        "pt" => Ok(number),
        "pc" | "pi" => Ok(number * 12.0),
        "in" => Ok(number * 72.0),
        "cm" => Ok(number * 72.0 / 2.54),
        "mm" => Ok(number * 72.0 / 25.4),
        _ => Err(ResolveError::ConcreteValue {
            kind: "universal measure",
            detail: format!("unsupported unit {unit}"),
        }),
    }
}

fn parse_percent(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().map(|value| value / 100_000.0)
}

fn text_point_value(value: &TextPointValue) -> Option<f64> {
    match value {
        TextPointValue::Centipoints(value) => Some(f64::from(*value) / 100.0),
        TextPointValue::UniversalMeasure(value) => universal_measure_points(value).ok(),
    }
}

fn resolved_text_spacing(value: Option<&TextSpacing>) -> Option<ResolvedTextSpacing> {
    match value? {
        TextSpacing::Percent(value) => parse_percent(value).map(ResolvedTextSpacing::Percent),
        TextSpacing::Points(value) => Some(ResolvedTextSpacing::Points(f64::from(*value) / 100.0)),
    }
}

fn paragraph_alignment(alignment: Option<TextAlignment>) -> ParagraphAlignment {
    match alignment.unwrap_or(TextAlignment::Left) {
        TextAlignment::Left => ParagraphAlignment::Left,
        TextAlignment::Center => ParagraphAlignment::Center,
        TextAlignment::Right => ParagraphAlignment::Right,
        TextAlignment::Justified | TextAlignment::JustifiedLow => ParagraphAlignment::Justified,
        TextAlignment::Distributed | TextAlignment::ThaiDistributed => {
            ParagraphAlignment::Distributed
        }
    }
}

fn paragraph_base_direction(right_to_left: Option<bool>) -> oxoffice_layout::TextDirection {
    match right_to_left {
        Some(true) => oxoffice_layout::TextDirection::RightToLeft,
        Some(false) => oxoffice_layout::TextDirection::LeftToRight,
        None => oxoffice_layout::TextDirection::Auto,
    }
}

fn vertical_text_diagnostic(direction: TextDirection) -> Option<&'static str> {
    match direction {
        TextDirection::EastAsianVertical => {
            Some("east Asian vertical text rendered as rotated vertical text")
        }
        TextDirection::MongolianVertical => {
            Some("Mongolian vertical text rendered as rotated vertical-270 text")
        }
        TextDirection::WordArtVertical => {
            Some("WordArt vertical text rendered as rotated vertical text")
        }
        TextDirection::WordArtVerticalRtl => {
            Some("right-to-left WordArt vertical text rendered as rotated vertical-270 text")
        }
        TextDirection::Horizontal | TextDirection::Vertical | TextDirection::Vertical270 => None,
    }
}

fn resolve_standalone_text_body(
    context: &ResolveCtx<'_>,
    body: &CT_TextBody,
    table_style: &CT_TextCharacterProperties,
    source: FlattenedSource,
    hyperlinks: Option<&ScopedHyperlinkTargets>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(ResolvedTextBody, Vec<oxoffice_layout::TextDirection>), ResolveError> {
    let mut properties = default_body_properties();
    merge_body_properties(&mut properties, &body.body_properties);
    let resolved = body
        .paragraphs()
        .iter()
        .map(|paragraph| {
            let effective = context.effective_table_text_properties(
                Some(table_style),
                paragraph.properties.as_ref(),
                None,
            );
            let paragraph_properties = &effective.paragraph;
            let runs = paragraph
                .runs
                .iter()
                .map(|run| match run {
                    TextRun::Run(run) => {
                        let mut style = context.resolve_table_run_style(
                            table_style,
                            paragraph.properties.as_ref(),
                            run.properties.as_ref(),
                        )?;
                        style.hyperlink_url = resolve_direct_hyperlink(
                            run.properties.as_ref(),
                            source,
                            hyperlinks,
                            diagnostics,
                        );
                        Ok(ResolvedTextRun::Text {
                            text: run.text.value.clone(),
                            style,
                        })
                    }
                    TextRun::Break(_) => Ok(ResolvedTextRun::Break),
                    TextRun::Field(field) => {
if field.text.is_none()
                        && field.field_type.as_deref().is_some_and(|kind| kind == "datetime" || (1..=13).any(|index| kind == format!("datetime{index}")))
                    {
                        diagnostics.push(Diagnostic { message: "uncached automatic date field has no computed date for static rendering".to_owned() });
                    }
                        let mut style = context.resolve_table_run_style(
                            table_style,
                            paragraph.properties.as_ref(),
                            field.run_properties.as_ref(),
                        )?;
                        style.hyperlink_url = resolve_direct_hyperlink(
                            field.run_properties.as_ref(),
                            source,
                            hyperlinks,
                            diagnostics,
                        );
                        Ok(ResolvedTextRun::Field {
                            text: field
                                .text
                                .as_ref()
                                .map(|text| text.value.clone())
                                .unwrap_or_default(),
                            field_type: field.field_type.clone(),
                            style,
                        })
                    }
                })
                .collect::<Result<Vec<_>, ResolveError>>()?;
            let direction = paragraph_base_direction(paragraph_properties.right_to_left);
            Ok((
                ResolvedParagraph {
                    level: paragraph_properties.level.unwrap_or(0),
                    left_margin: emu_to_points(i64::from(
                        paragraph_properties.left_margin.unwrap_or(0),
                    )),
                    right_margin: emu_to_points(i64::from(
                        paragraph_properties.right_margin.unwrap_or(0),
                    )),
                    indent: emu_to_points(i64::from(paragraph_properties.indent.unwrap_or(0))),
                    alignment: paragraph_alignment(paragraph_properties.alignment),
                    line_spacing: resolved_text_spacing(paragraph_properties.line_spacing.as_ref()),
                    space_before: resolved_text_spacing(paragraph_properties.space_before.as_ref()),
                    space_after: resolved_text_spacing(paragraph_properties.space_after.as_ref()),
                    bullet: paragraph_properties
                        .bullet
                        .as_ref()
                        .and_then(|bullet| context.resolve_bullet(bullet).transpose())
                        .transpose()?,
                    end_style: context.resolve_table_run_style(
                        table_style,
                        paragraph.properties.as_ref(),
                        paragraph.end_properties.as_ref(),
                    )?,
                    runs,
                },
                direction,
            ))
        })
        .collect::<Result<Vec<_>, ResolveError>>()?;
    let (paragraphs, directions) = resolved.into_iter().unzip();
    Ok((resolved_text_body(&properties, paragraphs)?, directions))
}

fn resolved_anchor(anchor: DrawingTextAnchor) -> TextAnchor {
    match anchor {
        DrawingTextAnchor::Top => TextAnchor::Top,
        DrawingTextAnchor::Center => TextAnchor::Center,
        DrawingTextAnchor::Bottom => TextAnchor::Bottom,
        DrawingTextAnchor::Justified => TextAnchor::Justified,
        DrawingTextAnchor::Distributed => TextAnchor::Distributed,
    }
}

fn resolved_text_body(
    properties: &CT_TextBodyProperties,
    paragraphs: Vec<ResolvedParagraph>,
) -> Result<ResolvedTextBody, ResolveError> {
    let insets = TextInsets {
        left: coordinate_points(properties.left_inset.as_ref())?,
        top: coordinate_points(properties.top_inset.as_ref())?,
        right: coordinate_points(properties.right_inset.as_ref())?,
        bottom: coordinate_points(properties.bottom_inset.as_ref())?,
    };
    let anchor = resolved_anchor(properties.anchor.unwrap_or(DrawingTextAnchor::Top));
    let vertical = match properties
        .vertical
        .unwrap_or(DrawingTextVertical::Horizontal)
    {
        DrawingTextVertical::Horizontal => TextDirection::Horizontal,
        DrawingTextVertical::Vertical => TextDirection::Vertical,
        DrawingTextVertical::Vertical270 => TextDirection::Vertical270,
        DrawingTextVertical::EastAsianVertical => TextDirection::EastAsianVertical,
        DrawingTextVertical::MongolianVertical => TextDirection::MongolianVertical,
        DrawingTextVertical::WordArtVertical => TextDirection::WordArtVertical,
        DrawingTextVertical::WordArtVerticalRtl => TextDirection::WordArtVerticalRtl,
    };
    let autofit = match properties.autofit.clone().unwrap_or(TextAutofit::NoAutofit) {
        TextAutofit::NoAutofit => ResolvedAutofit::None,
        TextAutofit::ShapeAutofit => ResolvedAutofit::Shape,
        TextAutofit::Normal(normal) => ResolvedAutofit::Normal {
            font_scale: normal.font_scale.as_deref().and_then(parse_percent),
            line_spacing_reduction: normal
                .line_spacing_reduction
                .as_deref()
                .and_then(parse_percent),
        },
    };
    Ok(ResolvedTextBody {
        insets,
        anchor,
        wrap: properties.wrap.unwrap_or(TextWrap::Square) != TextWrap::None,
        vertical,
        space_first_last_paragraph: properties.space_first_last_paragraph.unwrap_or(false),
        autofit,
        paragraphs,
    })
}

#[derive(Clone, Copy)]
struct PassRules {
    source: FlattenedSource,
    emit_non_placeholders: bool,
}

fn push_unmatched(keys: &mut Vec<PlaceholderKey>, key: PlaceholderKey) {
    if !keys
        .iter()
        .any(|existing| placeholder_keys_match(&key, existing))
    {
        keys.push(key);
    }
}

fn find_group_lineage(
    children: &[ShapeTreeChild],
    target: &ShapeTreeChild,
    lineage: &[u32],
    found: &mut Vec<u32>,
) -> bool {
    let mut pending = children
        .iter()
        .rev()
        .map(|c| (c, lineage.to_vec()))
        .collect::<Vec<_>>();
    while let Some((child, mut ancestors)) = pending.pop() {
        if std::ptr::eq(child, target) {
            *found = ancestors;
            return true;
        }
        let descendants = match child {
            ShapeTreeChild::GroupShape(group) => {
                if let Some(id) = child.non_visual_id() {
                    ancestors.push(id);
                }
                Some(group.children.as_slice())
            }
            ShapeTreeChild::AlternateContent(alternate) => alternate.selected_fallback(),
            _ => None,
        };
        if let Some(descendants) = descendants {
            pending.extend(descendants.iter().rev().map(|c| (c, ancestors.clone())));
        }
    }
    false
}

// Geometry was materialized at the source extent before timing evaluation.
// Keep its local coordinates and text frame in the same scaled space as bounds.
// The candidate is private, so a failed coordinate never reaches publication.
fn scale_materialized_timeline_geometry(geometry: &mut ResolvedGeometry, x: f64, y: f64) -> bool {
    let ResolvedGeometry::Custom { paths, text_rect } = geometry else {
        return true;
    };
    let point = |point: &mut Point| {
        point.x *= x;
        point.y *= y;
        point.x.is_finite() && point.y.is_finite()
    };
    let paths_finite = paths.iter_mut().all(|path| {
        path.commands.iter_mut().all(|command| match command {
            PathCommand::MoveTo(p) | PathCommand::LineTo(p) => point(p),
            PathCommand::CurveTo { c1, c2, to } => point(c1) && point(c2) && point(to),
            PathCommand::Close => true,
        })
    });
    let text_finite = text_rect.as_mut().is_none_or(|rect| {
        rect.x *= x;
        rect.y *= y;
        rect.width *= x;
        rect.height *= y;
        [rect.x, rect.y, rect.width, rect.height]
            .into_iter()
            .all(f64::is_finite)
    });
    paths_finite && text_finite
}

fn apply_timeline_state(
    mut slide: ResolvedSlide,
    identities: Vec<ResolvedShapeIdentity>,
    mut state: EvaluatedFrameState,
    mut group_geometries: BTreeMap<u32, TimelineGroupGeometry>,
) -> ResolvedTimelineSlide {
    let invalid_targets = state
        .shapes
        .iter()
        .filter_map(|(target, evaluated)| (!evaluated.is_finite()).then_some(*target))
        .collect::<Vec<_>>();
    for target in invalid_targets {
        state.shapes.insert(target, EvaluatedShapeState::default());
        state.diagnostics.push(Diagnostic {
            message: format!("non-finite timeline state ignored for target {target}"),
        });
    }
    let slide_size = slide.size;
    for (group_id, bounds) in timeline_group_bounds(&slide, &identities) {
        group_geometries
            .entry(group_id)
            .or_insert_with(|| TimelineGroupGeometry::from_page_bounds(bounds));
    }
    let mut diagnosed_group_wipes = HashSet::new();
    let shape_states = identities
        .iter()
        .zip(&mut slide.shapes)
        .map(|(identity, shape)| {
            if identity.source != FlattenedSource::Slide {
                return EvaluatedShapeState::default();
            }
            let mut evaluated = EvaluatedShapeState::default();
            let original_bounds = shape.bounds;
            let original_shape = shape.clone();
            let mut group_transform = Transform::IDENTITY;
            let mut group_page_clips = Vec::new();
            for group_id in &identity.containing_group_ids {
                let Some(target_state) = state.shapes.get(group_id) else {
                    continue;
                };
                evaluated.visible &= target_state.visible;
                evaluated.opacity *= target_state.opacity;
                let Some(geometry) = group_geometries.get(group_id).copied() else {
                    continue;
                };
                let target_transform =
                    target_state.resolved_page_transform(geometry.center(), slide_size);
                group_transform = target_transform.then(group_transform);
                if target_state.local_opacity_mask((1.0, 1.0)).is_some()
                    && diagnosed_group_wipes.insert(*group_id)
                {
                    state.diagnostics.push(Diagnostic {
                        message: format!("group-target soft wipe {group_id} retains the flattened hard-clip fallback; soft masks qualify direct leaf owners"),
                    });
                }
                if let Some(clip) = target_state.clip {
                    group_page_clips.push(group_clip_page_points(clip, geometry, group_transform));
                }
            }
            let direct_state = identity.shape_id.and_then(|id| state.shapes.get(&id));
            if let Some(direct_state) = direct_state {
                evaluated.visible &= direct_state.visible;
                evaluated.opacity *= direct_state.opacity;
                evaluated.clip = intersect_optional_rect(evaluated.clip, direct_state.clip);
                evaluated.retain_source_wipe(direct_state);
            }
            let (scale_x, scale_y, rotation_deg) = direct_state.map_or(
                (1.0, 1.0, 0.0),
                EvaluatedShapeState::resolved_shape_geometry,
            );
            let original_page_bounds = resolved_shape_page_bounds(&original_shape);
            let original_page_center = (
                original_page_bounds.x + original_page_bounds.width / 2.0,
                original_page_bounds.y + original_page_bounds.height / 2.0,
            );
            let page_motion = direct_state.map_or(Transform::IDENTITY, |direct_state| {
                direct_state.resolved_motion_translation(original_page_center, slide_size)
            });
            evaluated.transform = page_motion.then(group_transform);
            let animated_bounds = Rect {
                x: original_bounds.x + original_bounds.width * (1.0 - scale_x) / 2.0,
                y: original_bounds.y + original_bounds.height * (1.0 - scale_y) / 2.0,
                width: original_bounds.width * scale_x,
                height: original_bounds.height * scale_y,
            };
            let animated_rotation = shape.rotation_deg + rotation_deg;
            let mut animated_shape = shape.clone();
            animated_shape.bounds = animated_bounds;
            animated_shape.rotation_deg = animated_rotation;
            animated_shape.group_transform = animated_shape
                .group_transform
                .then(page_motion)
                .then(group_transform);
            let geometry_is_finite = scale_materialized_timeline_geometry(
                &mut animated_shape.geometry, scale_x, scale_y,
            );
            if geometry_is_finite {
                for page_clip in group_page_clips {
                    match page_clip_for_shape(page_clip, &animated_shape) {
                        Some(clip) => evaluated.push_oriented_clip(clip),
                        None => state.diagnostics.push(Diagnostic {
                            message: format!(
                                "group timeline clip ignored for shape {}",
                                identity.shape_id.unwrap_or(0)
                            ),
                        }),
                    }
                }
            }
            if geometry_is_finite && evaluated.is_finite()
                && [
                    animated_bounds.x,
                    animated_bounds.y,
                    animated_bounds.width,
                    animated_bounds.height,
                    animated_rotation,
                ]
                .into_iter()
                .all(f64::is_finite)
            {
                *shape = animated_shape;
            } else {
                state.diagnostics.push(Diagnostic {
                    message: format!(
                        "non-finite timeline state ignored for shape {}",
                        identity.shape_id.unwrap_or(0)
                    ),
                });
                evaluated = EvaluatedShapeState::default();
            }
            evaluated
        })
        .collect();
    ResolvedTimelineSlide {
        slide,
        identities,
        shape_states,
        state,
    }
}

#[derive(Clone, Copy)]
struct TimelineGroupGeometry {
    normalized_to_page: Transform,
}

impl TimelineGroupGeometry {
    fn from_page_bounds(bounds: Rect) -> Self {
        Self {
            normalized_to_page: Transform {
                a: bounds.width,
                d: bounds.height,
                e: bounds.x,
                f: bounds.y,
                ..Transform::IDENTITY
            },
        }
    }

    fn center(self) -> (f64, f64) {
        let center = self.normalized_to_page.apply(Point { x: 0.5, y: 0.5 });
        (center.x, center.y)
    }
}

fn timeline_group_geometries(children: &[ShapeTreeChild]) -> BTreeMap<u32, TimelineGroupGeometry> {
    let mut geometries = BTreeMap::new();
    collect_timeline_group_geometries(children, Transform::IDENTITY, &mut geometries);
    geometries
}

fn collect_timeline_group_geometries(
    children: &[ShapeTreeChild],
    parent_to_page: Transform,
    geometries: &mut BTreeMap<u32, TimelineGroupGeometry>,
) {
    let mut pending = children
        .iter()
        .rev()
        .map(|c| (c, parent_to_page))
        .collect::<Vec<_>>();
    while let Some((child, parent)) = pending.pop() {
        match child {
            ShapeTreeChild::GroupShape(group) => {
                let child_to_parent = group
                    .group_transform()
                    .and_then(group_affine)
                    .map_or(Transform::IDENTITY, |(transform, _)| transform);
                if let Some((id, geometry)) = child
                    .non_visual_id()
                    .zip(group.group_transform().and_then(group_extent_geometry))
                {
                    geometries.insert(
                        id,
                        TimelineGroupGeometry {
                            normalized_to_page: geometry.then(parent),
                        },
                    );
                }
                pending.extend(
                    group
                        .children
                        .iter()
                        .rev()
                        .map(|c| (c, child_to_parent.then(parent))),
                );
            }
            ShapeTreeChild::AlternateContent(alternate) => {
                if let Some(fallback) = alternate.selected_fallback() {
                    pending.extend(fallback.iter().rev().map(|c| (c, parent)));
                }
            }
            _ => {}
        }
    }
}

fn group_extent_geometry(transform: &CT_Transform2D) -> Option<Transform> {
    let offset = transform.offset?;
    let extent = transform.extent?;
    let x = emu_to_points(offset.x.0);
    let y = emu_to_points(offset.y.0);
    let width = emu_to_points(extent.cx.0);
    let height = emu_to_points(extent.cy.0);
    let center_x = x + width / 2.0;
    let center_y = y + height / 2.0;
    let mut geometry = Transform {
        a: width,
        d: height,
        e: x,
        f: y,
        ..Transform::IDENTITY
    }
    .then(Transform::rotate_about(
        f64::from(transform.rotation.0) / 60_000.0,
        center_x,
        center_y,
    ));
    if transform.flip_horizontal || transform.flip_vertical {
        geometry = geometry.then(Transform {
            a: if transform.flip_horizontal { -1.0 } else { 1.0 },
            d: if transform.flip_vertical { -1.0 } else { 1.0 },
            e: if transform.flip_horizontal {
                2.0 * center_x
            } else {
                0.0
            },
            f: if transform.flip_vertical {
                2.0 * center_y
            } else {
                0.0
            },
            ..Transform::IDENTITY
        });
    }
    [
        geometry.a, geometry.b, geometry.c, geometry.d, geometry.e, geometry.f,
    ]
    .into_iter()
    .all(f64::is_finite)
    .then_some(geometry)
}

fn timeline_group_bounds(
    slide: &ResolvedSlide,
    identities: &[ResolvedShapeIdentity],
) -> BTreeMap<u32, Rect> {
    let mut bounds = BTreeMap::new();
    for (identity, shape) in identities.iter().zip(&slide.shapes) {
        if identity.source != FlattenedSource::Slide {
            continue;
        }
        let shape_bounds = resolved_shape_page_bounds(shape);
        for group_id in &identity.containing_group_ids {
            bounds
                .entry(*group_id)
                .and_modify(|group_bounds| *group_bounds = union_rect(*group_bounds, shape_bounds))
                .or_insert(shape_bounds);
        }
    }
    bounds
}

fn resolved_shape_page_bounds(shape: &ResolvedShape) -> Rect {
    resolved_shape_page_transform(shape).transform_rect_bbox(Rect {
        x: 0.0,
        y: 0.0,
        width: shape.bounds.width,
        height: shape.bounds.height,
    })
}

fn resolved_shape_page_transform(shape: &ResolvedShape) -> Transform {
    let center_x = shape.bounds.width / 2.0;
    let center_y = shape.bounds.height / 2.0;
    let flip = Transform {
        a: if shape.flip_h { -1.0 } else { 1.0 },
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
        ..Transform::IDENTITY
    };
    Transform::rotate_about(shape.rotation_deg, center_x, center_y)
        .then(flip)
        .then(timeline_translation(shape.bounds.x, shape.bounds.y))
        .then(shape.group_transform)
}

fn group_clip_page_points(
    clip: Rect,
    group_geometry: TimelineGroupGeometry,
    group_transform: Transform,
) -> [Point; 4] {
    let normalized_to_page = group_geometry.normalized_to_page.then(group_transform);
    [
        Point {
            x: clip.x,
            y: clip.y,
        },
        Point {
            x: clip.x + clip.width,
            y: clip.y,
        },
        Point {
            x: clip.x + clip.width,
            y: clip.y + clip.height,
        },
        Point {
            x: clip.x,
            y: clip.y + clip.height,
        },
    ]
    .map(|point| normalized_to_page.apply(point))
}

fn page_clip_for_shape(page_clip: [Point; 4], shape: &ResolvedShape) -> Option<[Point; 4]> {
    if shape.bounds.width.abs() <= f64::EPSILON || shape.bounds.height.abs() <= f64::EPSILON {
        return None;
    }
    let page_to_local = inverse_transform(resolved_shape_page_transform(shape))?;
    Some(page_clip.map(|point| {
        let local = page_to_local.apply(point);
        Point {
            x: local.x / shape.bounds.width,
            y: local.y / shape.bounds.height,
        }
    }))
}

fn inverse_transform(transform: Transform) -> Option<Transform> {
    let determinant = transform.a * transform.d - transform.b * transform.c;
    if !determinant.is_finite() || determinant.abs() <= f64::EPSILON {
        return None;
    }
    let inverse = Transform {
        a: transform.d / determinant,
        b: -transform.b / determinant,
        c: -transform.c / determinant,
        d: transform.a / determinant,
        e: (transform.c * transform.f - transform.d * transform.e) / determinant,
        f: (transform.b * transform.e - transform.a * transform.f) / determinant,
    };
    [
        inverse.a, inverse.b, inverse.c, inverse.d, inverse.e, inverse.f,
    ]
    .into_iter()
    .all(f64::is_finite)
    .then_some(inverse)
}

fn union_rect(left: Rect, right: Rect) -> Rect {
    let x = left.x.min(right.x);
    let y = left.y.min(right.y);
    let far_x = (left.x + left.width).max(right.x + right.width);
    let far_y = (left.y + left.height).max(right.y + right.height);
    Rect {
        x,
        y,
        width: far_x - x,
        height: far_y - y,
    }
}

fn intersect_rect(left: Rect, right: Rect) -> Rect {
    let x = left.x.max(right.x);
    let y = left.y.max(right.y);
    let far_x = (left.x + left.width).min(right.x + right.width);
    let far_y = (left.y + left.height).min(right.y + right.height);
    Rect {
        x,
        y,
        width: (far_x - x).max(0.0),
        height: (far_y - y).max(0.0),
    }
}

fn timeline_translation(x: f64, y: f64) -> Transform {
    Transform {
        e: x,
        f: y,
        ..Transform::IDENTITY
    }
}

fn intersect_optional_rect(left: Option<Rect>, right: Option<Rect>) -> Option<Rect> {
    match (left, right) {
        (None, None) => None,
        (Some(rect), None) | (None, Some(rect)) => Some(rect),
        (Some(left), Some(right)) => Some(intersect_rect(left, right)),
    }
}

fn collect_owned_items<'a>(
    children: &'a [ShapeTreeChild],
    source: FlattenedSource,
    parent: Transform,
    issues: u8,
    path: &[usize],
    output: &mut Vec<(Vec<usize>, FlattenedItem<'a>)>,
) {
    let mut pending = children
        .iter()
        .enumerate()
        .rev()
        .map(|(ordinal, c)| {
            let mut own_path = path.to_vec();
            own_path.push(ordinal);
            (c, parent, issues, own_path)
        })
        .collect::<Vec<_>>();
    while let Some((child, parent, issues, path)) = pending.pop() {
        if let ShapeTreeChild::GroupShape(group) = child {
            let (transform, own_issues) = group
                .group_transform()
                .and_then(group_affine)
                .unwrap_or((Transform::IDENTITY, GROUP_ZERO_X | GROUP_ZERO_Y));
            pending.extend(group.children.iter().enumerate().rev().map(|(ordinal, c)| {
                let mut own_path = path.clone();
                own_path.push(ordinal);
                (c, transform.then(parent), issues | own_issues, own_path)
            }));
        } else {
            let mut leaf = Vec::new();
            push_flattened_shape(&mut leaf, source, child, parent, issues);
            output.push((path, leaf.remove(0)));
        }
    }
}

fn emit_tree<'a>(
    children: &'a [ShapeTreeChild],
    rules: PassRules,
    output: &mut Vec<FlattenedItem<'a>>,
) {
    let mut emitted_latent = Vec::new();
    emit_tree_inner(
        children,
        rules,
        Transform::IDENTITY,
        0,
        &mut emitted_latent,
        output,
    );
}

fn emit_tree_inner<'a>(
    children: &'a [ShapeTreeChild],
    rules: PassRules,
    parent_transform: Transform,
    parent_issues: u8,
    emitted_latent: &mut Vec<PlaceholderKey>,
    output: &mut Vec<FlattenedItem<'a>>,
) {
    let mut pending = children
        .iter()
        .rev()
        .map(|c| (c, parent_transform, parent_issues))
        .collect::<Vec<_>>();
    while let Some((child, parent, inherited_issues)) = pending.pop() {
        if child.hidden() {
            continue;
        }
        match child {
            ShapeTreeChild::GroupShape(group) => {
                let (transform, issues) = group
                    .group_transform()
                    .and_then(group_affine)
                    .unwrap_or((Transform::IDENTITY, 0));
                pending.extend(
                    group
                        .children
                        .iter()
                        .rev()
                        .map(|c| (c, transform.then(parent), inherited_issues | issues)),
                );
            }
            ShapeTreeChild::AlternateContent(alternate) if alternate.chart_choice().is_none() => {
                if let Some(fallback) = alternate.selected_fallback() {
                    pending.extend(fallback.iter().rev().map(|c| (c, parent, inherited_issues)));
                }
            }
            _ => emit_leaf(
                child,
                rules,
                parent,
                inherited_issues,
                emitted_latent,
                output,
            ),
        }
    }
}

fn emit_leaf<'a>(
    child: &'a ShapeTreeChild,
    rules: PassRules,
    group_transform: Transform,
    group_issues: u8,
    emitted_latent: &mut Vec<PlaceholderKey>,
    output: &mut Vec<FlattenedItem<'a>>,
) {
    let placeholder = child_placeholder(child);
    if rules.source == FlattenedSource::Slide {
        if let Some(placeholder) = placeholder {
            let ph_type = placeholder.effective_type();
            if is_latent(&ph_type) {
                let occupied = child_is_occupied(child);
                let already_emitted = emitted_latent
                    .iter()
                    .any(|key| placeholder_keys_match(&placeholder.key(), key));
                if !occupied || already_emitted {
                    return;
                }
                push_unmatched(emitted_latent, placeholder.key());
            }
        }
        push_flattened_shape(output, rules.source, child, group_transform, group_issues);
        return;
    }

    let Some(placeholder) = placeholder else {
        if rules.emit_non_placeholders {
            push_flattened_shape(output, rules.source, child, group_transform, group_issues);
        }
        return;
    };
    // Native occupied master/layout footer shapes are authoring templates,
    // never inherited visible content. Ordinary master/layout shapes above
    // retain their existing showMasterSp policy.
    let _ = (placeholder, emitted_latent);
}

fn push_flattened_shape<'a>(
    output: &mut Vec<FlattenedItem<'a>>,
    source: FlattenedSource,
    child: &'a ShapeTreeChild,
    group_transform: Transform,
    group_issues: u8,
) {
    let (group_scale, group_transform, group_issues) =
        split_group_transform(group_transform, group_issues);
    output.push(FlattenedItem::Shape {
        source,
        child,
        group_scale,
        group_transform,
        group_issues,
    });
}

fn split_group_transform(transform: Transform, mut issues: u8) -> ((f64, f64), Transform, u8) {
    let scale_x = transform.a.hypot(transform.b);
    let scale_y = transform.c.hypot(transform.d);
    let dot = transform.a * transform.c + transform.b * transform.d;
    let denominator = scale_x * scale_y;
    let is_rigid_after_scale = [
        transform.a,
        transform.b,
        transform.c,
        transform.d,
        transform.e,
        transform.f,
        scale_x,
        scale_y,
    ]
    .into_iter()
    .all(f64::is_finite)
        && scale_x > f64::EPSILON
        && scale_y > f64::EPSILON
        && (dot / denominator).abs() <= 1.0e-9;

    if !is_rigid_after_scale {
        issues |= GROUP_SHEAR;
        return ((1.0, 1.0), transform, issues);
    }

    (
        (scale_x, scale_y),
        Transform {
            a: transform.a / scale_x,
            b: transform.b / scale_x,
            c: transform.c / scale_y,
            d: transform.d / scale_y,
            e: transform.e,
            f: transform.f,
        },
        issues,
    )
}

fn group_affine(transform: &CT_Transform2D) -> Option<(Transform, u8)> {
    let offset = transform.offset?;
    let extent = transform.extent?;
    let child_offset = transform.child_offset?;
    let child_extent = transform.child_extent?;
    let mut issues = 0;
    let x = emu_to_points(offset.x.0);
    let y = emu_to_points(offset.y.0);
    let width = emu_to_points(extent.cx.0);
    let height = emu_to_points(extent.cy.0);
    let child_x = emu_to_points(child_offset.x.0);
    let child_y = emu_to_points(child_offset.y.0);
    let scale_x = if child_extent.cx.0 == 0 {
        issues |= GROUP_ZERO_X;
        1.0
    } else {
        extent.cx.0 as f64 / child_extent.cx.0 as f64
    };
    let scale_y = if child_extent.cy.0 == 0 {
        issues |= GROUP_ZERO_Y;
        1.0
    } else {
        extent.cy.0 as f64 / child_extent.cy.0 as f64
    };
    let mut affine = Transform {
        a: scale_x,
        b: 0.0,
        c: 0.0,
        d: scale_y,
        e: x - child_x * scale_x,
        f: y - child_y * scale_y,
    };
    let center_x = x + width / 2.0;
    let center_y = y + height / 2.0;
    if transform.flip_horizontal || transform.flip_vertical {
        affine = affine.then(Transform {
            a: if transform.flip_horizontal { -1.0 } else { 1.0 },
            b: 0.0,
            c: 0.0,
            d: if transform.flip_vertical { -1.0 } else { 1.0 },
            e: if transform.flip_horizontal {
                2.0 * center_x
            } else {
                0.0
            },
            f: if transform.flip_vertical {
                2.0 * center_y
            } else {
                0.0
            },
        });
    }
    affine = affine.then(Transform::rotate_about(
        f64::from(transform.rotation.0) / 60_000.0,
        center_x,
        center_y,
    ));
    Some((affine, issues))
}

fn child_placeholder(child: &ShapeTreeChild) -> Option<&CT_Placeholder> {
    match child {
        ShapeTreeChild::Shape(shape) => shape.placeholder.as_ref(),
        ShapeTreeChild::Picture(picture) => picture.placeholder.as_ref(),
        _ => None,
    }
}

fn child_is_occupied(child: &ShapeTreeChild) -> bool {
    match child {
        ShapeTreeChild::Shape(shape) => shape.text_body.as_ref().is_some_and(|body| {
            !body.plain_text().trim().is_empty()
                || body.paragraphs().iter().any(|paragraph| {
                    paragraph
                        .runs
                        .iter()
                        .any(|run| matches!(run, TextRun::Field(_)))
                })
        }),
        ShapeTreeChild::Picture(picture) => picture.blip_fill.is_some(),
        _ => false,
    }
}

fn default_body_properties() -> CT_TextBodyProperties {
    let mut properties = CT_TextBodyProperties::default();
    properties.left_inset = Some(Coordinate32Value::Emu(91_440));
    properties.top_inset = Some(Coordinate32Value::Emu(45_720));
    properties.right_inset = Some(Coordinate32Value::Emu(91_440));
    properties.bottom_inset = Some(Coordinate32Value::Emu(45_720));
    properties.anchor = Some(DrawingTextAnchor::Top);
    properties.wrap = Some(TextWrap::Square);
    properties.vertical = Some(DrawingTextVertical::Horizontal);
    properties.space_first_last_paragraph = Some(false);
    properties.autofit = Some(TextAutofit::NoAutofit);
    properties
}

fn merge_body_properties(target: &mut CT_TextBodyProperties, source: &CT_TextBodyProperties) {
    if let Some(value) = &source.left_inset {
        target.left_inset = Some(value.clone());
    }
    if let Some(value) = &source.top_inset {
        target.top_inset = Some(value.clone());
    }
    if let Some(value) = &source.right_inset {
        target.right_inset = Some(value.clone());
    }
    if let Some(value) = &source.bottom_inset {
        target.bottom_inset = Some(value.clone());
    }
    if let Some(value) = source.anchor {
        target.anchor = Some(value);
    }
    if let Some(value) = source.wrap {
        target.wrap = Some(value);
    }
    if let Some(value) = source.vertical {
        target.vertical = Some(value);
    }
    if let Some(value) = source.space_first_last_paragraph {
        target.space_first_last_paragraph = Some(value);
    }
    if let Some(value) = &source.autofit {
        target.autofit = Some(value.clone());
    }
}

fn blend_fill_colors(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

// PowerPoint's documented two-colour endpoint and mirrored three-colour
// gradients use channel-directed gamma 1.875. Other stop lists remain linear.
// This PPT resolver samples the curve for existing encoded-RGB Paint consumers.
// Word colour/tint policy and source-derived pattern palette weights stay intact.
fn powerpoint_fill_colors(a: Color, b: Color, t: f64) -> Color {
    fn channel(a: f64, b: f64, t: f64) -> f64 {
        let weight = if b >= a {
            1.0 - (1.0 - t).powf(1.875)
        } else {
            t.powf(1.875)
        };
        a + (b - a) * weight
    }
    let t = t.clamp(0.0, 1.0);
    Color {
        r: channel(a.r, b.r, t),
        g: channel(a.g, b.g, t),
        b: channel(a.b, b.b, t),
        a: a.a + (b.a - a.a) * t,
    }
}

fn powerpoint_gradient_stops(stops: &[GradientStop]) -> Vec<GradientStop> {
    let mut ordered = stops.to_vec();
    ordered.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    let gamma = ordered.len() == 2 && ordered[0].offset == 0.0 && ordered[1].offset == 1.0
        || ordered.len() == 3
            && ordered[0].offset == 0.0
            && ordered[2].offset == 1.0
            && ordered[0].color == ordered[2].color;
    if !gamma {
        return ordered;
    }
    let mut sampled = Vec::with_capacity(ordered.len() + 257);
    for i in 0..=256 {
        let offset = f64::from(i) / 256.0;
        sampled.push(GradientStop {
            offset,
            color: path_gradient_color(&ordered, offset),
        });
    }
    // Retain exact original stops, including coincident hard colour boundaries.
    sampled.extend(ordered);
    sampled.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    sampled
}

fn path_gradient_color(stops: &[GradientStop], distance: f64) -> Color {
    let Some(first) = stops.first() else {
        return Color::BLACK;
    };
    let gamma = stops.len() == 2 && stops[0].offset == 0.0 && stops[1].offset == 1.0
        || stops.len() == 3
            && stops[0].offset == 0.0
            && stops[2].offset == 1.0
            && stops[0].color == stops[2].color;
    let distance = distance.clamp(0.0, 1.0);
    if distance <= first.offset {
        return first.color;
    }
    for pair in stops.windows(2) {
        if distance <= pair[1].offset {
            let width = pair[1].offset - pair[0].offset;
            let t = if width > 0.0 {
                (distance - pair[0].offset) / width
            } else {
                1.0
            };
            return if gamma {
                powerpoint_fill_colors(pair[0].color, pair[1].color, t)
            } else {
                blend_fill_colors(pair[0].color, pair[1].color, t)
            };
        }
    }
    stops.last().map(|stop| stop.color).unwrap_or(first.color)
}

// Native PowerPoint 16.113.4 source-owned preset tiles, 8pt with 64 samples per axis.
// Original PDF SHA82e0bfb2cd978c59ebac96d72020e2d28fc2a0bead9728ad8ba3e8b1e34674c0.
// Each two-bit weight is background, half foreground, or foreground. Equal rows
// share only constant data, retaining every sample from the independently captured tiles.
const NATIVE_PATTERN_ROW_WEIGHTS: [u128; 77] = [
    0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,
    0xaaaa0000000000000000000000000000,
    0xaaaaaaaa00000000aaaaaaaa00000000,
    0x0000aaaaaaaa00000000aaaaaaaa0000,
    0x00000000aaaaaaaa00000000aaaaaaaa,
    0xaaaa00000000aaaaaaaa00000000aaaa,
    0x00000000000000000000000000000000,
    0xaaaa000000000000aaaa000000000000,
    0x0000aaaa000000000000aaaa00000000,
    0x00000000aaaa000000000000aaaa0000,
    0x000000000000aaaa000000000000aaaa,
    0xaaaaaaaaaaaaaaaa0000000000000000,
    0x0000000000000000aaaaaaaaaaaaaaaa,
    0x0000000000000000aaaa000000000000,
    0x0000000000000000000000000000aaaa,
    0x000000000000000000000000aaaa0000,
    0x00000000000000000000aaaa00000000,
    0x000000000000aaaaaaaa000000000000,
    0x00000000aaaa00000000aaaa00000000,
    0x0000aaaa0000000000000000aaaa0000,
    0xaaaa000000000000000000000000aaaa,
    0xaaaa555500000000000000005555aaaa,
    0x5555aaaa5555000000005555aaaa5555,
    0x00005555aaaa55555555aaaa55550000,
    0x000000005555aaaaaaaa555500000000,
    0x000000000000aaaa0000000000000000,
    0xaaaa0000aaaa0000aaaa0000aaaa0000,
    0xaaaa5555000000000000000000005555,
    0x5555aaaa555500000000000000000000,
    0x00005555aaaa55550000000000000000,
    0x000000005555aaaa5555000000000000,
    0x0000000000005555aaaa555500000000,
    0x00000000000000005555aaaa55550000,
    0x000000000000000000005555aaaa5555,
    0x5555000000000000000000005555aaaa,
    0xaaaa0000aaaaaaaa000000000000aaaa,
    0x00000000aaaaaaaa0000000000000000,
    0x000000000000000000000000aaaaaaaa,
    0x000000000000aaaaaaaa0000aaaaaaaa,
    0xaaaaaaaa0000aaaaaaaa000000000000,
    0xaaaaaaaa000000000000000000000000,
    0x0000000000000000aaaaaaaa00000000,
    0xaaaa000000000000aaaaaaaa0000aaaa,
    0x0000aaaa0000aaaa0000aaaa0000aaaa,
    0xaaaa00000000000000000000aaaa0000,
    0x00000000aaaa0000aaaa000000000000,
    0x0000aaaa0000aaaa000000000000aaaa,
    0x000000000000aaaa0000aaaa0000aaaa,
    0xaaaaaaaaaaaa0000aaaaaaaaaaaa0000,
    0xaaaa0000aaaaaaaaaaaa0000aaaaaaaa,
    0x0000aaaaaaaaaaaa0000aaaaaaaaaaaa,
    0xaaaaaaaa0000aaaaaaaaaaaa0000aaaa,
    0xaaaaaaaaaaaa0000aaaaaaaaaaaaaaaa,
    0xaaaaaaaaaaaaaaaaaaaaaaaaaaaa0000,
    0xaaaaaaaaaaaaaaaa0000aaaaaaaaaaaa,
    0x0000aaaaaaaaaaaaaaaaaaaaaaaaaaaa,
    0xaaaa0000000000000000aaaa00000000,
    0x0000aaaa00000000aaaa000000000000,
    0x0000aaaa000000000000000000000000,
    0x00000000aaaa00000000000000000000,
    0x00000000aaaaaaaaaaaa000000000000,
    0x0000aaaaaaaaaaaaaaaaaaaa00000000,
    0xaaaa000000000000aaaa00000000aaaa,
    0xaaaa000000000000aaaaaaaaaaaaaaaa,
    0xaaaa00000000aaaaaaaa000000000000,
    0xaaaaaaaaaaaaaaaaaaaa000000000000,
    0x00000000aaaa00000000aaaa0000aaaa,
    0x0000aaaa0000aaaa0000aaaa00000000,
    0x0000aaaa000000000000aaaa0000aaaa,
    0x000000000000aaaa0000aaaa00000000,
    0xaaaaaaaa00000000000000000000aaaa,
    0xaaaaaaaaaaaa00000000000000000000,
    0x0000aaaaaaaaaaaa0000000000000000,
    0x000000000000aaaaaaaaaaaa00000000,
    0x0000000000000000aaaaaaaaaaaa0000,
    0x00000000000000000000aaaaaaaaaaaa,
    0xaaaa00000000000000000000aaaaaaaa,
];

fn native_pattern_rows(preset: &str) -> Option<&'static [u8; 64]> {
    match preset {
        "cross" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1,
        ]),
        "dkDnDiag" => Some(&[
            2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5,
            5, 5, 5, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5,
            5, 5, 5, 5, 5, 5,
        ]),
        "dkHorz" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 6, 6, 6,
        ]),
        "dkUpDiag" => Some(&[
            4, 4, 4, 4, 4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 5, 5, 5, 5, 5,
            5, 5, 5, 4, 4, 4, 4, 4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 5, 5,
            5, 5, 5, 5, 5, 5,
        ]),
        "dkVert" => Some(&[
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 2, 2,
        ]),
        "dashDnDiag" => Some(&[
            6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 7, 7, 7, 7, 7, 7, 7, 7, 8, 8, 8, 8, 8,
            8, 8, 8, 9, 9, 9, 9, 9, 9, 9, 9, 10, 10, 10, 10, 10, 10, 10, 10, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 6, 6, 6, 6, 6, 6,
        ]),
        "dashHorz" => Some(&[
            11, 11, 11, 11, 11, 11, 11, 11, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 6, 6, 6, 12, 12, 12, 12, 12, 12, 12, 12, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
        ]),
        "dashUpDiag" => Some(&[
            6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 10, 10, 10, 10, 10, 10, 10, 10, 9, 9,
            9, 9, 9, 9, 9, 9, 8, 8, 8, 8, 8, 8, 8, 8, 7, 7, 7, 7, 7, 7, 7, 7, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 6, 6, 6, 6, 6, 6,
        ]),
        "dashVert" => Some(&[
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
            13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
        ]),
        "diagBrick" => Some(&[
            14, 14, 14, 14, 14, 14, 14, 14, 15, 15, 15, 15, 15, 15, 15, 15, 16, 16, 16, 16, 16, 16,
            16, 16, 13, 13, 13, 13, 13, 13, 13, 13, 17, 17, 17, 17, 17, 17, 17, 17, 18, 18, 18, 18,
            18, 18, 18, 18, 19, 19, 19, 19, 19, 19, 19, 19, 20, 20, 20, 20, 20, 20, 20, 20,
        ]),
        "diagCross" => Some(&[
            21, 21, 21, 21, 21, 21, 21, 21, 22, 22, 22, 22, 22, 22, 22, 22, 23, 23, 23, 23, 23, 23,
            23, 23, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 23, 23, 23, 23,
            23, 23, 23, 23, 22, 22, 22, 22, 22, 22, 22, 22, 21, 21, 21, 21, 21, 21, 21, 21,
        ]),
        "divot" => Some(&[
            6, 6, 6, 6, 6, 6, 6, 6, 25, 25, 25, 25, 25, 25, 25, 25, 13, 13, 13, 13, 13, 13, 13, 13,
            25, 25, 25, 25, 25, 25, 25, 25, 6, 6, 6, 6, 6, 6, 6, 6, 1, 1, 1, 1, 1, 1, 1, 1, 14, 14,
            14, 14, 14, 14, 14, 14, 1, 1, 1, 1, 1, 1, 1, 1,
        ]),
        "dotDmnd" => Some(&[
            1, 1, 1, 1, 1, 1, 1, 1, 6, 6, 6, 6, 6, 6, 6, 6, 9, 9, 9, 9, 9, 9, 9, 9, 6, 6, 6, 6, 6,
            6, 6, 6, 13, 13, 13, 13, 13, 13, 13, 13, 6, 6, 6, 6, 6, 6, 6, 6, 9, 9, 9, 9, 9, 9, 9,
            9, 6, 6, 6, 6, 6, 6, 6, 6,
        ]),
        "dotGrid" => Some(&[
            26, 26, 26, 26, 26, 26, 26, 26, 6, 6, 6, 6, 6, 6, 6, 6, 1, 1, 1, 1, 1, 1, 1, 1, 6, 6,
            6, 6, 6, 6, 6, 6, 1, 1, 1, 1, 1, 1, 1, 1, 6, 6, 6, 6, 6, 6, 6, 6, 1, 1, 1, 1, 1, 1, 1,
            1, 6, 6, 6, 6, 6, 6, 6, 6,
        ]),
        "dnDiag" => Some(&[
            27, 27, 27, 27, 27, 27, 27, 27, 28, 28, 28, 28, 28, 28, 28, 28, 29, 29, 29, 29, 29, 29,
            29, 29, 30, 30, 30, 30, 30, 30, 30, 30, 31, 31, 31, 31, 31, 31, 31, 31, 32, 32, 32, 32,
            32, 32, 32, 32, 33, 33, 33, 33, 33, 33, 33, 33, 34, 34, 34, 34, 34, 34, 34, 34,
        ]),
        "horz" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 6, 6, 6,
        ]),
        "horzBrick" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
            13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
        ]),
        "lgCheck" => Some(&[
            11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
            11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
            12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
        ]),
        "lgConfetti" => Some(&[
            35, 35, 35, 35, 35, 35, 35, 35, 36, 36, 36, 36, 36, 36, 36, 36, 37, 37, 37, 37, 37, 37,
            37, 37, 38, 38, 38, 38, 38, 38, 38, 38, 39, 39, 39, 39, 39, 39, 39, 39, 40, 40, 40, 40,
            40, 40, 40, 40, 41, 41, 41, 41, 41, 41, 41, 41, 42, 42, 42, 42, 42, 42, 42, 42,
        ]),
        "lgGrid" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1,
        ]),
        "ltDnDiag" => Some(&[
            7, 7, 7, 7, 7, 7, 7, 7, 8, 8, 8, 8, 8, 8, 8, 8, 9, 9, 9, 9, 9, 9, 9, 9, 10, 10, 10, 10,
            10, 10, 10, 10, 7, 7, 7, 7, 7, 7, 7, 7, 8, 8, 8, 8, 8, 8, 8, 8, 9, 9, 9, 9, 9, 9, 9, 9,
            10, 10, 10, 10, 10, 10, 10, 10,
        ]),
        "ltHorz" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 6, 6, 6,
        ]),
        "ltUpDiag" => Some(&[
            10, 10, 10, 10, 10, 10, 10, 10, 9, 9, 9, 9, 9, 9, 9, 9, 8, 8, 8, 8, 8, 8, 8, 8, 7, 7,
            7, 7, 7, 7, 7, 7, 10, 10, 10, 10, 10, 10, 10, 10, 9, 9, 9, 9, 9, 9, 9, 9, 8, 8, 8, 8,
            8, 8, 8, 8, 7, 7, 7, 7, 7, 7, 7, 7,
        ]),
        "ltVert" => Some(&[
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7,
        ]),
        "narHorz" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 6, 6, 6, 6, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 6,
            6, 6, 6, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 6, 6, 6, 6, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6,
            6, 6, 6, 6, 6, 6,
        ]),
        "narVert" => Some(&[
            43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43,
            43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43,
            43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43,
        ]),
        "openDmnd" => Some(&[
            44, 44, 44, 44, 44, 44, 44, 44, 8, 8, 8, 8, 8, 8, 8, 8, 45, 45, 45, 45, 45, 45, 45, 45,
            25, 25, 25, 25, 25, 25, 25, 25, 45, 45, 45, 45, 45, 45, 45, 45, 8, 8, 8, 8, 8, 8, 8, 8,
            44, 44, 44, 44, 44, 44, 44, 44, 14, 14, 14, 14, 14, 14, 14, 14,
        ]),
        "pct10" => Some(&[
            1, 1, 1, 1, 1, 1, 1, 1, 6, 6, 6, 6, 6, 6, 6, 6, 13, 13, 13, 13, 13, 13, 13, 13, 6, 6,
            6, 6, 6, 6, 6, 6, 1, 1, 1, 1, 1, 1, 1, 1, 6, 6, 6, 6, 6, 6, 6, 6, 13, 13, 13, 13, 13,
            13, 13, 13, 6, 6, 6, 6, 6, 6, 6, 6,
        ]),
        "pct20" => Some(&[
            7, 7, 7, 7, 7, 7, 7, 7, 6, 6, 6, 6, 6, 6, 6, 6, 9, 9, 9, 9, 9, 9, 9, 9, 6, 6, 6, 6, 6,
            6, 6, 6, 7, 7, 7, 7, 7, 7, 7, 7, 6, 6, 6, 6, 6, 6, 6, 6, 9, 9, 9, 9, 9, 9, 9, 9, 6, 6,
            6, 6, 6, 6, 6, 6,
        ]),
        "pct25" => Some(&[
            7, 7, 7, 7, 7, 7, 7, 7, 9, 9, 9, 9, 9, 9, 9, 9, 7, 7, 7, 7, 7, 7, 7, 7, 9, 9, 9, 9, 9,
            9, 9, 9, 7, 7, 7, 7, 7, 7, 7, 7, 9, 9, 9, 9, 9, 9, 9, 9, 7, 7, 7, 7, 7, 7, 7, 7, 9, 9,
            9, 9, 9, 9, 9, 9,
        ]),
        "pct30" => Some(&[
            26, 26, 26, 26, 26, 26, 26, 26, 8, 8, 8, 8, 8, 8, 8, 8, 26, 26, 26, 26, 26, 26, 26, 26,
            10, 10, 10, 10, 10, 10, 10, 10, 26, 26, 26, 26, 26, 26, 26, 26, 8, 8, 8, 8, 8, 8, 8, 8,
            26, 26, 26, 26, 26, 26, 26, 26, 10, 10, 10, 10, 10, 10, 10, 10,
        ]),
        "pct40" => Some(&[
            26, 26, 26, 26, 26, 26, 26, 26, 43, 43, 43, 43, 43, 43, 43, 43, 26, 26, 26, 26, 26, 26,
            26, 26, 46, 46, 46, 46, 46, 46, 46, 46, 26, 26, 26, 26, 26, 26, 26, 26, 43, 43, 43, 43,
            43, 43, 43, 43, 26, 26, 26, 26, 26, 26, 26, 26, 47, 47, 47, 47, 47, 47, 47, 47,
        ]),
        "pct5" => Some(&[
            1, 1, 1, 1, 1, 1, 1, 1, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 13, 13, 13, 13, 13, 13, 13, 13, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 6, 6, 6, 6, 6, 6, 6, 6,
        ]),
        "pct50" => Some(&[
            26, 26, 26, 26, 26, 26, 26, 26, 43, 43, 43, 43, 43, 43, 43, 43, 26, 26, 26, 26, 26, 26,
            26, 26, 43, 43, 43, 43, 43, 43, 43, 43, 26, 26, 26, 26, 26, 26, 26, 26, 43, 43, 43, 43,
            43, 43, 43, 43, 26, 26, 26, 26, 26, 26, 26, 26, 43, 43, 43, 43, 43, 43, 43, 43,
        ]),
        "pct60" => Some(&[
            48, 48, 48, 48, 48, 48, 48, 48, 43, 43, 43, 43, 43, 43, 43, 43, 49, 49, 49, 49, 49, 49,
            49, 49, 43, 43, 43, 43, 43, 43, 43, 43, 48, 48, 48, 48, 48, 48, 48, 48, 43, 43, 43, 43,
            43, 43, 43, 43, 49, 49, 49, 49, 49, 49, 49, 49, 43, 43, 43, 43, 43, 43, 43, 43,
        ]),
        "pct70" => Some(&[
            50, 50, 50, 50, 50, 50, 50, 50, 51, 51, 51, 51, 51, 51, 51, 51, 50, 50, 50, 50, 50, 50,
            50, 50, 51, 51, 51, 51, 51, 51, 51, 51, 50, 50, 50, 50, 50, 50, 50, 50, 51, 51, 51, 51,
            51, 51, 51, 51, 50, 50, 50, 50, 50, 50, 50, 50, 51, 51, 51, 51, 51, 51, 51, 51,
        ]),
        "pct75" => Some(&[
            50, 50, 50, 50, 50, 50, 50, 50, 0, 0, 0, 0, 0, 0, 0, 0, 51, 51, 51, 51, 51, 51, 51, 51,
            0, 0, 0, 0, 0, 0, 0, 0, 50, 50, 50, 50, 50, 50, 50, 50, 0, 0, 0, 0, 0, 0, 0, 0, 51, 51,
            51, 51, 51, 51, 51, 51, 0, 0, 0, 0, 0, 0, 0, 0,
        ]),
        "pct80" => Some(&[
            52, 52, 52, 52, 52, 52, 52, 52, 0, 0, 0, 0, 0, 0, 0, 0, 53, 53, 53, 53, 53, 53, 53, 53,
            0, 0, 0, 0, 0, 0, 0, 0, 52, 52, 52, 52, 52, 52, 52, 52, 0, 0, 0, 0, 0, 0, 0, 0, 53, 53,
            53, 53, 53, 53, 53, 53, 0, 0, 0, 0, 0, 0, 0, 0,
        ]),
        "pct90" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 54, 54, 54, 54,
            54, 54, 54, 54, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            55, 55, 55, 55, 55, 55, 55, 55,
        ]),
        "plaid" => Some(&[
            26, 26, 26, 26, 26, 26, 26, 26, 43, 43, 43, 43, 43, 43, 43, 43, 26, 26, 26, 26, 26, 26,
            26, 26, 43, 43, 43, 43, 43, 43, 43, 43, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
            11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
        ]),
        "shingle" => Some(&[
            37, 37, 37, 37, 37, 37, 37, 37, 56, 56, 56, 56, 56, 56, 56, 56, 57, 57, 57, 57, 57, 57,
            57, 57, 36, 36, 36, 36, 36, 36, 36, 36, 41, 41, 41, 41, 41, 41, 41, 41, 15, 15, 15, 15,
            15, 15, 15, 15, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14,
        ]),
        "smCheck" => Some(&[
            5, 5, 5, 5, 5, 5, 5, 5, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 5, 5, 5, 5, 5,
            5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 5, 5,
            5, 5, 5, 5, 5, 5,
        ]),
        "smConfetti" => Some(&[
            1, 1, 1, 1, 1, 1, 1, 1, 13, 13, 13, 13, 13, 13, 13, 13, 58, 58, 58, 58, 58, 58, 58, 58,
            15, 15, 15, 15, 15, 15, 15, 15, 25, 25, 25, 25, 25, 25, 25, 25, 14, 14, 14, 14, 14, 14,
            14, 14, 59, 59, 59, 59, 59, 59, 59, 59, 16, 16, 16, 16, 16, 16, 16, 16,
        ]),
        "smGrid" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 0, 0, 0, 0, 0, 0, 0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7,
        ]),
        "solidDmnd" => Some(&[
            25, 25, 25, 25, 25, 25, 25, 25, 60, 60, 60, 60, 60, 60, 60, 60, 61, 61, 61, 61, 61, 61,
            61, 61, 53, 53, 53, 53, 53, 53, 53, 53, 61, 61, 61, 61, 61, 61, 61, 61, 60, 60, 60, 60,
            60, 60, 60, 60, 25, 25, 25, 25, 25, 25, 25, 25, 6, 6, 6, 6, 6, 6, 6, 6,
        ]),
        "sphere" => Some(&[
            50, 50, 50, 50, 50, 50, 50, 50, 62, 62, 62, 62, 62, 62, 62, 62, 63, 63, 63, 63, 63, 63,
            63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 50, 50, 50, 50, 50, 50, 50, 50, 64, 64, 64, 64,
            64, 64, 64, 64, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65, 65,
        ]),
        "trellis" => Some(&[
            0, 0, 0, 0, 0, 0, 0, 0, 3, 3, 3, 3, 3, 3, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0, 5, 5, 5, 5, 5,
            5, 5, 5, 0, 0, 0, 0, 0, 0, 0, 0, 3, 3, 3, 3, 3, 3, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0, 5, 5,
            5, 5, 5, 5, 5, 5,
        ]),
        "upDiag" => Some(&[
            34, 34, 34, 34, 34, 34, 34, 34, 33, 33, 33, 33, 33, 33, 33, 33, 32, 32, 32, 32, 32, 32,
            32, 32, 31, 31, 31, 31, 31, 31, 31, 31, 30, 30, 30, 30, 30, 30, 30, 30, 29, 29, 29, 29,
            29, 29, 29, 29, 28, 28, 28, 28, 28, 28, 28, 28, 27, 27, 27, 27, 27, 27, 27, 27,
        ]),
        "vert" => Some(&[
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1,
        ]),
        "wave" => Some(&[
            6, 6, 6, 6, 6, 6, 6, 6, 17, 17, 17, 17, 17, 17, 17, 17, 66, 66, 66, 66, 66, 66, 66, 66,
            40, 40, 40, 40, 40, 40, 40, 40, 6, 6, 6, 6, 6, 6, 6, 6, 17, 17, 17, 17, 17, 17, 17, 17,
            66, 66, 66, 66, 66, 66, 66, 66, 40, 40, 40, 40, 40, 40, 40, 40,
        ]),
        "weave" => Some(&[
            7, 7, 7, 7, 7, 7, 7, 7, 67, 67, 67, 67, 67, 67, 67, 67, 9, 9, 9, 9, 9, 9, 9, 9, 68, 68,
            68, 68, 68, 68, 68, 68, 7, 7, 7, 7, 7, 7, 7, 7, 69, 69, 69, 69, 69, 69, 69, 69, 9, 9,
            9, 9, 9, 9, 9, 9, 46, 46, 46, 46, 46, 46, 46, 46,
        ]),
        "wdDnDiag" => Some(&[
            70, 70, 70, 70, 70, 70, 70, 70, 71, 71, 71, 71, 71, 71, 71, 71, 72, 72, 72, 72, 72, 72,
            72, 72, 60, 60, 60, 60, 60, 60, 60, 60, 73, 73, 73, 73, 73, 73, 73, 73, 74, 74, 74, 74,
            74, 74, 74, 74, 75, 75, 75, 75, 75, 75, 75, 75, 76, 76, 76, 76, 76, 76, 76, 76,
        ]),
        "wdUpDiag" => Some(&[
            76, 76, 76, 76, 76, 76, 76, 76, 75, 75, 75, 75, 75, 75, 75, 75, 74, 74, 74, 74, 74, 74,
            74, 74, 73, 73, 73, 73, 73, 73, 73, 73, 60, 60, 60, 60, 60, 60, 60, 60, 72, 72, 72, 72,
            72, 72, 72, 72, 71, 71, 71, 71, 71, 71, 71, 71, 70, 70, 70, 70, 70, 70, 70, 70,
        ]),
        "zigZag" => Some(&[
            20, 20, 20, 20, 20, 20, 20, 20, 19, 19, 19, 19, 19, 19, 19, 19, 18, 18, 18, 18, 18, 18,
            18, 18, 17, 17, 17, 17, 17, 17, 17, 17, 20, 20, 20, 20, 20, 20, 20, 20, 19, 19, 19, 19,
            19, 19, 19, 19, 18, 18, 18, 18, 18, 18, 18, 18, 17, 17, 17, 17, 17, 17, 17, 17,
        ]),
        _ => None,
    }
}
