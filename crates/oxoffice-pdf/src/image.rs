//! Image embedding for PDF output (JPEG pass-through, PNG decompression).

use oxoffice_media::{ImageFormat, probe};

/// Decoded image data ready for PDF embedding.
pub(crate) struct DecodedImage {
    /// Raw pixel data (RGB or grayscale) or JPEG bytes for pass-through.
    pub data: Vec<u8>,
    /// Alpha channel data (if present), for a soft mask.
    pub alpha: Option<Vec<u8>>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Color space: "DeviceRGB", "DeviceGray".
    pub color_space: &'static str,
    /// Whether data is raw JPEG (pass through with DCTDecode).
    pub is_jpeg: bool,
}

/// The shared decoder's paint eligibility. Classification may allocate a checked raster.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageDrawClass {
    Drawn,
    UnsupportedFormat { kind: &'static str },
    CouldNotDecode,
    NotThisDecoder,
}

/// Classify actual image bytes before considering a declared metafile MIME type.
pub fn classify_image_draw(data: &[u8], content_type: &str) -> ImageDrawClass {
    use ImageDrawClass::*;
    match ImageFormat::sniff(data) {
        Some(ImageFormat::Png | ImageFormat::Jpeg) => {
            if decode_image(data, content_type).is_some() {
                Drawn
            } else {
                CouldNotDecode
            }
        }
        Some(ImageFormat::Emf) => {
            if decode_emf(data).is_some() {
                Drawn
            } else {
                UnsupportedFormat { kind: "emf" }
            }
        }
        Some(ImageFormat::Wmf) => UnsupportedFormat { kind: "wmf" },
        Some(_) => NotThisDecoder,
        None => match content_type {
            "image/emf" | "image/x-emf" => UnsupportedFormat { kind: "emf" },
            "image/wmf" | "image/x-wmf" => UnsupportedFormat { kind: "wmf" },
            _ => NotThisDecoder,
        },
    }
}

/// Decode image bytes into a format suitable for PDF embedding.
///
/// JPEG images are passed through directly (the PDF viewer decodes them).
/// PNG images are decoded to raw RGB/RGBA pixels.
pub(crate) fn decode_image(data: &[u8], content_type: &str) -> Option<DecodedImage> {
    match ImageFormat::sniff(data) {
        Some(ImageFormat::Png) => decode_png(data),
        Some(ImageFormat::Jpeg) => decode_jpeg(data),
        Some(ImageFormat::Emf) => decode_emf(data),
        _ if content_type.contains("jpeg") || content_type.contains("jpg") => decode_jpeg(data),
        _ => decode_png_or_other(data),
    }
}

fn decode_jpeg(data: &[u8]) -> Option<DecodedImage> {
    let info = probe(data)?;
    if info.format != ImageFormat::Jpeg {
        return None;
    }

    Some(DecodedImage {
        data: data.to_vec(),
        alpha: None,
        width: info.width_px,
        height: info.height_px,
        color_space: "DeviceRGB",
        is_jpeg: true,
    })
}

fn decode_png_or_other(data: &[u8]) -> Option<DecodedImage> {
    // Use a minimal PNG decoder. We need to decode to raw pixels.
    // For simplicity, we'll parse the PNG ourselves for common cases,
    // or fall back to a basic RGBA decode.
    decode_png(data)
}

