//! Embedded fonts: embedding, removing and listing them, their embedding licences, and the checks on font files (EOT and sfnt tables).

use super::*;

use oxoffice_layout::font_embedding::{
    MAX_EMBEDDED_FACE_BYTES, SfntTables, check_embedding_license,
};

pub(super) const EMBEDDED_FONT_CONTENT_TYPE: &str = "application/x-fontdata";
const EOT_VERSION: u32 = 0x0002_0001;
const EOT_MAGIC: u16 = 0x504C;
const EOT_FLAG_COMPRESSED: u32 = 0x0000_0004;
const EOT_FIXED_HEADER_LEN: usize = 80;

/// The caller's explicit authorization and exact license identity.
///
/// PresentationML has no place for the license, so it is checked when a face
/// is embedded and is not written to the package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontEmbeddingLicense {
    pub authorized: bool,
    pub identity: String,
}

impl FontEmbeddingLicense {
    pub fn new(authorized: bool, identity: impl Into<String>) -> Self {
        Self {
            authorized,
            identity: identity.into(),
        }
    }
}

/// Caller-owned TrueType or OpenType bytes for one embedded face.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedFont {
    pub kind: EmbeddedFontKind,
    pub data: Vec<u8>,
    pub license: FontEmbeddingLicense,
}

impl EmbeddedFont {
    pub fn new(kind: EmbeddedFontKind, data: Vec<u8>, license: FontEmbeddingLicense) -> Self {
        Self {
            kind,
            data,
            license,
        }
    }
}

/// How a stored embedded font part is encoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedFontFormat {
    /// Embedded OpenType without MicroType Express compression, the form this
    /// crate writes.
    Eot,
    /// Embedded OpenType with MicroType Express compression, as PowerPoint
    /// writes it. The part is preserved but not decoded.
    CompressedEot,
    /// A bare TrueType or OpenType font.
    Sfnt,
    /// A missing part or bytes in no recognized form.
    Unknown,
}

/// One embedded face as the package stores it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedFontFace {
    pub kind: EmbeddedFontKind,
    pub relationship_id: String,
    /// The resolved font part, or `None` when the relationship is missing,
    /// external, or of another type.
    pub part_name: Option<String>,
    pub format: EmbeddedFontFormat,
    pub stored_len: usize,
    /// The sfnt bytes, when the stored form can be decoded. Faces that
    /// reference one part share its decoded bytes.
    pub data: Option<std::sync::Arc<[u8]>>,
}

/// One `p:embeddedFont` entry and its faces in schema order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedTypeface {
    pub typeface: String,
    pub panose: Option<String>,
    pub pitch_family: Option<i8>,
    pub charset: Option<i8>,
    pub faces: Vec<EmbeddedFontFace>,
}

pub(super) const MAX_EMBEDDED_DECODED_BYTES: usize = 256 * 1024 * 1024;
pub(super) type DecodedEmbeddedFont = (EmbeddedFontFormat, Option<std::sync::Arc<[u8]>>);

