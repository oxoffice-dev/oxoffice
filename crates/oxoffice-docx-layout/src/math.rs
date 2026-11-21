use oxoffice_docx_oxml::math::{
    FractionType, LimitLocation, MathAccent, MathArgument, MathDelimiter, MathExpression,
    MathFraction, MathJustification, MathLimit, MathMatrix, MathNary, MathPreSubSuperscript,
    MathProperties, MathRadical, MathRun, MathScript, MathStyle, MathSubSuperscript,
    MatrixBaseJustification, OfficeMath,
};
use oxoffice_layout::{
    Color, Diagnostic, FontManager, GlyphRun, GroupElement, Point, PositionedElement, Rect, Result,
    Transform, walk,
};

const DEFAULT_MATH_FONT: &str = "Caladea";
const DEFAULT_MATH_SIZE: f64 = 11.0;
const SCRIPT_SCALE: f64 = 0.7;
const SUBSCRIPT_SHIFT_EM: f64 = 0.18;
const SUPERSCRIPT_SHIFT_EM: f64 = 0.43;
const GAP_EM: f64 = 0.12;
const RULE_EM: f64 = 0.055;
const MAX_DELIMITER_SCALE: f64 = 4.0;
const EQUATION_SPACING_EM: f64 = 0.107;
const LIMIT_GAP_EM: f64 = -0.29;
const MATH_TEXT_X_SCALE: f64 = 1.1;
const NARY_OPERATOR_X_SCALE: f64 = 1.89;
const NARY_UPPER_GAP_EM: f64 = 0.482;
const NARY_LOWER_GAP_EM: f64 = 0.3;

#[derive(Debug, Clone, Copy)]
struct MathLayoutStyle {
    font_size: f64,
    root_size: f64,
    display: bool,
    cramped: bool,
    script_level: u8,
}

impl MathLayoutStyle {
    fn new(font_size: f64, display: bool) -> Self {
        Self {
            font_size,
            root_size: font_size,
            display,
            cramped: false,
            script_level: 0,
        }
    }

