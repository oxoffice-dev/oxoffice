//! Format-neutral image metadata and media naming.

use std::collections::HashSet;

/// Allocates collision-free numbered media part names.
#[derive(Clone, Debug)]
pub struct MediaNamer {
    directory: String,
    stem: String,
    occupied: HashSet<usize>,
    next: usize,
}

impl MediaNamer {
    /// Scans existing package part names for positive numeric suffixes.
    pub fn scan<'a>(directory: &str, stem: &str, existing: impl Iterator<Item = &'a str>) -> Self {
        let directory = normalize_directory(directory);
        let mut occupied = HashSet::new();
        for name in existing {
            if let Some(index) = media_index(name, &directory, stem) {
                occupied.insert(index);
            }
        }
        let next = occupied
            .iter()
            .copied()
            .max()
            .and_then(|index| index.checked_add(1))
            .unwrap_or(1);

        Self {
            directory,
            stem: stem.to_owned(),
            occupied,
            next,
        }
    }

    /// Returns the next free package part name using `extension` verbatim.
    pub fn next_part_name(&mut self, extension: &str) -> String {
        while self.occupied.contains(&self.next) {
            self.next = self.next.checked_add(1).unwrap_or(1);
        }

        let index = self.next;
        self.occupied.insert(index);
        self.next = index.checked_add(1).unwrap_or(1);
        let separator = if self.directory == "/" { "" } else { "/" };
        format!(
            "{}{separator}{}{index}.{extension}",
            self.directory, self.stem
        )
    }
}

fn normalize_directory(directory: &str) -> String {
    let directory = directory.trim_matches('/');
    if directory.is_empty() {
        "/".to_owned()
    } else {
        format!("/{directory}")
    }
}

fn media_index(name: &str, directory: &str, stem: &str) -> Option<usize> {
    let (name_directory, filename) = name.rsplit_once('/')?;
    let name_directory = if name_directory.is_empty() {
        "/"
    } else {
        name_directory
    };
    if !name_directory.eq_ignore_ascii_case(directory) {
        return None;
    }
    let prefix = filename.get(..stem.len())?;
    if !prefix.eq_ignore_ascii_case(stem) {
        return None;
    }
    let suffix = &filename[stem.len()..];
    let (digits, extension) = suffix.split_once('.')?;
    if digits.is_empty()
        || extension.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok().filter(|index| *index > 0)
}

/// Returns whether `value` is a parameter-free MIME content type.
///
/// Both the type and subtype must use the RFC token alphabet. Parameters,
/// whitespace, empty components, and non-ASCII bytes are rejected.
pub fn is_safe_content_type(value: &str) -> bool {
    let Some((type_name, subtype)) = value.split_once('/') else {
        return false;
    };
    !type_name.is_empty()
        && !subtype.is_empty()
        && !subtype.contains('/')
        && type_name.bytes().all(is_mime_token_byte)
        && subtype.bytes().all(is_mime_token_byte)
}

fn is_mime_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Checks bytes for a supported audio or video container content type.
///
/// `Some(true)` means the known signature matches, `Some(false)` means it does
/// not match, and `None` means the content type is not classified here. The
/// classifier recognizes MP3, RIFF WAVE, and ISO base media containers without
/// claiming to identify the codec inside a container.
pub fn audio_video_signature_matches(data: &[u8], content_type: &str) -> Option<bool> {
    if matches_content_type(content_type, &["audio/mpeg", "audio/mp3"]) {
        Some(
            data.starts_with(b"ID3")
                || data
                    .get(..2)
                    .is_some_and(|prefix| prefix[0] == 0xff && prefix[1] & 0xe0 == 0xe0),
        )
    } else if matches_content_type(content_type, &["audio/wav", "audio/wave", "audio/x-wav"]) {
        Some(data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WAVE"))
    } else if matches_content_type(content_type, &["audio/mp4", "video/mp4", "application/mp4"]) {
        Some(data.get(4..8) == Some(b"ftyp"))
    } else {
        None
    }
}

fn matches_content_type(content_type: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| content_type.eq_ignore_ascii_case(candidate))
}