impl Presentation {
    /// Embeds a single sfnt face after explicit license authorization.
    pub fn embed_font(&mut self, typeface: &str, font: EmbeddedFont) -> Result<()> {
        validate_font_embedding_license(&font.license).map_err(Error::InvalidFontEmbedding)?;
        let mut entry = CT_EmbeddedFontListEntry::new(typeface).map_err(font_model_error)?;
        let sfnt = SfntTables::parse(&font.data).map_err(Error::InvalidFontEmbedding)?;
        let eot = sfnt
            .embedded_open_type()
            .map_err(Error::InvalidFontEmbedding)?;
        let mut staged = self.clone();
        let existing = staged
            .presentation
            .embedded_font_mut(typeface)
            .map_err(font_model_error)?
            .and_then(|entry| entry.face(font.kind))
            .map(str::to_owned);
        let previous = existing
            .as_deref()
            .map(|id| staged.checked_font_target(id))
            .transpose()?;
        if let Some(part) = &previous
            && staged.package.get_part(part) == Some(eot.as_slice())
            && staged.embed_true_type_fonts()? == Some(true)
        {
            return Ok(());
        }
        let reusable = match (&existing, &previous) {
            (Some(id), Some(part)) => {
                staged
                    .presentation
                    .embedded_font_reference_count(id)
                    .map_err(font_model_error)?
                    == 1
                    && font_incoming_relationship_count(&staged.package, part) == 1
            }
            _ => false,
        };
        let (id, part) = if reusable {
            (existing.clone().unwrap(), previous.clone().unwrap())
        } else {
            let part = MediaNamer::scan(
                "/ppt/fonts",
                "font",
                staged.package.parts.keys().map(String::as_str),
            )
            .next_part_name("fntdata");
            let id = staged
                .package
                .get_or_create_part_rels(&staged.presentation_part)
                .add(
                    rel_types::FONT,
                    &relative_part_target(&staged.presentation_part, &part),
                );
            (id, part)
        };
        staged.package.set_part(&part, eot);
        register_content_type(
            &mut staged.package,
            &part,
            "fntdata",
            EMBEDDED_FONT_CONTENT_TYPE,
        );
        match staged
            .presentation
            .embedded_font_mut(typeface)
            .map_err(font_model_error)?
        {
            Some(current) => {
                current
                    .set_face(font.kind, id.clone())
                    .map_err(font_model_error)?;
            }
            None => {
                entry
                    .set_face(font.kind, id.clone())
                    .map_err(font_model_error)?;
                staged
                    .presentation
                    .push_embedded_font(entry)
                    .map_err(font_model_error)?;
            }
        }
        if let Some(old) = existing.filter(|old| old != &id) {
            staged.release_font_relationship(&old)?;
        }
        staged.presentation.set_embed_true_type_fonts(Some(true));
        self.commit_candidate(staged)
    }

    /// Removes one face while retaining preserved font descriptions and payloads.
    pub fn remove_embedded_font(
        &mut self,
        typeface: &str,
        kind: EmbeddedFontKind,
    ) -> Result<Option<EmbeddedFontFace>> {
        let mut staged = self.clone();
        let Some(id) = staged
            .presentation
            .embedded_font_mut(typeface)
            .map_err(font_model_error)?
            .and_then(|entry| entry.face(kind))
            .map(str::to_owned)
        else {
            return Ok(None);
        };
        self.checked_font_target(&id)?;
        let removed = self
            .embedded_fonts()?
            .into_iter()
            .find(|entry| entry.typeface == typeface)
            .and_then(|entry| entry.faces.into_iter().find(|face| face.kind == kind))
            .expect("checked modeled face");
        staged
            .presentation
            .embedded_font_mut(typeface)
            .map_err(font_model_error)?
            .unwrap()
            .remove_face(kind)
            .map_err(font_model_error)?;
        staged.release_font_relationship(&id)?;
        self.commit_candidate(staged)?;
        Ok(Some(removed))
    }

    /// Removes an entry only when it contains no opaque source payload.
    pub fn remove_font(&mut self, typeface: &str) -> Result<Option<EmbeddedTypeface>> {
        let mut staged = self.clone();
        let Some(entry) = staged
            .presentation
            .remove_embedded_font(typeface)
            .map_err(font_model_error)?
        else {
            return Ok(None);
        };
        for (_, id) in entry.faces() {
            self.checked_font_target(id)?;
        }
        let removed = self
            .embedded_fonts()?
            .into_iter()
            .find(|font| font.typeface == typeface)
            .expect("checked modeled entry");
        for (_, id) in entry.faces() {
            staged.release_font_relationship(id)?;
        }
        self.commit_candidate(staged)?;
        Ok(Some(removed))
    }

