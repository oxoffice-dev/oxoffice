//! Shape-style inheritance resolution.

use std::fmt;

use oxoffice_drawing::color::ColorChoice;
use oxoffice_drawing::effect::CT_EffectList;
use oxoffice_drawing::fill::{Fill, GradientGeometry};
use oxoffice_drawing::line::CT_LineProperties;
use oxoffice_drawing::shape_props::CT_ShapeProperties;
use oxoffice_drawing::style_ref::FontCollectionIndex;
use oxoffice_drawing::table::CT_TableStyle;
use oxoffice_drawing::theme::CT_EffectStyle;
use oxoffice_pptx_oxml::connector::CT_ConnectionShape;
use oxoffice_pptx_oxml::shape_tree::{CT_Shape, CT_ShapeStyle};

use crate::ResolveCtx;

/// The modelled shape-style properties after theme lookup and direct overlays.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectiveShapeStyle {
    pub fill: Option<Fill>,
    pub line: Option<CT_LineProperties>,
    pub effects: Option<CT_EffectList>,
    pub font_collection: Option<FontCollectionIndex>,
}

/// Errors produced while resolving a shape against its theme format scheme.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveError {
    StyleIndexOutOfRange {
        reference: &'static str,
        index: u32,
        available: usize,
    },
    UnresolvedPlaceholderColor {
        reference: &'static str,
    },
    ConcreteValue {
        kind: &'static str,
        detail: String,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StyleIndexOutOfRange {
                reference,
                index,
                available,
            } => write!(
                formatter,
                "{reference} style index {index} is outside the {available} available entries"
            ),
            Self::UnresolvedPlaceholderColor { reference } => write!(
                formatter,
                "{reference} style retains an unresolved phClr placeholder colour"
            ),
            Self::ConcreteValue { kind, detail } => {
                write!(
                    formatter,
                    "cannot resolve {kind} to a concrete value: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for ResolveError {}

impl ResolveCtx<'_> {
    /// Resolves one ordinary shape's modelled theme and direct style properties.
    pub fn effective_shape_style(
        &self,
        shape: &CT_Shape,
    ) -> Result<EffectiveShapeStyle, ResolveError> {
        let (mut effective, referenced_effect_style) =
            self.shape_style_for_placement(shape, false)?;
        effective.effects = resolve_effects(
            referenced_effect_style,
            shape.style(),
            &shape.shape_properties,
        )?;
        Ok(effective)
    }

    // The boundsless validator cannot normalize shape-relative gradient rotation.
    // Placed rendering retains its existing substitution and normalization policy.
    pub(crate) fn shape_style_for_placement(
        &self,
        shape: &CT_Shape,
        unplaced: bool,
    ) -> Result<(EffectiveShapeStyle, Option<CT_EffectStyle>), ResolveError> {
        let matrix = &self.theme.theme_elements.format_scheme;
        let referenced_effect_style = self.referenced_effect_style(shape.style())?;
        let mut effective = if let Some(style) = shape.style() {
            EffectiveShapeStyle {
                fill: referenced_fill(
                    style.fill_reference.index,
                    &matrix.fill_styles,
                    &matrix.background_fill_styles,
                )?,
                line: matrix_entry("line", style.line_reference.index, &matrix.line_styles)?,
                effects: None,
                font_collection: Some(style.font_reference.index),
            }
        } else {
            EffectiveShapeStyle::default()
        };

        if let Some(explicit) = &shape.shape_properties.fill {
            effective.fill = Some(explicit.clone());
        }
        if let Some(explicit) = &shape.shape_properties.line {
            if let Some(line) = effective.line.as_mut() {
                overlay_line(line, explicit);
            } else {
                effective.line = Some(explicit.clone());
            }
        }

        let fill_reference = shape
            .style()
            .and_then(|style| style.fill_reference.color.as_ref());
        let line_reference = shape
            .style()
            .and_then(|style| style.line_reference.color.as_ref());
        if let Some(fill) = effective.fill.as_mut()
            && !(unplaced
                && matches!(fill, Fill::Gradient(gradient) if gradient.rotate_with_shape == Some(false)))
        {
            substitute_fill(fill, fill_reference, "fill")?;
        }
        if let Some(line) = effective.line.as_mut()
            && let Some(fill) = line.fill.as_mut()
            && !(unplaced
                && matches!(fill, Fill::Gradient(gradient) if gradient.rotate_with_shape == Some(false)))
        {
            substitute_fill(fill, line_reference, "line")?;
        }
        Ok((effective, referenced_effect_style))
    }

    /// A table style whose borders given as a theme line (`a:lnRef`) take
    /// that line, its colour replaced by the reference's, as a shape's
    /// `a:lnRef` resolves; index 0 is no line. A border that gives a line
    /// keeps it. A reference the theme cannot resolve leaves its border
    /// unset, and the second value says one did.
    pub(crate) fn table_style_with_theme_lines(
        &self,
        style: &CT_TableStyle,
    ) -> (CT_TableStyle, bool) {
        let mut unresolved = false;
        let lines = &self.theme.theme_elements.format_scheme.line_styles;
        let mut style = style.clone();
        let regions = [
            &mut style.whole_table,
            &mut style.band1_horizontal,
            &mut style.band2_horizontal,
            &mut style.band1_vertical,
            &mut style.band2_vertical,
            &mut style.first_column,
            &mut style.last_column,
            &mut style.first_row,
            &mut style.last_row,
            &mut style.north_west_cell,
            &mut style.north_east_cell,
            &mut style.south_west_cell,
            &mut style.south_east_cell,
        ];
        for region in regions {
            let Some(borders) = region
                .as_deref_mut()
                .and_then(|region| region.cell_style.as_mut())
                .and_then(|cell| cell.borders.as_mut())
            else {
                continue;
            };
            let references = borders.line_references.clone();
            let edges = [
                (&mut borders.left, references.left),
                (&mut borders.right, references.right),
                (&mut borders.top, references.top),
                (&mut borders.bottom, references.bottom),
                (&mut borders.inside_horizontal, references.inside_horizontal),
                (&mut borders.inside_vertical, references.inside_vertical),
            ];
            for (edge, reference) in edges {
                let Some(reference) = reference.filter(|_| edge.is_none()) else {
                    continue;
                };
                if reference.index == 0 {
                    let mut none = CT_LineProperties::default();
                    none.fill = Some(Fill::NoFill(Default::default()));
                    *edge = Some(none);
                    continue;
                }
                let line = matrix_entry("line", reference.index, lines).and_then(|mut line| {
                    if let Some(fill) = line.as_mut().and_then(|line| line.fill.as_mut()) {
                        substitute_fill(fill, reference.color.as_ref(), "line")?;
                    }
                    Ok(line)
                });
                match line {
                    Ok(line) => *edge = line,
                    Err(_) => unresolved = true,
                }
            }
        }
        (style, unresolved)
    }

    /// Resolves the effects of an ordinary shape or a connector: the theme
    /// effect its style's `a:effectRef` selects, replaced as a whole by a
    /// direct `a:effectLst`. Index 0 and an empty direct list both leave no
    /// effect.
    pub(crate) fn effective_effects(
        &self,
        style: Option<&CT_ShapeStyle>,
        properties: &CT_ShapeProperties,
    ) -> Result<Option<CT_EffectList>, ResolveError> {
        let referenced_effect_style = self.referenced_effect_style(style)?;
        resolve_effects(referenced_effect_style, style, properties)
    }

    fn referenced_effect_style(
        &self,
        style: Option<&CT_ShapeStyle>,
    ) -> Result<Option<CT_EffectStyle>, ResolveError> {
        match style {
            Some(style) => matrix_entry(
                "effect",
                style.effect_reference.index,
                &self.theme.theme_elements.format_scheme.effect_styles,
            ),
            None => Ok(None),
        }
    }

    /// Resolves one connector's line from its style's theme line reference and
    /// its direct `a:ln`, the same way an ordinary shape's line resolves.
    pub(crate) fn effective_connector_line(
        &self,
        connector: &CT_ConnectionShape,
        unplaced: bool,
    ) -> Result<Option<CT_LineProperties>, ResolveError> {
        let style = connector.style();
        let mut line = match style {
            Some(style) => matrix_entry(
                "line",
                style.line_reference.index,
                &self.theme.theme_elements.format_scheme.line_styles,
            )?,
            None => None,
        };
        if let Some(explicit) = &connector.shape_properties.line {
            if let Some(line) = line.as_mut() {
                overlay_line(line, explicit);
            } else {
                line = Some(explicit.clone());
            }
        }
        if let Some(fill) = line.as_mut().and_then(|line| line.fill.as_mut())
            && !(unplaced
                && matches!(fill, Fill::Gradient(gradient) if gradient.rotate_with_shape == Some(false)))
        {
            let reference = style.and_then(|style| style.line_reference.color.as_ref());
            substitute_fill(fill, reference, "line")?;
        }
        Ok(line)
    }
}

/// Chooses the effects a shape draws from its referenced theme effect style
/// and its direct `a:effectLst`, then resolves their placeholder colours.
fn resolve_effects(
    referenced_effect_style: Option<CT_EffectStyle>,
    style: Option<&CT_ShapeStyle>,
    properties: &CT_ShapeProperties,
) -> Result<Option<CT_EffectList>, ResolveError> {
    let has_explicit_unmodelled_effect = properties.has_unmodelled_effect();
    let unmodelled_effect_has_placeholder = if has_explicit_unmodelled_effect {
        properties.has_unmodelled_effect_placeholder_color()
    } else {
        referenced_effect_style
            .as_ref()
            .is_some_and(|style| style.has_unmodelled_effect_placeholder_color())
    };
    if unmodelled_effect_has_placeholder {
        return Err(ResolveError::UnresolvedPlaceholderColor {
            reference: "effect",
        });
    }
    let mut effects = if has_explicit_unmodelled_effect {
        None
    } else if let Some(explicit) = &properties.effects {
        Some(explicit.clone())
    } else {
        referenced_effect_style
            .filter(|style| !style.has_unmodelled_effect())
            .and_then(|style| style.effect_list)
    };
    if let Some(effects) = effects.as_mut() {
        let effect_reference = style.and_then(|style| style.effect_reference.color.as_ref());
        if let Some(shadow) = effects.outer_shadow.as_mut()
            && let Some(color) = shadow.color.as_mut()
        {
            substitute_color(color, effect_reference, "effect")?;
        }
        if effects.has_unmodelled_placeholder_color() {
            return Err(ResolveError::UnresolvedPlaceholderColor {
                reference: "effect",
            });
        }
    }
    Ok(effects)
}

fn matrix_entry<T: Clone>(
    reference: &'static str,
    index: u32,
    entries: &[T],
) -> Result<Option<T>, ResolveError> {
    if index == 0 {
        return Ok(None);
    }
    let offset = usize::try_from(index - 1).ok();
    offset
        .and_then(|offset| entries.get(offset))
        .cloned()
        .map(Some)
        .ok_or(ResolveError::StyleIndexOutOfRange {
            reference,
            index,
            available: entries.len(),
        })
}

pub(crate) fn referenced_fill(
    index: u32,
    normal: &[Fill],
    background: &[Fill],
) -> Result<Option<Fill>, ResolveError> {
    if index > 1000 {
        matrix_entry("background fill", index - 1000, background)
    } else {
        matrix_entry("fill", index, normal)
    }
}

fn overlay_line(target: &mut CT_LineProperties, source: &CT_LineProperties) {
    if source.width.is_some() {
        target.width = source.width;
    }
    if source.cap.is_some() {
        target.cap = source.cap;
    }
    if source.fill.is_some() {
        target.fill.clone_from(&source.fill);
    }
    if source.dash.is_some() {
        target.dash.clone_from(&source.dash);
    }
    if source.join.is_some() {
        target.join.clone_from(&source.join);
    }
    if source.head_end.is_some() {
        target.head_end.clone_from(&source.head_end);
    }
    if source.tail_end.is_some() {
        target.tail_end.clone_from(&source.tail_end);
    }
}

pub(crate) fn substitute_fill(
    fill: &mut Fill,
    reference: Option<&ColorChoice>,
    kind: &'static str,
) -> Result<(), ResolveError> {
    match fill {
        Fill::Solid(fill) => substitute_optional_color(&mut fill.color, reference, kind),
        Fill::Gradient(fill) => {
            // Match the paint boundary before resolving colours that cannot be drawn.
            if matches!(fill.flip.as_deref(), Some("x" | "y" | "xy"))
                || fill.tile_rect.is_some()
                || matches!(fill.geometry.as_ref(), Some(GradientGeometry::Path(path)) if !path.fill_to_rect.as_ref().is_some_and(|rect| {
                    [rect.left, rect.top, rect.right, rect.bottom].into_iter().all(|value| value.is_some_and(|value| value.0 == 50_000))
                }))
            {
                return Ok(());
            }
            for stop in &mut fill.stops {
                if stop.color.is_none() {
                    break;
                }
                substitute_optional_color(&mut stop.color, reference, kind)?;
            }
            Ok(())
        }
        Fill::Pattern(fill) => {
            substitute_optional_color(&mut fill.foreground, reference, kind)?;
            substitute_optional_color(&mut fill.background, reference, kind)
        }
        Fill::NoFill(_) | Fill::Blip(_) => Ok(()),
    }
}

fn substitute_optional_color(
    color: &mut Option<ColorChoice>,
    reference: Option<&ColorChoice>,
    kind: &'static str,
) -> Result<(), ResolveError> {
    if let Some(color) = color {
        substitute_color(color, reference, kind)?;
    }
    Ok(())
}

fn substitute_color(
    color: &mut ColorChoice,
    reference: Option<&ColorChoice>,
    kind: &'static str,
) -> Result<(), ResolveError> {
    if !matches!(color, ColorChoice::Scheme { value, .. } if value == "phClr") {
        return Ok(());
    }
    let placeholder_transforms = color.transforms().to_vec();
    let Some(mut replacement) = reference.cloned() else {
        return Err(ResolveError::UnresolvedPlaceholderColor { reference: kind });
    };
    if matches!(&replacement, ColorChoice::Scheme { value, .. } if value == "phClr") {
        return Err(ResolveError::UnresolvedPlaceholderColor { reference: kind });
    }
    transforms_mut(&mut replacement).extend(placeholder_transforms);
    *color = replacement;
    Ok(())
}

fn transforms_mut(color: &mut ColorChoice) -> &mut Vec<oxoffice_drawing::color::ColorTransform> {
    match color {
        ColorChoice::Srgb { transforms, .. }
        | ColorChoice::Scheme { transforms, .. }
        | ColorChoice::System { transforms, .. }
        | ColorChoice::Preset { transforms, .. } => transforms,
    }
}
