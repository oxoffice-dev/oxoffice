//! Page-to-image rendering using tiny-skia software rasterizer.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::io::Cursor;

use jpeg_encoder::{ColorType as JpegColorType, Encoder as JpegEncoder};
use oxoffice_layout::{
    Effect, LayoutResult, LineCap, LineJoin, PageFrame, Paint as LayoutPaint, Path as LayoutPath,
    PathCommand, PositionedElement, Stroke as LayoutStroke,
};
use tiny_skia::{
    FillRule, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, StrokeDash, Transform,
};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Image container and encoding options for raster output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterFormat {
    /// Encode each selected page as a PNG.
    Png {
        /// Leave unpainted pixels transparent instead of compositing on white.
        transparent_background: bool,
    },
    /// Encode each selected page as an opaque JPEG.
    Jpeg {
        /// JPEG quality from 1 through 100.
        quality: u8,
    },
    /// Encode selected pages into one opaque, uncompressed, multi-page TIFF.
    Tiff,
}

/// Options for raster image export.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterOptions {
    /// Dots per inch. Must be finite and positive.
    pub dpi: f64,
    /// Requested image container and format-specific options.
    pub format: RasterFormat,
}

/// Encoded raster output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RasterOutput {
    /// One image per selected page, used for PNG and JPEG.
    SeparatePages(Vec<Vec<u8>>),
    /// One multi-directory TIFF stream for every selected page.
    MultiPageTiff(Vec<u8>),
}

/// A reason a raster export request could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RasterError {
    /// DPI was NaN, infinite, zero, or negative.
    InvalidDpi,
    /// No page was selected.
    EmptyPageSelection,
    /// A selected zero-based page index does not exist.
    PageOutOfRange {
        page_index: usize,
        page_count: usize,
    },
    /// A selected zero-based page index appears more than once.
    DuplicatePage { page_index: usize },
    /// JPEG quality must be in the inclusive 1 through 100 range.
    InvalidJpegQuality { quality: u8 },
    /// The requested raster dimensions cannot be represented.
    InvalidDimensions,
    /// The rendered pixmap could not be encoded into the requested format.
    EncodeFailed { format: &'static str },
}

impl fmt::Display for RasterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDpi => formatter.write_str("raster DPI must be finite and positive"),
            Self::EmptyPageSelection => formatter.write_str("at least one raster page is required"),
            Self::PageOutOfRange {
                page_index,
                page_count,
            } => write!(
                formatter,
                "raster page index {page_index} is out of range for {page_count} pages"
            ),
            Self::DuplicatePage { page_index } => {
                write!(
                    formatter,
                    "raster page index {page_index} was selected more than once"
                )
            }
            Self::InvalidJpegQuality { quality } => {
                write!(formatter, "JPEG quality {quality} is outside 1 through 100")
            }
            Self::InvalidDimensions => {
                formatter.write_str("raster dimensions are invalid for the requested format")
            }
            Self::EncodeFailed { format } => write!(formatter, "{format} raster encoding failed"),
        }
    }
}

impl Error for RasterError {}

/// Render a single page to PNG bytes.
///
/// # Arguments
/// * `layout` - The layout result containing all pages and font data
/// * `page_index` - 0-based page index to render
/// * `dpi` - Dots per inch (72 = 1:1 with points, 150 = 2x, 300 = 4.17x)
pub fn render_page_to_png(layout: &LayoutResult, page_index: usize, dpi: f64) -> Option<Vec<u8>> {
    let page = layout.pages.get(page_index)?;
    let pixmap = render_page_to_pixmap_with_background(page, &layout.fonts, dpi, false)?;
    pixmap.encode_png().ok()
}

/// Render all pages to PNG bytes.
pub fn render_all_pages(layout: &LayoutResult, dpi: f64) -> Vec<Vec<u8>> {
    layout
        .pages
        .iter()
        .filter_map(|page| {
            let pixmap = render_page_to_pixmap_with_background(page, &layout.fonts, dpi, false)?;
            pixmap.encode_png().ok()
        })
        .collect()
}

