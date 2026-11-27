//! Telling what a compound file holds (an OLE2 file, `D0 CF 11 E0`): a
//! password-protected Office package, or a legacy Excel, Word or PowerPoint
//! file, by the names of the streams in its directory (MS-CFB). A package
//! is told by its content, never its extension, so a facade refuses each of
//! these with its own plain message instead of a ZIP error.

/// The signature every compound file starts with.
const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// The largest regular sector number; the ones above end a chain.
const MAX_REGULAR_SECTOR: u32 = 0xFFFF_FFFA;
/// The FAT sectors the header lists itself.
const HEADER_FAT_SECTORS: usize = 109;

/// What a compound file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompoundFile {
    /// A password-protected Office package: an `EncryptionInfo` stream.
    Encrypted,
    /// An Excel 97-2003 workbook (`.xls`): a `Workbook` or `Book` stream.
    Workbook,
    /// A Word 97-2003 document (`.doc`): a `WordDocument` stream.
    Document,
    /// A PowerPoint 97-2003 presentation (`.ppt`): a `PowerPoint Document`
    /// stream.
    Presentation,
    /// Any other compound file, or one whose directory does not read.
    Other,
}

/// What `bytes` holds when it is a compound file, `None` when it is not one.
pub fn compound_file(bytes: &[u8]) -> Option<CompoundFile> {
    if !bytes.starts_with(&SIGNATURE) {
        return None;
    }
    let names = directory_names(bytes).unwrap_or_default();
    let has = |stream: &str| names.iter().any(|name| name.eq_ignore_ascii_case(stream));
    Some(if has("EncryptionInfo") {
        CompoundFile::Encrypted
    } else if has("Workbook") || has("Book") {
        CompoundFile::Workbook
    } else if has("WordDocument") {
        CompoundFile::Document
    } else if has("PowerPoint Document") {
        CompoundFile::Presentation
    } else {
        CompoundFile::Other
    })
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The names of the storages and streams directly in the root storage, or
/// `None` when the header, the FAT or the directory does not read.
fn directory_names(bytes: &[u8]) -> Option<Vec<String>> {
    let shift = u16_at(bytes, 0x1E)?;
    if !(7..=16).contains(&shift) {
        return None;
    }
    let size = 1usize << shift;
    let sector = |number: u32| -> Option<&[u8]> {
        let start = (number as usize).checked_add(1)?.checked_mul(size)?;
        bytes
            .get(start..start.checked_add(size)?.min(bytes.len()))
            .filter(|sector| !sector.is_empty())
    };
    // A chain longer than the file has sectors loops.
    let limit = bytes.len() / size + 1;

    let mut fat = Vec::new();
    for index in 0..HEADER_FAT_SECTORS {
        let number = u32_at(bytes, 0x4C + 4 * index)?;
        if number <= MAX_REGULAR_SECTOR {
            fat.push(number);
        }
    }
    let mut difat = u32_at(bytes, 0x44)?;
    let mut steps = 0;
    while difat <= MAX_REGULAR_SECTOR {
        steps += 1;
        if steps > limit {
            return None;
        }
        let entries = sector(difat)?;
        let per_sector = size / 4 - 1;
        for index in 0..per_sector {
            let number = u32_at(entries, 4 * index)?;
            if number <= MAX_REGULAR_SECTOR {
                fat.push(number);
            }
        }
        difat = u32_at(entries, 4 * per_sector)?;
    }
    let next = |number: u32| -> Option<u32> {
        let per_sector = size / 4;
        let fat_sector = sector(*fat.get(number as usize / per_sector)?)?;
        u32_at(fat_sector, 4 * (number as usize % per_sector))
    };

    // Every directory entry, in order: its name and its left sibling,
    // right sibling and child, or `None` for an unused one.
    let mut entries: Vec<Option<(String, [u32; 3])>> = Vec::new();
    let mut current = u32_at(bytes, 0x30)?;
    let mut steps = 0;
    while current <= MAX_REGULAR_SECTOR {
        steps += 1;
        if steps > limit {
            return None;
        }
        for entry in sector(current)?.chunks_exact(128) {
            let length = usize::from(u16_at(entry, 0x40)?);
            if entry[0x42] == 0 || !(2..=64).contains(&length) {
                entries.push(None);
                continue;
            }
            let units: Vec<u16> = entry[..length - 2]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let links = [
                u32_at(entry, 0x44)?,
                u32_at(entry, 0x48)?,
                u32_at(entry, 0x4C)?,
            ];
            entries.push(Some((String::from_utf16_lossy(&units), links)));
        }
        current = next(current)?;
    }
    // The children of the root storage (entry 0) are the tree its child
    // starts, walked by the siblings only: a stream inside an embedded
    // object (`ObjectPool/_1/Workbook` in a `.doc`) is no stream of the
    // file.
    let (_, [_, _, child]) = entries.first()?.as_ref()?;
    let mut names = Vec::new();
    let mut pending = vec![*child];
    let mut seen = vec![false; entries.len()];
    while let Some(id) = pending.pop() {
        let Some(Some((name, [left, right, _]))) = entries.get(id as usize) else {
            continue;
        };
        if std::mem::replace(&mut seen[id as usize], true) {
            return None;
        }
        names.push(name.clone());
        pending.extend([*left, *right]);
    }
    Some(names)
}
