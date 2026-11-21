//! Audio and video: reading, adding, replacing, extracting and removing media, the media store, and the checks on picture and media bytes.

use super::*;

// Decoding is validation only. Original owned bytes are exported unchanged.
const SEMANTIC_MEDIA_BYTES: usize = 128 * 1024 * 1024;
const SEMANTIC_DECODED_BYTES: usize = 64 * 1024 * 1024;
const SEMANTIC_MEDIA_DIMENSION: usize = 16_384;
const SEMANTIC_MEDIA_FRAMES: usize = 1_024;

fn semantic_pixel_bytes(width: usize, height: usize) -> std::result::Result<usize, &'static str> {
    let size = width.checked_mul(height).and_then(|n| n.checked_mul(4));
    if width == 0
        || height == 0
        || width > SEMANTIC_MEDIA_DIMENSION
        || height > SEMANTIC_MEDIA_DIMENSION
        || size.is_none_or(|n| n > SEMANTIC_DECODED_BYTES)
    {
        return Err("picture dimensions exceed the bounded decode limit");
    }
    Ok(size.unwrap_or(0))
}

pub(crate) fn validate_semantic_picture(
    data: &[u8],
    format: oxoffice_media::ImageFormat,
) -> std::result::Result<(), &'static str> {
    use oxoffice_media::ImageFormat;
    let invalid = || "invalid or unsupported picture body";
    if data.len() > SEMANTIC_MEDIA_BYTES {
        return Err("picture exceeds the compressed byte limit");
    }
    match format {
        ImageFormat::Png => {
            let info = oxoffice_media::probe(data).ok_or_else(invalid)?;
            semantic_pixel_bytes(info.width_px as usize, info.height_px as usize)?;
            // Check every chunk extent and CRC, including chunks after pixel data.
            let mut position = 8usize;
            let mut ended = false;
            while position < data.len() {
                let length = data.get(position..position + 4).ok_or_else(invalid)?;
                let length = u32::from_be_bytes(length.try_into().map_err(|_| invalid())?) as usize;
                let end = position
                    .checked_add(12)
                    .and_then(|n| n.checked_add(length))
                    .filter(|n| *n <= data.len())
                    .ok_or_else(invalid)?;
                let kind = &data[position + 4..position + 8];
                if matches!(kind, b"acTL" | b"fcTL" | b"fdAT") {
                    return Err("animated PNG body is unsupported");
                }
                let mut crc = !0u32;
                for byte in &data[position + 4..end - 4] {
                    crc ^= u32::from(*byte);
                    for _ in 0..8 {
                        crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
                    }
                }
                let expected =
                    u32::from_be_bytes(data[end - 4..end].try_into().map_err(|_| invalid())?);
                if !crc != expected {
                    return Err(invalid());
                }
                if kind == b"IEND" {
                    if length != 0 || end != data.len() {
                        return Err(invalid());
                    }
                    ended = true;
                }
                position = end;
            }
            if !ended {
                return Err(invalid());
            }
            tiny_skia::Pixmap::decode_png(data).map_err(|_| invalid())?;
        }
        ImageFormat::Jpeg => {
            use zune_jpeg::zune_core::{bytestream::ZCursor, options::DecoderOptions};
            if !semantic_jpeg_has_complete_scan(data) {
                return Err(invalid());
            }
            let options = DecoderOptions::default()
                .set_max_width(SEMANTIC_MEDIA_DIMENSION)
                .set_max_height(SEMANTIC_MEDIA_DIMENSION)
                .set_strict_mode(true)
                .jpeg_set_max_scans(SEMANTIC_MEDIA_FRAMES);
            let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
            decoder.decode_headers().map_err(|_| invalid())?;
            let (width, height) = decoder.dimensions().ok_or_else(invalid)?;
            semantic_pixel_bytes(width, height)?;
            if decoder
                .output_buffer_size()
                .is_none_or(|n| n > SEMANTIC_DECODED_BYTES)
            {
                return Err(invalid());
            }
            decoder.decode().map_err(|_| invalid())?;
        }
        ImageFormat::Gif => {
            let mut options = gif::DecodeOptions::new();
            options.set_memory_limit(gif::MemoryLimit::Bytes(
                std::num::NonZeroU64::new(SEMANTIC_DECODED_BYTES as u64).ok_or_else(invalid)?,
            ));
            options.check_frame_consistency(true);
            options.set_color_output(gif::ColorOutput::RGBA);
            let mut decoder = options
                .read_info(std::io::Cursor::new(data))
                .map_err(|_| invalid())?;
            semantic_pixel_bytes(usize::from(decoder.width()), usize::from(decoder.height()))?;
            let mut total = 0usize;
            let mut frames = 0usize;
            while let Some(frame) = decoder.next_frame_info().map_err(|_| invalid())? {
                frames += 1;
                let bytes =
                    semantic_pixel_bytes(usize::from(frame.width), usize::from(frame.height))?;
                total = total
                    .checked_add(bytes)
                    .filter(|n| *n <= SEMANTIC_DECODED_BYTES)
                    .ok_or_else(invalid)?;
                if frames > SEMANTIC_MEDIA_FRAMES {
                    return Err(invalid());
                }
                let mut pixels = vec![0; bytes];
                decoder
                    .read_into_buffer(&mut pixels)
                    .map_err(|_| invalid())?;
            }
            if frames == 0 || data.last() != Some(&0x3b) {
                return Err(invalid());
            }
        }
        ImageFormat::Tiff => {
            let mut limits = tiff::decoder::Limits::default();
            limits.decoding_buffer_size = SEMANTIC_DECODED_BYTES;
            limits.intermediate_buffer_size = 16 * 1024 * 1024;
            let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(data))
                .map_err(|_| invalid())?
                .with_limits(limits);
            let mut total = 0usize;
            for page in 0..SEMANTIC_MEDIA_FRAMES {
                let (width, height) = decoder.dimensions().map_err(|_| invalid())?;
                semantic_pixel_bytes(width as usize, height as usize)?;
                let layout = decoder.image_buffer_layout().map_err(|_| invalid())?;
                total = total
                    .checked_add(layout.complete_len)
                    .filter(|n| *n <= SEMANTIC_DECODED_BYTES)
                    .ok_or_else(invalid)?;
                let mut decoded = tiff::decoder::DecodingResult::U8(Vec::new());
                let actual = decoder
                    .read_image_to_buffer(&mut decoded)
                    .map_err(|_| invalid())?;
                if decoded.as_buffer(0).as_bytes().len() != actual.complete_len {
                    return Err(invalid());
                }
                if !decoder.more_images() {
                    return Ok(());
                }
                if page + 1 == SEMANTIC_MEDIA_FRAMES {
                    return Err(invalid());
                }
                decoder.next_image().map_err(|_| invalid())?;
            }
        }
        ImageFormat::Bmp => validate_semantic_bmp(data)?,
        _ => return Err(invalid()),
    }
    Ok(())
}