/// Minimal PNG decoding to raw RGB + optional alpha.
fn decode_png(data: &[u8]) -> Option<DecodedImage> {
    // Validate PNG signature
    if data.len() < 8 || &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }

    let mut pos = 8;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut idat_data = Vec::new();
    // Palette entries as RGB triples, for indexed images.
    let mut palette: Vec<u8> = Vec::new();
    // Per-palette-entry alpha. Only meaningful for indexed images, where tRNS
    // is a list of alpha bytes indexed the same way as PLTE.
    let mut palette_alpha: Vec<u8> = Vec::new();

    while pos + 8 <= data.len() {
        let chunk_len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let chunk_type = &data[pos + 4..pos + 8];
        let chunk_data_start = pos + 8;
        let chunk_data_end = chunk_data_start.checked_add(chunk_len)?;
        let chunk_end = chunk_data_end.checked_add(4)?;
        if chunk_end > data.len() {
            return None;
        }

        match chunk_type {
            b"IHDR" => {
                if chunk_len >= 13 {
                    let d = &data[chunk_data_start..];
                    width = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
                    height = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
                    bit_depth = d[8];
                    color_type = d[9];
                    interlace = d[12];
                }
            }
            b"IDAT" => {
                idat_data.extend_from_slice(&data[chunk_data_start..chunk_data_end]);
            }
            b"PLTE" => {
                palette.clear();
                palette.extend_from_slice(&data[chunk_data_start..chunk_data_end]);
            }
            b"tRNS" => {
                palette_alpha.clear();
                palette_alpha.extend_from_slice(&data[chunk_data_start..chunk_data_end]);
            }
            b"IEND" => break,
            _ => {}
        }

        pos = chunk_end;
    }

    if width == 0 || height == 0 || idat_data.is_empty() || bit_depth != 8 || interlace > 1 {
        return None;
    }

    // Decompress the IDAT data (zlib deflate)
    let decompressed = miniz_oxide::inflate::decompress_to_vec_zlib(&idat_data).ok()?;

    // Unfilter the scanlines
    let channels: usize = match color_type {
        0 => 1, // Grayscale
        2 => 3, // RGB
        3 => 1, // Indexed: one palette index per pixel
        4 => 2, // Grayscale + Alpha
        6 => 4, // RGBA
        _ => return None,
    };

    let width_usize = usize::try_from(width).ok()?;
    let height_usize = usize::try_from(height).ok()?;
    let stride = width_usize.checked_mul(channels)?;
    let output_bytes = stride.checked_mul(height_usize)?;
    let passes: &[(usize, usize, usize, usize)] = if interlace == 1 {
        &[
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ]
    } else {
        &[(0, 0, 1, 1)]
    };
    let mut expected = 0usize;
    for &(x0, y0, dx, dy) in passes {
        if x0 >= width_usize || y0 >= height_usize {
            continue;
        }
        let pass_width = (width_usize - 1 - x0) / dx + 1;
        let pass_height = (height_usize - 1 - y0) / dy + 1;
        expected = expected.checked_add(
            pass_width
                .checked_mul(channels)?
                .checked_add(1)?
                .checked_mul(pass_height)?,
        )?;
    }
    if decompressed.len() < expected {
        return None;
    }
    let mut unfiltered = vec![0u8; output_bytes];
    let mut cursor = 0usize;
    for &(x0, y0, dx, dy) in passes {
        if x0 >= width_usize || y0 >= height_usize {
            continue;
        }
        let pass_width = (width_usize - 1 - x0) / dx + 1;
        let pass_stride = pass_width.checked_mul(channels)?;
        let mut previous = vec![0u8; pass_stride];
        let mut row = vec![0u8; pass_stride];
        for y in (y0..height_usize).step_by(dy) {
            let filter = *decompressed.get(cursor)?;
            cursor = cursor.checked_add(1)?;
            let end = cursor.checked_add(pass_stride)?;
            let raw = decompressed.get(cursor..end)?;
            // Keep the established noninterlaced unknown-filter passthrough.
            // Adam7 has no such legacy representation and rejects bad filters.
            let filter = if interlace == 0 && filter > 4 {
                0
            } else {
                filter
            };
            unfilter_png_row(raw, &previous, &mut row, channels, filter)?;
            for (column, x) in (x0..width_usize).step_by(dx).enumerate() {
                let destination = y * stride + x * channels;
                unfiltered[destination..destination + channels]
                    .copy_from_slice(&row[column * channels..(column + 1) * channels]);
            }
            std::mem::swap(&mut row, &mut previous);
            cursor = end;
        }
    }

    // Separate color and alpha channels
    match color_type {
        0 => {
            // Grayscale
            Some(DecodedImage {
                data: unfiltered,
                alpha: None,
                width,
                height,
                color_space: "DeviceGray",
                is_jpeg: false,
            })
        }
        2 => {
            // RGB
            Some(DecodedImage {
                data: unfiltered,
                alpha: None,
                width,
                height,
                color_space: "DeviceRGB",
                is_jpeg: false,
            })
        }
        3 => {
            // Indexed colour: every byte is an index into the palette.
            // Word embeds these routinely, because most tools default to a
            // palette when an image has few colours.
            if palette.len() < 3 {
                return None;
            }
            let entries = palette.len() / 3;
            let pixel_count = unfiltered.len() / channels;
            let mut rgb = Vec::with_capacity(pixel_count * 3);
            let mut alpha = Vec::with_capacity(pixel_count);
            let mut all_opaque = true;
            for &index in unfiltered.iter().take(pixel_count) {
                let i = index as usize;
                if i < entries {
                    rgb.extend_from_slice(&palette[i * 3..i * 3 + 3]);
                } else {
                    // An index past the end of the palette is malformed. Emit
                    // black rather than dropping the whole image.
                    rgb.extend_from_slice(&[0, 0, 0]);
                }
                // tRNS may cover only the first few entries. Anything it does
                // not mention is opaque.
                let a = palette_alpha.get(i).copied().unwrap_or(255);
                if a != 255 {
                    all_opaque = false;
                }
                alpha.push(a);
            }
            Some(DecodedImage {
                data: rgb,
                // Match the RGBA path: no SMask when nothing is transparent.
                alpha: if all_opaque { None } else { Some(alpha) },
                width,
                height,
                color_space: "DeviceRGB",
                is_jpeg: false,
            })
        }
        4 => {
            // Grayscale + Alpha
            let pixel_count = unfiltered.len() / channels;
            let mut gray = Vec::with_capacity(pixel_count);
            let mut alpha = Vec::with_capacity(pixel_count);
            for i in 0..pixel_count {
                gray.push(unfiltered[i * 2]);
                alpha.push(unfiltered[i * 2 + 1]);
            }
            Some(DecodedImage {
                data: gray,
                alpha: Some(alpha),
                width,
                height,
                color_space: "DeviceGray",
                is_jpeg: false,
            })
        }
        6 => {
            // RGBA
            let pixel_count = unfiltered.len() / channels;
            let mut rgb = Vec::with_capacity(pixel_count * 3);
            let mut alpha = Vec::with_capacity(pixel_count);
            let mut all_opaque = true;
            for i in 0..pixel_count {
                rgb.push(unfiltered[i * 4]);
                rgb.push(unfiltered[i * 4 + 1]);
                rgb.push(unfiltered[i * 4 + 2]);
                let a = unfiltered[i * 4 + 3];
                alpha.push(a);
                if a != 255 {
                    all_opaque = false;
                }
            }
            // Skip alpha channel if fully opaque — avoids unnecessary SMask
            // that can cause subtle color rendering differences in PDF viewers
            Some(DecodedImage {
                data: rgb,
                alpha: if all_opaque { None } else { Some(alpha) },
                width,
                height,
                color_space: "DeviceRGB",
                is_jpeg: false,
            })
        }
        _ => None,
    }
}

fn unfilter_png_row(
    raw: &[u8],
    previous: &[u8],
    out: &mut [u8],
    channels: usize,
    filter: u8,
) -> Option<()> {
    for (i, &value) in raw.iter().enumerate() {
        let left = if i >= channels { out[i - channels] } else { 0 };
        let up = previous[i];
        let upper_left = if i >= channels {
            previous[i - channels]
        } else {
            0
        };
        let predictor = match filter {
            0 => 0,
            1 => left,
            2 => up,
            3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
            4 => paeth_predictor(i32::from(left), i32::from(up), i32::from(upper_left)),
            _ => return None,
        };
        out[i] = value.wrapping_add(predictor);
    }
    Some(())
}