/// An image format supported by OOXML consumers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageFormat {
    /// Portable Network Graphics.
    Png,
    /// Joint Photographic Experts Group.
    Jpeg,
    /// Graphics Interchange Format.
    Gif,
    /// Windows bitmap.
    Bmp,
    /// Tagged Image File Format.
    Tiff,
    /// WebP.
    Webp,
    /// Scalable Vector Graphics.
    Svg,
    /// Enhanced Metafile.
    Emf,
    /// Windows Metafile.
    Wmf,
}

impl ImageFormat {
    /// Identifies an image format from its magic bytes.
    pub fn sniff(data: &[u8]) -> Option<Self> {
        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if data.starts_with(b"\xff\xd8") {
            Some(Self::Jpeg)
        } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if data.starts_with(b"BM") {
            Some(Self::Bmp)
        } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
            Some(Self::Tiff)
        } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
            Some(Self::Webp)
        } else if is_svg(data) {
            Some(Self::Svg)
        } else if data.get(..4) == Some(&[1, 0, 0, 0]) && data.get(40..44) == Some(b" EMF") {
            Some(Self::Emf)
        } else if data.starts_with(b"\xd7\xcd\xc6\x9a") || is_standard_wmf(data) {
            Some(Self::Wmf)
        } else {
            None
        }
    }

    /// Identifies an image format from a case-insensitive extension.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "gif" => Some(Self::Gif),
            "bmp" => Some(Self::Bmp),
            "tif" | "tiff" => Some(Self::Tiff),
            "webp" => Some(Self::Webp),
            "svg" => Some(Self::Svg),
            "emf" => Some(Self::Emf),
            "wmf" | "wmz" => Some(Self::Wmf),
            _ => None,
        }
    }

    /// Returns the canonical filename extension without a leading dot.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Webp => "webp",
            Self::Svg => "svg",
            Self::Emf => "emf",
            Self::Wmf => "wmf",
        }
    }

    /// Returns the canonical MIME content type.
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Tiff => "image/tiff",
            Self::Webp => "image/webp",
            Self::Svg => "image/svg+xml",
            Self::Emf => "image/emf",
            Self::Wmf => "image/wmf",
        }
    }
}

/// Resolves an image format from bytes first, then its filename extension.
///
/// Unknown images use PNG as the compatibility default.
pub fn resolve(data: &[u8], filename: &str) -> ImageFormat {
    ImageFormat::sniff(data)
        .or_else(|| {
            filename
                .rsplit_once('.')
                .and_then(|(_, extension)| ImageFormat::from_extension(extension))
        })
        .unwrap_or(ImageFormat::Png)
}

/// Metadata available from an image header without decoding its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageInfo {
    /// Detected image format.
    pub format: ImageFormat,
    /// Image width in pixels.
    pub width_px: u32,
    /// Image height in pixels.
    pub height_px: u32,
    /// Horizontal dots per inch when declared by the image.
    pub dpi_x: Option<f64>,
    /// Vertical dots per inch when declared by the image.
    pub dpi_y: Option<f64>,
    /// Bits used for one channel.
    pub bit_depth: u8,
    /// Number of colour channels, including alpha when present.
    pub channels: u8,
    /// Whether the header declares an alpha channel.
    pub has_alpha: bool,
}

/// An image's physical size in English Metric Units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSize {
    /// Width in English Metric Units.
    pub width_emu: i64,
    /// Height in English Metric Units.
    pub height_emu: i64,
}

impl ImageInfo {
    /// Calculates the physical image size using declared or fallback DPI.
    ///
    /// Each declared finite positive axis DPI takes precedence over
    /// `default_dpi`. The conversion truncates fractional EMU toward zero.
    /// Returns `None` when either axis has no finite positive effective DPI or
    /// the converted size is outside the `i64` range.
    pub fn native_size(&self, default_dpi: f64) -> Option<NativeSize> {
        let dpi_x = effective_dpi(self.dpi_x, default_dpi)?;
        let dpi_y = effective_dpi(self.dpi_y, default_dpi)?;
        Some(NativeSize {
            width_emu: pixels_to_emu(self.width_px, dpi_x)?,
            height_emu: pixels_to_emu(self.height_px, dpi_y)?,
        })
    }
}

