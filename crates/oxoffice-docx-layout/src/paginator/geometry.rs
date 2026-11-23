//! Page geometry: the body band, columns, page borders and line numbering of a section.

use super::*;

/// One resolved column track in points, left to right.
#[derive(Debug, Clone, Copy)]
pub struct ColumnTrack {
    /// Left edge, measured from the page's left edge.
    pub x: f64,
    /// Track measure.
    pub width: f64,
}

/// A section's page border frame, resolved to points.
#[derive(Debug, Clone)]
pub struct PageBorderFrame {
    /// Which physical pages of the section carry the frame.
    pub display: ST_PageBorderDisplay,
    /// Whether the edge offsets are measured from the page or from the text.
    pub offset_from: ST_PageBorderOffset,
    /// Whether the frame draws in front of the page content.
    pub in_front: bool,
    /// The four edges, reusing the paragraph border carrier so the frame goes
    /// through the same renderer.
    pub edges: CT_PBdr,
}

/// A section's margin line numbering.
#[derive(Debug, Clone)]
pub struct LineNumbering {
    /// Interval at which a number is printed.
    pub count_by: u32,
    /// First line number of the section.
    pub start: u32,
    /// Gap between the number and the track it labels, in points.
    pub distance: f64,
    /// Where numbering starts over.
    pub restart: ST_LineNumberRestart,
    /// Number font, resolved before pagination because the paginator holds the
    /// font manager immutably. `None` when no font resolved, and then nothing
    /// is drawn.
    pub font_id: Option<oxoffice_layout::FontId>,
}

/// Page geometry derived from section properties.
#[derive(Debug, Clone)]
pub struct PageGeometry {
    pub page_width: f64,
    pub page_height: f64,
    pub margin_top: f64,
    pub margin_right: f64,
    pub margin_bottom: f64,
    pub margin_left: f64,
    pub header_distance: f64,
    pub footer_distance: f64,
    /// Resolved column tracks, left to right.
    ///
    /// **Empty for a single-column section**, which is what keeps that section
    /// on the exact `text_measure` expression it has always used. A neutral
    /// one-track form would reassociate the arithmetic and move every
    /// recorded baseline in the workspace.
    pub columns: Vec<ColumnTrack>,
    /// Whether a rule is drawn between the tracks.
    pub column_separator: bool,
    /// The page border frame, when the section declares one.
    pub page_borders: Option<PageBorderFrame>,
    /// Margin line numbering, when the section declares it.
    pub line_numbers: Option<LineNumbering>,
    /// Vertical alignment of the body band within the page.
    ///
    /// `None` and `Top` take the untouched code path, and so does `Both`,
    /// which preserves its source value and lays out as `Top`.
    pub vertical_alignment: Option<ST_VerticalJc>,
    /// Whether even displayed pages swap their inside and outside margins.
    ///
    /// The gutter is already folded into the inside margin when this is set,
    /// so the swap alone puts it on the binding edge of either page.
    pub mirror_margins: bool,
    /// Whether two consecutive paragraphs are separated by the sum of the
    /// first one's space after and the second one's space before, rather than
    /// by the larger of the two.
    ///
    /// A document setting, carried with the geometry like `mirror_margins`
    /// because every pagination pass reads the geometry.
    pub do_not_use_html_paragraph_auto_spacing: bool,
    /// Degrees the section's body band rotates for `w:sectPr/w:textDirection`.
    ///
    /// `None` is the horizontal section, which keeps the band and the
    /// arithmetic it always had. This is the render projection over the authored
    /// property.
    pub body_rotation: Option<f64>,
}

impl PageGeometry {
    /// Content area width, which is one column track's measure.
    pub fn content_width(&self) -> f64 {
        match self.columns.first() {
            Some(track) => track.width,
            None => self.text_measure(),
        }
    }

    /// The full text measure, ignoring any column tracks.
    ///
    /// Headers, footers and page furniture span the text width whatever the
    /// body's columns do.
    pub fn text_measure(&self) -> f64 {
        self.page_width - self.margin_left - self.margin_right
    }

    /// Content area height.
    pub fn content_height(&self) -> f64 {
        self.page_height - self.margin_top - self.margin_bottom
    }

    /// The measure body blocks are broken at.
    ///
    /// A section whose `w:textDirection` is vertical lays its body out in a
    /// same-centre transposed box, so the measure runs down the page instead
    /// of across it. Every other section takes `content_width` unchanged.
    pub fn body_measure(&self) -> f64 {
        match self.body_rotation {
            Some(_) => self.content_height(),
            None => self.content_width(),
        }
    }

    /// The same page with its column tracks removed.
    pub(super) fn without_columns(&self) -> PageGeometry {
        PageGeometry {
            columns: Vec::new(),
            column_separator: false,
            ..self.clone()
        }
    }
}

impl Default for PageGeometry {
    fn default() -> Self {
        // US Letter with 1" margins
        PageGeometry {
            page_width: 612.0,
            page_height: 792.0,
            margin_top: 72.0,
            margin_right: 72.0,
            margin_bottom: 72.0,
            margin_left: 72.0,
            header_distance: 36.0,
            footer_distance: 36.0,
            columns: Vec::new(),
            column_separator: false,
            page_borders: None,
            line_numbers: None,
            vertical_alignment: None,
            mirror_margins: false,
            do_not_use_html_paragraph_auto_spacing: false,
            body_rotation: None,
        }
    }
}