// The player uses deterministic raster approximations, not exact native GDI coverage.
fn emf_rgba_bytes(width: u32, height: u32) -> Option<usize> {
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(4)?;
    if bytes == 0 || bytes > 64 * 1024 * 1024 {
        return None;
    }
    usize::try_from(bytes).ok()
}
#[derive(Clone, Copy)]
struct EmfPen {
    color: tiny_skia::Color,
    width: f64,
    geometric: bool,
    style: u32,
}
impl EmfPen {
    fn cosmetic(color: tiny_skia::Color) -> Self {
        Self {
            color,
            width: 0.0,
            geometric: false,
            style: 0,
        }
    }
}
fn decode_emf(data: &[u8]) -> Option<DecodedImage> {
    let info = probe(data)?;
    if info.format != ImageFormat::Emf {
        return None;
    }
    let declared = usize::try_from(emf_u32(data, 48)?).ok()?;
    let data = data.get(..declared)?;
    let scale = (1024 / info.width_px.max(info.height_px)).clamp(1, 8);
    let (out_w, out_h) = (
        info.width_px.checked_mul(scale)?,
        info.height_px.checked_mul(scale)?,
    );
    emf_rgba_bytes(out_w, out_h)?;
    let bounds = (emf_i32(data, 8)?, emf_i32(data, 12)?);
    let positive_bounds = i64::from(emf_i32(data, 16)?) - i64::from(bounds.0) + 1 > 0
        && i64::from(emf_i32(data, 20)?) - i64::from(bounds.1) + 1 > 0;
    let origin = if positive_bounds {
        (f64::from(bounds.0), f64::from(bounds.1))
    } else {
        (
            f64::from(emf_i32(data, 24)?) * 96.0 / 2540.0,
            f64::from(emf_i32(data, 28)?) * 96.0 / 2540.0,
        )
    };
    // Invalid reference-device metrics use an explicit96dpi mapping approximation.
    let dpi = |pixels, mm| {
        if pixels > 0 && mm > 0 {
            f64::from(pixels) * 25.4 / f64::from(mm)
        } else {
            96.0
        }
    };
    let dpi = (
        dpi(emf_i32(data, 72)?, emf_i32(data, 80)?),
        dpi(emf_i32(data, 76)?, emf_i32(data, 84)?),
    );
    let mut pixmap = tiny_skia::Pixmap::new(out_w, out_h)?;
    let mut player = EmfPlayer {
        state: EmfState::default(),
        saved: Vec::new(),
        objects: std::collections::HashMap::new(),
        path: None,
        closed_path: None,
        origin,
        scale: f64::from(scale),
        dpi,
        drew: false,
    };
    let mut offset = usize::try_from(emf_u32(data, 4)?).ok()?;
    while offset + 8 <= data.len() {
        let kind = emf_u32(data, offset)?;
        let size = usize::try_from(emf_u32(data, offset + 4)?).ok()?;
        if size < 8 || size % 4 != 0 {
            return None;
        }
        let record = data.get(offset..offset.checked_add(size)?)?;
        if kind == 14 {
            break; // EMR_EOF
        }
        player.play(kind, record, &mut pixmap);
        offset += size;
    }
    if !player.drew || !pixmap.pixels().iter().any(|pixel| pixel.alpha() != 0) {
        return None;
    }

    let pixel_count = pixmap.width() as usize * pixmap.height() as usize;
    let mut rgb = Vec::with_capacity(pixel_count * 3);
    let mut alpha = Vec::with_capacity(pixel_count);
    for pixel in pixmap.pixels() {
        let colour = pixel.demultiply();
        rgb.extend_from_slice(&[colour.red(), colour.green(), colour.blue()]);
        alpha.push(colour.alpha());
    }
    let opaque = alpha.iter().all(|&value| value == 255);
    Some(DecodedImage {
        data: rgb,
        alpha: (!opaque).then_some(alpha),
        width: pixmap.width(),
        height: pixmap.height(),
        color_space: "DeviceRGB",
        is_jpeg: false,
    })
}

fn emf_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn emf_i32(data: &[u8], offset: usize) -> Option<i32> {
    emf_u32(data, offset).map(|value| value as i32)
}