    /// Returns the presence-sensitive PowerPoint font-use setting.
    pub fn embed_true_type_fonts(&self) -> Result<Option<bool>> {
        self.presentation
            .embed_true_type_fonts()
            .map_err(font_model_error)
    }
    pub fn save_subset_fonts(&self) -> Result<Option<bool>> {
        self.presentation
            .save_subset_fonts()
            .map_err(font_model_error)
    }
    pub fn set_embed_true_type_fonts(&mut self, value: Option<bool>) -> Result<()> {
        if self.embed_true_type_fonts().ok() == Some(value) {
            return Ok(());
        }
        let mut staged = self.clone();
        staged.presentation.set_embed_true_type_fonts(value);
        self.commit_candidate(staged)
    }
    pub fn set_save_subset_fonts(&mut self, value: Option<bool>) -> Result<()> {
        if self.save_subset_fonts().ok() == Some(value) {
            return Ok(());
        }
        let mut staged = self.clone();
        staged.presentation.set_save_subset_fonts(value);
        self.commit_candidate(staged)
    }

    pub(super) fn checked_font_target(&self, id: &str) -> Result<String> {
        let relationship = self
            .package
            .get_part_rels(&self.presentation_part)
            .and_then(|rels| rels.get_by_id(id))
            .filter(|rel| rel.rel_type == rel_types::FONT && !relationship_is_external(rel))
            .ok_or_else(|| {
                Error::InvalidFontEmbedding(
                    "font relationship is missing, foreign or external".to_owned(),
                )
            })?;
        let part = OpcPackage::resolve_rel_target(&self.presentation_part, &relationship.target);
        if self.package.get_part(&part).is_none()
            || self.package.content_types.content_type_for(&part)
                != Some(EMBEDDED_FONT_CONTENT_TYPE)
        {
            return Err(Error::InvalidFontEmbedding(
                "font relationship does not target a font-data part".to_owned(),
            ));
        }
        Ok(part)
    }

    fn release_font_relationship(&mut self, id: &str) -> Result<()> {
        let part = self.checked_font_target(id)?;
        if self
            .presentation
            .embedded_font_reference_count(id)
            .map_err(font_model_error)?
            != 0
        {
            return Ok(());
        }
        self.package
            .get_part_rels_mut(&self.presentation_part)
            .unwrap()
            .items
            .retain(|rel| rel.id != id);
        prune_unreachable_parts(&mut self.package, &HashSet::from([part]));
        Ok(())
    }

    /// Lists supported font entries, sharing decoded bytes for each physical part.
    pub fn embedded_fonts(&self) -> Result<Vec<EmbeddedTypeface>> {
        presentation_embedded_fonts(&self.package, &self.presentation_part, &self.presentation)
    }
}

pub(super) fn font_model_error(error: OxmlError) -> Error {
    Error::InvalidFontEmbedding(error.to_string())
}

fn font_incoming_relationship_count(package: &OpcPackage, target: &str) -> usize {
    std::iter::once(("/", &package.package_rels))
        .chain(
            package
                .part_rels
                .iter()
                .map(|(source, rels)| (source.as_str(), rels)),
        )
        .flat_map(|(source, rels)| rels.items.iter().map(move |rel| (source, rel)))
        .filter(|(source, rel)| {
            !relationship_is_external(rel)
                && OpcPackage::resolve_rel_target(source, &rel.target).eq_ignore_ascii_case(target)
        })
        .count()
}

fn validate_font_embedding_license(
    license: &FontEmbeddingLicense,
) -> std::result::Result<(), String> {
    check_embedding_license(license.authorized, &license.identity)
}