    fn argument(self) -> Self {
        Self {
            display: false,
            ..self
        }
    }
    fn cramped(self) -> Self {
        Self {
            display: false,
            cramped: true,
            ..self
        }
    }
    fn with_size(self, font_size: f64) -> Self {
        Self {
            font_size,
            display: false,
            ..self
        }
    }
    fn script(self, scale: f64) -> Self {
        Self {
            font_size: self.font_size * scale,
            display: false,
            script_level: self.script_level.saturating_add(1).min(2),
            ..self
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MeasuredMath {
    pub(crate) width: f64,
    pub(crate) ascent: f64,
    pub(crate) descent: f64,
    pub(crate) group: GroupElement,
    // Font-requested white space above ink, retained when a box owns the row top.
    top_padding: f64,
    // The completed terminal atom has this correction in width already. Script
    // attachment consumes that advance before applying its own positioning rule.
    terminal_italic_advance: f64,
}

impl MeasuredMath {
    pub(crate) fn height(&self) -> f64 {
        self.ascent + self.descent
    }

    fn for_script_attachment(mut self) -> Self {
        self.width -= self.terminal_italic_advance;
        self.terminal_italic_advance = 0.0;
        self
    }

    fn empty() -> Self {
        Self {
            width: 0.0,
            ascent: 0.0,
            descent: 0.0,
            group: group(Vec::new()),
            terminal_italic_advance: 0.0,
            top_padding: 0.0,
        }
    }
}

pub(crate) fn layout_officemath(
    equation: &OfficeMath,
    fm: &mut FontManager,
    font_size: f64,
    color: Color,
    available_width: f64,
    math_properties: Option<&MathProperties>,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(MeasuredMath, bool)> {
    let size = finite_positive(font_size).unwrap_or(DEFAULT_MATH_SIZE);
    let font_family = math_properties
        .and_then(|properties| properties.math_font.as_deref())
        .filter(|family| !family.is_empty())
        .or(Some(DEFAULT_MATH_FONT));
    let mut equation = equation.clone();
    apply_limit_defaults(&mut equation, math_properties);
    if equation.has_unsupported_content() {
        diagnostics.push(Diagnostic {
            message: format!(
                "OfficeMath content at {source_path} was preserved but could not be rendered"
            ),
        });
    }

    match &equation {
        OfficeMath::Inline(value) => Ok((
            layout_expressions(
                &value.expressions,
                fm,
                font_family,
                MathLayoutStyle::new(size, false),
                color,
                size * EQUATION_SPACING_EM,
                source_path,
                diagnostics,
            )?,
            false,
        )),
        OfficeMath::Display(value) => {
            let mut equations = Vec::with_capacity(value.equations.len());
            for (index, equation) in value.equations.iter().enumerate() {
                equations.push(layout_expressions(
                    &equation.expressions,
                    fm,
                    font_family,
                    MathLayoutStyle::new(size, true),
                    color,
                    size * EQUATION_SPACING_EM,
                    &format!("{source_path}/equation/{index}"),
                    diagnostics,
                )?);
            }
            let inter_spacing = math_properties
                .and_then(|properties| properties.inter_spacing)
                .map_or(0.0, twips_to_points);
            let mut measured = layout_spaced_sequence(equations, inter_spacing);
            let font_id = fm.resolve_font_for_text(font_family, false, false, "x")?;
            let data = fm.font_data(font_id)?;
            if let Some(leading) = ttf_parser::Face::parse(&data.data, data.face_index)
                .ok()
                .and_then(|face| {
                    Some(
                        f64::from(face.tables().math?.constants?.math_leading().value) * size
                            / f64::from(face.units_per_em()),
                    )
                })
            {
                // A display frame starts above its ink by MathLeading. Face
                // ascenders include blank space and differ across constructs.
                let shift = leading.max(0.0) + measured.top_padding - ink_bounds(&measured, fm).y;
                let run_only = value.equations.iter().all(|equation| {
                    equation
                        .expressions
                        .iter()
                        .all(|expression| matches!(expression, MathExpression::Run(_)))
                });
                let lower = if run_only { 0.0 } else { -measured.ascent };
                let shift = shift.clamp(lower, size * MAX_DELIMITER_SCALE);
                measured.group.transform.f += shift;
                measured.ascent += shift;
            }
            let left_margin = math_properties
                .and_then(|properties| properties.left_margin)
                .map_or(0.0, twips_to_points);
            let right_margin = math_properties
                .and_then(|properties| properties.right_margin)
                .map_or(0.0, twips_to_points);
            let intra_spacing = math_properties
                .and_then(|properties| properties.intra_spacing)
                .map_or(0.0, twips_to_points);
            let content_width = (available_width - left_margin - right_margin).max(0.0);
            let x = match value
                .properties
                .justification
                .or_else(|| math_properties.and_then(|properties| properties.justification))
                .unwrap_or(MathJustification::CenterGroup)
            {
                MathJustification::Left => left_margin + intra_spacing,
                MathJustification::Right => {
                    left_margin + (content_width - measured.width - intra_spacing).max(0.0)
                }
                MathJustification::Center | MathJustification::CenterGroup => {
                    left_margin + ((content_width - measured.width) / 2.0).max(0.0)
                }
            };
            let width = available_width.max(measured.width);
            Ok((
                MeasuredMath {
                    terminal_italic_advance: 0.0,
                    top_padding: 0.0,
                    width,
                    // Word adds no space above or below a display equation:
                    // its line is as tall as the equation, and m:preSp and
                    // m:postSp change nothing, with m:dispDef on or off.
                    ascent: measured.ascent,
                    descent: measured.descent,
                    group: group(vec![translated(measured.group, x, 0.0)]),
                },
                true,
            ))
        }
    }
}

fn ink_bounds(value: &MeasuredMath, fm: &FontManager) -> Rect {
    fn include(bounds: &mut Option<Rect>, rect: Rect) {
        *bounds = Some(match *bounds {
            None => rect,
            Some(old) => {
                let x = old.x.min(rect.x);
                let y = old.y.min(rect.y);
                Rect {
                    x,
                    y,
                    width: (old.x + old.width).max(rect.x + rect.width) - x,
                    height: (old.y + old.height).max(rect.y + rect.height) - y,
                }
            }
        });
    }
    let mut bounds = None;
    let mut complete = true;
    walk(&value.group.children, &mut |element, transform| {
        let transform = transform.then(value.group.transform);
        match element {
            PositionedElement::Text(run) => {
                let Ok(data) = fm.font_data(run.font_id) else {
                    complete = false;
                    return;
                };
                let Ok(mut face) = ttf_parser::Face::parse(&data.data, data.face_index) else {
                    complete = false;
                    return;
                };
                for (tag, coordinate) in &data.variations {
                    if face
                        .set_variation(ttf_parser::Tag::from_bytes(tag), *coordinate)
                        .is_none()
                    {
                        complete = false;
                        return;
                    }
                }
                let scale = run.font_size / f64::from(face.units_per_em());
                let mut pen = run.origin.x;
                for (index, glyph) in run.glyph_ids.iter().enumerate() {
                    if let Some(ink) = face.glyph_bounding_box(ttf_parser::GlyphId(*glyph)) {
                        let mut rect = Rect {
                            x: pen + f64::from(ink.x_min) * scale,
                            y: run.origin.y - f64::from(ink.y_max) * scale,
                            width: (f64::from(ink.x_max) - f64::from(ink.x_min)) * scale,
                            height: (f64::from(ink.y_max) - f64::from(ink.y_min)) * scale,
                        };
                        if data.synthetic_italic {
                            let top = (run.origin.y - rect.y) * 12.0_f64.to_radians().tan();
                            let bottom =
                                (run.origin.y - rect.y - rect.height) * 12.0_f64.to_radians().tan();
                            rect.x += top.min(bottom);
                            rect.width += (top - bottom).abs();
                        }
                        if data.synthetic_bold {
                            let half_stroke = run.font_size / 48.0;
                            rect.x -= half_stroke;
                            rect.y -= half_stroke;
                            rect.width += 2.0 * half_stroke;
                            rect.height += 2.0 * half_stroke;
                        }
                        include(&mut bounds, transform.transform_rect_bbox(rect));
                    }
                    pen += run.advances.get(index).copied().unwrap_or(0.0);
                }
            }
            PositionedElement::Line {
                start, end, width, ..
            } => {
                let half = width / 2.0;
                include(
                    &mut bounds,
                    transform.transform_rect_bbox(Rect {
                        x: start.x.min(end.x) - half,
                        y: start.y.min(end.y) - half,
                        width: (start.x - end.x).abs() + width,
                        height: (start.y - end.y).abs() + width,
                    }),
                );
            }
            _ => {}
        }
    });
    if complete {
        bounds.unwrap_or(Rect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        })
    } else {
        Rect {
            x: 0.0,
            y: 0.0,
            width: value.width,
            height: value.height(),
        }
    }
}

fn math_script_context(
    fm: &mut FontManager,
    family: Option<&str>,
    context: MathLayoutStyle,
    levels: u8,
) -> Result<MathLayoutStyle> {
    let font_id = fm.resolve_font_for_text(family, false, false, "x")?;
    let data = fm.font_data(font_id)?;
    let level = context.script_level.saturating_add(levels).min(2);
    let ratio = ttf_parser::Face::parse(&data.data, data.face_index)
        .ok()
        .and_then(|face| face.tables().math)
        .and_then(|math| math.constants)
        .map(|c| {
            f64::from(if level == 1 {
                c.script_percent_scale_down()
            } else {
                c.script_script_percent_scale_down()
            }) / 100.0
        })
        .filter(|ratio| *ratio > 0.0 && *ratio <= 1.0);
    if let Some(ratio) = ratio {
        Ok(MathLayoutStyle {
            font_size: context.root_size * ratio,
            display: false,
            script_level: level,
            ..context
        })
    } else {
        // Fonts without MATH keep the historical recursive fallback size.
        Ok((0..levels).fold(context, |style, _| style.script(SCRIPT_SCALE)))
    }
}

fn space_after_script(fm: &mut FontManager, family: Option<&str>, size: f64) -> Result<f64> {
    let font_id = fm.resolve_font_for_text(family, false, false, "x")?;
    let data = fm.font_data(font_id)?;
    Ok(ttf_parser::Face::parse(&data.data, data.face_index)
        .ok()
        .and_then(|face| {
            let value = face.tables().math?.constants?.space_after_script().value;
            Some((f64::from(value) * size / f64::from(face.units_per_em())).clamp(0.0, size))
        })
        .unwrap_or(0.0))
}

fn script_shifts(
    base: &MeasuredMath,
    subscript: &MeasuredMath,
    superscript: &MeasuredMath,
    fm: &mut FontManager,
    family: Option<&str>,
    context: MathLayoutStyle,
) -> Result<(f64, f64)> {
    let font_size = context.font_size;
    let font_id = fm.resolve_font_for_text(family, false, false, "x")?;
    let data = fm.font_data(font_id)?;
    let face = ttf_parser::Face::parse(&data.data, data.face_index).ok();
    let Some((face, c)) = face
        .as_ref()
        .and_then(|face| Some((face, face.tables().math?.constants?)))
    else {
        return Ok((
            font_size * SUBSCRIPT_SHIFT_EM,
            font_size * SUPERSCRIPT_SHIFT_EM,
        ));
    };
    let em = font_size / f64::from(face.units_per_em());
    let base_ink = ink_bounds(base, fm);
    let sub_ink = ink_bounds(subscript, fm);
    let sup_ink = ink_bounds(superscript, fm);
    let simple = single_math_run(base)
        .filter(|run| run.glyph_ids.len() == 1)
        .is_some_and(|run| {
            !face
                .tables()
                .math
                .and_then(|m| m.glyph_info)
                .and_then(|i| i.extended_shapes)
                .is_some_and(|coverage| {
                    coverage
                        .get(ttf_parser::GlyphId(run.glyph_ids[0]))
                        .is_some()
                })
        });
    let initial_down = if simple {
        f64::from(c.subscript_shift_down().value) * em
    } else {
        base_ink.y + base_ink.height - base.ascent
            + f64::from(c.subscript_baseline_drop_min().value) * em
    };
    let initial_up = if simple {
        f64::from(if context.cramped {
            c.superscript_shift_up_cramped().value
        } else {
            c.superscript_shift_up().value
        }) * em
    } else {
        base.ascent - base_ink.y - f64::from(c.superscript_baseline_drop_max().value) * em
    };
    let mut down = initial_down
        .max(subscript.ascent - sub_ink.y - f64::from(c.subscript_top_max().value) * em);
    let up = initial_up.max(
        sup_ink.y + sup_ink.height - superscript.ascent
            + f64::from(c.superscript_bottom_min().value) * em,
    );
    if subscript.width > 0.0 && superscript.width > 0.0 {
        let actual_gap = down + sub_ink.y
            - subscript.ascent
            - (-up + sup_ink.y + sup_ink.height - superscript.ascent);
        down += (f64::from(c.sub_superscript_gap_min().value) * em - actual_gap).max(0.0);
    }
    Ok((down.max(0.0), up.max(0.0)))
}

fn apply_script_alternates(face: &ttf_parser::Face<'_>, level: u8, glyphs: &mut [u16]) {
    if level == 0 {
        return;
    }
    let Some(gsub) = face.tables().gsub else {
        return;
    };
    let Some(feature) = gsub.features.find(ttf_parser::Tag::from_bytes(b"ssty")) else {
        return;
    };
    for lookup_index in feature.lookup_indices {
        let Some(lookup) = gsub.lookups.get(lookup_index) else {
            continue;
        };
        for glyph in glyphs.iter_mut() {
            for subtable in lookup
                .subtables
                .into_iter::<ttf_parser::gsub::SubstitutionSubtable<'_>>()
            {
                let id = ttf_parser::GlyphId(*glyph);
                let replacement = match subtable {
                    ttf_parser::gsub::SubstitutionSubtable::Single(
                        ttf_parser::gsub::SingleSubstitution::Format1 { coverage, delta },
                    ) => coverage
                        .get(id)
                        .map(|_| ttf_parser::GlyphId(glyph.wrapping_add_signed(delta))),
                    ttf_parser::gsub::SubstitutionSubtable::Single(
                        ttf_parser::gsub::SingleSubstitution::Format2 {
                            coverage,
                            substitutes,
                        },
                    ) => coverage.get(id).and_then(|i| substitutes.get(i)),
                    ttf_parser::gsub::SubstitutionSubtable::Alternate(table) => table
                        .coverage
                        .get(id)
                        .and_then(|i| table.alternate_sets.get(i))
                        .and_then(|set| {
                            set.alternates
                                .get(u16::from(level - 1))
                                .or_else(|| set.alternates.get(0))
                        }),
                    _ => None,
                };
                if let Some(replacement) = replacement.filter(|id| id.0 < face.number_of_glyphs()) {
                    *glyph = replacement.0;
                    break;
                }
            }
        }
    }
}

fn single_math_run(value: &MeasuredMath) -> Option<&GlyphRun> {
    let mut children = value.group.children.as_slice();
    loop {
        match children {
            [PositionedElement::Text(run)] => return Some(run),
            [PositionedElement::Group(group)] => children = &group.children,
            _ => return None,
        }
    }
}

fn italic_correction(value: &MeasuredMath, fm: &FontManager) -> Option<f64> {
    let run = single_math_run(value)?;
    let data = fm.font_data(run.font_id).ok()?;
    let face = ttf_parser::Face::parse(&data.data, data.face_index).ok()?;
    let math = face.tables().math?;
    let glyph = ttf_parser::GlyphId(*run.glyph_ids.last()?);
    let correction = math
        .glyph_info
        .and_then(|info| info.italic_corrections)
        .and_then(|corrections| corrections.get(glyph))
        .map_or(0, |value| value.value);
    Some(
        (f64::from(correction) * run.font_size / f64::from(face.units_per_em()))
            .clamp(0.0, run.font_size * MAX_DELIMITER_SCALE),
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_math_variant(
    text: &str,
    fm: &mut FontManager,
    family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    target_height: Option<f64>,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<MeasuredMath>> {
    let font_size = context.font_size;
    let font_id = fm.resolve_font_for_text(family, false, false, text)?;
    let data = fm.font_data(font_id)?;
    let Ok(face) = ttf_parser::Face::parse(&data.data, data.face_index) else {
        return Ok(None);
    };
    let Some(math) = face.tables().math else {
        return Ok(None);
    };
    let Some(glyph) = text
        .chars()
        .next()
        .and_then(|character| face.glyph_index(character))
    else {
        return Ok(None);
    };
    let Some(construction) = math
        .variants
        .and_then(|variants| variants.vertical_constructions.get(glyph))
    else {
        return Ok(None);
    };
    let em = font_size / f64::from(face.units_per_em());
    let target = target_height
        .unwrap_or_else(|| {
            math.constants.map_or(font_size, |c| {
                f64::from(
                    c.delimited_sub_formula_min_height()
                        .max(c.display_operator_min_height()),
                ) * em
            })
        })
        .clamp(0.0, font_size * MAX_DELIMITER_SCALE);
    let mut selected = None;
    for variant in construction.variants {
        selected = Some(variant.variant_glyph);
        if f64::from(variant.advance_measurement) * em >= target {
            break;
        }
    }
    let Some(glyph) = selected else {
        return Ok(None);
    };
    let Some(ink) = face.glyph_bounding_box(glyph) else {
        return Ok(None);
    };
    let mut measured = layout_text(
        text,
        fm,
        family,
        context,
        color,
        false,
        false,
        source_path,
        diagnostics,
    )?;
    let width = f64::from(face.glyph_hor_advance(glyph).unwrap_or(0)) * em;
    measured.width = width;
    measured.ascent = measured.ascent.max(f64::from(ink.y_max) * em);
    measured.descent = measured.descent.max(-f64::from(ink.y_min) * em);
    if let Some(PositionedElement::Text(run)) = measured.group.children.first_mut() {
        run.glyph_ids = vec![glyph.0];
        run.advances = vec![width];
        run.origin.y = measured.ascent;
    }
    Ok(Some(measured))
}

fn twips_to_points(value: u32) -> f64 {
    value as f64 / 20.0
}

fn apply_limit_defaults(equation: &mut OfficeMath, properties: Option<&MathProperties>) {
    let Some(properties) = properties else {
        return;
    };
    let equations = match equation {
        OfficeMath::Inline(equation) => std::slice::from_mut(equation),
        OfficeMath::Display(display) => display.equations.as_mut_slice(),
    };
    for equation in equations {
        apply_expression_limit_defaults(&mut equation.expressions, properties);
    }
}

fn apply_expression_limit_defaults(
    expressions: &mut [MathExpression],
    properties: &MathProperties,
) {
    fn argument(argument: &mut MathArgument, properties: &MathProperties) {
        apply_expression_limit_defaults(&mut argument.expressions, properties);
    }

    for expression in expressions {
        match expression {
            MathExpression::Run(_) => {}
            MathExpression::Fraction(value) => {
                argument(&mut value.numerator, properties);
                argument(&mut value.denominator, properties);
            }
            MathExpression::Subscript(value) | MathExpression::Superscript(value) => {
                argument(&mut value.base, properties);
                argument(&mut value.script, properties);
            }
            MathExpression::SubSuperscript(value) => {
                argument(&mut value.base, properties);
                argument(&mut value.subscript, properties);
                argument(&mut value.superscript, properties);
            }
            MathExpression::PreSubSuperscript(value) => {
                argument(&mut value.base, properties);
                argument(&mut value.subscript, properties);
                argument(&mut value.superscript, properties);
            }
            MathExpression::Radical(value) => {
                argument(&mut value.degree, properties);
                argument(&mut value.base, properties);
            }
            MathExpression::Matrix(value) => {
                for row in &mut value.rows {
                    for cell in &mut row.cells {
                        argument(cell, properties);
                    }
                }
            }
            MathExpression::LowerLimit(value) | MathExpression::UpperLimit(value) => {
                argument(&mut value.base, properties);
                argument(&mut value.limit, properties);
            }
            MathExpression::Nary(value) => {
                if value.limit_location.is_none() {
                    let is_integral = value.character.is_empty()
                        || value.character.chars().any(|character| {
                            matches!(character, '\u{222b}' | '\u{222c}' | '\u{222d}' | '\u{222e}')
                        });
                    value.limit_location = if is_integral {
                        properties.integral_limit_location
                    } else {
                        properties.nary_limit_location
                    };
                }
                argument(&mut value.base, properties);
                argument(&mut value.subscript, properties);
                argument(&mut value.superscript, properties);
            }
            MathExpression::Delimiter(value) => {
                for argument_value in &mut value.arguments {
                    argument(argument_value, properties);
                }
            }
            MathExpression::Accent(value) => argument(&mut value.base, properties),
        }
    }
}

fn layout_expressions(
    expressions: &[MathExpression],
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    spacing: f64,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    let font_id = fm.resolve_font_for_text(font_family, false, false, "x")?;
    let data = fm.font_data(font_id)?;
    let has_math = ttf_parser::Face::parse(&data.data, data.face_index)
        .ok()
        .is_some_and(|face| face.tables().math.is_some());
    let spacing = if has_math { 0.0 } else { spacing };
    let mut measured = Vec::with_capacity(expressions.len());
    for (index, expression) in expressions.iter().enumerate() {
        measured.push(layout_expression(
            expression,
            fm,
            font_family,
            context,
            color,
            &format!("{source_path}/expression/{index}"),
            diagnostics,
        )?);
        if has_math
            && (matches!(
                expression,
                MathExpression::Nary(_)
                    | MathExpression::LowerLimit(_)
                    | MathExpression::UpperLimit(_)
            ) || matches!(expression, MathExpression::Subscript(script)
                    if matches!(script.base.expressions.as_slice(), [MathExpression::Run(run)]
                        if run.properties.normal == Some(true) && run.text == "lim")))
            && index + 1 < expressions.len()
        {
            let next_is_operator = matches!(&expressions[index + 1], MathExpression::Run(run)
                if run.text.starts_with(['+', '−', '=']));
            if !next_is_operator && let Some(last) = measured.last_mut() {
                last.width += font_size * 3.0 / 18.0;
            }
        }
    }
    Ok(layout_spaced_sequence(measured, spacing))
}

fn layout_argument(
    argument: &MathArgument,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    layout_expressions(
        &argument.expressions,
        fm,
        font_family,
        context.argument(),
        color,
        0.0,
        source_path,
        diagnostics,
    )
}

fn layout_expression(
    expression: &MathExpression,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    match expression {
        MathExpression::Run(value) => layout_run(
            value,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::Fraction(value) => layout_fraction(
            value,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::Subscript(value) => layout_script(
            value,
            false,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::Superscript(value) => layout_script(
            value,
            true,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::SubSuperscript(value) => layout_sub_superscript(
            value,
            false,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::PreSubSuperscript(value) => layout_pre_sub_superscript(
            value,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::Radical(value) => layout_radical(
            value,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::Matrix(value) => layout_matrix(
            value,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::LowerLimit(value) => layout_limit(
            value,
            false,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::UpperLimit(value) => layout_limit(
            value,
            true,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::Nary(value) => layout_nary(
            value,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::Delimiter(value) => layout_delimiter(
            value,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
        MathExpression::Accent(value) => layout_accent(
            value,
            fm,
            font_family,
            context,
            color,
            source_path,
            diagnostics,
        ),
    }
}

fn layout_run(
    run: &MathRun,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    let style = if run.properties.normal == Some(true) || run.properties.literal == Some(true) {
        MathStyle::Plain
    } else {
        run.properties.style.unwrap_or(MathStyle::Italic)
    };
    let bold = matches!(style, MathStyle::Bold | MathStyle::BoldItalic);
    let italic = matches!(style, MathStyle::Italic | MathStyle::BoldItalic);
    // Mathematical alphabets are glyph choices, not synthetic font slant. Use
    // them only when the requested physical face covers the complete run.
    if italic
        && !bold
        && run.text.chars().all(|c| {
            !c.is_alphabetic()
                || c.is_ascii_alphabetic()
                || matches!(
                    c,
                    'π' | 'α' | 'β' | 'θ' | 'λ' | 'μ' | 'σ' | 'ω' | 'ε' | 'ϕ' | 'φ' | '∂'
                )
        })
    {
        let mapped: String = run
            .text
            .chars()
            .map(|character| match character {
                'h' => 'ℎ',
                'A'..='Z' => {
                    char::from_u32(0x1d434 + character as u32 - 'A' as u32).unwrap_or(character)
                }
                'a'..='z' => {
                    char::from_u32(0x1d44e + character as u32 - 'a' as u32).unwrap_or(character)
                }
                'π' => '𝜋',
                'α' => '𝛼',
                'β' => '𝛽',
                'θ' => '𝜃',
                'λ' => '𝜆',
                'μ' => '𝜇',
                'σ' => '𝜎',
                'ω' => '𝜔',
                'ε' => '𝜀',
                'ϕ' => '𝜙',
                'φ' => '𝜑',
                '∂' => '𝜕',
                _ => character,
            })
            .collect();
        {
            let font_id = fm.resolve_font_for_text(font_family, false, false, &run.text)?;
            let shaped = fm.shape_text(font_id, &mapped, font_size)?;
            let data = fm.font_data(font_id)?;
            let has_math = ttf_parser::Face::parse(&data.data, data.face_index)
                .ok()
                .is_some_and(|face| face.tables().math.is_some());
            if has_math && !shaped.glyph_ids.contains(&0) {
                let mut measured = layout_text(
                    &mapped,
                    fm,
                    font_family,
                    context,
                    color,
                    false,
                    false,
                    source_path,
                    diagnostics,
                )?;
                if let Some(PositionedElement::Text(glyphs)) = measured.group.children.first_mut() {
                    let data = fm.font_data(glyphs.font_id)?;
                    if let Ok(face) = ttf_parser::Face::parse(&data.data, data.face_index)
                        && let Some(corrections) = face
                            .tables()
                            .math
                            .and_then(|math| math.glyph_info)
                            .and_then(|info| info.italic_corrections)
                        && mapped.chars().count() == glyphs.glyph_ids.len()
                    {
                        let logical: Vec<_> = run.text.chars().collect();
                        for (index, character) in logical.iter().copied().enumerate() {
                            let ordinary = character.is_ascii_alphabetic()
                                || matches!(
                                    character,
                                    'α' | 'β' | 'θ' | 'λ' | 'μ' | 'σ' | 'ω' | 'ε' | 'ϕ' | 'φ' | '∂'
                                );
                            let boundary = logical.get(index + 1).is_none_or(|next| {
                                matches!(
                                    next,
                                    '+' | '−'
                                        | '='
                                        | '×'
                                        | '⋅'
                                        | '÷'
                                        | '±'
                                        | '∓'
                                        | '∪'
                                        | '∩'
                                        | '≤'
                                        | '≥'
                                        | '≠'
                                        | '≈'
                                        | '≡'
                                        | '∼'
                                        | '∝'
                                        | '∈'
                                        | '⊂'
                                        | '⇒'
                                        | '←'
                                        | '\u{2009}'
                                        | '\u{2004}'
                                        | '\u{2001}'
                                )
                            });
                            if ordinary
                                && boundary
                                && let Some(value) =
                                    corrections.get(ttf_parser::GlyphId(glyphs.glyph_ids[index]))
                            {
                                let correction = f64::from(value.value) * font_size
                                    / f64::from(face.units_per_em());
                                glyphs.advances[index] += correction;
                                measured.width += correction;
                                if index + 1 == logical.len() {
                                    measured.terminal_italic_advance = correction;
                                }
                            }
                        }
                    }
                    glyphs.text.clone_from(&run.text);
                }
                return Ok(measured);
            }
        }
    }
    layout_text(
        &run.text,
        fm,
        font_family,
        context,
        color,
        bold,
        italic,
        source_path,
        diagnostics,
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_text(
    text: &str,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    bold: bool,
    italic: bool,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    let family = font_family
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_MATH_FONT);
    let font_id = fm.resolve_font_for_text(Some(family), bold, italic, text)?;
    let mut shaped = fm.shape_text(font_id, text, font_size)?;
    let metrics = fm.metrics(font_id, font_size)?;
    let data = fm.font_data(font_id)?;
    let math_alphabet_covered = ttf_parser::Face::parse(&data.data, data.face_index)
        .ok()
        .is_some_and(|face| face.tables().math.is_some());
    if context.script_level > 0
        && let Ok(face) = ttf_parser::Face::parse(&data.data, data.face_index)
    {
        let original = shaped.glyph_ids.clone();
        apply_script_alternates(&face, context.script_level, &mut shaped.glyph_ids);
        let em = font_size / f64::from(face.units_per_em());
        for (index, (old, new)) in original.iter().zip(&shaped.glyph_ids).enumerate() {
            let difference = (f64::from(
                face.glyph_hor_advance(ttf_parser::GlyphId(*new))
                    .unwrap_or(0),
            ) - f64::from(
                face.glyph_hor_advance(ttf_parser::GlyphId(*old))
                    .unwrap_or(0),
            )) * em;
            if let Some(advance) = shaped.advances.get_mut(index) {
                *advance += difference;
                shaped.width += difference;
            }
        }
    }
    let mut leading_space = 0.0;
    if math_alphabet_covered && text.chars().count() == shaped.advances.len() {
        // Math atom glue uses the conventional medium and thick mu spaces.
        // It belongs to operator layout, not to the stored logical string.
        for (index, character) in text.chars().enumerate() {
            let gap = match character {
                '+' | '−' | '×' | '⋅' | '÷' | '±' | '∓' | '∪' | '∩' => {
                    font_size * 4.0 / 18.0
                }
                '=' | '≤' | '≥' | '≠' | '≈' | '≡' | '∼' | '∝' | '∈' | '⊂' | '⇒' | '←' => {
                    font_size * 5.0 / 18.0
                }
                _ => 0.0,
            };
            if index == 0 {
                leading_space += gap;
            } else {
                shaped.advances[index - 1] += gap;
            }
            shaped.advances[index] += gap;
            shaped.width += 2.0 * gap;
        }
    }
    if shaped.glyph_ids.contains(&0) {
        diagnostics.push(Diagnostic {
            message: format!("OfficeMath text at {source_path} contains an unavailable glyph"),
        });
    }
    Ok(horizontally_scaled(
        MeasuredMath {
            terminal_italic_advance: 0.0,
            top_padding: 0.0,
            width: shaped.width.max(0.0),
            ascent: metrics.ascent.max(0.0),
            descent: metrics.descent.max(0.0),
            group: group(vec![PositionedElement::Text(GlyphRun {
                origin: Point {
                    x: leading_space,
                    y: metrics.ascent.max(0.0),
                },
                font_id,
                font_size,
                glyph_ids: shaped.glyph_ids,
                advances: shaped.advances,
                text: text.to_owned(),
                source: None,
                color,
                bold,
                italic,
                field_kind: None,
                field_source: None,
                note: None,
                note_reference_source: None,
                tab_aligned: None,
            })]),
        },
        if math_alphabet_covered {
            1.0
        } else {
            MATH_TEXT_X_SCALE
        },
    ))
}

#[allow(clippy::too_many_arguments)]
fn layout_fraction(
    value: &MathFraction,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    let font_id = fm.resolve_font_for_text(font_family, false, false, "+")?;
    let font_data = fm.font_data(font_id)?;
    let face = ttf_parser::Face::parse(&font_data.data, font_data.face_index).ok();
    let constants = face
        .as_ref()
        .and_then(|face| face.tables().math)
        .and_then(|math| math.constants);
    let em = face
        .as_ref()
        .map_or(0.0, |face| font_size / f64::from(face.units_per_em()));
    let child_size = font_size;
    let numerator = layout_argument(
        &value.numerator,
        fm,
        font_family,
        context.with_size(child_size),
        color,
        &format!("{source_path}/numerator"),
        diagnostics,
    )?;
    let denominator = layout_argument(
        &value.denominator,
        fm,
        font_family,
        context.with_size(child_size).cramped(),
        color,
        &format!("{source_path}/denominator"),
        diagnostics,
    )?;
    if matches!(
        value.fraction_type,
        FractionType::Linear | FractionType::Skewed
    ) {
        let slash = layout_text(
            "/",
            fm,
            font_family,
            context,
            color,
            false,
            false,
            source_path,
            diagnostics,
        )?;
        return Ok(layout_sequence(vec![numerator, slash, denominator]));
    }
    let direct_bar = value.numerator.expressions.iter().chain(&value.denominator.expressions)
        .any(|expression| matches!(expression, MathExpression::Fraction(fraction) if fraction.fraction_type == FractionType::Bar));
    let padding = if constants.is_some() && value.fraction_type == FractionType::Bar {
        // Observed nested-bar clearance is atom glue, not a MATH table constant.
        if direct_bar {
            font_size * 2.0 / 18.0
        } else {
            0.0
        }
    } else {
        font_size * 0.06
    };
    let width = numerator.width.max(denominator.width) + padding * 2.0;
    let numerator_x = (width - numerator.width) / 2.0;
    let denominator_x = (width - denominator.width) / 2.0;
    let (rule, rule_y, ascent, descent, numerator_y, denominator_y) = if let Some(c) = constants {
        let rule = if value.fraction_type == FractionType::NoBar {
            0.0
        } else {
            f64::from(c.fraction_rule_thickness().value).max(0.0) * em
        };
        let axis = f64::from(c.axis_height().value) * em;
        let numerator_ink = ink_bounds(&numerator, fm);
        let denominator_ink = ink_bounds(&denominator, fm);
        let (up, down, num_gap, den_gap) = if context.display {
            (
                c.fraction_numerator_display_style_shift_up().value,
                c.fraction_denominator_display_style_shift_down().value,
                c.fraction_num_display_style_gap_min().value,
                c.fraction_denom_display_style_gap_min().value,
            )
        } else {
            (
                c.fraction_numerator_shift_up().value,
                c.fraction_denominator_shift_down().value,
                c.fraction_numerator_gap_min().value,
                c.fraction_denominator_gap_min().value,
            )
        };
        let up = (f64::from(up) * em).max(
            numerator_ink.y + numerator_ink.height - numerator.ascent
                + axis
                + rule / 2.0
                + f64::from(num_gap).max(0.0) * em,
        );
        let down = (f64::from(down) * em).max(
            -axis + rule / 2.0 + f64::from(den_gap).max(0.0) * em - denominator_ink.y
                + denominator.ascent,
        );
        let ascent = (up + numerator.ascent).max(axis + rule / 2.0).max(0.0);
        let descent = (down + denominator.descent).max(0.0);
        (
            rule,
            ascent - axis,
            ascent,
            descent,
            ascent - up - numerator.ascent,
            ascent + down - denominator.ascent,
        )
    } else {
        // Preserve legacy non-MATH baselines while clearing measured ink. Full
        // face metrics contain empty space, so they are not clearance boxes.
        let gap = font_size * GAP_EM;
        let rule = font_size * RULE_EM;
        let numerator_ink = ink_bounds(&numerator, fm);
        let denominator_ink = ink_bounds(&denominator, fm);
        let axis = font_size * 0.25;
        let ascent = (numerator.height() + font_size * 0.55 + rule / 2.0)
            .max(numerator_ink.y + numerator_ink.height + gap + rule / 2.0 + axis);
        let rule_y = ascent - axis;
        let denominator_y =
            (ascent - font_size * 0.36).max(rule_y + rule / 2.0 + gap - denominator_ink.y);
        (
            rule,
            rule_y,
            ascent,
            (denominator_y + denominator.height() - ascent).max(0.0),
            0.0,
            denominator_y,
        )
    };
    let mut children = vec![
        translated(numerator.group, numerator_x, numerator_y),
        translated(denominator.group, denominator_x, denominator_y),
    ];
    if value.fraction_type != FractionType::NoBar {
        children.push(PositionedElement::Line {
            start: Point {
                x: padding / 2.0,
                y: rule_y,
            },
            end: Point {
                x: width - padding / 2.0,
                y: rule_y,
            },
            width: rule,
            color,
            dash_pattern: None,
        });
    }
    Ok(MeasuredMath {
        terminal_italic_advance: 0.0,
        top_padding: numerator.top_padding,
        width,
        ascent,
        descent,
        group: group(children),
    })
}

#[allow(clippy::too_many_arguments)]
fn layout_script(
    value: &MathScript,
    superscript: bool,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let script_context = math_script_context(fm, font_family, context, 1)?;
    let base = layout_argument(
        &value.base,
        fm,
        font_family,
        context,
        color,
        &format!("{source_path}/base"),
        diagnostics,
    )?;
    let script = layout_argument(
        &value.script,
        fm,
        font_family,
        if superscript {
            script_context
        } else {
            script_context.cramped()
        },
        color,
        &format!("{source_path}/script"),
        diagnostics,
    )?;
    layout_script_pair(base, script, superscript, context, fm, font_family)
}

fn layout_script_pair(
    base: MeasuredMath,
    script: MeasuredMath,
    superscript: bool,
    context: MathLayoutStyle,
    fm: &mut FontManager,
    font_family: Option<&str>,
) -> Result<MeasuredMath> {
    let base = base.for_script_attachment();
    let font_size = context.font_size;
    let empty = MeasuredMath::empty();
    let (down, up) = if superscript {
        script_shifts(&base, &empty, &script, fm, font_family, context)?
    } else {
        script_shifts(&base, &script, &empty, fm, font_family, context)?
    };
    let shift = if superscript { up } else { down };
    let (ascent, descent, script_baseline) = if superscript {
        (
            base.ascent.max(shift + script.ascent),
            base.descent.max((script.descent - shift).max(0.0)),
            -shift,
        )
    } else {
        (
            base.ascent.max((script.ascent - shift).max(0.0)),
            base.descent.max(shift + script.descent),
            shift,
        )
    };
    let base_y = ascent - base.ascent;
    let script_y = ascent + script_baseline - script.ascent;
    let correction = italic_correction(&base, fm);
    let gap = if superscript {
        correction.unwrap_or(font_size * GAP_EM / 2.0)
    } else {
        correction.map_or(font_size * GAP_EM / 2.0, |_| 0.0)
    };
    let after = space_after_script(fm, font_family, font_size)?;
    Ok(MeasuredMath {
        terminal_italic_advance: 0.0,
        top_padding: if script_y < base_y {
            script.top_padding
        } else {
            base.top_padding
        },
        width: base.width + gap + script.width + after,
        ascent,
        descent,
        group: group(vec![
            translated(base.group, 0.0, base_y),
            translated(script.group, base.width + gap, script_y),
        ]),
    })
}

#[allow(clippy::too_many_arguments)]
fn layout_sub_superscript(
    value: &MathSubSuperscript,
    pre: bool,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    layout_three_part_script(
        &value.base,
        &value.subscript,
        &value.superscript,
        pre,
        fm,
        font_family,
        context,
        color,
        source_path,
        diagnostics,
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_pre_sub_superscript(
    value: &MathPreSubSuperscript,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    layout_three_part_script(
        &value.base,
        &value.subscript,
        &value.superscript,
        true,
        fm,
        font_family,
        context,
        color,
        source_path,
        diagnostics,
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_three_part_script(
    base_value: &MathArgument,
    subscript_value: &MathArgument,
    superscript_value: &MathArgument,
    pre: bool,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let script_context = math_script_context(fm, font_family, context, 1)?;
    let base = layout_argument(
        base_value,
        fm,
        font_family,
        context,
        color,
        &format!("{source_path}/base"),
        diagnostics,
    )?;
    let subscript = layout_argument(
        subscript_value,
        fm,
        font_family,
        script_context,
        color,
        &format!("{source_path}/subscript"),
        diagnostics,
    )?;
    let superscript = layout_argument(
        superscript_value,
        fm,
        font_family,
        script_context,
        color,
        &format!("{source_path}/superscript"),
        diagnostics,
    )?;
    let shifts = script_shifts(&base, &subscript, &superscript, fm, font_family, context)?;
    let correction = italic_correction(&base, fm);
    let after = space_after_script(fm, font_family, context.font_size)?;
    let mut measured = layout_measured_scripts(
        base,
        subscript,
        superscript,
        pre,
        context,
        shifts,
        correction,
    );
    measured.width += after;
    Ok(measured)
}

fn layout_measured_scripts(
    base: MeasuredMath,
    subscript: MeasuredMath,
    superscript: MeasuredMath,
    pre: bool,
    context: MathLayoutStyle,
    shifts: (f64, f64),
    correction: Option<f64>,
) -> MeasuredMath {
    let base = if pre {
        base
    } else {
        base.for_script_attachment()
    };
    let font_size = context.font_size;
    let (subscript_shift, superscript_shift) = shifts;
    let ascent = base.ascent.max(superscript_shift + superscript.ascent);
    let descent = base.descent.max(subscript_shift + subscript.descent);
    let script_width = subscript
        .width
        .max(superscript.width + correction.unwrap_or(0.0));
    let gap = correction.map_or(font_size * GAP_EM / 2.0, |_| 0.0);
    let upper_correction = correction.unwrap_or(0.0);
    let (script_x, base_x) = if pre {
        (0.0, script_width + gap)
    } else {
        (base.width + gap, 0.0)
    };
    MeasuredMath {
        terminal_italic_advance: 0.0,
        top_padding: if superscript_shift + superscript.ascent > base.ascent {
            superscript.top_padding
        } else {
            base.top_padding
        },
        width: base.width + gap + script_width,
        ascent,
        descent,
        group: group(vec![
            translated(base.group, base_x, ascent - base.ascent),
            translated(
                superscript.group,
                script_x + upper_correction,
                ascent - superscript_shift - superscript.ascent,
            ),
            translated(
                subscript.group,
                script_x,
                ascent + subscript_shift - subscript.ascent,
            ),
        ]),
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_radical(
    value: &MathRadical,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    let base = layout_argument(
        &value.base,
        fm,
        font_family,
        context.cramped(),
        color,
        &format!("{source_path}/base"),
        diagnostics,
    )?;
    let font_id = fm.resolve_font_for_text(font_family, false, false, "√")?;
    let data = fm.font_data(font_id)?;
    let face = ttf_parser::Face::parse(&data.data, data.face_index).ok();
    if let Some((face, c)) = face
        .as_ref()
        .and_then(|face| Some((face, face.tables().math?.constants?)))
    {
        let em = font_size / f64::from(face.units_per_em());
        let ink = ink_bounds(&base, fm);
        let gap = f64::from(if context.display {
            c.radical_display_style_vertical_gap().value
        } else {
            c.radical_vertical_gap().value
        })
        .max(0.0)
            * em;
        let extra = f64::from(c.radical_extra_ascender().value).max(0.0) * em;
        let rule = f64::from(c.radical_rule_thickness().value).max(0.0) * em;
        if let Some(root) = layout_math_variant(
            "√",
            fm,
            font_family,
            context,
            color,
            Some(ink.height + gap + rule),
            source_path,
            diagnostics,
        )? {
            let root_ink = ink_bounds(&root, fm);
            let degree = if value.hide_degree {
                MeasuredMath::empty()
            } else {
                let degree_context = math_script_context(fm, font_family, context, 2)?;
                layout_argument(
                    &value.degree,
                    fm,
                    font_family,
                    degree_context,
                    color,
                    &format!("{source_path}/degree"),
                    diagnostics,
                )?
            };
            let before = f64::from(c.radical_kern_before_degree().value) * em;
            let after = f64::from(c.radical_kern_after_degree().value) * em;
            let root_x = if value.hide_degree {
                0.0
            } else {
                (before + degree.width + after).max(0.0)
            };
            let base_x = root_x + root.width;
            let rise = (gap + rule + extra - ink.y).max(0.0);
            let base_y = rise;
            let rule_y = base_y + ink.y - gap - rule / 2.0;
            let root_y = rule_y - rule / 2.0 - root_ink.y;
            let ascent = base_y + base.ascent;
            let height = (base_y + base.height()).max(root_y + root.height());
            let mut children = vec![
                translated(root.group, root_x, root_y),
                translated(base.group, base_x, base_y),
                PositionedElement::Line {
                    start: Point {
                        x: base_x,
                        y: rule_y,
                    },
                    end: Point {
                        x: base_x + base.width,
                        y: rule_y,
                    },
                    width: rule,
                    color,
                    dash_pattern: None,
                },
            ];
            if !value.hide_degree {
                let degree_y = (root_y
                    + root_ink.y
                    + root_ink.height
                        * (1.0 - f64::from(c.radical_degree_bottom_raise_percent()) / 100.0)
                    - degree.height())
                .max(0.0);
                children.push(translated(degree.group, before.max(0.0), degree_y));
            }
            return Ok(MeasuredMath {
                terminal_italic_advance: 0.0,
                top_padding: extra,
                width: base_x + base.width,
                ascent,
                descent: (height - ascent).max(0.0),
                group: group(children),
            });
        }
    }
    let root = layout_text(
        "√",
        fm,
        font_family,
        context,
        color,
        false,
        false,
        source_path,
        diagnostics,
    )?;
    let gap = font_size * GAP_EM;
    let rule = font_size * RULE_EM;
    let target_height = base.height() + gap + rule;
    let root = vertically_scaled(root, target_height);
    let degree_context = math_script_context(fm, font_family, context, 2)?;
    let degree = if value.hide_degree {
        MeasuredMath::empty()
    } else {
        layout_argument(
            &value.degree,
            fm,
            font_family,
            degree_context,
            color,
            &format!("{source_path}/degree"),
            diagnostics,
        )?
    };
    let degree_overlap = degree.width * 0.35;
    let root_x = (degree.width - degree_overlap).max(0.0);
    let base_x = root_x + root.width;
    let ascent = root.ascent.max(base.ascent + gap + rule);
    let descent = root.descent.max(base.descent);
    let base_y = ascent - base.ascent;
    let mut children = vec![
        translated(root.group, root_x, ascent - root.ascent),
        translated(base.group, base_x, base_y),
        PositionedElement::Line {
            start: Point {
                x: base_x,
                y: base_y - gap / 2.0,
            },
            end: Point {
                x: base_x + base.width,
                y: base_y - gap / 2.0,
            },
            width: rule,
            color,
            dash_pattern: None,
        },
    ];
    if degree.width > 0.0 {
        children.push(translated(degree.group, 0.0, 0.0));
    }
    Ok(horizontally_scaled(
        MeasuredMath {
            terminal_italic_advance: 0.0,
            top_padding: 0.0,
            width: base_x + base.width,
            ascent,
            descent,
            group: group(children),
        },
        0.9,
    ))
}

#[allow(clippy::too_many_arguments)]
fn layout_matrix(
    value: &MathMatrix,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    if value.rows.is_empty() {
        return Ok(MeasuredMath::empty());
    }
    let column_count = value
        .rows
        .iter()
        .map(|row| row.cells.len())
        .max()
        .unwrap_or(0);
    let mut rows = Vec::with_capacity(value.rows.len());
    let mut column_widths = vec![0.0_f64; column_count];
    for (row_index, row) in value.rows.iter().enumerate() {
        let mut cells = Vec::with_capacity(row.cells.len());
        for (column_index, cell) in row.cells.iter().enumerate() {
            let measured = layout_argument(
                cell,
                fm,
                font_family,
                context,
                color,
                &format!("{source_path}/row/{row_index}/cell/{column_index}"),
                diagnostics,
            )?;
            column_widths[column_index] = column_widths[column_index].max(measured.width);
            cells.push(measured);
        }
        rows.push(cells);
    }
    let column_gap = value
        .properties
        .column_spacing
        .map_or(font_size * 1.06, |value| value as f64 / 20.0);
    let row_gap = value
        .properties
        .row_spacing
        .map_or(0.0, |value| value as f64 / 20.0);
    let width =
        column_widths.iter().sum::<f64>() + column_gap * column_count.saturating_sub(1) as f64;
    let row_metrics = rows
        .iter()
        .map(|row| {
            (
                row.iter().map(|cell| cell.ascent).fold(0.0, f64::max),
                row.iter().map(|cell| cell.descent).fold(0.0, f64::max),
            )
        })
        .collect::<Vec<_>>();
    let height = row_metrics
        .iter()
        .map(|(ascent, descent)| ascent + descent)
        .sum::<f64>()
        + row_gap * rows.len().saturating_sub(1) as f64;
    let ascent = match value.properties.base_justification {
        Some(MatrixBaseJustification::Top) => row_metrics[0].0,
        Some(MatrixBaseJustification::Bottom) => {
            height - row_metrics.last().map_or(0.0, |(_, descent)| *descent)
        }
        Some(MatrixBaseJustification::Center) | None => {
            let (row_ascent, row_descent) = row_metrics[0];
            height / 2.0 + (row_ascent - row_descent) / 2.0
        }
    };
    let mut children = Vec::new();
    let mut y = 0.0;
    for (row, (row_ascent, row_descent)) in rows.into_iter().zip(row_metrics) {
        let mut x = 0.0;
        for (column_index, cell) in row.into_iter().enumerate() {
            let cell_x = x + (column_widths[column_index] - cell.width) / 2.0;
            children.push(translated(cell.group, cell_x, y + row_ascent - cell.ascent));
            x += column_widths[column_index] + column_gap;
        }
        y += row_ascent + row_descent + row_gap;
    }
    Ok(MeasuredMath {
        terminal_italic_advance: 0.0,
        top_padding: 0.0,
        width,
        ascent,
        descent: height - ascent,
        group: group(children),
    })
}

#[allow(clippy::too_many_arguments)]
fn layout_limit(
    value: &MathLimit,
    upper: bool,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    let script_context = math_script_context(fm, font_family, context, 1)?;
    let base = layout_argument(
        &value.base,
        fm,
        font_family,
        context,
        color,
        &format!("{source_path}/base"),
        diagnostics,
    )?;
    let limit = layout_argument(
        &value.limit,
        fm,
        font_family,
        script_context,
        color,
        &format!("{source_path}/limit"),
        diagnostics,
    )?;
    let gap = font_size * LIMIT_GAP_EM;
    let width = base.width.max(limit.width);
    if upper {
        let limit_height = limit.height();
        let ascent = limit.height() + gap + base.ascent;
        Ok(MeasuredMath {
            terminal_italic_advance: 0.0,
            top_padding: 0.0,
            width,
            ascent,
            descent: base.descent,
            group: group(vec![
                translated(limit.group, (width - limit.width) / 2.0, 0.0),
                translated(base.group, (width - base.width) / 2.0, limit_height + gap),
            ]),
        })
    } else {
        let ascent = base.ascent;
        let base_height = base.height();
        Ok(MeasuredMath {
            terminal_italic_advance: 0.0,
            top_padding: 0.0,
            width,
            ascent,
            descent: base.descent + gap + limit.height(),
            group: group(vec![
                translated(base.group, (width - base.width) / 2.0, 0.0),
                translated(limit.group, (width - limit.width) / 2.0, base_height + gap),
            ]),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_nary(
    value: &MathNary,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    let script_context = math_script_context(fm, font_family, context, 1)?;
    let operator_text = if value.character.is_empty() {
        "∫"
    } else {
        value.character.as_str()
    };
    let operator = layout_text(
        operator_text,
        fm,
        font_family,
        context,
        color,
        false,
        false,
        source_path,
        diagnostics,
    )?;
    let operator = if value.grow == Some(false) {
        operator
    } else if let Some(variant) = layout_math_variant(
        operator_text,
        fm,
        font_family,
        context,
        color,
        None,
        source_path,
        diagnostics,
    )? {
        variant
    } else {
        horizontally_scaled(operator, NARY_OPERATOR_X_SCALE)
    };
    let base = layout_argument(
        &value.base,
        fm,
        font_family,
        context,
        color,
        &format!("{source_path}/base"),
        diagnostics,
    )?;
    let under_over = value.limit_location == Some(LimitLocation::UnderOver);
    let mut subscript = MeasuredMath::empty();
    let mut superscript = MeasuredMath::empty();
    if !value.hide_subscript && !value.subscript.expressions.is_empty() {
        subscript = layout_argument(
            &value.subscript,
            fm,
            font_family,
            script_context,
            color,
            &format!("{source_path}/subscript"),
            diagnostics,
        )?;
    }
    if !value.hide_superscript && !value.superscript.expressions.is_empty() {
        superscript = layout_argument(
            &value.superscript,
            fm,
            font_family,
            script_context,
            color,
            &format!("{source_path}/superscript"),
            diagnostics,
        )?;
    }
    let decorated = if under_over {
        let mut decorated = operator;
        if subscript.width > 0.0 {
            decorated = stack_below(decorated, subscript, font_size);
        }
        if superscript.width > 0.0 {
            decorated = stack_above(decorated, superscript, font_size);
        }
        decorated
    } else if subscript.width == 0.0 && superscript.width == 0.0 {
        operator
    } else {
        // Both limits attach to the operator, never to the already decorated
        // lower-limit group, whose width includes the lower script.
        let shifts = script_shifts(
            &operator,
            &subscript,
            &superscript,
            fm,
            font_family,
            context,
        )?;
        let correction = italic_correction(&operator, fm);
        let after = space_after_script(fm, font_family, font_size)?;
        let mut measured = layout_measured_scripts(
            operator,
            subscript,
            superscript,
            false,
            context,
            shifts,
            correction,
        );
        if let Some(correction) = correction {
            for child in measured.group.children.iter_mut().skip(1) {
                if let PositionedElement::Group(group) = child {
                    group.transform.e -= correction;
                }
            }
            measured.width -= correction;
        }
        measured.width += after;
        measured
    };
    Ok(layout_sequence(vec![decorated, base]))
}

fn stack_above(base: MeasuredMath, limit: MeasuredMath, font_size: f64) -> MeasuredMath {
    let gap = font_size * NARY_UPPER_GAP_EM;
    let width = base.width.max(limit.width);
    let limit_height = limit.height();
    let ascent = limit_height + gap + base.ascent;
    MeasuredMath {
        terminal_italic_advance: 0.0,
        top_padding: 0.0,
        width,
        ascent,
        descent: base.descent,
        group: group(vec![
            translated(limit.group, (width - limit.width) / 2.0, 0.0),
            translated(base.group, (width - base.width) / 2.0, limit_height + gap),
        ]),
    }
}

fn stack_below(base: MeasuredMath, limit: MeasuredMath, font_size: f64) -> MeasuredMath {
    let gap = font_size * NARY_LOWER_GAP_EM;
    let width = base.width.max(limit.width);
    let base_height = base.height();
    MeasuredMath {
        terminal_italic_advance: 0.0,
        top_padding: 0.0,
        width,
        ascent: base.ascent,
        descent: base.descent + gap + limit.height(),
        group: group(vec![
            translated(base.group, (width - base.width) / 2.0, 0.0),
            translated(limit.group, (width - limit.width) / 2.0, base_height + gap),
        ]),
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_delimiter(
    value: &MathDelimiter,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    let mut arguments = Vec::new();
    for (index, argument) in value.arguments.iter().enumerate() {
        if index > 0 && !value.separator_character.is_empty() {
            arguments.push(layout_text(
                &value.separator_character,
                fm,
                font_family,
                context,
                color,
                false,
                false,
                source_path,
                diagnostics,
            )?);
        }
        arguments.push(layout_argument(
            argument,
            fm,
            font_family,
            context,
            color,
            &format!("{source_path}/argument/{index}"),
            diagnostics,
        )?);
    }
    let contents = layout_sequence(arguments);
    let target_height = if value.grow == Some(false) {
        font_size
    } else {
        contents.height() * 1.08
    };
    let mut pieces = Vec::new();
    if !value.begin_character.is_empty() {
        pieces.push(vertically_scaled(
            layout_text(
                &value.begin_character,
                fm,
                font_family,
                context,
                color,
                false,
                false,
                source_path,
                diagnostics,
            )?,
            target_height,
        ));
    }
    pieces.push(contents);
    if !value.end_character.is_empty() {
        pieces.push(vertically_scaled(
            layout_text(
                &value.end_character,
                fm,
                font_family,
                context,
                color,
                false,
                false,
                source_path,
                diagnostics,
            )?,
            target_height,
        ));
    }
    Ok(layout_sequence(pieces))
}

#[allow(clippy::too_many_arguments)]
fn layout_accent(
    value: &MathAccent,
    fm: &mut FontManager,
    font_family: Option<&str>,
    context: MathLayoutStyle,
    color: Color,
    source_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<MeasuredMath> {
    let font_size = context.font_size;
    let script_context = math_script_context(fm, font_family, context, 1)?;
    let base = layout_argument(
        &value.base,
        fm,
        font_family,
        context,
        color,
        &format!("{source_path}/base"),
        diagnostics,
    )?;
    let accent_text = if value.character.is_empty() {
        "̂"
    } else {
        value.character.as_str()
    };
    let gap = font_size * GAP_EM / 2.0;
    if matches!(accent_text, "¯" | "̅" | "̲") {
        let rule = font_size * RULE_EM;
        return Ok(MeasuredMath {
            terminal_italic_advance: 0.0,
            top_padding: 0.0,
            width: base.width,
            ascent: base.ascent + gap + rule,
            descent: base.descent,
            group: group(vec![
                translated(base.group, 0.0, gap + rule),
                PositionedElement::Line {
                    start: Point {
                        x: 0.0,
                        y: rule / 2.0,
                    },
                    end: Point {
                        x: base.width,
                        y: rule / 2.0,
                    },
                    width: rule,
                    color,
                    dash_pattern: None,
                },
            ]),
        });
    }
    let accent = layout_text(
        accent_text,
        fm,
        font_family,
        script_context,
        color,
        false,
        false,
        source_path,
        diagnostics,
    )?;
    let width = base.width.max(accent.width);
    let accent_raise = font_size * 0.262;
    let ascent = base.ascent + accent_raise;
    Ok(MeasuredMath {
        terminal_italic_advance: 0.0,
        top_padding: 0.0,
        width,
        ascent,
        descent: base.descent.max(accent.descent),
        group: group(vec![
            translated(accent.group, (width - accent.width) / 2.0, 0.0),
            translated(base.group, (width - base.width) / 2.0, accent_raise),
        ]),
    })
}

fn vertically_scaled(value: MeasuredMath, target_height: f64) -> MeasuredMath {
    let height = value.height();
    if height <= 0.0 || target_height <= height {
        return value;
    }
    let scale = (target_height / height).min(MAX_DELIMITER_SCALE);
    MeasuredMath {
        terminal_italic_advance: value.terminal_italic_advance,
        top_padding: value.top_padding * scale,
        width: value.width,
        ascent: value.ascent * scale,
        descent: value.descent * scale,
        group: GroupElement {
            transform: Transform {
                d: scale,
                ..Transform::IDENTITY
            },
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children: value.group.children,
        },
    }
}

fn horizontally_scaled(value: MeasuredMath, scale: f64) -> MeasuredMath {
    if scale <= 0.0 || (scale - 1.0).abs() <= f64::EPSILON {
        return value;
    }
    MeasuredMath {
        terminal_italic_advance: value.terminal_italic_advance * scale,
        top_padding: value.top_padding,
        width: value.width * scale,
        ascent: value.ascent,
        descent: value.descent,
        group: GroupElement {
            transform: Transform {
                a: scale,
                ..Transform::IDENTITY
            },
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children: value.group.children,
        },
    }
}

fn layout_sequence(values: Vec<MeasuredMath>) -> MeasuredMath {
    layout_spaced_sequence(values, 0.0)
}

fn layout_spaced_sequence(values: Vec<MeasuredMath>, spacing: f64) -> MeasuredMath {
    if values.is_empty() {
        return MeasuredMath::empty();
    }
    let ascent = values.iter().map(|value| value.ascent).fold(0.0, f64::max);
    let descent = values.iter().map(|value| value.descent).fold(0.0, f64::max);
    let width = values.iter().map(|value| value.width).sum::<f64>()
        + spacing.max(0.0) * values.len().saturating_sub(1) as f64;
    let top_padding = values
        .iter()
        .max_by(|left, right| left.ascent.total_cmp(&right.ascent))
        .map_or(0.0, |value| value.top_padding);
    let terminal_italic_advance = values
        .last()
        .map_or(0.0, |value| value.terminal_italic_advance);
    let mut x = 0.0;
    let mut children = Vec::with_capacity(values.len());
    for value in values {
        children.push(translated(value.group, x, ascent - value.ascent));
        x += value.width + spacing.max(0.0);
    }
    MeasuredMath {
        terminal_italic_advance,
        top_padding,
        width,
        ascent,
        descent,
        group: group(children),
    }
}

fn group(children: Vec<PositionedElement>) -> GroupElement {
    GroupElement {
        transform: Transform::IDENTITY,
        clip: None,
        opacity: 1.0,
        effects: Vec::new(),
        children,
    }
}

fn translated(mut group: GroupElement, x: f64, y: f64) -> PositionedElement {
    group.transform = group.transform.then(Transform {
        e: x,
        f: y,
        ..Transform::IDENTITY
    });
    PositionedElement::Group(group)
}

fn finite_positive(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}
