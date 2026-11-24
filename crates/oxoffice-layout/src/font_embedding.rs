//! Checks shared by Word and PowerPoint before a font is embedded: the
//! licence the caller asserts and the sfnt face itself.

use std::collections::HashMap;

/// The largest single face either format embeds.
pub const MAX_EMBEDDED_FACE_BYTES: usize = 64 * 1024 * 1024;

/// Check the caller's licence assertion for embedding a font: explicit
/// authorization, and an identity that is not blank, holds at most 4096
/// bytes and no control character (XML cannot carry most of them, and a
/// tab or line break would not survive as typed).
pub fn check_embedding_license(authorized: bool, identity: &str) -> Result<(), String> {
    if !authorized {
        return Err("font embedding requires explicit caller authorization".to_owned());
    }
    if identity.trim().is_empty() {
        return Err("font embedding requires an exact license identity".to_owned());
    }
    if identity.len() > 4096 {
        return Err("font embedding license identity exceeds 4096 bytes".to_owned());
    }
    if identity
        .chars()
        .any(|ch| ch < ' ' || ch == '\u{fffe}' || ch == '\u{ffff}')
    {
        return Err(
            "font embedding license identity contains unsupported XML characters".to_owned(),
        );
    }
    Ok(())
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

/// The `OS/2`, `head` and `name` tables of one TrueType or OpenType face,
/// bounds-checked against the font bytes. [`SfntTables::parse`] is the
/// rule both formats apply before embedding a face.
#[derive(Clone, Copy, Debug)]
pub struct SfntTables<'a> {
    data: &'a [u8],
    os2: &'a [u8],
    head: &'a [u8],
    name: &'a [u8],
}

impl<'a> SfntTables<'a> {
    /// Parse one face: a single TrueType or OpenType face of at most
    /// [`MAX_EMBEDDED_FACE_BYTES`], a sound table directory (printable tags,
    /// none twice, every table inside the file and after the directory), and `OS/2`,
    /// `head` and `name` tables of their minimum lengths with a readable
    /// `name` table.
    pub fn parse(data: &'a [u8]) -> std::result::Result<Self, String> {
        match data.first_chunk::<4>() {
            Some(b"\x00\x01\x00\x00" | b"OTTO" | b"true") => {}
            Some(b"ttcf") => {
                return Err(
                    "a font collection cannot be embedded, pass one TrueType or OpenType face"
                        .to_owned(),
                );
            }
            _ => return Err("embedded font bytes are not a supported sfnt font".to_owned()),
        }
        let malformed = || "embedded font bytes have a malformed sfnt table directory".to_owned();
        let table_count = read_u16_be(data, 4).ok_or_else(malformed)?;
        if table_count == 0 || table_count > 512 {
            return Err(malformed());
        }
        if data.len() > MAX_EMBEDDED_FACE_BYTES || data.len() < 12 + usize::from(table_count) * 16 {
            return Err(malformed());
        }
        let mut directory = HashMap::new();
        for index in 0..usize::from(table_count) {
            let record = 12 + index * 16;
            let tag: [u8; 4] = data[record..record + 4].try_into().unwrap();
            if !tag.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
                return Err(malformed());
            }
            let offset = read_u32_be(data, record + 8).ok_or_else(malformed)? as usize;
            let length = read_u32_be(data, record + 12).ok_or_else(malformed)? as usize;
            let end = offset.checked_add(length).ok_or_else(malformed)?;
            let bytes = data.get(offset..end).ok_or_else(malformed)?;
            if directory.insert(tag, bytes).is_some()
                || (length != 0 && offset < 12 + usize::from(table_count) * 16)
            {
                return Err(malformed());
            }
        }
        let table = |tag: &[u8; 4]| -> std::result::Result<Option<&'a [u8]>, String> {
            Ok(directory.get(tag).copied())
        };
        let required = |tag: &[u8; 4]| {
            table(tag)?.ok_or_else(|| {
                format!(
                    "embedded font has no {} table",
                    String::from_utf8_lossy(tag).trim_end()
                )
            })
        };
        let tables = Self {
            data,
            os2: required(b"OS/2")?,
            head: required(b"head")?,
            name: required(b"name")?,
        };
        if tables.os2.len() < 78 || tables.head.len() < 54 {
            return Err("embedded font has a truncated OS/2 or head table".to_owned());
        }
        let minimum_os2 = match tables.os2_version() {
            0 => 78,
            1 => 86,
            2..=4 => 96,
            _ => 100,
        };
        if tables.os2.len() < minimum_os2 {
            return Err("embedded font has a truncated OS/2 table".to_owned());
        }
        let count = read_u16_be(tables.name, 2).ok_or_else(malformed)? as usize;
        let strings = read_u16_be(tables.name, 4).ok_or_else(malformed)? as usize;
        let format = read_u16_be(tables.name, 0).ok_or_else(malformed)?;
        if format > 1 || strings < 6 + count * 12 || strings > tables.name.len() {
            return Err(malformed());
        }
        for index in 0..count {
            let record = 6 + index * 12;
            let platform = read_u16_be(tables.name, record).ok_or_else(malformed)?;
            let length = read_u16_be(tables.name, record + 8).ok_or_else(malformed)? as usize;
            let offset = read_u16_be(tables.name, record + 10).ok_or_else(malformed)? as usize;
            if tables
                .name
                .get(strings + offset..strings + offset + length)
                .is_none()
                || (matches!(platform, 0 | 3) && !length.is_multiple_of(2))
            {
                return Err(malformed());
            }
        }
        Ok(tables)
    }

    /// The whole face.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The `OS/2` table, at least as long as its version requires.
    pub fn os2(&self) -> &'a [u8] {
        self.os2
    }

    /// The `head` table, at least 54 bytes.
    pub fn head(&self) -> &'a [u8] {
        self.head
    }

    /// The `name` table, its records inside it.
    pub fn name(&self) -> &'a [u8] {
        self.name
    }

    /// The `OS/2` table version.
    pub fn os2_version(&self) -> u16 {
        read_u16_be(self.os2, 0).unwrap_or(0)
    }
}