fn effective_dpi(declared: Option<f64>, default_dpi: f64) -> Option<f64> {
    declared
        .filter(|dpi| dpi.is_finite() && *dpi > 0.0)
        .or_else(|| (default_dpi.is_finite() && default_dpi > 0.0).then_some(default_dpi))
}

fn pixels_to_emu(pixels: u32, dpi: f64) -> Option<i64> {
    let emu = f64::from(pixels) * 914_400.0 / dpi;
    (emu.is_finite() && emu < i64::MAX as f64).then_some(emu as i64)
}

/// Reads image metadata from a supported raster header.
///
/// Unsupported, malformed, and truncated data returns `None`.
pub fn probe(data: &[u8]) -> Option<ImageInfo> {
    match ImageFormat::sniff(data)? {
        ImageFormat::Png => probe_png(data),
        ImageFormat::Jpeg => probe_jpeg(data),
        ImageFormat::Gif => probe_gif(data),
        ImageFormat::Bmp => probe_bmp(data),
        ImageFormat::Webp => probe_webp(data),
        ImageFormat::Emf => probe_emf(data),
        ImageFormat::Tiff | ImageFormat::Svg | ImageFormat::Wmf => None,
    }
}

/// Metafile bounds are inclusive device pixels. The frame is in hundredths
/// of a millimetre and supplies dimensions only when those bounds are unusable.
fn probe_emf(data: &[u8]) -> Option<ImageInfo> {
    let header_bytes = usize::try_from(read_le_u32(data, 4)?).ok()?;
    if header_bytes < 88 || header_bytes > data.len() || read_le_u32(data, 44)? != 0x10000 {
        return None;
    }
    let extent = |offset: usize, inclusive: bool| -> Option<(u32, u32)> {
        let left = i64::from(read_le_i32(data, offset)?);
        let top = i64::from(read_le_i32(data, offset + 4)?);
        let right = i64::from(read_le_i32(data, offset + 8)?);
        let bottom = i64::from(read_le_i32(data, offset + 12)?);
        let (width, height) = if inclusive {
            (right - left + 1, bottom - top + 1)
        } else {
            ((right - left) * 96 / 2540, (bottom - top) * 96 / 2540)
        };
        let width = u32::try_from(width).ok().filter(|value| *value > 0)?;
        let height = u32::try_from(height).ok().filter(|value| *value > 0)?;
        Some((width, height))
    };
    let (width_px, height_px) = extent(8, true).or_else(|| extent(24, false))?;
    Some(ImageInfo {
        format: ImageFormat::Emf,
        width_px,
        height_px,
        dpi_x: None,
        dpi_y: None,
        // A vector header declares neither channel depth nor alpha. Actual
        // raster eligibility belongs to the bounded metafile player.
        bit_depth: 0,
        channels: 0,
        has_alpha: false,
    })
}

fn probe_png(data: &[u8]) -> Option<ImageInfo> {
    if read_be_u32(data, 8)? != 13 || data.get(12..16)? != b"IHDR" {
        return None;
    }
    let width_px = read_be_u32(data, 16)?;
    let height_px = read_be_u32(data, 20)?;
    if width_px == 0 || height_px == 0 {
        return None;
    }

    let bit_depth = *data.get(24)?;
    let color_type = *data.get(25)?;
    let (channels, has_alpha) = match (color_type, bit_depth) {
        (0, 1 | 2 | 4 | 8 | 16) => (1, false),
        (2, 8 | 16) => (3, false),
        (3, 1 | 2 | 4 | 8) => (1, false),
        (4, 8 | 16) => (2, true),
        (6, 8 | 16) => (4, true),
        _ => return None,
    };
    if data.get(26..28)? != [0, 0] || *data.get(28)? > 1 {
        return None;
    }
    data.get(29..33)?;

    let mut info = ImageInfo {
        format: ImageFormat::Png,
        width_px,
        height_px,
        dpi_x: None,
        dpi_y: None,
        bit_depth,
        channels,
        has_alpha,
    };
    let mut offset = 33usize;
    while offset < data.len() {
        let length = usize::try_from(read_be_u32(data, offset)?).ok()?;
        let kind_start = offset.checked_add(4)?;
        let payload_start = kind_start.checked_add(4)?;
        let payload_end = payload_start.checked_add(length)?;
        let chunk_end = payload_end.checked_add(4)?;
        let kind = data.get(kind_start..payload_start)?;
        let payload = data.get(payload_start..payload_end)?;
        data.get(payload_end..chunk_end)?;

        match kind {
            b"pHYs" if length == 9 => match payload[8] {
                0 => {}
                1 => {
                    info.dpi_x = Some(f64::from(read_be_u32(payload, 0)?) * 0.0254);
                    info.dpi_y = Some(f64::from(read_be_u32(payload, 4)?) * 0.0254);
                }
                _ => return None,
            },
            b"pHYs" => return None,
            b"IDAT" | b"IEND" => return Some(info),
            _ => {}
        }
        offset = chunk_end;
    }
    Some(info)
}