// JPEG decoders may recover missing EOI even in strict mode. Walk declared
// segments and escaped entropy bytes to require the actual scan terminator.
fn semantic_jpeg_has_complete_scan(data: &[u8]) -> bool {
    if data.get(..2) != Some(&[0xff, 0xd8]) {
        return false;
    }
    let mut position = 2usize;
    let mut scanned = false;
    while position < data.len() {
        if data[position] != 0xff {
            return false;
        }
        while data.get(position) == Some(&0xff) {
            position += 1;
        }
        let Some(&marker) = data.get(position) else {
            return false;
        };
        position += 1;
        if marker == 0xd9 {
            return scanned;
        }
        if marker == 0x01 {
            continue;
        }
        if matches!(marker, 0x00 | 0xd0..=0xd8) {
            return false;
        }
        let Some(length) = data.get(position..position + 2) else {
            return false;
        };
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        let Some(end) = position
            .checked_add(length)
            .filter(|end| length >= 2 && *end <= data.len())
        else {
            return false;
        };
        position = end;
        if marker != 0xda {
            continue;
        }
        scanned = true;
        loop {
            let Some(&byte) = data.get(position) else {
                return false;
            };
            if byte != 0xff {
                position += 1;
                continue;
            }
            let marker_start = position;
            while data.get(position) == Some(&0xff) {
                position += 1;
            }
            let Some(&marker) = data.get(position) else {
                return false;
            };
            if marker == 0 || matches!(marker, 0xd0..=0xd7) {
                position += 1;
                continue;
            }
            position = marker_start;
            break;
        }
    }
    false
}