/// Render selected zero-based pages to the requested raster image format.
pub fn render_pages(
    layout: &LayoutResult,
    page_indices: &[usize],
    options: RasterOptions,
) -> Result<RasterOutput, RasterError> {
    validate_request(layout, page_indices, options)?;

    match options.format {
        RasterFormat::Png {
            transparent_background,
        } => page_indices
            .iter()
            .map(|&page_index| {
                let pixmap = render_selected_pixmap(
                    layout,
                    page_index,
                    options.dpi,
                    transparent_background,
                )?;
                pixmap
                    .encode_png()
                    .map_err(|_| RasterError::EncodeFailed { format: "PNG" })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(RasterOutput::SeparatePages),
        RasterFormat::Jpeg { quality } => page_indices
            .iter()
            .map(|&page_index| {
                let pixmap = render_selected_pixmap(layout, page_index, options.dpi, false)?;
                encode_jpeg(&pixmap, quality)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(RasterOutput::SeparatePages),
        RasterFormat::Tiff => {
            encode_tiff(layout, page_indices, options.dpi).map(RasterOutput::MultiPageTiff)
        }
    }
}

fn validate_request(
    layout: &LayoutResult,
    page_indices: &[usize],
    options: RasterOptions,
) -> Result<(), RasterError> {
    if !options.dpi.is_finite() || options.dpi <= 0.0 {
        return Err(RasterError::InvalidDpi);
    }
    if page_indices.is_empty() {
        return Err(RasterError::EmptyPageSelection);
    }
    if let RasterFormat::Jpeg { quality } = options.format
        && !(1..=100).contains(&quality)
    {
        return Err(RasterError::InvalidJpegQuality { quality });
    }
    let page_count = layout.pages.len();
    let mut seen = HashSet::with_capacity(page_indices.len());
    for &page_index in page_indices {
        if page_index >= page_count {
            return Err(RasterError::PageOutOfRange {
                page_index,
                page_count,
            });
        }
        if !seen.insert(page_index) {
            return Err(RasterError::DuplicatePage { page_index });
        }
    }
    Ok(())
}

fn render_selected_pixmap(
    layout: &LayoutResult,
    page_index: usize,
    dpi: f64,
    transparent_background: bool,
) -> Result<Pixmap, RasterError> {
    let page = layout
        .pages
        .get(page_index)
        .ok_or(RasterError::InvalidDimensions)?;
    render_page_to_pixmap_with_background(page, &layout.fonts, dpi, transparent_background)
        .ok_or(RasterError::InvalidDimensions)
}

fn encode_jpeg(pixmap: &Pixmap, quality: u8) -> Result<Vec<u8>, RasterError> {
    let width = u16::try_from(pixmap.width()).map_err(|_| RasterError::InvalidDimensions)?;
    let height = u16::try_from(pixmap.height()).map_err(|_| RasterError::InvalidDimensions)?;
    let rgb = opaque_rgb(pixmap);
    let mut output = Vec::new();
    JpegEncoder::new(&mut output, quality)
        .encode(&rgb, width, height, JpegColorType::Rgb)
        .map_err(|_| RasterError::EncodeFailed { format: "JPEG" })?;
    Ok(output)
}

fn encode_tiff(
    layout: &LayoutResult,
    page_indices: &[usize],
    dpi: f64,
) -> Result<Vec<u8>, RasterError> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut encoder = tiff::encoder::TiffEncoder::new(&mut cursor)
            .map_err(|_| RasterError::EncodeFailed { format: "TIFF" })?;
        for &page_index in page_indices {
            let pixmap = render_selected_pixmap(layout, page_index, dpi, false)?;
            let rgb = opaque_rgb(&pixmap);
            encoder
                .write_image::<tiff::encoder::colortype::RGB8>(
                    pixmap.width(),
                    pixmap.height(),
                    &rgb,
                )
                .map_err(|_| RasterError::EncodeFailed { format: "TIFF" })?;
        }
    }
    Ok(cursor.into_inner())
}

fn opaque_rgb(pixmap: &Pixmap) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(pixmap.width() as usize * pixmap.height() as usize * 3);
    for pixel in pixmap.pixels() {
        let alpha = pixel.alpha();
        rgb.push(pixel.red().saturating_add(255 - alpha));
        rgb.push(pixel.green().saturating_add(255 - alpha));
        rgb.push(pixel.blue().saturating_add(255 - alpha));
    }
    rgb
}

fn render_page_to_pixmap_with_background(
    page: &PageFrame,
    fonts: &[oxoffice_layout::FontData],
    dpi: f64,
    transparent_background: bool,
) -> Option<Pixmap> {
    let scale = dpi / 72.0; // points to pixels
    let width = (page.width * scale).ceil() as u32;
    let height = (page.height * scale).ceil() as u32;

    let mut pixmap = Pixmap::new(width, height)?;

    if !transparent_background {
        pixmap.fill(tiny_skia::Color::WHITE);
    }

    let transform = Transform::from_scale(scale as f32, scale as f32);

    if let Some(background) = &page.background {
        let path = LayoutPath::rect(oxoffice_layout::Rect {
            x: 0.0,
            y: 0.0,
            width: page.width,
            height: page.height,
        });
        render_path(&mut pixmap, &path, Some(background), None, transform, None);
    }
    render_elements(&mut pixmap, fonts, &page.elements, transform, None);

    Some(pixmap)
}

fn render_elements(
    pixmap: &mut Pixmap,
    fonts: &[oxoffice_layout::FontData],
    elements: &[PositionedElement],
    transform: Transform,
    mask: Option<&Mask>,
) {
    for element in elements {
        match element {
            PositionedElement::FilledRect { rect, color } => {
                let Some(rect) = tiny_skia::Rect::from_xywh(
                    rect.x as f32,
                    rect.y as f32,
                    rect.width as f32,
                    rect.height as f32,
                ) else {
                    continue;
                };
                let mut paint = solid_paint(*color);
                paint.anti_alias = false;
                pixmap.fill_rect(rect, &paint, transform, mask);
            }
            PositionedElement::Line {
                start,
                end,
                width,
                color,
                dash_pattern,
            } => {
                let mut builder = PathBuilder::new();
                builder.move_to(start.x as f32, start.y as f32);
                builder.line_to(end.x as f32, end.y as f32);
                let Some(path) = builder.finish() else {
                    continue;
                };
                let dash = dash_pattern
                    .and_then(|(on, off)| StrokeDash::new(vec![on as f32, off as f32], 0.0));
                let stroke = Stroke {
                    width: *width as f32,
                    dash,
                    ..Stroke::default()
                };
                pixmap.stroke_path(&path, &solid_paint(*color), &stroke, transform, mask);
            }
            PositionedElement::Text(glyph_run) => {
                if let Some(font_data) = fonts.iter().find(|font| font.id == glyph_run.font_id) {
                    render_glyph_run(pixmap, glyph_run, font_data, transform, mask);
                }
            }
            PositionedElement::MultilingualText(glyph_run) => {
                if glyph_run.is_valid()
                    && let Some(font_data) = fonts.iter().find(|font| font.id == glyph_run.font_id)
                {
                    render_positioned_glyph_run(
                        pixmap,
                        &glyph_run.legacy_projection(),
                        font_data,
                        transform,
                        mask,
                        &glyph_run.x_offsets,
                        &glyph_run.y_offsets,
                        &glyph_run.y_advances,
                    );
                }
            }
            PositionedElement::Image {
                rect,
                data,
                content_type,
                ..
            } => {
                if !data.is_empty() {
                    render_image(pixmap, rect, data, content_type, transform, mask);
                }
            }
            PositionedElement::LinkAnnotation { .. } => {}
            PositionedElement::Path(element) => render_path(
                pixmap,
                &element.path,
                element.fill.as_ref(),
                element.stroke.as_ref(),
                transform,
                mask,
            ),
            PositionedElement::Group(group) => {
                let child_transform = transform.pre_concat(layout_transform(group.transform));
                let child_mask = match group.clip.as_ref() {
                    Some(clip) => {
                        let Some(path) = build_path(clip) else {
                            continue;
                        };
                        let mut child_mask = match mask {
                            Some(parent) => parent.clone(),
                            None => {
                                let Some(mask) = Mask::new(pixmap.width(), pixmap.height()) else {
                                    continue;
                                };
                                mask
                            }
                        };
                        if mask.is_some() {
                            child_mask.intersect_path(
                                &path,
                                fill_rule(clip.fill_rule),
                                true,
                                child_transform,
                            );
                        } else {
                            child_mask.fill_path(
                                &path,
                                fill_rule(clip.fill_rule),
                                true,
                                child_transform,
                            );
                        }
                        Some(child_mask)
                    }
                    None => None,
                };
                let child_mask_ref = child_mask.as_ref().or(mask);
                let opacity = if group.opacity.is_finite() {
                    group.opacity.clamp(0.0, 1.0) as f32
                } else {
                    1.0
                };
                if opacity == 0.0 {
                    continue;
                }
                if opacity == 1.0 && group.effects.is_empty() {
                    render_elements(
                        pixmap,
                        fonts,
                        &group.children,
                        child_transform,
                        child_mask_ref,
                    );
                } else if let Some(mut layer) = Pixmap::new(pixmap.width(), pixmap.height()) {
                    render_elements(
                        &mut layer,
                        fonts,
                        &group.children,
                        child_transform,
                        child_mask_ref,
                    );
                    if group.effects.is_empty() {
                        pixmap.draw_pixmap(
                            0,
                            0,
                            layer.as_ref(),
                            &PixmapPaint {
                                opacity,
                                ..PixmapPaint::default()
                            },
                            Transform::identity(),
                            None,
                        );
                    } else if let Some(mut composited) =
                        Pixmap::new(pixmap.width(), pixmap.height())
                    {
                        render_group_effects(
                            &mut composited,
                            &layer,
                            &group.effects,
                            child_transform,
                            mask,
                        );
                        composited.draw_pixmap(
                            0,
                            0,
                            layer.as_ref(),
                            &PixmapPaint::default(),
                            Transform::identity(),
                            None,
                        );
                        apply_linear_opacity_masks(
                            &mut composited,
                            &group.effects,
                            child_transform,
                        );
                        pixmap.draw_pixmap(
                            0,
                            0,
                            composited.as_ref(),
                            &PixmapPaint {
                                opacity,
                                ..PixmapPaint::default()
                            },
                            Transform::identity(),
                            None,
                        );
                    }
                }
            }
            PositionedElement::MarkedContent { children, .. } => {
                render_elements(pixmap, fonts, children, transform, mask);
            }
            _ => {}
        }
    }
}

// Render one mask per effect, not one copy of the children. The mask shader
// uses only alpha, then multiplies the complete premultiplied painted group.
fn apply_linear_opacity_masks(pixmap: &mut Pixmap, effects: &[Effect], transform: Transform) {
    for effect in effects {
        let Effect::LinearOpacityMask {
            bounds,
            start,
            end,
            stops,
        } = effect
        else {
            continue;
        };
        if !effect.linear_opacity_mask_is_valid() {
            pixmap.data_mut().fill(0);
            return;
        }
        let rect = tiny_skia::Rect::from_xywh(
            bounds.x as f32,
            bounds.y as f32,
            bounds.width as f32,
            bounds.height as f32,
        );
        let shader = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(start.x as f32, start.y as f32),
            tiny_skia::Point::from_xy(end.x as f32, end.y as f32),
            stops
                .iter()
                .map(|stop| {
                    tiny_skia::GradientStop::new(
                        stop.offset as f32,
                        tiny_skia::Color::from_rgba(1.0, 1.0, 1.0, stop.color.a as f32).unwrap(),
                    )
                })
                .collect(),
            tiny_skia::SpreadMode::Pad,
            Transform::identity(),
        );
        let Some((rect, shader)) = rect.zip(shader) else {
            pixmap.data_mut().fill(0);
            return;
        };
        let Some(mut alpha) = Pixmap::new(pixmap.width(), pixmap.height()) else {
            pixmap.data_mut().fill(0);
            return;
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        alpha.fill_rect(rect, &paint, transform, None);
        for (pixel, mask) in pixmap
            .data_mut()
            .chunks_exact_mut(4)
            .zip(alpha.data().chunks_exact(4))
        {
            for component in pixel {
                *component = ((u16::from(*component) * u16::from(mask[3]) + 127) / 255) as u8;
            }
        }
    }
}

fn render_group_effects(
    pixmap: &mut Pixmap,
    layer: &Pixmap,
    effects: &[Effect],
    transform: Transform,
    mask: Option<&Mask>,
) {
    for effect in effects {
        if let Effect::OuterShadow {
            dx,
            dy,
            blur,
            color,
        } = effect
        {
            let Some((left, top, right, bottom)) = alpha_bounds(layer) else {
                continue;
            };
            let (scale_x, scale_y) = transform.get_scale();
            let scale = ((scale_x + scale_y) * 0.5).max(0.0);
            let blur_radius = ((*blur as f32 * scale).max(0.0).ceil() as usize)
                .min(layer.width().max(layer.height()) as usize);
            let source_width = right - left + 1;
            let source_height = bottom - top + 1;
            let width = source_width.saturating_add(blur_radius.saturating_mul(2));
            let height = source_height.saturating_add(blur_radius.saturating_mul(2));
            let Some(pixel_count) = width.checked_mul(height) else {
                continue;
            };
            let mut alpha = vec![0_u8; pixel_count];
            for y in 0..source_height {
                let source_start = ((top + y) * layer.width() as usize + left) * 4;
                let source = &layer.data()[source_start..source_start + source_width * 4];
                let target_start = (y + blur_radius) * width + blur_radius;
                for (target, pixel) in alpha[target_start..target_start + source_width]
                    .iter_mut()
                    .zip(source.chunks_exact(4))
                {
                    *target = pixel[3];
                }
            }
            box_blur_alpha(&mut alpha, width, height, blur_radius);

            let (Ok(shadow_width), Ok(shadow_height)) =
                (u32::try_from(width), u32::try_from(height))
            else {
                continue;
            };
            let Some(mut shadow) = Pixmap::new(shadow_width, shadow_height) else {
                continue;
            };
            let color_alpha = color.a.clamp(0.0, 1.0);
            for (pixel, source_alpha) in shadow.data_mut().chunks_exact_mut(4).zip(alpha) {
                let alpha = (f64::from(source_alpha) * color_alpha).round() as u8;
                pixel[0] = (color.r.clamp(0.0, 1.0) * f64::from(alpha)).round() as u8;
                pixel[1] = (color.g.clamp(0.0, 1.0) * f64::from(alpha)).round() as u8;
                pixel[2] = (color.b.clamp(0.0, 1.0) * f64::from(alpha)).round() as u8;
                pixel[3] = alpha;
            }

            let mut origin = tiny_skia::Point::from_xy(0.0, 0.0);
            let mut offset = tiny_skia::Point::from_xy(*dx as f32, *dy as f32);
            transform.map_point(&mut origin);
            transform.map_point(&mut offset);
            pixmap.draw_pixmap(
                left as i32 - blur_radius as i32 + (offset.x - origin.x).round() as i32,
                top as i32 - blur_radius as i32 + (offset.y - origin.y).round() as i32,
                shadow.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                mask,
            );
        }
    }
}

fn alpha_bounds(layer: &Pixmap) -> Option<(usize, usize, usize, usize)> {
    let width = layer.width() as usize;
    let mut left = width;
    let mut top = layer.height() as usize;
    let mut right = 0;
    let mut bottom = 0;
    let mut found = false;
    for (index, pixel) in layer.data().chunks_exact(4).enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        let x = index % width;
        let y = index / width;
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
        found = true;
    }
    found.then_some((left, top, right, bottom))
}

