//! Options of pictures, text boxes and watermarks, and their DrawingML values.

use super::*;

/// Horizontal frame used to position a floating Word drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingHorizontalRelativeFrom {
    Page,
    Margin,
    Column,
    Character,
    LeftMargin,
    RightMargin,
    InsideMargin,
    OutsideMargin,
}

/// Vertical frame used to position a floating Word drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingVerticalRelativeFrom {
    Page,
    Margin,
    Paragraph,
    Line,
    TopMargin,
    BottomMargin,
    InsideMargin,
    OutsideMargin,
}

/// Horizontal alignment used instead of a floating drawing offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingHorizontalAlignment {
    Left,
    Center,
    Right,
    Inside,
    Outside,
}

/// Vertical alignment used instead of a floating drawing offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingVerticalAlignment {
    Top,
    Center,
    Bottom,
    Inside,
    Outside,
}

/// Text wrapping policy for a floating Word drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawingWrap {
    #[default]
    None,
    Square,
    TopAndBottom,
    Tight,
    Through,
}

/// Picture crop in DrawingML thousandths of one percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PictureCrop {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Floating placement for a picture or text box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PictureAnchor {
    pub horizontal_relative_from: DrawingHorizontalRelativeFrom,
    pub horizontal_offset: Length,
    pub horizontal_alignment: Option<DrawingHorizontalAlignment>,
    pub vertical_relative_from: DrawingVerticalRelativeFrom,
    pub vertical_offset: Length,
    pub vertical_alignment: Option<DrawingVerticalAlignment>,
    pub wrap: DrawingWrap,
    pub distance_top: Length,
    pub distance_bottom: Length,
    pub distance_left: Length,
    pub distance_right: Length,
    pub relative_height: u32,
    pub behind_text: bool,
}

/// Options for one story-scoped picture.
#[derive(Debug, Clone, PartialEq)]
pub struct PictureOptions {
    pub width: Length,
    pub height: Length,
    pub crop: Option<PictureCrop>,
    pub anchor: Option<PictureAnchor>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub title: Option<String>,
    pub decorative: bool,
}

/// Text flow direction inside an authored Word text box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextBoxDirection {
    #[default]
    Horizontal,
    Vertical,
    Vertical270,
}

/// Options for one floating Word text box.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBoxOptions {
    pub width: Length,
    pub height: Length,
    pub anchor: PictureAnchor,
    pub rotation_degrees: f64,
    pub text_direction: TextBoxDirection,
    pub fill_color: Option<String>,
}

/// Appearance of one section-variant image watermark.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageWatermarkOptions {
    pub width: Length,
    pub height: Length,
    pub rotation_degrees: f64,
    pub opacity: f64,
}

/// Appearance of one section-variant text watermark.
#[derive(Debug, Clone, PartialEq)]
pub struct TextWatermarkOptions {
    pub width: Length,
    pub height: Length,
    pub rotation_degrees: f64,
    pub color: String,
    pub font_family: Option<String>,
    pub opacity: f64,
}

impl Default for TextWatermarkOptions {
    fn default() -> Self {
        Self {
            width: Length::pt(468.0),
            height: Length::pt(117.0),
            rotation_degrees: 315.0,
            color: "D9D9D9".to_owned(),
            font_family: Some("Calibri".to_owned()),
            opacity: 0.5,
        }
    }
}

impl From<DrawingHorizontalRelativeFrom> for ST_RelativeFromH {
    fn from(value: DrawingHorizontalRelativeFrom) -> Self {
        match value {
            DrawingHorizontalRelativeFrom::Page => Self::Page,
            DrawingHorizontalRelativeFrom::Margin => Self::Margin,
            DrawingHorizontalRelativeFrom::Column => Self::Column,
            DrawingHorizontalRelativeFrom::Character => Self::Character,
            DrawingHorizontalRelativeFrom::LeftMargin => Self::LeftMargin,
            DrawingHorizontalRelativeFrom::RightMargin => Self::RightMargin,
            DrawingHorizontalRelativeFrom::InsideMargin => Self::InsideMargin,
            DrawingHorizontalRelativeFrom::OutsideMargin => Self::OutsideMargin,
        }
    }
}

impl From<DrawingVerticalRelativeFrom> for ST_RelativeFromV {
    fn from(value: DrawingVerticalRelativeFrom) -> Self {
        match value {
            DrawingVerticalRelativeFrom::Page => Self::Page,
            DrawingVerticalRelativeFrom::Margin => Self::Margin,
            DrawingVerticalRelativeFrom::Paragraph => Self::Paragraph,
            DrawingVerticalRelativeFrom::Line => Self::Line,
            DrawingVerticalRelativeFrom::TopMargin => Self::TopMargin,
            DrawingVerticalRelativeFrom::BottomMargin => Self::BottomMargin,
            DrawingVerticalRelativeFrom::InsideMargin => Self::InsideMargin,
            DrawingVerticalRelativeFrom::OutsideMargin => Self::OutsideMargin,
        }
    }
}

impl From<DrawingHorizontalAlignment> for AnchorAlignH {
    fn from(value: DrawingHorizontalAlignment) -> Self {
        match value {
            DrawingHorizontalAlignment::Left => Self::Left,
            DrawingHorizontalAlignment::Center => Self::Center,
            DrawingHorizontalAlignment::Right => Self::Right,
            DrawingHorizontalAlignment::Inside => Self::Inside,
            DrawingHorizontalAlignment::Outside => Self::Outside,
        }
    }
}

impl From<DrawingVerticalAlignment> for AnchorAlignV {
    fn from(value: DrawingVerticalAlignment) -> Self {
        match value {
            DrawingVerticalAlignment::Top => Self::Top,
            DrawingVerticalAlignment::Center => Self::Center,
            DrawingVerticalAlignment::Bottom => Self::Bottom,
            DrawingVerticalAlignment::Inside => Self::Inside,
            DrawingVerticalAlignment::Outside => Self::Outside,
        }
    }
}

impl From<DrawingWrap> for WrapType {
    fn from(value: DrawingWrap) -> Self {
        match value {
            DrawingWrap::None => Self::None,
            DrawingWrap::Square => Self::Square,
            DrawingWrap::TopAndBottom => Self::TopAndBottom,
            DrawingWrap::Tight => Self::Tight,
            DrawingWrap::Through => Self::Through,
        }
    }
}