fn validate_semantic_bmp(data: &[u8]) -> std::result::Result<(), &'static str> {
    let invalid = || "invalid or unsupported BMP pixel body";
    let u16_at = |offset| -> std::result::Result<u16, &'static str> {
        Ok(u16::from_le_bytes(
            data.get(offset..offset + 2)
                .ok_or_else(invalid)?
                .try_into()
                .map_err(|_| invalid())?,
        ))
    };
    let u32_at = |offset| -> std::result::Result<u32, &'static str> {
        Ok(u32::from_le_bytes(
            data.get(offset..offset + 4)
                .ok_or_else(invalid)?
                .try_into()
                .map_err(|_| invalid())?,
        ))
    };
    let header = u32_at(14)? as usize;
    let (width, height, planes, bits, palette_entry, colors) = if header == 12 {
        (
            usize::from(u16_at(18)?),
            usize::from(u16_at(20)?),
            u16_at(22)?,
            u16_at(24)?,
            3usize,
            0usize,
        )
    } else if matches!(header, 40 | 52 | 56 | 108 | 124) {
        if u32_at(30)? != 0 {
            return Err("compressed BMP body is unsupported");
        }
        let width = i32::from_le_bytes(u32_at(18)?.to_le_bytes());
        let height = i32::from_le_bytes(u32_at(22)?.to_le_bytes());
        if width <= 0 || height == i32::MIN {
            return Err(invalid());
        }
        (
            width as usize,
            height.unsigned_abs() as usize,
            u16_at(26)?,
            u16_at(28)?,
            4usize,
            u32_at(46)? as usize,
        )
    } else {
        return Err(invalid());
    };
    semantic_pixel_bytes(width, height)?;
    if planes != 1 || !matches!(bits, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(invalid());
    }
    let palette = if bits <= 8 {
        let maximum = 1usize << bits;
        if colors > maximum {
            return Err(invalid());
        }
        if colors == 0 { maximum } else { colors }
    } else {
        colors
    };
    let minimum = 14usize
        .checked_add(header)
        .and_then(|n| {
            palette
                .checked_mul(palette_entry)
                .and_then(|p| n.checked_add(p))
        })
        .ok_or_else(invalid)?;
    let offset = u32_at(10)? as usize;
    let stride = width
        .checked_mul(usize::from(bits))
        .and_then(|n| n.checked_add(31))
        .map(|n| n / 32 * 4)
        .ok_or_else(invalid)?;
    let end = height
        .checked_mul(stride)
        .and_then(|n| offset.checked_add(n))
        .ok_or_else(invalid)?;
    let declared = u32_at(2)? as usize;
    if offset < minimum || end > data.len() || declared < end || declared > data.len() {
        return Err(invalid());
    }
    if bits <= 8 {
        for row in 0..height {
            for column in 0..width {
                let bit = column * usize::from(bits);
                let value = data[offset + row * stride + bit / 8];
                let shift = 8 - usize::from(bits) - bit % 8;
                let index = usize::from((value >> shift) & ((1u16 << bits) - 1) as u8);
                if index >= palette {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}

/// One inspected media source resolved through the slide relationship scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaLocation {
    Embedded {
        part_name: String,
        content_type: String,
    },
    Linked {
        target: String,
    },
}

/// Caller bytes and package metadata for one embedded media payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddedMediaInput<'a> {
    pub bytes: &'a [u8],
    pub filename: &'a str,
    pub content_type: &'a str,
}

/// A new embedded or external media source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaSourceInput<'a> {
    Embedded(EmbeddedMediaInput<'a>),
    Linked {
        target: &'a str,
        content_type: &'a str,
    },
}

/// Required poster bytes for a newly authored media picture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaPoster<'a> {
    pub bytes: &'a [u8],
    pub filename: &'a str,
}

/// The supported media playback settings retained by inspection and mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaPlaybackSettings {
    pub trim_start_ms: Option<u64>,
    pub trim_end_ms: Option<u64>,
    pub volume: u32,
    pub looped: bool,
    pub show_when_stopped: bool,
    pub trigger: MediaPlaybackTrigger,
}

impl Default for MediaPlaybackSettings {
    fn default() -> Self {
        Self {
            trim_start_ms: None,
            trim_end_ms: None,
            volume: 100_000,
            looped: false,
            show_when_stopped: false,
            trigger: MediaPlaybackTrigger::OnClick,
        }
    }
}

/// A non-fatal limitation discovered while inspecting media.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaDiagnostic {
    UnsupportedContentType { content_type: String },
    MissingPlaybackTiming,
    UnsupportedPlaybackCommand { command: String },
    MissingPoster,
}

/// One media picture and its package-resolved playback state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaInfo {
    pub slide_index: usize,
    pub shape_id: u32,
    pub kind: MediaKind,
    pub source: MediaLocation,
    pub poster_relationship_id: Option<String>,
    pub settings: MediaPlaybackSettings,
    pub diagnostics: Vec<MediaDiagnostic>,
}

impl MediaStore {
    pub(super) fn scan(package: &OpcPackage) -> Self {
        Self::scan_hashing(package, |bytes| MediaId::from_bytes(bytes))
    }