fn box_blur_alpha(alpha: &mut [u8], width: usize, height: usize, radius: usize) {
    if radius == 0 || width == 0 || height == 0 {
        return;
    }
    let mut horizontal = vec![0_u8; alpha.len()];
    let mut prefix = vec![0_u64; width.max(height) + 1];
    let diameter = radius.saturating_mul(2).saturating_add(1) as u64;

    for y in 0..height {
        let row = &alpha[y * width..(y + 1) * width];
        let target = &mut horizontal[y * width..(y + 1) * width];
        prefix[0] = 0;
        for (index, value) in row.iter().enumerate() {
            prefix[index + 1] = prefix[index] + u64::from(*value);
        }
        for (x, target) in target.iter_mut().enumerate() {
            let left = x.saturating_sub(radius);
            let right = (x + radius + 1).min(width);
            *target = ((prefix[right] - prefix[left]) / diameter) as u8;
        }
    }

    for x in 0..width {
        prefix[0] = 0;
        for y in 0..height {
            prefix[y + 1] = prefix[y] + u64::from(horizontal[y * width + x]);
        }
        for y in 0..height {
            let top = y.saturating_sub(radius);
            let bottom = (y + radius + 1).min(height);
            alpha[y * width + x] = ((prefix[bottom] - prefix[top]) / diameter) as u8;
        }
    }
}

