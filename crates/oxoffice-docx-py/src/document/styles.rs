//! Style snapshots and the formatting a new or changed style takes.

use super::*;

#[pyclass(name = "Style", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyStyle {
    pub style_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub style_type: String,
    pub linked_style: Option<String>,
    pub next_style: Option<String>,
    pub priority: Option<u32>,
    pub auto_redefine: Option<bool>,
    pub hidden: Option<bool>,
    pub semi_hidden: Option<bool>,
    pub unhide_when_used: Option<bool>,
    pub quick_format: Option<bool>,
    pub locked: Option<bool>,
    pub is_default: bool,
}

#[pymethods]
impl PyStyle {
    #[new]
    #[pyo3(signature = (*, style_id, name, based_on, style_type, linked_style, next_style, priority, auto_redefine, hidden, semi_hidden, unhide_when_used, quick_format, locked, is_default))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        style_id: String,
        name: Option<String>,
        based_on: Option<String>,
        style_type: String,
        linked_style: Option<String>,
        next_style: Option<String>,
        priority: Option<u32>,
        auto_redefine: Option<bool>,
        hidden: Option<bool>,
        semi_hidden: Option<bool>,
        unhide_when_used: Option<bool>,
        quick_format: Option<bool>,
        locked: Option<bool>,
        is_default: bool,
    ) -> Self {
        Self {
            style_id,
            name,
            based_on,
            style_type,
            linked_style,
            next_style,
            priority,
            auto_redefine,
            hidden,
            semi_hidden,
            unhide_when_used,
            quick_format,
            locked,
            is_default,
        }
    }
}

/// `set_style` line spacing. Omission keeps the stored line and rule.
/// Explicit None is `Given(None)` and clears both. `add_style` never stores
/// a clear: None and omission both stay unset on a new style.
pub(super) enum StyleLineSpacingArg {
    Omitted,
    Given(Option<Py<PyAny>>),
}

impl<'a, 'py> FromPyObject<'a, 'py> for StyleLineSpacingArg {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        if obj.is_none() {
            Ok(Self::Given(None))
        } else {
            Ok(Self::Given(Some(obj.to_owned().unbind())))
        }
    }
}

impl StyleFormatting {
    pub(super) fn run_properties(
        &self,
        py: Python<'_>,
        existing: Option<&oxoffice_docx::CT_RPr>,
    ) -> PyResult<Option<oxoffice_docx::CT_RPr>> {
        let supplied = self.font_name.is_some()
            || self.font_size.is_some()
            || self.bold.is_some()
            || self.italic.is_some()
            || self.color.is_some()
            || self.underline.is_some();
        if !supplied {
            return Ok(None);
        }
        let mut properties = existing.cloned().unwrap_or_default();
        if let Some(name) = &self.font_name {
            properties.font_ascii = Some(name.clone());
            properties.font_hansi = Some(name.clone());
            properties.font_east_asia = Some(name.clone());
            properties.font_cs = Some(name.clone());
        }
        if let Some(emu) = self.font_size {
            let half_points = emu / 6350;
            let size = u32::try_from(half_points).map_err(|_| {
                PyValueError::new_err("style font_size must fit nonnegative half-points")
            })?;
            if emu < 0 {
                return Err(PyValueError::new_err("style font_size cannot be negative"));
            }
            properties.sz = Some(oxoffice_docx::HalfPoint(size));
            properties.sz_cs = Some(oxoffice_docx::HalfPoint(size));
        }
        if let Some(value) = self.bold {
            properties.bold = Some(value);
            properties.bold_cs = Some(value);
        }
        if let Some(value) = self.italic {
            properties.italic = Some(value);
            properties.italic_cs = Some(value);
        }
        if let Some((r, g, b)) = self.color {
            properties.color = Some(format!("{r:02X}{g:02X}{b:02X}"));
        }
        if let Some(value) = &self.underline {
            let value = value.bind(py);
            let code = if value.is_instance_of::<PyBool>() {
                i32::from(value.extract::<bool>()?)
            } else {
                value.extract::<i32>()?
            };
            use oxoffice_docx_oxml::shared::ST_Underline;
            properties.underline = Some(match code {
                0 => ST_Underline::None,
                1 => ST_Underline::Single,
                2 => ST_Underline::Words,
                3 => ST_Underline::Double,
                4 => ST_Underline::Dotted,
                6 => ST_Underline::Thick,
                7 => ST_Underline::Dash,
                9 => ST_Underline::DotDash,
                10 => ST_Underline::DotDotDash,
                11 => ST_Underline::Wave,
                _ => return Err(PyValueError::new_err("unsupported underline style")),
            });
        }
        Ok(Some(properties))
    }