/// Inspects the complete stored envelope before exposing a borrowed sfnt body.
pub(super) fn checked_embedded_font(bytes: &[u8]) -> (EmbeddedFontFormat, Option<&[u8]>) {
    if bytes.len() <= MAX_EMBEDDED_FACE_BYTES && SfntTables::parse(bytes).is_ok() {
        return (EmbeddedFontFormat::Sfnt, Some(bytes));
    }
    let unknown = (EmbeddedFontFormat::Unknown, None);
    if bytes.len() < EOT_FIXED_HEADER_LEN
        || read_u16_le(bytes, 34) != Some(EOT_MAGIC)
        || read_u32_le(bytes, 0).map(|size| size as usize) != Some(bytes.len())
    {
        return unknown;
    }
    let version = read_u32_le(bytes, 8).unwrap();
    if !matches!(version, 0x0001_0000 | 0x0002_0001 | 0x0002_0002) {
        return unknown;
    }
    let flags = read_u32_le(bytes, 12).unwrap();
    let mut cursor = EOT_FIXED_HEADER_LEN;
    for _ in 0..if version == 0x0001_0000 { 4 } else { 5 } {
        if read_u16_le(bytes, cursor) != Some(0) {
            return unknown;
        }
        let Some(length) = read_u16_le(bytes, cursor + 2).map(usize::from) else {
            return unknown;
        };
        cursor += 4;
        if !length.is_multiple_of(2) || bytes.get(cursor..cursor + length).is_none() {
            return unknown;
        }
        cursor += length;
    }
    if version == 0x0002_0002 {
        // Root checksum, EUDC codepage, padding, signature and EUDC payload.
        if bytes.get(cursor..cursor + 10).is_none() || read_u16_le(bytes, cursor + 8) != Some(0) {
            return unknown;
        }
        let Some(length) = read_u16_le(bytes, cursor + 10).map(usize::from) else {
            return unknown;
        };
        cursor += 12;
        if bytes.get(cursor..cursor + length).is_none() {
            return unknown;
        }
        cursor += length;
        if bytes.get(cursor..cursor + 8).is_none() {
            return unknown;
        }
        let length = read_u32_le(bytes, cursor + 4).unwrap() as usize;
        cursor += 8;
        let Some(end) = cursor.checked_add(length) else {
            return unknown;
        };
        if bytes.get(cursor..end).is_none() {
            return unknown;
        }
        cursor = end;
    }
    let size = read_u32_le(bytes, 4).unwrap() as usize;
    if cursor.checked_add(size) != Some(bytes.len()) {
        return unknown;
    }
    if flags & EOT_FLAG_COMPRESSED != 0 {
        return (EmbeddedFontFormat::CompressedEot, None);
    }
    if flags != 0 || size > MAX_EMBEDDED_FACE_BYTES {
        return (EmbeddedFontFormat::Eot, None);
    }
    let body = &bytes[cursor..];
    (
        EmbeddedFontFormat::Eot,
        SfntTables::parse(body).is_ok().then_some(body),
    )
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset.checked_add(2)?)
        .map(|slice| u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset.checked_add(4)?)
        .map(|slice| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u16_be(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset.checked_add(2)?)
        .map(|slice| u16::from_be_bytes([slice[0], slice[1]]))
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset.checked_add(4)?)
        .map(|slice| u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// What PowerPoint reads from a checked face to write its embedded font.
trait SfntFace<'a> {
    fn panose(&self) -> &'a [u8];
    fn code_page_ranges(&self) -> (u32, u32);
    fn name_utf16le(&self, name_id: u16) -> Vec<u8>;
    fn embedded_open_type(&self) -> std::result::Result<Vec<u8>, String>;
}