fn render_path(
    pixmap: &mut Pixmap,
    path: &LayoutPath,
    fill: Option<&LayoutPaint>,
    stroke: Option<&LayoutStroke>,
    transform: Transform,
    mask: Option<&Mask>,
) {
    let Some(path_geometry) = build_path(path) else {
        return;
    };
    let fill_texture = fill.and_then(grid_texture);
    if let Some(paint) =
        fill.and_then(|paint| raster_paint_with_texture(paint, fill_texture.as_ref()))
    {
        let paint_mask =
            fill.and_then(|paint| gradient_domain_mask(pixmap, paint, transform, mask));
        pixmap.fill_path(
            &path_geometry,
            &paint,
            fill_rule(path.fill_rule),
            transform,
            paint_mask.as_ref().or(mask),
        );
    }
    let stroke_texture = stroke.and_then(|stroke| grid_texture(&stroke.paint));
    if let Some(stroke) = stroke
        && let Some(paint) = raster_paint_with_texture(&stroke.paint, stroke_texture.as_ref())
    {
        let paint_mask = gradient_domain_mask(pixmap, &stroke.paint, transform, mask);
        let stroke = raster_stroke(stroke);
        pixmap.stroke_path(
            &path_geometry,
            &paint,
            &stroke,
            transform,
            paint_mask.as_ref().or(mask),
        );
    }
}