fn probe_jpeg(data: &[u8]) -> Option<ImageInfo> {
    let mut offset = 2usize;
    let mut dpi_x = None;
    let mut dpi_y = None;

    while offset < data.len() {
        if *data.get(offset)? != 0xff {
            return None;
        }
        while data.get(offset) == Some(&0xff) {
            offset = offset.checked_add(1)?;
        }
        let marker = *data.get(offset)?;
        offset = offset.checked_add(1)?;
        if marker == 0 || marker == 0xd9 {
            return None;
        }
        if marker == 0x01 || marker == 0xd8 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }

        let segment_length = usize::from(read_be_u16(data, offset)?);
        if segment_length < 2 {
            return None;
        }
        let payload_start = offset.checked_add(2)?;
        let segment_end = offset.checked_add(segment_length)?;
        let payload = data.get(payload_start..segment_end)?;

        if marker == 0xe0 && payload.starts_with(b"JFIF\0") {
            if payload.len() < 12 {
                return None;
            }
            let density_x = read_be_u16(payload, 8)?;
            let density_y = read_be_u16(payload, 10)?;
            if density_x == 0 || density_y == 0 {
                return None;
            }
            match payload[7] {
                0 => {}
                1 => {
                    dpi_x = Some(f64::from(density_x));
                    dpi_y = Some(f64::from(density_y));
                }
                2 => {
                    dpi_x = Some(f64::from(density_x) * 2.54);
                    dpi_y = Some(f64::from(density_y) * 2.54);
                }
                _ => return None,
            }
        }

        if is_jpeg_sof(marker) {
            if payload.len() < 6 {
                return None;
            }
            let bit_depth = payload[0];
            let height_px = u32::from(read_be_u16(payload, 1)?);
            let width_px = u32::from(read_be_u16(payload, 3)?);
            let channels = payload[5];
            let component_bytes = usize::from(channels).checked_mul(3)?;
            if width_px == 0
                || height_px == 0
                || bit_depth == 0
                || channels == 0
                || payload.len() < 6usize.checked_add(component_bytes)?
            {
                return None;
            }
            return Some(ImageInfo {
                format: ImageFormat::Jpeg,
                width_px,
                height_px,
                dpi_x,
                dpi_y,
                bit_depth,
                channels,
                has_alpha: false,
            });
        }
        offset = segment_end;
    }
    None
}

fn is_jpeg_sof(marker: u8) -> bool {
    matches!(
        marker,
        0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc5 | 0xc6 | 0xc7 | 0xc9 | 0xca | 0xcb | 0xcd | 0xce | 0xcf
    )
}

fn probe_gif(data: &[u8]) -> Option<ImageInfo> {
    let width_px = u32::from(read_le_u16(data, 6)?);
    let height_px = u32::from(read_le_u16(data, 8)?);
    let packed = *data.get(10)?;
    data.get(11..13)?;
    if width_px == 0 || height_px == 0 {
        return None;
    }
    Some(ImageInfo {
        format: ImageFormat::Gif,
        width_px,
        height_px,
        dpi_x: None,
        dpi_y: None,
        bit_depth: ((packed >> 4) & 0x07) + 1,
        channels: 3,
        has_alpha: false,
    })
}