fn emf_i16(data: &[u8], offset: usize) -> Option<i16> {
    Some(i16::from_le_bytes(
        data.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn emf_f32(data: &[u8], offset: usize) -> Option<f32> {
    emf_u32(data, offset).map(f32::from_bits)
}

/// A COLORREF is 0x00BBGGRR.
fn emf_colour(value: u32) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        255,
    )
}

#[derive(Clone, Copy)]
enum EmfObject {
    /// A pen colour and logical width, or the null pen.
    Pen(Option<EmfPen>),
    /// A solid brush colour, or the null brush.
    Brush(Option<tiny_skia::Color>),
    Other,
}

#[derive(Clone)]
struct EmfState {
    world: [f64; 6],
    window_origin: (f64, f64),
    window_extent: (f64, f64),
    viewport_origin: (f64, f64),
    viewport_extent: (f64, f64),
    map_mode: u32,
    pen: Option<EmfPen>,
    brush: Option<tiny_skia::Color>,
    fill_rule: tiny_skia::FillRule,
    current: (f64, f64),
}

impl Default for EmfState {
    fn default() -> Self {
        Self {
            world: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            window_origin: (0.0, 0.0),
            window_extent: (1.0, 1.0),
            viewport_origin: (0.0, 0.0),
            viewport_extent: (1.0, 1.0),
            map_mode: 1,
            pen: Some(EmfPen::cosmetic(tiny_skia::Color::BLACK)),
            brush: Some(tiny_skia::Color::WHITE),
            fill_rule: tiny_skia::FillRule::EvenOdd,
            current: (0.0, 0.0),
        }
    }
}

struct EmfPlayer {
    state: EmfState,
    saved: Vec<EmfState>,
    objects: std::collections::HashMap<u32, EmfObject>,
    /// The figure being built inside BEGINPATH and ENDPATH.
    path: Option<tiny_skia::PathBuilder>,
    /// The path ENDPATH closed, for FILLPATH and STROKEPATH.
    closed_path: Option<tiny_skia::Path>,
    /// The picture frame's top-left corner in device pixels.
    origin: (f64, f64),
    scale: f64,
    dpi: (f64, f64),
    drew: bool,
}

impl EmfPlayer {
    /// Logical coordinates to raster pixels: world transform, then the
    /// window-to-viewport mapping, then the frame.
    fn map(&self, (x, y): (f64, f64)) -> (f32, f32) {
        let [m11, m12, m21, m22, dx, dy] = self.state.world;
        let page = (m11 * x + m21 * y + dx, m12 * x + m22 * y + dy);
        let state = &self.state;
        let (sx, sy) = match state.map_mode {
            2..=6 => {
                let divisor = [254.0, 2540.0, 100.0, 1000.0, 1440.0][(state.map_mode - 2) as usize];
                (self.dpi.0 / divisor, -self.dpi.1 / divisor)
            }
            7 | 8 => {
                let (sx, sy) = (
                    state.viewport_extent.0 / state.window_extent.0,
                    state.viewport_extent.1 / state.window_extent.1,
                );
                if state.map_mode == 7 {
                    let physical = (sx.abs() / self.dpi.0).min(sy.abs() / self.dpi.1);
                    (
                        sx.signum() * physical * self.dpi.0,
                        sy.signum() * physical * self.dpi.1,
                    )
                } else {
                    (sx, sy)
                }
            }
            _ => (1.0, 1.0),
        };
        let device = (
            (page.0 - state.window_origin.0) * sx + state.viewport_origin.0,
            (page.1 - state.window_origin.1) * sy + state.viewport_origin.1,
        );
        (
            ((device.0 - self.origin.0) * self.scale) as f32,
            ((device.1 - self.origin.1) * self.scale) as f32,
        )
    }

    /// How many raster pixels one logical unit spans, averaged over x and y.
    fn length_scale(&self) -> f64 {
        let zero = self.map((0.0, 0.0));
        let x = self.map((1.0, 0.0));
        let y = self.map((0.0, 1.0));
        let distance = |a: (f32, f32), b: (f32, f32)| f64::from((a.0 - b.0).hypot(a.1 - b.1));
        (distance(zero, x) + distance(zero, y)) / 2.0
    }

    fn play(&mut self, kind: u32, record: &[u8], pixmap: &mut tiny_skia::Pixmap) {
        let _ = self.play_record(kind, record, pixmap);
    }

    fn play_record(
        &mut self,
        kind: u32,
        record: &[u8],
        pixmap: &mut tiny_skia::Pixmap,
    ) -> Option<()> {
        let pair = |offset: usize| -> Option<(f64, f64)> {
            Some((
                f64::from(emf_i32(record, offset)?),
                f64::from(emf_i32(record, offset + 4)?),
            ))
        };
        match kind {
            9 => {
                let extent = pair(8)?;
                if matches!(self.state.map_mode, 7 | 8) && extent.0 != 0.0 && extent.1 != 0.0 {
                    self.state.window_extent = extent;
                }
            }
            10 => self.state.window_origin = pair(8)?,
            11 => {
                let extent = pair(8)?;
                if matches!(self.state.map_mode, 7 | 8) && extent.0 != 0.0 && extent.1 != 0.0 {
                    self.state.viewport_extent = extent;
                }
            }
            12 => self.state.viewport_origin = pair(8)?,
            17 => {
                let mode = emf_u32(record, 8)?;
                if (1..=8).contains(&mode) {
                    self.state.map_mode = mode;
                }
            }
            19 => {
                self.state.fill_rule = if emf_u32(record, 8)? == 2 {
                    tiny_skia::FillRule::Winding
                } else {
                    tiny_skia::FillRule::EvenOdd
                }
            }
            31 | 32 => {
                if matches!(self.state.map_mode, 7 | 8) {
                    let (xn, xd, yn, yd) = (
                        emf_i32(record, 8)?,
                        emf_i32(record, 12)?,
                        emf_i32(record, 16)?,
                        emf_i32(record, 20)?,
                    );
                    if xn == 0 || xd == 0 || yn == 0 || yd == 0 {
                        return None;
                    }
                    let old = if kind == 31 {
                        self.state.viewport_extent
                    } else {
                        self.state.window_extent
                    };
                    let extent = (
                        old.0 * f64::from(xn) / f64::from(xd),
                        old.1 * f64::from(yn) / f64::from(yd),
                    );
                    if !extent.0.is_finite() || !extent.1.is_finite() {
                        return None;
                    }
                    if kind == 31 {
                        self.state.viewport_extent = extent;
                    } else {
                        self.state.window_extent = extent;
                    }
                }
            }
            33 => self.saved.push(self.state.clone()),
            34 => {
                let relative = emf_i32(record, 8)?;
                if relative >= 0 {
                    return None;
                }
                let depth = usize::try_from(relative.unsigned_abs()).ok()?;
                let index = self.saved.len().checked_sub(depth)?;
                self.state = self.saved[index].clone();
                self.saved.truncate(index);
            }
            35 | 36 => {
                let mut xform = [0.0; 6];
                for (index, value) in xform.iter_mut().enumerate() {
                    *value = f64::from(emf_f32(record, 8 + index * 4)?);
                }
                let mode = if kind == 35 { 4 } else { emf_u32(record, 32)? };
                if !xform.iter().all(|value| value.is_finite()) || !(1..=4).contains(&mode) {
                    return None;
                }
                self.state.world = match mode {
                    1 => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                    2 => emf_multiply(xform, self.state.world),
                    3 => emf_multiply(self.state.world, xform),
                    _ => xform,
                };
            }
            37 => {
                let handle = emf_u32(record, 8)?;
                let object = if handle & 0x8000_0000 != 0 {
                    emf_stock_object(handle & 0x7fff_ffff)
                } else {
                    *self.objects.get(&handle)?
                };
                match object {
                    EmfObject::Pen(pen) => self.state.pen = pen,
                    EmfObject::Brush(brush) => self.state.brush = brush,
                    EmfObject::Other => {}
                }
            }
            38 | 95 => {
                let extended = kind == 95;
                let offset = if extended { 28 } else { 12 };
                let style = emf_u32(record, offset)?;
                if extended {
                    let count = usize::try_from(emf_u32(record, 48)?).ok()?;
                    record.get(52..52usize.checked_add(count.checked_mul(4)?)?)?;
                }
                let brush = if extended { emf_u32(record, 36)? } else { 0 };
                let color = emf_colour(emf_u32(record, if extended { 40 } else { 24 })?);
                let width = if extended {
                    f64::from(emf_u32(record, 32)?)
                } else {
                    f64::from(emf_i32(record, 16)?).abs()
                };
                let pen = (style & 0xf == 0 && brush == 0).then_some(EmfPen {
                    color,
                    width,
                    geometric: style & 0x10000 != 0,
                    style,
                });
                self.objects
                    .insert(emf_u32(record, 8)?, EmfObject::Pen(pen));
            }
            39 => {
                let style = emf_u32(record, 12)?;
                let color = emf_colour(emf_u32(record, 16)?);
                self.objects.insert(
                    emf_u32(record, 8)?,
                    EmfObject::Brush((style == 0).then_some(color)),
                );
            }
            40 => {
                self.objects.remove(&emf_u32(record, 8)?);
            }
            // Any other object creation still takes its handle slot.
            82 | 93 | 94 => {
                self.objects.insert(emf_u32(record, 8)?, EmfObject::Other);
            }
            27 => {
                self.state.current = pair(8)?;
                let (x, y) = self.map(self.state.current);
                if let Some(path) = &mut self.path {
                    path.move_to(x, y);
                }
            }
            54 => {
                let to = pair(8)?;
                self.figure(&[self.state.current, to], false, false, true, pixmap);
                self.state.current = to;
            }
            42..=44 => {
                let (left, top) = pair(8)?;
                let (right, bottom) = pair(16)?;
                let corner = if kind == 44 { pair(24)? } else { (0.0, 0.0) };
                let points = emf_box_outline(kind, left, top, right, bottom, corner);
                self.bezier_figure(&points, pixmap);
            }
            2..=6 | 85..=89 => {
                let wide = kind < 80;
                let count = usize::try_from(emf_u32(record, 24)?).ok()?;
                let points = emf_points(record, 28, count, wide)?;
                let base = if wide { kind } else { kind - 83 };
                if base == 2 && points.len() % 3 != 1 || base == 5 && points.len() % 3 != 0 {
                    return None;
                }
                match base {
                    // POLYBEZIER, POLYBEZIERTO
                    2 | 5 => {
                        let mut all = Vec::with_capacity(points.len() + 1);
                        if base == 5 {
                            all.push(self.state.current);
                        }
                        all.extend(points);
                        if base == 5
                            && let Some(last) = all.last()
                        {
                            self.state.current = *last;
                        }
                        if base == 2 && !all.is_empty() {
                            let point = self.map(all[0]);
                            if let Some(path) = &mut self.path {
                                path.move_to(point.0, point.1);
                            }
                        }
                        self.curve(&all, pixmap);
                    }
                    // POLYGON, POLYLINE, POLYLINETO
                    _ => {
                        let mut all = Vec::with_capacity(points.len() + 1);
                        if base == 6 {
                            all.push(self.state.current);
                        }
                        all.extend(points);
                        if base == 6
                            && let Some(last) = all.last()
                        {
                            self.state.current = *last;
                        }
                        self.figure(&all, base == 3, base == 3, base == 6, pixmap);
                    }
                }
            }
            7 | 8 | 90 | 91 => {
                let wide = kind < 80;
                let polygons = usize::try_from(emf_u32(record, 24)?).ok()?;
                let total = usize::try_from(emf_u32(record, 28)?).ok()?;
                record.get(32..32usize.checked_add(polygons.checked_mul(4)?)?)?;
                let counts = (0..polygons)
                    .map(|index| usize::try_from(emf_u32(record, 32 + index * 4)?).ok())
                    .collect::<Option<Vec<_>>>()?;
                let points = emf_points(record, 32 + polygons * 4, total, wide)?;
                let closed = matches!(kind, 8 | 91);
                let mut builder = tiny_skia::PathBuilder::new();
                let mut start = 0usize;
                for count in counts {
                    let figure = points.get(start..start.checked_add(count)?)?;
                    start += count;
                    self.add_polyline(&mut builder, figure, closed);
                }
                if let Some(path) = self.path.take() {
                    let mut combined = path;
                    if let Some(built) = builder.finish() {
                        combined.push_path(&built);
                    }
                    self.path = Some(combined);
                } else {
                    self.finish(builder, closed, pixmap);
                }
            }
            68 => {
                self.path = None;
                self.closed_path = None;
            }
            59 => {
                self.path = Some(tiny_skia::PathBuilder::new());
                self.closed_path = None;
            }
            60 => {
                self.closed_path = self.path.take().and_then(tiny_skia::PathBuilder::finish);
            }
            61 => {
                if let Some(path) = &mut self.path {
                    path.close();
                }
            }
            62..=64 => {
                let path = self
                    .closed_path
                    .take()
                    .or_else(|| self.path.take().and_then(tiny_skia::PathBuilder::finish))?;
                let state = self.state.clone();
                if kind != 64 {
                    self.fill(&path, state.brush, pixmap);
                }
                if kind != 62 {
                    self.stroke(&path, state.pen, pixmap);
                }
            }
            81 => self.stretch_dib(record, pixmap)?,
            _ => {}
        }
        Some(())
    }

    fn add_polyline(
        &self,
        builder: &mut tiny_skia::PathBuilder,
        points: &[(f64, f64)],
        closed: bool,
    ) {
        for (index, point) in points.iter().enumerate() {
            let (x, y) = self.map(*point);
            if index == 0 {
                builder.move_to(x, y);
            } else {
                builder.line_to(x, y);
            }
        }
        if closed && points.len() > 2 {
            builder.close();
        }
    }

    /// Draws or, inside a path bracket, adds a poly line or polygon.
    fn figure(
        &mut self,
        points: &[(f64, f64)],
        closed: bool,
        filled: bool,
        continue_path: bool,
        pixmap: &mut tiny_skia::Pixmap,
    ) {
        if points.len() < 2 {
            return;
        }
        if let Some(mut path) = self.path.take() {
            for (index, point) in points.iter().enumerate() {
                let (x, y) = self.map(*point);
                if index == 0 && (!continue_path || path.last_point().is_none()) {
                    path.move_to(x, y);
                } else if index > 0 {
                    path.line_to(x, y);
                }
            }
            if closed {
                path.close();
            }
            self.path = Some(path);
            return;
        }
        let mut builder = tiny_skia::PathBuilder::new();
        self.add_polyline(&mut builder, points, closed);
        self.finish(builder, filled, pixmap);
    }

    /// Draws or adds a run of cubic Bezier segments: a start point, then
    /// three points per segment.
    fn curve(&mut self, points: &[(f64, f64)], pixmap: &mut tiny_skia::Pixmap) {
        let Some(first) = points.first() else {
            return;
        };
        let in_path = self.path.is_some();
        let mut builder = self.path.take().unwrap_or_default();
        let (x, y) = self.map(*first);
        if !in_path || builder.last_point().is_none() {
            builder.move_to(x, y);
        }
        for segment in points[1..].chunks_exact(3) {
            let [a, b, c] = [segment[0], segment[1], segment[2]].map(|point| self.map(point));
            builder.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
        }
        if in_path {
            self.path = Some(builder);
        } else {
            self.finish(builder, false, pixmap);
        }
    }

    /// A closed outline of Bezier segments (rectangle, ellipse, rounded
    /// rectangle), drawn with the brush and the pen.
    fn bezier_figure(&mut self, points: &[(f64, f64)], pixmap: &mut tiny_skia::Pixmap) {
        let in_path = self.path.is_some();
        let mut builder = self.path.take().unwrap_or_default();
        let (x, y) = self.map(points[0]);
        builder.move_to(x, y);
        for segment in points[1..].chunks_exact(3) {
            let [a, b, c] = [segment[0], segment[1], segment[2]].map(|point| self.map(point));
            builder.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
        }
        builder.close();
        if in_path {
            self.path = Some(builder);
        } else {
            self.finish(builder, true, pixmap);
        }
    }

    fn finish(
        &mut self,
        builder: tiny_skia::PathBuilder,
        filled: bool,
        pixmap: &mut tiny_skia::Pixmap,
    ) {
        let Some(path) = builder.finish() else {
            return;
        };
        let state = self.state.clone();
        if filled {
            self.fill(&path, state.brush, pixmap);
        }
        self.stroke(&path, state.pen, pixmap);
    }

    fn fill(
        &mut self,
        path: &tiny_skia::Path,
        brush: Option<tiny_skia::Color>,
        pixmap: &mut tiny_skia::Pixmap,
    ) {
        let Some(colour) = brush else {
            return;
        };
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(colour);
        paint.anti_alias = true;
        pixmap.fill_path(
            path,
            &paint,
            self.state.fill_rule,
            tiny_skia::Transform::identity(),
            None,
        );
        self.drew = true;
    }

    fn stroke(
        &mut self,
        path: &tiny_skia::Path,
        pen: Option<EmfPen>,
        pixmap: &mut tiny_skia::Pixmap,
    ) {
        let Some(pen) = pen else {
            return;
        };
        // A zero width is one device pixel wide, whatever the transform.
        let width = if pen.width <= 0.0 {
            self.scale
        } else {
            (pen.width
                * if pen.geometric {
                    self.length_scale()
                } else {
                    self.scale
                })
            .max(1.0)
        };
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(pen.color);
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: width as f32,
            line_cap: match pen.style & 0xf00 {
                0x100 => tiny_skia::LineCap::Square,
                0x200 => tiny_skia::LineCap::Butt,
                _ => tiny_skia::LineCap::Round,
            },
            line_join: match pen.style & 0xf000 {
                0x1000 => tiny_skia::LineJoin::Bevel,
                0x2000 => tiny_skia::LineJoin::Miter,
                _ => tiny_skia::LineJoin::Round,
            },
            ..tiny_skia::Stroke::default()
        };
        pixmap.stroke_path(
            path,
            &paint,
            &stroke,
            tiny_skia::Transform::identity(),
            None,
        );
        self.drew = true;
    }

    /// RGB ternary raster operations use the actual destination and solid pattern.
    /// Pixel-center nearest sampling preserves source crop, signs and affine basis.
    fn stretch_dib(&mut self, record: &[u8], pixmap: &mut tiny_skia::Pixmap) -> Option<()> {
        if record.len() < 80 {
            return None;
        }
        let rop = (emf_u32(record, 68)? >> 16) as u8;
        // D is an identity operation even without a source bitmap.
        if rop == 0xaa {
            return Some(());
        }
        let needs_source = ((rop ^ (rop >> 2)) & 0x33) != 0;
        let (bitmap, bottom_up) = if needs_source {
            if emf_u32(record, 64)? != 0 {
                return None;
            }
            let section = |offset, length| {
                let at = usize::try_from(emf_u32(record, offset)?).ok()?;
                let len = usize::try_from(emf_u32(record, length)?).ok()?;
                if at < 80 {
                    return None;
                }
                record.get(at..at.checked_add(len)?)
            };
            let info = section(48, 52)?;
            let bits = section(56, 60)?;
            (Some(emf_dib_pixmap(info, bits)?), emf_i32(info, 8)? > 0)
        } else {
            (None, false)
        };
        let pattern = if (rop & 15) != (rop >> 4) {
            self.state.brush?
        } else {
            tiny_skia::Color::BLACK
        }
        .to_color_u8();
        let p = [pattern.red(), pattern.green(), pattern.blue()];
        let dest = (
            f64::from(emf_i32(record, 24)?),
            f64::from(emf_i32(record, 28)?),
        );
        let size = (
            f64::from(emf_i32(record, 72)?),
            f64::from(emf_i32(record, 76)?),
        );
        let source = (
            f64::from(emf_i32(record, 32)?),
            f64::from(emf_i32(record, 36)?),
        );
        let span = (
            f64::from(emf_i32(record, 40)?),
            f64::from(emf_i32(record, 44)?),
        );
        if needs_source && (span.0 == 0.0 || span.1 == 0.0) || size.0 == 0.0 || size.1 == 0.0 {
            return None;
        }
        let a = self.map(dest);
        let b = self.map((dest.0 + size.0, dest.1));
        let c = self.map((dest.0, dest.1 + size.1));
        let (a, b, c) = (
            (f64::from(a.0), f64::from(a.1)),
            (f64::from(b.0), f64::from(b.1)),
            (f64::from(c.0), f64::from(c.1)),
        );
        let (bx, by, cx, cy) = (b.0 - a.0, b.1 - a.1, c.0 - a.0, c.1 - a.1);
        let determinant = bx * cy - by * cx;
        if ![a.0, a.1, bx, by, cx, cy, determinant]
            .iter()
            .all(|x| x.is_finite())
            || determinant == 0.0
        {
            return None;
        }
        let corners = [a, b, c, (a.0 + bx + cx, a.1 + by + cy)];
        let min_x = corners
            .iter()
            .map(|v| v.0)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as u32;
        let min_y = corners
            .iter()
            .map(|v| v.1)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as u32;
        let max_x = (corners
            .iter()
            .map(|v| v.0)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .max(0.0) as u32)
            .min(pixmap.width());
        let max_y = (corners
            .iter()
            .map(|v| v.1)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .max(0.0) as u32)
            .min(pixmap.height());
        let width = pixmap.width() as usize;
        for y in min_y..max_y {
            for x in min_x..max_x {
                let (dx, dy) = (f64::from(x) + 0.5 - a.0, f64::from(y) + 0.5 - a.1);
                let (u, v) = (
                    (dx * cy - dy * cx) / determinant,
                    (dy * bx - dx * by) / determinant,
                );
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let s = if let Some(bitmap) = &bitmap {
                    let sx = (source.0 + u * span.0).floor();
                    let sy = if bottom_up {
                        f64::from(bitmap.height()) - source.1 - span.1 + v * span.1
                    } else {
                        source.1 + v * span.1
                    }
                    .floor();
                    if sx < 0.0
                        || sy < 0.0
                        || sx >= f64::from(bitmap.width())
                        || sy >= f64::from(bitmap.height())
                    {
                        continue;
                    }
                    let color = bitmap.pixel(sx as u32, sy as u32)?.demultiply();
                    [color.red(), color.green(), color.blue()]
                } else {
                    [0; 3]
                };
                let at = (y as usize * width + x as usize) * 4;
                let d = pixmap.pixels()[at / 4].demultiply();
                let d = [d.red(), d.green(), d.blue()];
                let pixel = &mut pixmap.data_mut()[at..at + 4];
                for channel in 0..3 {
                    pixel[channel] = emf_rop(rop, p[channel], s[channel], d[channel]);
                }
                pixel[3] = 255;
                self.drew = true;
            }
        }
        Some(())
    }
}