fn build_path(path: &LayoutPath) -> Option<tiny_skia::Path> {
    let mut builder = PathBuilder::new();
    for command in &path.commands {
        match command {
            PathCommand::MoveTo(point) => builder.move_to(point.x as f32, point.y as f32),
            PathCommand::LineTo(point) => builder.line_to(point.x as f32, point.y as f32),
            PathCommand::CurveTo { c1, c2, to } => builder.cubic_to(
                c1.x as f32,
                c1.y as f32,
                c2.x as f32,
                c2.y as f32,
                to.x as f32,
                to.y as f32,
            ),
            PathCommand::Close => builder.close(),
        }
    }
    builder.finish()
}

fn fill_rule(rule: oxoffice_layout::FillRule) -> FillRule {
    match rule {
        oxoffice_layout::FillRule::NonZero => FillRule::Winding,
        oxoffice_layout::FillRule::EvenOdd => FillRule::EvenOdd,
    }
}

fn grid_texture(paint: &LayoutPaint) -> Option<Pixmap> {
    let LayoutPaint::Grid {
        bounds,
        columns,
        rows,
        colors,
    } = paint
    else {
        return None;
    };
    oxoffice_layout::Paint::grid(*bounds, *columns, *rows, colors.clone())?;
    let mut pixmap = Pixmap::new(u32::from(*columns), u32::from(*rows))?;
    for (pixel, color) in pixmap.pixels_mut().iter_mut().zip(colors) {
        let a = (color.a * 255.0).round() as u8;
        *pixel = tiny_skia::PremultipliedColorU8::from_rgba(
            (color.r * f64::from(a)).round() as u8,
            (color.g * f64::from(a)).round() as u8,
            (color.b * f64::from(a)).round() as u8,
            a,
        )?;
    }
    Some(pixmap)
}

fn raster_paint_with_texture<'a>(
    paint: &LayoutPaint,
    texture: Option<&'a Pixmap>,
) -> Option<Paint<'a>> {
    if let LayoutPaint::Grid {
        bounds,
        columns,
        rows,
        ..
    } = paint
    {
        let texture = texture?;
        let transform = Transform::from_row(
            (bounds.width / f64::from(*columns)) as f32,
            0.0,
            0.0,
            (bounds.height / f64::from(*rows)) as f32,
            bounds.x as f32,
            bounds.y as f32,
        );
        return Some(Paint {
            shader: tiny_skia::Pattern::new(
                texture.as_ref(),
                tiny_skia::SpreadMode::Repeat,
                tiny_skia::FilterQuality::Nearest,
                1.0,
                transform,
            ),
            ..Paint::default()
        });
    }
    raster_paint(paint)
}