    /// The store [`MediaStore::scan`] gives for `package`, hashing only the
    /// media whose bytes this store does not hold.
    pub(super) fn rescan(&self, package: &OpcPackage) -> Self {
        let known = self
            .by_id
            .iter()
            .flat_map(|(id, bucket)| {
                bucket
                    .iter()
                    .map(move |entry| (Arc::as_ptr(&entry.bytes), *id))
            })
            .collect::<HashMap<_, _>>();
        Self::scan_hashing(package, |bytes| {
            known
                .get(&Arc::as_ptr(bytes))
                .copied()
                .unwrap_or_else(|| MediaId::from_bytes(bytes))
        })
    }

    fn scan_hashing(package: &OpcPackage, id_of: impl Fn(&Arc<Vec<u8>>) -> MediaId) -> Self {
        let namer = MediaNamer::scan(
            "/ppt/media",
            "image",
            package.parts.keys().map(String::as_str),
        );
        let media_namer = MediaNamer::scan(
            "/ppt/media",
            "media",
            package.parts.keys().map(String::as_str),
        );
        let mut by_id: HashMap<MediaId, Vec<MediaEntry>> = HashMap::new();
        let mut parts = package
            .parts
            .iter()
            .filter(|(part_name, _)| {
                part_name
                    .get(.."/ppt/media/".len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("/ppt/media/"))
            })
            .collect::<Vec<_>>();
        parts.sort_unstable_by_key(|(part_name, _)| part_name.as_str());
        for (part_name, bytes) in parts {
            let format = resolve(bytes, part_name);
            let extension = part_name
                .rsplit_once('.')
                .map_or_else(|| format.extension(), |(_, extension)| extension)
                .to_owned();
            let content_type = package
                .content_types
                .content_type_for(part_name)
                .unwrap_or_else(|| format.content_type())
                .to_owned();
            by_id.entry(id_of(bytes)).or_default().push(MediaEntry {
                part_name: part_name.clone(),
                bytes: bytes.clone(),
                extension,
                content_type,
                newly_inserted: false,
            });
        }
        Self {
            by_id,
            namer,
            media_namer,
        }
    }

    pub(super) fn insert(
        &mut self,
        package: &mut OpcPackage,
        bytes: &[u8],
        filename: &str,
    ) -> String {
        self.insert_with_id(package, MediaId::from_bytes(bytes), bytes, filename)
    }

    #[allow(dead_code)]
    fn insert_with_id(
        &mut self,
        package: &mut OpcPackage,
        media_id: MediaId,
        bytes: &[u8],
        filename: &str,
    ) -> String {
        if let Some(existing) = self
            .by_id
            .get(&media_id)
            .and_then(|bucket| bucket.iter().find(|entry| entry.bytes.as_slice() == bytes))
        {
            return existing.part_name.clone();
        }

        let format = resolve(bytes, filename);
        let extension = format.extension();
        let content_type = format.content_type();
        let part_name = self.namer.next_part_name(extension);
        let bytes = Arc::new(bytes.to_vec());
        package.set_part(&part_name, Arc::clone(&bytes));
        register_content_type(package, &part_name, extension, content_type);
        self.by_id.entry(media_id).or_default().push(MediaEntry {
            part_name: part_name.clone(),
            bytes,
            extension: extension.to_owned(),
            content_type: content_type.to_owned(),
            newly_inserted: true,
        });
        part_name
    }

    pub(super) fn insert_explicit(
        &mut self,
        package: &mut OpcPackage,
        bytes: &[u8],
        extension: &str,
        content_type: &str,
    ) -> String {
        let media_id = MediaId::from_bytes(bytes);
        if let Some(existing) = self.by_id.get(&media_id).and_then(|bucket| {
            bucket.iter().find(|entry| {
                entry.bytes.as_slice() == bytes
                    && entry.extension == extension
                    && entry.content_type == content_type
            })
        }) {
            return existing.part_name.clone();
        }
        let part_name = self.media_namer.next_part_name(extension);
        let bytes = Arc::new(bytes.to_vec());
        package.set_part(&part_name, Arc::clone(&bytes));
        register_content_type(package, &part_name, extension, content_type);
        self.by_id.entry(media_id).or_default().push(MediaEntry {
            part_name: part_name.clone(),
            bytes,
            extension: extension.to_owned(),
            content_type: content_type.to_owned(),
            newly_inserted: true,
        });
        part_name
    }

    pub(super) fn write_new_parts(&self, package: &mut OpcPackage) {
        for entry in self.by_id.values().flatten() {
            if entry.newly_inserted {
                package.set_part(&entry.part_name, Arc::clone(&entry.bytes));
                register_content_type(
                    package,
                    &entry.part_name,
                    &entry.extension,
                    &entry.content_type,
                );
            }
        }
    }
}