fn probe_bmp(data: &[u8]) -> Option<ImageInfo> {
    data.get(..14)?;
    let dib_size = read_le_u32(data, 14)?;
    let (width_px, height_px, bits_per_pixel, dpi_x, dpi_y, compression, alpha_mask, core_header) =
        if dib_size == 12 {
            data.get(14..26)?;
            if read_le_u16(data, 22)? != 1 {
                return None;
            }
            (
                u32::from(read_le_u16(data, 18)?),
                u32::from(read_le_u16(data, 20)?),
                read_le_u16(data, 24)?,
                None,
                None,
                0,
                0,
                true,
            )
        } else if matches!(dib_size, 40 | 52 | 56 | 108 | 124) {
            let dib_end = 14usize.checked_add(usize::try_from(dib_size).ok()?)?;
            data.get(14..dib_end)?;
            let width = read_le_i32(data, 18)?;
            let height = read_le_i32(data, 22)?;
            if width <= 0 || height == 0 || read_le_u16(data, 26)? != 1 {
                return None;
            }
            let width_px = u32::try_from(width).ok()?;
            let height_px = height
                .checked_abs()
                .and_then(|value| u32::try_from(value).ok())?;
            let ppm_x = read_le_i32(data, 38)?;
            let ppm_y = read_le_i32(data, 42)?;
            let compression = read_le_u32(data, 30)?;
            if dib_size == 40 && compression == 3 {
                data.get(dib_end..dib_end.checked_add(12)?)?;
            }
            (
                width_px,
                height_px,
                read_le_u16(data, 28)?,
                (ppm_x > 0).then(|| f64::from(ppm_x) * 0.0254),
                (ppm_y > 0).then(|| f64::from(ppm_y) * 0.0254),
                compression,
                if dib_size >= 56 {
                    read_le_u32(data, 66)?
                } else {
                    0
                },
                false,
            )
        } else {
            return None;
        };
    if width_px == 0 || height_px == 0 {
        return None;
    }
    let (bit_depth, channels, has_alpha) =
        match (core_header, bits_per_pixel, compression, alpha_mask != 0) {
            (true, 1 | 4 | 8, 0, _) => (u8::try_from(bits_per_pixel).ok()?, 1, false),
            (true, 24, 0, _) => (8, 3, false),
            (false, 1 | 4 | 8, 0, _) => (u8::try_from(bits_per_pixel).ok()?, 1, false),
            (false, 16, 0 | 3, _) => (5, 3, false),
            (false, 24, 0, _) => (8, 3, false),
            (false, 32, 3 | 6, true) => (8, 4, true),
            (false, 32, 0 | 3, _) => (8, 3, false),
            _ => return None,
        };
    Some(ImageInfo {
        format: ImageFormat::Bmp,
        width_px,
        height_px,
        dpi_x,
        dpi_y,
        bit_depth,
        channels,
        has_alpha,
    })
}