fn raster_paint(paint: &LayoutPaint) -> Option<Paint<'static>> {
    match paint {
        LayoutPaint::Solid(color) => Some(solid_paint(*color)),
        LayoutPaint::Linear {
            start, end, stops, ..
        } => {
            let shader = tiny_skia::LinearGradient::new(
                tiny_skia::Point::from_xy(start.x as f32, start.y as f32),
                tiny_skia::Point::from_xy(end.x as f32, end.y as f32),
                gradient_stops(stops),
                tiny_skia::SpreadMode::Pad,
                Transform::identity(),
            )?;
            Some(Paint {
                shader,
                ..Paint::default()
            })
        }
        LayoutPaint::Radial {
            center,
            radius,
            focal,
            stops,
            ..
        } => {
            let shader = tiny_skia::RadialGradient::new(
                tiny_skia::Point::from_xy(focal.x as f32, focal.y as f32),
                0.0,
                tiny_skia::Point::from_xy(center.x as f32, center.y as f32),
                *radius as f32,
                gradient_stops(stops),
                tiny_skia::SpreadMode::Pad,
                Transform::identity(),
            )?;
            Some(Paint {
                shader,
                ..Paint::default()
            })
        }
        LayoutPaint::Tile { .. } | LayoutPaint::Grid { .. } => None,
    }
}

fn gradient_domain_mask(
    pixmap: &Pixmap,
    paint: &LayoutPaint,
    transform: Transform,
    parent: Option<&Mask>,
) -> Option<Mask> {
    let path = match paint {
        LayoutPaint::Linear {
            start, end, extend, ..
        } if *extend != (true, true) => linear_domain_path(
            pixmap.width(),
            pixmap.height(),
            *start,
            *end,
            *extend,
            transform,
        )?,
        LayoutPaint::Radial {
            center,
            radius,
            extend,
            ..
        } if !extend.1 => build_path(&LayoutPath::ellipse(oxoffice_layout::Rect {
            x: center.x - radius,
            y: center.y - radius,
            width: radius * 2.0,
            height: radius * 2.0,
        }))?,
        _ => return None,
    };
    let mut domain = match parent {
        Some(parent) => parent.clone(),
        None => Mask::new(pixmap.width(), pixmap.height())?,
    };
    if parent.is_some() {
        domain.intersect_path(&path, FillRule::Winding, true, transform);
    } else {
        domain.fill_path(&path, FillRule::Winding, true, transform);
    }
    Some(domain)
}

fn linear_domain_path(
    width: u32,
    height: u32,
    start: oxoffice_layout::Point,
    end: oxoffice_layout::Point,
    extend: (bool, bool),
    transform: Transform,
) -> Option<tiny_skia::Path> {
    let inverse = transform.invert()?;
    let mut corners = [
        tiny_skia::Point::from_xy(0.0, 0.0),
        tiny_skia::Point::from_xy(width as f32, 0.0),
        tiny_skia::Point::from_xy(width as f32, height as f32),
        tiny_skia::Point::from_xy(0.0, height as f32),
    ];
    inverse.map_points(&mut corners);

    let dx = (end.x - start.x) as f32;
    let dy = (end.y - start.y) as f32;
    let length_squared = dx * dx + dy * dy;
    if !length_squared.is_finite() || length_squared <= f32::EPSILON {
        return None;
    }
    let length = length_squared.sqrt();
    let normal_x = -dy / length;
    let normal_y = dx / length;
    let mut min_t = f32::INFINITY;
    let mut max_t = f32::NEG_INFINITY;
    let mut min_normal = f32::INFINITY;
    let mut max_normal = f32::NEG_INFINITY;
    for corner in corners {
        let x = corner.x - start.x as f32;
        let y = corner.y - start.y as f32;
        let t = (x * dx + y * dy) / length_squared;
        let normal = x * normal_x + y * normal_y;
        min_t = min_t.min(t);
        max_t = max_t.max(t);
        min_normal = min_normal.min(normal);
        max_normal = max_normal.max(normal);
    }
    let min_t = if extend.0 { min_t - 1.0 } else { 0.0 };
    let max_t = if extend.1 { max_t + 1.0 } else { 1.0 };
    let min_normal = min_normal - 1.0;
    let max_normal = max_normal + 1.0;
    let point = |t: f32, normal: f32| {
        tiny_skia::Point::from_xy(
            start.x as f32 + dx * t + normal_x * normal,
            start.y as f32 + dy * t + normal_y * normal,
        )
    };
    let mut builder = PathBuilder::new();
    let first = point(min_t, min_normal);
    builder.move_to(first.x, first.y);
    for corner in [
        point(max_t, min_normal),
        point(max_t, max_normal),
        point(min_t, max_normal),
    ] {
        builder.line_to(corner.x, corner.y);
    }
    builder.close();
    builder.finish()
}