/// `a` then `b`, as GDI XFORMs (eM11, eM12, eM21, eM22, eDx, eDy).
fn emf_multiply(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn emf_stock_object(index: u32) -> EmfObject {
    let grey = |level: u8| tiny_skia::Color::from_rgba8(level, level, level, 255);
    match index {
        0 => EmfObject::Brush(Some(tiny_skia::Color::WHITE)),
        1 => EmfObject::Brush(Some(grey(0xc0))),
        2 => EmfObject::Brush(Some(grey(0x80))),
        3 => EmfObject::Brush(Some(grey(0x40))),
        4 => EmfObject::Brush(Some(tiny_skia::Color::BLACK)),
        5 => EmfObject::Brush(None),
        6 => EmfObject::Pen(Some(EmfPen::cosmetic(tiny_skia::Color::WHITE))),
        7 => EmfObject::Pen(Some(EmfPen::cosmetic(tiny_skia::Color::BLACK))),
        8 => EmfObject::Pen(None),
        _ => EmfObject::Other,
    }
}

fn emf_points(record: &[u8], offset: usize, count: usize, wide: bool) -> Option<Vec<(f64, f64)>> {
    let step = if wide { 8 } else { 4 };
    record.get(offset..offset.checked_add(count.checked_mul(step)?)?)?;
    (0..count)
        .map(|index| {
            let at = offset + index * step;
            Some(if wide {
                (
                    f64::from(emf_i32(record, at)?),
                    f64::from(emf_i32(record, at + 4)?),
                )
            } else {
                (
                    f64::from(emf_i16(record, at)?),
                    f64::from(emf_i16(record, at + 2)?),
                )
            })
        })
        .collect()
}

/// The outline of EMR_ELLIPSE (42), EMR_RECTANGLE (43) or EMR_ROUNDRECT (44)
/// as a start point and cubic Bezier segments, in logical coordinates, so a
/// rotating world transform keeps its shape.
fn emf_box_outline(
    kind: u32,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    corner: (f64, f64),
) -> Vec<(f64, f64)> {
    const KAPPA: f64 = 0.552_284_749_830_793_4;
    let line = |from: (f64, f64), to: (f64, f64)| {
        [
            (
                from.0 + (to.0 - from.0) / 3.0,
                from.1 + (to.1 - from.1) / 3.0,
            ),
            (
                from.0 + (to.0 - from.0) * 2.0 / 3.0,
                from.1 + (to.1 - from.1) * 2.0 / 3.0,
            ),
            to,
        ]
    };
    let (rx, ry) = match kind {
        42 => ((right - left) / 2.0, (bottom - top) / 2.0),
        44 => (
            (corner.0 / 2.0).min((right - left) / 2.0),
            (corner.1 / 2.0).min((bottom - top) / 2.0),
        ),
        _ => (0.0, 0.0),
    };
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let mut points = vec![(left + rx, top)];
    let push_line = |points: &mut Vec<(f64, f64)>, to: (f64, f64)| {
        let from = *points.last().expect("outline has a start");
        points.extend(line(from, to));
    };
    push_line(&mut points, (right - rx, top));
    points.extend([
        (right - rx + kx, top),
        (right, top + ry - ky),
        (right, top + ry),
    ]);
    push_line(&mut points, (right, bottom - ry));
    points.extend([
        (right, bottom - ry + ky),
        (right - rx + kx, bottom),
        (right - rx, bottom),
    ]);
    push_line(&mut points, (left + rx, bottom));
    points.extend([
        (left + rx - kx, bottom),
        (left, bottom - ry + ky),
        (left, bottom - ry),
    ]);
    push_line(&mut points, (left, top + ry));
    points.extend([
        (left, top + ry - ky),
        (left + rx - kx, top),
        (left + rx, top),
    ]);
    points
}

/// Every standard ternary ROP is an eight-entry Boolean table indexed P,S,D.
fn emf_rop(rop: u8, p: u8, s: u8, d: u8) -> u8 {
    let mut result = 0;
    for index in 0..8 {
        if rop & (1 << index) != 0 {
            result |= (if index & 4 != 0 { p } else { !p })
                & (if index & 2 != 0 { s } else { !s })
                & (if index & 1 != 0 { d } else { !d });
        }
    }
    result
}
fn emf_dib_pixmap(info: &[u8], bits: &[u8]) -> Option<tiny_skia::Pixmap> {
    let header = usize::try_from(emf_u32(info, 0)?).ok()?;
    if header < 40 {
        return None;
    }
    info.get(..header)?;
    let (width, height) = (emf_i32(info, 4)?, emf_i32(info, 8)?);
    if width <= 0 || height == 0 || info.get(12..14)? != 1u16.to_le_bytes() {
        return None;
    }
    let (width, rows) = (width as u32, height.unsigned_abs());
    let bytes = emf_rgba_bytes(width, rows)?;
    let depth = u16::from_le_bytes(info.get(14..16)?.try_into().ok()?);
    let compression = emf_u32(info, 16)?;
    if !matches!(depth, 1 | 4 | 8 | 24 | 32)
        || !(compression == 0 || compression == 3 && depth == 32)
    {
        return None;
    }
    let mut masks = [0x00ff0000u32, 0x0000ff00, 0x000000ff];
    if compression == 3 {
        for (i, mask) in masks.iter_mut().enumerate() {
            *mask = emf_u32(info, 40 + i * 4)?;
            if *mask == 0
                || (*mask >> mask.trailing_zeros()).count_ones()
                    != (*mask >> mask.trailing_zeros()).ilog2() + 1
            {
                return None;
            }
        }
        if masks[0] & masks[1] != 0 || masks[0] & masks[2] != 0 || masks[1] & masks[2] != 0 {
            return None;
        }
    }
    let colors = emf_u32(info, 32)?;
    let palette_count = if depth <= 8 {
        let limit = 1u32 << depth;
        if colors > limit {
            return None;
        }
        if colors == 0 { limit } else { colors }
    } else {
        0
    };
    let palette = info.get(header..header.checked_add(palette_count as usize * 4)?)?;
    let stride = (width as usize)
        .checked_mul(usize::from(depth))?
        .checked_add(31)?
        .checked_div(32)?
        .checked_mul(4)?;
    bits.get(..stride.checked_mul(rows as usize)?)?;
    let mut rgba = Vec::with_capacity(bytes);
    for y in 0..rows as usize {
        let row = if height > 0 { rows as usize - 1 - y } else { y };
        let line = &bits[row * stride..(row + 1) * stride];
        for x in 0..width as usize {
            let rgb = match depth {
                24 => [line[x * 3 + 2], line[x * 3 + 1], line[x * 3]],
                32 => {
                    let value = emf_u32(line, x * 4)?;
                    masks.map(|mask| {
                        let shift = mask.trailing_zeros();
                        let max = mask >> shift;
                        (((u64::from((value & mask) >> shift) * 255) + u64::from(max) / 2)
                            / u64::from(max)) as u8
                    })
                }
                _ => {
                    let bit = x * usize::from(depth);
                    let index = (line[bit / 8] >> (8 - usize::from(depth) - bit % 8))
                        & ((1u16 << depth) - 1) as u8;
                    let at = usize::from(index) * 4;
                    let color = palette.get(at..at + 3)?;
                    [color[2], color[1], color[0]]
                }
            };
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(width, rows)?)
}

fn paeth_predictor(a: i32, b: i32, c: i32) -> u8 {
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}
