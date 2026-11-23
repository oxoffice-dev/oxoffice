//! CSS generation from OXML properties.

use oxoffice_docx_layout::Color;
use oxoffice_docx_layout::engine::{automatic_text_color, run_fill, shading_color};
use oxoffice_docx_oxml::properties::{CT_PPr, CT_RPr, CT_Shd};
use oxoffice_docx_oxml::shared::{ST_Jc, ST_Underline};

use crate::sanitize::{font_family, hex_color};

/// Generate base CSS styles for the HTML document.
pub(crate) fn generate_base_css() -> String {
    r#"body { font-family: 'Calibri', 'Arial', sans-serif; font-size: 11pt; line-height: 1.4; margin: 1in; color: #000; }
h1, h2, h3, h4, h5, h6 { margin-top: 0.5em; margin-bottom: 0.3em; }
h1 { font-size: 2em; }
h2 { font-size: 1.5em; }
h3 { font-size: 1.17em; }
h4 { font-size: 1em; }
h5 { font-size: 0.83em; }
h6 { font-size: 0.67em; }
p { margin: 0 0 8pt 0; }
table { border-collapse: collapse; margin: 8pt 0; }
td, th { padding: 4pt 6pt; vertical-align: top; }
ul, ol { margin: 4pt 0; padding-left: 36pt; }
a { color: #0563C1; text-decoration: underline; }"#
        .to_string()
}

/// Generate inline CSS style string from paragraph properties.
pub(crate) fn paragraph_style(ppr: Option<&CT_PPr>) -> String {
    let Some(ppr) = ppr else {
        return String::new();
    };

    let mut styles = Vec::new();

    // Text alignment
    if let Some(jc) = &ppr.jc {
        let align = match jc {
            // `start` and `end` follow the paragraph's direction, as its
            // `dir` attribute gives it.
            ST_Jc::Left => "left",
            ST_Jc::Start => "start",
            ST_Jc::Center => "center",
            ST_Jc::Right => "right",
            ST_Jc::End => "end",
            ST_Jc::Both | ST_Jc::Distribute => "justify",
        };
        styles.push(format!("text-align:{align}"));
    }

    // Spacing before/after
    if let Some(sp) = ppr.space_before {
        let pt = sp.to_pt();
        if pt > 0.0 {
            styles.push(format!("margin-top:{pt:.1}pt"));
        }
    }
    if let Some(sp) = ppr.space_after {
        let pt = sp.to_pt();
        styles.push(format!("margin-bottom:{pt:.1}pt"));
    }

    // Indentation
    if let Some(ind) = ppr.ind_left {
        let pt = ind.to_pt();
        if pt > 0.0 {
            styles.push(format!("margin-left:{pt:.1}pt"));
        }
    }
    if let Some(ind) = ppr.ind_right {
        let pt = ind.to_pt();
        if pt > 0.0 {
            styles.push(format!("margin-right:{pt:.1}pt"));
        }
    }
    // Word ignores `w:firstLine` when `w:hanging` is there.
    if let Some(ind) = ppr.ind_hanging {
        let pt = ind.to_pt();
        if pt > 0.0 {
            styles.push(format!("text-indent:-{pt:.1}pt"));
        }
    } else if let Some(ind) = ppr.ind_first_line {
        let pt = ind.to_pt();
        if pt > 0.0 {
            styles.push(format!("text-indent:{pt:.1}pt"));
        }
    }

    // Background shading, as layout paints it.
    if let Some(color) = ppr.shading.as_deref().and_then(shading_color) {
        styles.push(format!("background-color:{}", css_color(color)));
    }

    // Line spacing
    if let Some(ls) = ppr.line_spacing {
        let rule = ppr.line_rule.as_deref().unwrap_or("auto");
        match rule {
            "auto" => {
                let factor = ls.0 as f64 / 240.0;
                if (factor - 1.0).abs() > 0.01 {
                    styles.push(format!("line-height:{factor:.2}"));
                }
            }
            "exact" | "atLeast" => {
                let pt = ls.to_pt();
                styles.push(format!("line-height:{pt:.1}pt"));
            }
            _ => {}
        }
    }

    if styles.is_empty() {
        String::new()
    } else {
        styles.join(";")
    }
}

/// The background a `w:shd` paints, as layout paints it, as a CSS colour.
pub(crate) fn shading_background(shading: Option<&CT_Shd>) -> Option<String> {
    shading.and_then(shading_color).map(css_color)
}

/// The fill a paragraph paints behind its runs: its own shading, or else
/// `behind`, the fill of the cell it is in.
pub(crate) fn paragraph_fill(ppr: Option<&CT_PPr>, behind: Option<Color>) -> Option<Color> {
    ppr.and_then(|ppr| ppr.shading.as_deref())
        .and_then(shading_color)
        .or(behind)
}

/// A colour as `#RRGGBB`.
fn css_color(color: Color) -> String {
    format!("#{}", color.to_hex())
}

/// Generate inline CSS style string from run properties. `behind` is the
/// fill of the paragraph or cell the run is in: automatic text is white on
/// a dark fill, as layout draws it.
pub(crate) fn run_style(rpr: Option<&CT_RPr>, behind: Option<Color>) -> String {
    let automatic_white = |fill: Option<Color>| automatic_text_color(fill) == Color::WHITE;
    let Some(rpr) = rpr else {
        return if automatic_white(behind) {
            "color:#FFFFFF".to_owned()
        } else {
            String::new()
        };
    };

    let mut styles = Vec::new();

    // Font family
    if let Some(family) = rpr.font_ascii.as_deref().and_then(font_family) {
        styles.push(format!("font-family:{family}"));
    }

    // Font size (half-points → pt)
    if let Some(sz) = rpr.sz {
        let pt = sz.0 as f64 / 2.0;
        styles.push(format!("font-size:{pt}pt"));
    }

    // The fill behind the text: the highlight, which Word draws over the
    // shading, or else the shading, as layout paints them.
    let fill = run_fill(rpr);

    // Color
    let automatic = rpr.color_theme.is_none()
        && rpr
            .color
            .as_deref()
            .is_none_or(|raw| raw.eq_ignore_ascii_case("auto"));
    if let Some(raw) = &rpr.color
        && raw != "000000"
        && let Some(color) = hex_color(raw)
    {
        styles.push(format!("color:{color}"));
    } else if automatic && automatic_white(fill.or(behind)) {
        styles.push("color:#FFFFFF".to_owned());
    }

    if let Some(fill) = fill {
        styles.push(format!("background-color:{}", css_color(fill)));
    }

    // Character spacing
    if let Some(spacing) = rpr.spacing {
        let pt = spacing.to_pt();
        if pt.abs() > 0.01 {
            styles.push(format!("letter-spacing:{pt:.1}pt"));
        }
    }

    // Small caps
    if rpr.small_caps == Some(true) {
        styles.push("font-variant:small-caps".to_string());
    }

    // All caps
    if rpr.caps == Some(true) {
        styles.push("text-transform:uppercase".to_string());
    }

    if styles.is_empty() {
        String::new()
    } else {
        styles.join(";")
    }
}

/// Determine which semantic HTML tags to wrap around run content.
pub(crate) struct RunTags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub superscript: bool,
    pub subscript: bool,
}

pub(crate) fn run_tags(rpr: Option<&CT_RPr>) -> RunTags {
    let Some(rpr) = rpr else {
        return RunTags {
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            superscript: false,
            subscript: false,
        };
    };

    RunTags {
        bold: rpr.bold == Some(true),
        italic: rpr.italic == Some(true),
        // `w:u w:val="none"` turns an underline off, as Google Docs writes it
        // on every list item.
        underline: rpr
            .underline
            .is_some_and(|underline| underline != ST_Underline::None),
        strike: rpr.strike == Some(true) || rpr.dstrike == Some(true),
        superscript: rpr.vert_align.as_deref() == Some("superscript"),
        subscript: rpr.vert_align.as_deref() == Some("subscript"),
    }
}