fn gradient_stops(stops: &[oxoffice_layout::GradientStop]) -> Vec<tiny_skia::GradientStop> {
    let mut normalized: Vec<(f64, oxoffice_layout::Color)> = stops
        .iter()
        .map(|stop| {
            let offset = if stop.offset.is_nan() {
                0.0
            } else {
                stop.offset.clamp(0.0, 1.0)
            };
            (offset, stop.color)
        })
        .collect();
    normalized.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut deduplicated: Vec<(f64, oxoffice_layout::Color)> = Vec::with_capacity(normalized.len());
    for stop in normalized {
        if let Some(previous) = deduplicated.last_mut()
            && previous.0 == stop.0
        {
            *previous = stop;
        } else {
            deduplicated.push(stop);
        }
    }

    deduplicated
        .into_iter()
        .map(|(offset, color)| tiny_skia::GradientStop::new(offset as f32, skia_color(color)))
        .collect()
}

fn solid_paint(color: oxoffice_layout::Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(
        (color.r.clamp(0.0, 1.0) * 255.0) as u8,
        (color.g.clamp(0.0, 1.0) * 255.0) as u8,
        (color.b.clamp(0.0, 1.0) * 255.0) as u8,
        (color.a.clamp(0.0, 1.0) * 255.0) as u8,
    );
    paint
}

fn skia_color(color: oxoffice_layout::Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba(
        color.r.clamp(0.0, 1.0) as f32,
        color.g.clamp(0.0, 1.0) as f32,
        color.b.clamp(0.0, 1.0) as f32,
        color.a.clamp(0.0, 1.0) as f32,
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT)
}

fn raster_stroke(stroke: &LayoutStroke) -> Stroke {
    Stroke {
        width: stroke.width as f32,
        line_cap: match stroke.cap {
            LineCap::Butt => tiny_skia::LineCap::Butt,
            LineCap::Round => tiny_skia::LineCap::Round,
            LineCap::Square => tiny_skia::LineCap::Square,
        },
        line_join: match stroke.join {
            LineJoin::Miter => tiny_skia::LineJoin::Miter,
            LineJoin::Round => tiny_skia::LineJoin::Round,
            LineJoin::Bevel => tiny_skia::LineJoin::Bevel,
        },
        dash: stroke.dash.as_ref().and_then(|dash| {
            StrokeDash::new(dash.iter().map(|value| *value as f32).collect(), 0.0)
        }),
        ..Stroke::default()
    }
}

fn layout_transform(transform: oxoffice_layout::Transform) -> Transform {
    Transform::from_row(
        transform.a as f32,
        transform.b as f32,
        transform.c as f32,
        transform.d as f32,
        transform.e as f32,
        transform.f as f32,
    )
}

/// Render a glyph run by extracting glyph outlines from the font.
fn render_glyph_run(
    pixmap: &mut Pixmap,
    glyph_run: &oxoffice_layout::GlyphRun,
    font_data: &oxoffice_layout::FontData,
    transform: Transform,
    mask: Option<&Mask>,
) {
    render_positioned_glyph_run(pixmap, glyph_run, font_data, transform, mask, &[], &[], &[]);
}

#[allow(clippy::too_many_arguments)]
fn render_positioned_glyph_run(
    pixmap: &mut Pixmap,
    glyph_run: &oxoffice_layout::GlyphRun,
    font_data: &oxoffice_layout::FontData,
    transform: Transform,
    mask: Option<&Mask>,
    x_offsets: &[f64],
    y_offsets: &[f64],
    y_advances: &[f64],
) {
    let Some(face) = crate::font::instanced_face(font_data) else {
        return;
    };

    let upem = face.units_per_em() as f64;
    let scale = glyph_run.font_size / upem;

    let mut paint = Paint::default();
    paint.set_color_rgba8(
        (glyph_run.color.r * 255.0) as u8,
        (glyph_run.color.g * 255.0) as u8,
        (glyph_run.color.b * 255.0) as u8,
        (glyph_run.color.a * 255.0) as u8,
    );
    paint.anti_alias = true;

    let mut x = glyph_run.origin.x;
    let mut y = glyph_run.origin.y;

    for (i, &glyph_id) in glyph_run.glyph_ids.iter().enumerate() {
        let gid = ttf_parser::GlyphId(glyph_id);

        // Build path from glyph outline
        let mut builder = GlyphPathBuilder::new();
        if face.outline_glyph(gid, &mut builder).is_some()
            && let Some(path) = builder.finish()
        {
            // Transform: translate to glyph position, scale from font units to points,
            // and flip Y (font coordinates are Y-up, pixmap is Y-down)
            let position_x = x + x_offsets.get(i).copied().unwrap_or(0.0);
            let position_y = y - y_offsets.get(i).copied().unwrap_or(0.0);
            let glyph_transform = if font_data.synthetic_italic {
                transform
                    .pre_translate(glyph_run.origin.x as f32, glyph_run.origin.y as f32)
                    .pre_concat(Transform::from_row(1.0, 0.0, -0.212_556_56, 1.0, 0.0, 0.0))
                    .pre_translate(
                        (position_x - glyph_run.origin.x) as f32,
                        (position_y - glyph_run.origin.y) as f32,
                    )
            } else {
                transform.pre_translate(position_x as f32, position_y as f32)
            };
            let outline_transform = glyph_transform.pre_scale(scale as f32, -(scale as f32));
            pixmap.fill_path(&path, &paint, FillRule::Winding, outline_transform, mask);
            if font_data.synthetic_bold
                && let Some(path) =
                    path.transform(Transform::from_scale(scale as f32, scale as f32))
            {
                // Round joins use a fixed path-unit tolerance in tiny-skia.
                // Stroke in points so the tolerance matches other painted paths.
                let stroke = Stroke {
                    width: (glyph_run.font_size / 24.0) as f32,
                    line_cap: tiny_skia::LineCap::Round,
                    line_join: tiny_skia::LineJoin::Round,
                    ..Stroke::default()
                };
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &stroke,
                    glyph_transform.pre_scale(1.0, -1.0),
                    mask,
                );
            }
        }

        if i < glyph_run.advances.len() {
            x += glyph_run.advances[i];
        }
        y -= y_advances.get(i).copied().unwrap_or(0.0);
    }
}