impl<'a> SfntFace<'a> for SfntTables<'a> {
    fn panose(&self) -> &'a [u8] {
        &self.os2[32..42]
    }

    fn code_page_ranges(&self) -> (u32, u32) {
        if self.os2_version() >= 1 && self.os2.len() >= 86 {
            (
                read_u32_be(self.os2, 78).unwrap_or(0),
                read_u32_be(self.os2, 82).unwrap_or(0),
            )
        } else {
            (0, 0)
        }
    }

    /// One `name` table string as UTF-16LE, preferring Windows English.
    fn name_utf16le(&self, name_id: u16) -> Vec<u8> {
        let count = read_u16_be(self.name, 2).unwrap_or(0);
        let strings = usize::from(read_u16_be(self.name, 4).unwrap_or(0));
        let mut best: Option<(u8, &[u8], bool)> = None;
        for index in 0..usize::from(count) {
            let record = 6 + index * 12;
            let (
                Some(platform),
                Some(encoding),
                Some(language),
                Some(id),
                Some(length),
                Some(offset),
            ) = (
                read_u16_be(self.name, record),
                read_u16_be(self.name, record + 2),
                read_u16_be(self.name, record + 4),
                read_u16_be(self.name, record + 6),
                read_u16_be(self.name, record + 8),
                read_u16_be(self.name, record + 10),
            )
            else {
                break;
            };
            if id != name_id {
                continue;
            }
            let rank = match (platform, encoding, language) {
                (3, 1, 0x0409) => 0,
                (3, 0 | 1 | 10, _) => 1,
                (0, _, _) => 2,
                (1, 0, 0) => 3,
                _ => continue,
            };
            let start = strings + usize::from(offset);
            let Some(bytes) = self.name.get(start..start + usize::from(length)) else {
                continue;
            };
            if platform == 1 && !bytes.is_ascii() {
                continue;
            }
            if best.is_none_or(|(best_rank, _, _)| rank < best_rank) {
                best = Some((rank, bytes, platform != 1));
            }
        }
        match best {
            Some((_, bytes, true)) => bytes
                .chunks_exact(2)
                .flat_map(|unit| [unit[1], unit[0]])
                .collect(),
            Some((_, bytes, false)) => bytes
                .iter()
                .flat_map(|byte| {
                    let unit = if byte.is_ascii() {
                        u16::from(*byte)
                    } else {
                        0xFFFD
                    };
                    unit.to_le_bytes()
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// Wraps the font in an uncompressed Embedded OpenType 2.1 header.
    fn embedded_open_type(&self) -> std::result::Result<Vec<u8>, String> {
        let too_large = || "embedded font is too large for Embedded OpenType".to_owned();
        let font_data_size = u32::try_from(self.data.len()).map_err(|_| too_large())?;
        let os2 = self.os2;
        let (code_page_1, code_page_2) = self.code_page_ranges();
        let mut header = Vec::with_capacity(EOT_FIXED_HEADER_LEN + 256);
        header.extend_from_slice(&[0; 4]);
        header.extend_from_slice(&font_data_size.to_le_bytes());
        header.extend_from_slice(&EOT_VERSION.to_le_bytes());
        header.extend_from_slice(&0_u32.to_le_bytes());
        header.extend_from_slice(self.panose());
        header.push(1);
        header.push(u8::from(read_u16_be(os2, 62).unwrap_or(0) & 1 != 0));
        header.extend_from_slice(&u32::from(read_u16_be(os2, 4).unwrap_or(400)).to_le_bytes());
        header.extend_from_slice(&read_u16_be(os2, 8).unwrap_or(0).to_le_bytes());
        header.extend_from_slice(&EOT_MAGIC.to_le_bytes());
        for offset in [42, 46, 50, 54] {
            header.extend_from_slice(&read_u32_be(os2, offset).unwrap_or(0).to_le_bytes());
        }
        header.extend_from_slice(&code_page_1.to_le_bytes());
        header.extend_from_slice(&code_page_2.to_le_bytes());
        header.extend_from_slice(&read_u32_be(self.head, 8).unwrap_or(0).to_le_bytes());
        header.extend_from_slice(&[0; 16]);
        debug_assert_eq!(header.len(), EOT_FIXED_HEADER_LEN);
        // Family, style, version and full names, each after a zero padding word.
        for name_id in [1, 2, 5, 4] {
            let name = self.name_utf16le(name_id);
            let size = u16::try_from(name.len()).map_err(|_| too_large())?;
            header.extend_from_slice(&0_u16.to_le_bytes());
            header.extend_from_slice(&size.to_le_bytes());
            header.extend_from_slice(&name);
        }
        // The padding word and an empty root string.
        header.extend_from_slice(&[0; 4]);
        let eot_size = u32::try_from(header.len() + self.data.len()).map_err(|_| too_large())?;
        header[0..4].copy_from_slice(&eot_size.to_le_bytes());
        header.extend_from_slice(self.data);
        Ok(header)
    }
}