pub(super) fn register_content_type(
    package: &mut OpcPackage,
    part_name: &str,
    extension: &str,
    content_type: &str,
) {
    match package.content_types.content_type_for(part_name) {
        Some(existing) if existing == content_type => {}
        Some(_) => package.content_types.add_override(part_name, content_type),
        None => package.content_types.add_default(extension, content_type),
    }
}

impl Presentation {
    /// Inspects the media pictures on one slide in z-order.
    pub fn media(&self, slide_index: usize) -> Result<Vec<MediaInfo>> {
        let record = self
            .slides
            .get(slide_index)
            .ok_or(Error::UnknownSlideIndex {
                index: slide_index,
                slide_count: self.slides.len(),
            })?;
        media_infos_for_slide(&self.package, slide_index, &record.part_name, &record.slide)
    }

    /// Adds one media picture, poster, relationship scope, and timing node atomically.
    #[allow(clippy::too_many_arguments)]
    pub fn add_media(
        &mut self,
        slide_index: usize,
        kind: MediaKind,
        source: MediaSourceInput<'_>,
        poster: MediaPoster<'_>,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
        settings: MediaPlaybackSettings,
    ) -> Result<ShapeRef<'_>> {
        self.require_slide_index(slide_index)?;
        validate_media_settings(settings)?;
        validate_media_source(source)?;
        if ImageFormat::sniff(poster.bytes).is_none() {
            return Err(invalid_media_mutation(
                "add media",
                format!("poster {} has unsupported image bytes", poster.filename),
            ));
        }
        if width.0 <= 0 || height.0 <= 0 {
            return Err(invalid_media_mutation(
                "add media",
                "media width and height must be positive",
            ));
        }

        let mut staged = self.clone();
        let slide_part = staged.slides[slide_index].part_name.clone();
        let shape_id = ShapeIdAllocator::scan(
            &staged.slides[slide_index]
                .slide
                .common_slide_data
                .shape_tree,
        )
        .allocate();
        let poster_part =
            staged
                .media_store
                .insert(&mut staged.package, poster.bytes, poster.filename);
        let mut relationships = staged
            .package
            .get_part_rels(&slide_part)
            .cloned()
            .unwrap_or_default();
        let poster_relationship_id = relationships.add(
            rel_types::IMAGE,
            &relative_part_target(&slide_part, &poster_part),
        );
        let (standard_relationship_id, microsoft_relationship_id, picture_source) =
            add_media_relationships(
                &mut staged.package,
                &mut staged.media_store,
                &slide_part,
                &mut relationships,
                kind,
                source,
                true,
            )?;
        let microsoft_relationship_id = microsoft_relationship_id
            .expect("new media pictures always include the Office media relationship");
        let picture = CT_Picture::new_media(
            shape_id,
            &format!("{} {shape_id}", media_kind_name(kind)),
            kind,
            match picture_source {
                PictureMediaSource::Embedded { .. } => PictureMediaSource::Embedded {
                    relationship_id: standard_relationship_id,
                },
                PictureMediaSource::Linked { .. } => PictureMediaSource::Linked {
                    relationship_id: standard_relationship_id,
                },
            },
            &microsoft_relationship_id,
            &poster_relationship_id,
            settings.trim_start_ms,
            settings.trim_end_ms,
            positioned_transform(left, top, width, height),
        )
        .map_err(|error| invalid_media_mutation("add media", error.to_string()))?;
        Arc::make_mut(&mut staged.slides[slide_index])
            .slide
            .common_slide_data
            .shape_tree
            .append_child(ShapeTreeChild::Picture(picture));
        let timing = Arc::make_mut(&mut staged.slides[slide_index])
            .slide
            .timing
            .get_or_insert_with(|| {
                CT_Timing::from_xml(
                    format!(r#"<p:timing xmlns:p="{P_NS}"><p:tnLst/></p:timing>"#).as_bytes(),
                )
                .expect("canonical media timing shell")
            });
        timing
            .add_media(
                kind == MediaKind::Video,
                shape_id,
                settings.volume,
                settings.looped,
                settings.show_when_stopped,
                settings.trigger,
            )
            .map_err(|error| invalid_media_mutation("add media", error.to_string()))?;
        staged.package.set_part_rels(&slide_part, relationships);
        self.commit_candidate(staged)?;
        self.slides[slide_index]
            .slide
            .common_slide_data
            .shape_tree
            .children
            .iter()
            .find(|child| child.non_visual_id() == Some(shape_id))
            .map(shape_ref)
            .ok_or_else(|| invalid_media_mutation("add media", "authored picture was not retained"))
    }