    pub(super) fn paragraph_properties(
        &self,
        py: Python<'_>,
        existing: Option<&oxoffice_docx::CT_PPr>,
    ) -> PyResult<Option<oxoffice_docx::CT_PPr>> {
        let other = self.space_before.is_some()
            || self.space_after.is_some()
            || self.left_indent.is_some()
            || self.right_indent.is_some()
            || self.first_line_indent.is_some()
            || self.alignment.is_some()
            || self.keep_with_next.is_some()
            || self.keep_together.is_some()
            || self.page_break_before.is_some()
            || self.borders.is_some()
            || self.shading.is_some()
            || self.tab_stops.is_some();
        // Clearing a line that is already absent must not create pPr.
        if !other
            && self.line_spacing.as_ref().is_some_and(Option::is_none)
            && existing.is_none_or(|properties| {
                properties.line_spacing.is_none() && properties.line_rule.is_none()
            })
        {
            return Ok(None);
        }
        let supplied = other || self.line_spacing.is_some();
        if !supplied {
            return Ok(None);
        }
        let mut properties = existing.cloned().unwrap_or_default();
        let twips = |emu: i64| -> PyResult<oxoffice_docx::Twips> {
            i32::try_from(emu / 635)
                .map(oxoffice_docx::Twips)
                .map_err(|_| PyValueError::new_err("style length exceeds signed twip range"))
        };
        if let Some(value) = self.space_before {
            properties.space_before = Some(twips(value)?);
        }
        if let Some(value) = self.space_after {
            properties.space_after = Some(twips(value)?);
        }
        if let Some(value) = self.left_indent {
            properties.ind_left = Some(twips(value)?);
        }
        if let Some(value) = self.right_indent {
            properties.ind_right = Some(twips(value)?);
        }
        if let Some(value) = self.first_line_indent {
            let value = twips(value)?.0;
            properties.ind_first_line = (value >= 0).then_some(oxoffice_docx::Twips(value));
            properties.ind_hanging = if value < 0 {
                Some(oxoffice_docx::Twips(value.checked_abs().ok_or_else(
                    || PyValueError::new_err("style hanging indent exceeds signed twip range"),
                )?))
            } else {
                None
            };
        }
        if let Some(value) = self.alignment {
            properties.jc = Some(match crate::formatting::alignment_from_int(value)? {
                oxoffice_docx::Alignment::Left => oxoffice_docx_oxml::ST_Jc::Left,
                oxoffice_docx::Alignment::Center => oxoffice_docx_oxml::ST_Jc::Center,
                oxoffice_docx::Alignment::Right => oxoffice_docx_oxml::ST_Jc::Right,
                oxoffice_docx::Alignment::Justify => oxoffice_docx_oxml::ST_Jc::Both,
            });
        }
        if let Some(spacing) = &self.line_spacing {
            match spacing {
                None => {
                    properties.line_spacing = None;
                    properties.line_rule = None;
                }
                Some(value) => {
                    let value = value.bind(py);
                    let message = "style line_spacing requires a numeric line multiple or a Length";
                    if value.is_instance_of::<PyBool>() {
                        return Err(PyTypeError::new_err(message));
                    }
                    if value.is_instance(&py.import("oxoffice.docx")?.getattr("Length")?)? {
                        properties.line_spacing = Some(twips(value.extract()?)?);
                        properties.line_rule = Some("exact".to_owned());
                    } else if value.is_instance_of::<PyInt>() || value.is_instance_of::<PyFloat>() {
                        let spacing = oxoffice_docx::auto_line_spacing(
                            crate::formatting::require_auto_line_spacing(value)?,
                        );
                        let kept = properties.line_rule.is_none()
                            && properties.line_spacing == Some(spacing);
                        properties.line_spacing = Some(spacing);
                        if !kept {
                            properties.line_rule = Some("auto".to_owned());
                        }
                    } else {
                        return Err(PyTypeError::new_err(message));
                    }
                }
            }
        }
        if let Some(value) = self.keep_with_next {
            properties.keep_next = Some(value);
        }
        if let Some(value) = self.keep_together {
            properties.keep_lines = Some(value);
        }
        if let Some(value) = self.page_break_before {
            properties.page_break_before = Some(value);
        }
        if let Some(value) = &self.shading {
            let fill = crate::formatting::paragraph_color_hex(value.bind(py), "style shading")?;
            let shading = properties.shading.get_or_insert_with(|| {
                Box::new(oxoffice_docx_oxml::properties::CT_Shd {
                    val: "clear".to_owned(),
                    color: Some("auto".to_owned()),
                    ..Default::default()
                })
            });
            shading.fill = Some(fill);
            shading.theme_fill = None;
            shading.theme_fill_tint = None;
            shading.theme_fill_shade = None;
        }
        if let Some(value) = &self.borders {
            let mapping = value.bind(py).cast::<PyMapping>()?;
            let items = mapping
                .items()?
                .extract::<Vec<(String, (String, u32, Py<PyAny>))>>()?;
            if items.is_empty() {
                properties.borders = None;
            } else {
                for (edge, (style, size, color)) in items {
                    let edge = crate::formatting::paragraph_border_edge(&edge)?;
                    let style = crate::table::border_style_from_name(&style)?;
                    let color = crate::formatting::paragraph_color_hex(
                        color.bind(py),
                        "style borders color",
                    )?;
                    let borders = properties
                        .borders
                        .get_or_insert_with(|| Box::new(oxoffice_docx_oxml::CT_PBdr::default()));
                    let slot = match edge {
                        oxoffice_docx::ParagraphBorderEdge::Top => &mut borders.top,
                        oxoffice_docx::ParagraphBorderEdge::Bottom => &mut borders.bottom,
                        oxoffice_docx::ParagraphBorderEdge::Left => &mut borders.left,
                        oxoffice_docx::ParagraphBorderEdge::Right => &mut borders.right,
                        oxoffice_docx::ParagraphBorderEdge::Between => &mut borders.between,
                        oxoffice_docx::ParagraphBorderEdge::Bar => &mut borders.bar,
                    };
                    use oxoffice_docx_oxml::ST_Border;
                    let style = match style {
                        oxoffice_docx::BorderStyle::None => ST_Border::None,
                        oxoffice_docx::BorderStyle::Single => ST_Border::Single,
                        oxoffice_docx::BorderStyle::Thick => ST_Border::Thick,
                        oxoffice_docx::BorderStyle::Double => ST_Border::Double,
                        oxoffice_docx::BorderStyle::Dotted => ST_Border::Dotted,
                        oxoffice_docx::BorderStyle::Dashed => ST_Border::Dashed,
                        oxoffice_docx::BorderStyle::DotDash => ST_Border::DotDash,
                        oxoffice_docx::BorderStyle::Wave => ST_Border::Wave,
                    };
                    let border =
                        slot.get_or_insert_with(|| oxoffice_docx_oxml::CT_BorderEdge::new(style));
                    border.val = style;
                    border.sz = Some(size);
                    border.color = Some(color);
                }
            }
        }
        if let Some(value) = &self.tab_stops {
            let entries = value.bind(py).extract::<Vec<(i64, i32, Option<i32>)>>()?;
            let mut tabs = Vec::with_capacity(entries.len());
            for (position, alignment, leader) in entries {
                use oxoffice_docx_oxml::{ST_TabJc, ST_TabLeader};
                let val = match crate::formatting::tab_alignment_from_int(alignment)? {
                    oxoffice_docx::TabAlignment::Left => ST_TabJc::Left,
                    oxoffice_docx::TabAlignment::Center => ST_TabJc::Center,
                    oxoffice_docx::TabAlignment::Right => ST_TabJc::Right,
                    oxoffice_docx::TabAlignment::Decimal => ST_TabJc::Decimal,
                    oxoffice_docx::TabAlignment::Bar => ST_TabJc::Bar,
                    oxoffice_docx::TabAlignment::Clear => ST_TabJc::Clear,
                    oxoffice_docx::TabAlignment::Num => ST_TabJc::Num,
                    oxoffice_docx::TabAlignment::List => ST_TabJc::List,
                    oxoffice_docx::TabAlignment::Start => ST_TabJc::Start,
                    oxoffice_docx::TabAlignment::End => ST_TabJc::End,
                };
                let leader =
                    crate::formatting::tab_leader_from_int(leader)?.map(|value| match value {
                        oxoffice_docx::TabLeader::None => ST_TabLeader::None,
                        oxoffice_docx::TabLeader::Dot => ST_TabLeader::Dot,
                        oxoffice_docx::TabLeader::Hyphen => ST_TabLeader::Hyphen,
                        oxoffice_docx::TabLeader::Underscore => ST_TabLeader::Underscore,
                        oxoffice_docx::TabLeader::Heavy => ST_TabLeader::Heavy,
                        oxoffice_docx::TabLeader::MiddleDot => ST_TabLeader::MiddleDot,
                    });
                let mut tab = oxoffice_docx_oxml::CT_TabStop::new(val, twips(position)?);
                tab.leader = leader;
                tabs.push(tab);
            }
            tabs.sort_by_key(|tab| tab.pos.0);
            properties.tabs = (!tabs.is_empty()).then_some(oxoffice_docx_oxml::CT_Tabs { tabs });
            properties
                .refresh_tabs_source(None)
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
        }
        Ok(Some(properties))
    }
}

/// The style ID Word derives from a style name: the name's ASCII letters,
/// digits and hyphens, so "Q&A" gives `QA`. A name with none of them, such as
/// a Japanese one, gets the first of `a`, `a0`, `a1` and so on that `document`
/// does not use, as Word numbers them. As in python-docx, Word's lowercase
/// built-in names `caption` and `heading 1` to `heading 9` keep their
/// capitalised IDs.
pub(super) fn style_id_from_name(document: &oxoffice_docx::Document, name: &str) -> String {
    match (name, name.strip_prefix("heading ")) {
        ("caption", _) => return "Caption".to_owned(),
        (_, Some(level)) if matches!(level.as_bytes(), [b'1'..=b'9']) => {
            return format!("Heading{level}");
        }
        _ => {}
    }
    let kept = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect::<String>();
    if !kept.is_empty() {
        return kept;
    }
    let mut fallback = "a".to_owned();
    let mut number = 0;
    while document.style(&fallback).is_some() {
        fallback = format!("a{number}");
        number += 1;
    }
    fallback
}