/// Convert a glyph outline to a tiny-skia Path.
struct GlyphPathBuilder {
    pb: PathBuilder,
}

impl GlyphPathBuilder {
    fn new() -> Self {
        GlyphPathBuilder {
            pb: PathBuilder::new(),
        }
    }

    fn finish(self) -> Option<tiny_skia::Path> {
        self.pb.finish()
    }
}

impl ttf_parser::OutlineBuilder for GlyphPathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.pb.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.pb.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.pb.close();
    }
}

/// Render an image onto the pixmap.
fn render_image(
    pixmap: &mut Pixmap,
    rect: &oxoffice_layout::Rect,
    data: &[u8],
    content_type: &str,
    transform: Transform,
    mask: Option<&Mask>,
) {
    // Decode the image
    let decoded = crate::image::decode_image(data, content_type);
    let Some(decoded) = decoded else {
        return;
    };

    // Create a pixmap from the decoded image data
    let img_pixmap = if decoded.is_jpeg {
        decode_jpeg_pixmap(data, decoded.width, decoded.height)
    } else if decoded.color_space == "DeviceRGB" {
        // Convert RGB to RGBA
        let mut rgba = Vec::with_capacity(decoded.width as usize * decoded.height as usize * 4);
        if let Some(alpha) = &decoded.alpha {
            for (rgb, &a) in decoded.data.chunks_exact(3).zip(alpha.iter()) {
                let color = tiny_skia::ColorU8::from_rgba(rgb[0], rgb[1], rgb[2], a).premultiply();
                rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
            }
        } else {
            for rgb in decoded.data.chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        let size = tiny_skia::IntSize::from_wh(decoded.width, decoded.height);
        size.and_then(|s| Pixmap::from_vec(rgba, s))
    } else if decoded.color_space == "DeviceGray" {
        // Convert grayscale to RGBA
        let mut rgba = Vec::with_capacity(decoded.width as usize * decoded.height as usize * 4);
        for &gray in &decoded.data {
            rgba.push(gray);
            rgba.push(gray);
            rgba.push(gray);
            rgba.push(255);
        }
        let size = tiny_skia::IntSize::from_wh(decoded.width, decoded.height);
        size.and_then(|s| Pixmap::from_vec(rgba, s))
    } else {
        return;
    };

    let Some(img_pixmap) = img_pixmap else {
        return;
    };

    // Calculate the transform to position and scale the image. A zero
    // dimension from a malformed image would make this an infinite scale.
    if decoded.width == 0 || decoded.height == 0 {
        return;
    }
    let sx = rect.width as f32 / decoded.width as f32;
    let sy = rect.height as f32 / decoded.height as f32;

    let img_transform = transform
        .pre_translate(rect.x as f32, rect.y as f32)
        .pre_scale(sx, sy);

    let pattern = tiny_skia::Pattern::new(
        img_pixmap.as_ref(),
        tiny_skia::SpreadMode::Pad,
        tiny_skia::FilterQuality::Bilinear,
        1.0,
        Transform::identity(),
    );

    let paint = Paint {
        shader: pattern,
        ..Paint::default()
    };

    let fill_rect =
        tiny_skia::Rect::from_xywh(0.0, 0.0, decoded.width as f32, decoded.height as f32);
    if let Some(fill_rect) = fill_rect {
        pixmap.fill_rect(fill_rect, &paint, img_transform, mask);
    }
}

fn decode_jpeg_pixmap(data: &[u8], width: u32, height: u32) -> Option<Pixmap> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
    let rgba = decoder.decode().ok()?;
    let (decoded_width, decoded_height) = decoder.dimensions()?;
    if decoded_width != width as usize || decoded_height != height as usize {
        return None;
    }
    let size = tiny_skia::IntSize::from_wh(width, height)?;
    Pixmap::from_vec(rgba, size)
}