fn probe_webp(data: &[u8]) -> Option<ImageInfo> {
    let riff_size = usize::try_from(read_le_u32(data, 4)?).ok()?;
    let riff_end = riff_size.checked_add(8)?;
    if riff_end > data.len() || riff_size < 12 || riff_size & 1 != 0 {
        return None;
    }
    let kind = data.get(12..16)?;
    let chunk_size = usize::try_from(read_le_u32(data, 16)?).ok()?;
    let payload_end = 20usize.checked_add(chunk_size)?;
    let padded_end = payload_end.checked_add(chunk_size & 1)?;
    if padded_end > riff_end {
        return None;
    }
    let payload = data.get(20..payload_end)?;
    let (width_px, height_px, has_alpha) = match kind {
        b"VP8 " => {
            if payload.len() < 10
                || payload[0] & 1 != 0
                || payload[0] & 0x10 == 0
                || (payload[0] >> 1) & 0x07 > 3
                || payload.get(3..6)? != b"\x9d\x01\x2a"
            {
                return None;
            }
            (
                u32::from(read_le_u16(payload, 6)? & 0x3fff),
                u32::from(read_le_u16(payload, 8)? & 0x3fff),
                false,
            )
        }
        b"VP8L" => {
            if payload.len() < 5 || payload[0] != 0x2f {
                return None;
            }
            let bits = read_le_u32(payload, 1)?;
            if bits >> 29 != 0 {
                return None;
            }
            (
                (bits & 0x3fff) + 1,
                ((bits >> 14) & 0x3fff) + 1,
                bits & (1 << 28) != 0,
            )
        }
        b"VP8X" => {
            if payload.len() < 10 || payload[0] & 0xc1 != 0 || payload.get(1..4)? != [0, 0, 0] {
                return None;
            }
            let width_px = read_le_u24(payload, 4)?.checked_add(1)?;
            let height_px = read_le_u24(payload, 7)?.checked_add(1)?;
            width_px.checked_mul(height_px)?;
            (width_px, height_px, payload[0] & 0x10 != 0)
        }
        _ => return None,
    };
    if width_px == 0 || height_px == 0 {
        return None;
    }
    if chunk_size & 1 != 0 && *data.get(payload_end)? != 0 {
        return None;
    }
    Some(ImageInfo {
        format: ImageFormat::Webp,
        width_px,
        height_px,
        dpi_x: None,
        dpi_y: None,
        bit_depth: 8,
        channels: if has_alpha { 4 } else { 3 },
        has_alpha,
    })
}

fn read_be_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_be_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_le_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_le_u24(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(3)?)?;
    Some(u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16))
}

fn read_le_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_le_i32(data: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn is_svg(data: &[u8]) -> bool {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let mut data = trim_ascii_start(data);
    loop {
        data = if data.starts_with(b"<?") {
            let Some(rest) = strip_through(data, b"?>") else {
                return false;
            };
            trim_ascii_start(rest)
        } else if data.starts_with(b"<!--") {
            let Some(rest) = strip_through(data, b"-->") else {
                return false;
            };
            trim_ascii_start(rest)
        } else if data.starts_with(b"<!DOCTYPE") {
            let Some(rest) = strip_doctype(data) else {
                return false;
            };
            trim_ascii_start(rest)
        } else {
            break;
        };
    }
    starts_with_svg_element(data)
}

fn starts_with_svg_element(data: &[u8]) -> bool {
    data.strip_prefix(b"<svg").is_some_and(|rest| {
        matches!(rest.first(), Some(b'>'))
            || rest.starts_with(b"/>")
            || matches!(rest.first(), Some(byte) if byte.is_ascii_whitespace())
    })
}

fn trim_ascii_start(mut data: &[u8]) -> &[u8] {
    while matches!(data.first(), Some(byte) if byte.is_ascii_whitespace()) {
        data = &data[1..];
    }
    data
}

fn strip_through<'a>(data: &'a [u8], terminator: &[u8]) -> Option<&'a [u8]> {
    let end = data
        .windows(terminator.len())
        .position(|window| window == terminator)?;
    data.get(end + terminator.len()..)
}

fn strip_doctype(data: &[u8]) -> Option<&[u8]> {
    let mut quote = None;
    let mut subset_depth = 0usize;
    let mut index = b"<!DOCTYPE".len();
    while index < data.len() {
        let byte = data[index];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }

        if data[index..].starts_with(b"<!--") {
            let rest = data.get(index + 4..)?;
            let end = rest.windows(3).position(|window| window == b"-->")?;
            index += 4 + end + 3;
            continue;
        }
        if data[index..].starts_with(b"<?") {
            let rest = data.get(index + 2..)?;
            let end = rest.windows(2).position(|window| window == b"?>")?;
            index += 2 + end + 2;
            continue;
        }

        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'[' => subset_depth += 1,
            b']' => subset_depth = subset_depth.saturating_sub(1),
            b'>' if subset_depth == 0 => return data.get(index + 1..),
            _ => {}
        }
        index += 1;
    }
    None
}

fn is_standard_wmf(data: &[u8]) -> bool {
    matches!(data.get(..2), Some([1, 0] | [2, 0]))
        && data.get(2..4) == Some(&[9, 0])
        && matches!(data.get(4..6), Some([0, 1] | [0, 3]))
}