    /// Replaces one media source while preserving its shape, poster, and timing.
    pub fn replace_media(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        source: MediaSourceInput<'_>,
    ) -> Result<()> {
        self.require_slide_index(slide_index)?;
        validate_media_source(source)?;
        let mut staged = self.clone();
        let slide_part = staged.slides[slide_index].part_name.clone();
        let picture = media_picture_shape(&staged.slides[slide_index].slide, shape_id)?;
        let media = picture.media.as_ref().expect("media picture").clone();
        let standard_relationship_id = picture.standard_media_relationship_id().map(str::to_owned);
        let has_office_media = !picture.office_media_relationship_ids().is_empty();
        let mut old_relationship_ids = picture.office_media_relationship_ids().to_vec();
        if !old_relationship_ids
            .iter()
            .any(|id| id == media.source.relationship_id())
        {
            old_relationship_ids.push(media.source.relationship_id().to_owned());
        }
        if let Some(standard_relationship_id) = &standard_relationship_id
            && !old_relationship_ids.contains(standard_relationship_id)
        {
            old_relationship_ids.push(standard_relationship_id.clone());
        }
        let mut relationships = staged
            .package
            .get_part_rels(&slide_part)
            .cloned()
            .unwrap_or_default();
        let old_relationships = old_relationship_ids
            .iter()
            .map(|relationship_id| {
                relationships
                    .get_by_id(relationship_id)
                    .cloned()
                    .ok_or_else(|| Error::MissingRelationship {
                        source_part: slide_part.clone(),
                        relationship_id: relationship_id.clone(),
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        let (standard_id, microsoft_id, picture_source) = add_media_relationships(
            &mut staged.package,
            &mut staged.media_store,
            &slide_part,
            &mut relationships,
            media.kind,
            source,
            has_office_media,
        )?;
        let source_id = microsoft_id.unwrap_or_else(|| standard_id.clone());
        let replacement_source = match picture_source {
            PictureMediaSource::Embedded { .. } => PictureMediaSource::Embedded {
                relationship_id: source_id,
            },
            PictureMediaSource::Linked { .. } => PictureMediaSource::Linked {
                relationship_id: source_id,
            },
        };
        let target_picture = Arc::make_mut(&mut staged.slides[slide_index])
            .slide
            .common_slide_data
            .shape_tree
            .children
            .iter_mut()
            .find(|child| child.non_visual_id() == Some(shape_id))
            .and_then(|child| match child {
                ShapeTreeChild::Picture(picture) => Some(picture),
                _ => None,
            })
            .ok_or_else(|| invalid_media_mutation("replace media", "media picture is missing"))?;
        target_picture
            .replace_media_relationship_ids(&standard_id, replacement_source)
            .map_err(|error| invalid_media_mutation("replace media", error.to_string()))?;
        let referenced = slide_relationship_ids(&staged.slides[slide_index].slide)?;
        let mut candidates = HashSet::new();
        relationships.items.retain(|relationship| {
            let Some(old) = old_relationships
                .iter()
                .find(|old| old.id == relationship.id)
            else {
                return true;
            };
            if referenced.contains(&old.id) {
                return true;
            }
            if !relationship_is_external(old) {
                candidates.insert(OpcPackage::resolve_rel_target(&slide_part, &old.target));
            }
            false
        });
        staged.package.set_part_rels(&slide_part, relationships);
        prune_unreachable_parts(&mut staged.package, &candidates);
        staged.media_store = staged.media_store.rescan(&staged.package);
        self.commit_candidate(staged)
    }

    /// Extracts embedded media bytes, or returns `None` for linked media.
    pub fn extract_media(&self, slide_index: usize, shape_id: u32) -> Result<Option<Vec<u8>>> {
        let record = self
            .slides
            .get(slide_index)
            .ok_or(Error::UnknownSlideIndex {
                index: slide_index,
                slide_count: self.slides.len(),
            })?;
        let media = media_picture(&record.slide, shape_id)?;
        let relationship_id = media.source.relationship_id();
        let relationship = self
            .package
            .get_part_rels(&record.part_name)
            .and_then(|relationships| relationships.get_by_id(relationship_id))
            .ok_or_else(|| Error::MissingRelationship {
                source_part: record.part_name.clone(),
                relationship_id: relationship_id.to_owned(),
            })?;
        if relationship_is_external(relationship) {
            return Ok(None);
        }
        let part_name = OpcPackage::resolve_rel_target(&record.part_name, &relationship.target);
        Ok(Some(required_part(&self.package, &part_name)?.to_vec()))
    }

    /// Removes one media picture and only its relationship-owned package candidates.
    pub fn remove_media(&mut self, slide_index: usize, shape_id: u32) -> Result<()> {
        self.require_slide_index(slide_index)?;
        let mut staged = self.clone();
        let slide_part = staged.slides[slide_index].part_name.clone();
        let picture = media_picture_shape(&staged.slides[slide_index].slide, shape_id)?;
        let media = picture.media.as_ref().expect("media picture").clone();
        let standard_relationship_id = picture.standard_media_relationship_id().map(str::to_owned);
        let office_relationship_ids = picture.office_media_relationship_ids().to_vec();
        let mut relationships = staged
            .package
            .get_part_rels(&slide_part)
            .cloned()
            .unwrap_or_default();
        let mut owned_ids = office_relationship_ids.into_iter().collect::<HashSet<_>>();
        owned_ids.insert(media.source.relationship_id().to_owned());
        if let Some(standard_relationship_id) = standard_relationship_id {
            owned_ids.insert(standard_relationship_id);
        }
        if let Some(poster_relationship_id) = media.poster_relationship_id {
            owned_ids.insert(poster_relationship_id);
        }
        for relationship_id in &owned_ids {
            relationships
                .get_by_id(relationship_id)
                .ok_or_else(|| Error::MissingRelationship {
                    source_part: slide_part.clone(),
                    relationship_id: relationship_id.clone(),
                })?;
        }
        Arc::make_mut(&mut staged.slides[slide_index])
            .slide
            .common_slide_data
            .shape_tree
            .remove_child_by_id(shape_id)
            .map_err(|error| invalid_media_mutation("remove media", error.to_string()))?
            .ok_or_else(|| invalid_media_mutation("remove media", "media picture is missing"))?;
        if let Some(timing) = &mut Arc::make_mut(&mut staged.slides[slide_index]).slide.timing {
            timing
                .remove_media(shape_id)
                .map_err(|error| invalid_media_mutation("remove media", error.to_string()))?;
        }
        let referenced = slide_relationship_ids(&staged.slides[slide_index].slide)?;
        let mut candidates = HashSet::new();
        relationships.items.retain(|relationship| {
            if !owned_ids.contains(&relationship.id) || referenced.contains(&relationship.id) {
                return true;
            }
            if !relationship_is_external(relationship) {
                candidates.insert(OpcPackage::resolve_rel_target(
                    &slide_part,
                    &relationship.target,
                ));
            }
            false
        });
        staged.package.set_part_rels(&slide_part, relationships);
        prune_unreachable_parts(&mut staged.package, &candidates);
        staged.media_store = staged.media_store.rescan(&staged.package);
        self.commit_candidate(staged)
    }
}

pub(super) fn invalid_media_mutation(operation: &'static str, message: impl Into<String>) -> Error {
    Error::InvalidMediaMutation {
        operation,
        message: message.into(),
    }
}

fn media_kind_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Audio => "Audio",
        MediaKind::Video => "Video",
    }
}

pub(super) fn media_relationship_type(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Audio => rel_types::AUDIO,
        MediaKind::Video => rel_types::VIDEO,
    }
}

fn media_picture(slide: &CT_Slide, shape_id: u32) -> Result<&PictureMedia> {
    media_picture_shape(slide, shape_id)?
        .media
        .as_ref()
        .ok_or_else(|| {
            invalid_media_mutation(
                "inspect media",
                format!("shape id {shape_id} is not a media picture"),
            )
        })
}

fn media_picture_shape(slide: &CT_Slide, shape_id: u32) -> Result<&CT_Picture> {
    slide
        .common_slide_data
        .shape_tree
        .children
        .iter()
        .find(|child| child.non_visual_id() == Some(shape_id))
        .and_then(|child| match child {
            ShapeTreeChild::Picture(picture) if picture.media.is_some() => Some(picture),
            _ => None,
        })
        .ok_or_else(|| {
            invalid_media_mutation(
                "inspect media",
                format!("shape id {shape_id} is not a media picture"),
            )
        })
}

pub(super) fn slide_relationship_ids(slide: &CT_Slide) -> Result<HashSet<String>> {
    let xml = slide
        .to_xml()
        .map_err(|error| invalid_media_mutation("inspect media references", error.to_string()))?;
    relationship_ids(&xml)
        .map(|ids| ids.into_iter().collect())
        .map_err(|error| invalid_media_mutation("inspect media references", error.to_string()))
}

pub(super) fn media_playback_settings(
    picture: &CT_Picture,
    timing: Option<&CT_Timing>,
    shape_id: u32,
) -> (MediaPlaybackSettings, Vec<MediaDiagnostic>) {
    let mut settings = MediaPlaybackSettings::default();
    (settings.trim_start_ms, settings.trim_end_ms) = picture.media_trim_bounds();
    let mut diagnostics = Vec::new();
    let Some(timing) = timing else {
        diagnostics.push(MediaDiagnostic::MissingPlaybackTiming);
        return (settings, diagnostics);
    };
    if let Some(media) = timing.media_for_shape(shape_id) {
        settings.volume = media.common.volume.unwrap_or(100_000);
        settings.looped = media.common.looped;
        settings.show_when_stopped = matches!(
            media.common.display,
            Some(MediaDisplayPolicy::ShowWhenStopped)
        );
    } else {
        diagnostics.push(MediaDiagnostic::MissingPlaybackTiming);
    }
    if let Some(trigger) = timing.media_playback_trigger_for_shape(shape_id) {
        settings.trigger = trigger;
    }
    if let Some(command) = timing.media_commands_for_shape(shape_id).first()
        && let MediaCommandKind::Other(command) = &command.command
    {
        diagnostics.push(MediaDiagnostic::UnsupportedPlaybackCommand {
            command: command.clone(),
        });
    }
    (settings, diagnostics)
}

fn validate_media_settings(settings: MediaPlaybackSettings) -> Result<()> {
    if settings.volume > 100_000 {
        return Err(invalid_media_mutation(
            "configure media",
            "volume must be between 0 and 100000",
        ));
    }
    Ok(())
}

fn validate_media_source(source: MediaSourceInput<'_>) -> Result<()> {
    let (bytes, filename, target, content_type) = match source {
        MediaSourceInput::Embedded(input) => (
            Some(input.bytes),
            Some(input.filename),
            None,
            input.content_type,
        ),
        MediaSourceInput::Linked {
            target,
            content_type,
        } => (None, None, Some(target), content_type),
    };
    if !is_safe_content_type(content_type) {
        return Err(invalid_media_mutation(
            "configure media",
            "content type must be an explicit MIME value",
        ));
    }
    if let Some(target) = target
        && (target.is_empty() || target.bytes().any(|byte| byte.is_ascii_control()))
    {
        return Err(invalid_media_mutation(
            "configure media",
            "linked target must be non-empty and contain no control characters",
        ));
    }
    if let (Some(bytes), Some(filename)) = (bytes, filename) {
        safe_media_extension(filename)?;
        if audio_video_signature_matches(bytes, content_type) == Some(false) {
            return Err(invalid_media_mutation(
                "configure media",
                format!("{filename} bytes do not match {content_type}"),
            ));
        }
    }
    Ok(())
}

fn safe_media_extension(filename: &str) -> Result<String> {
    let extension = filename
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 16
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .ok_or_else(|| {
            invalid_media_mutation(
                "configure media",
                format!("{filename} requires a safe filename extension"),
            )
        })?;
    Ok(extension.to_ascii_lowercase())
}

pub(super) fn is_known_media_content_type(content_type: &str) -> bool {
    audio_video_signature_matches(&[], content_type).is_some()
}

fn add_media_relationships(
    package: &mut OpcPackage,
    media_store: &mut MediaStore,
    slide_part: &str,
    relationships: &mut Relationships,
    kind: MediaKind,
    source: MediaSourceInput<'_>,
    include_office_media: bool,
) -> Result<(String, Option<String>, PictureMediaSource)> {
    match source {
        MediaSourceInput::Embedded(input) => {
            let extension = safe_media_extension(input.filename)?;
            let media_part =
                media_store.insert_explicit(package, input.bytes, &extension, input.content_type);
            let target = relative_part_target(slide_part, &media_part);
            let standard = relationships.add(media_relationship_type(kind), &target);
            let microsoft = include_office_media
                .then(|| relationships.add(rel_types::POWERPOINT_MEDIA, &target));
            Ok((
                standard.clone(),
                microsoft,
                PictureMediaSource::Embedded {
                    relationship_id: standard,
                },
            ))
        }
        MediaSourceInput::Linked { target, .. } => {
            let standard = relationships.add_external(media_relationship_type(kind), target);
            let microsoft = include_office_media
                .then(|| relationships.add_external(rel_types::POWERPOINT_MEDIA, target));
            Ok((
                standard.clone(),
                microsoft,
                PictureMediaSource::Linked {
                    relationship_id: standard,
                },
            ))
        }
    }
}
