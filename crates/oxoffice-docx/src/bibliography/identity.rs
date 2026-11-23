//! The GUID a source gets when its author leaves it empty.

use sha2::{Digest, Sha256};

use super::BibliographySource;
use super::source::authored_source_xml;

/// A version-4-shaped GUID derived from the SHA-256 of the source as it is
/// authored, without its GUID, and `attempt`: the same source always gets
/// the same value, and a later attempt the next one when the collection
/// already holds it. Word writes source GUIDs upper case in braces.
pub(super) fn derived_source_guid(source: &BibliographySource, attempt: u32) -> String {
    let mut blank = source.clone();
    blank.guid.clear();
    let mut hasher = Sha256::new();
    hasher.update(authored_source_xml(&blank).as_bytes());
    if attempt > 0 {
        hasher.update(attempt.to_le_bytes());
    }
    let mut bytes: [u8; 16] = hasher.finalize()[..16]
        .try_into()
        .expect("a SHA-256 digest holds 16 bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    format!(
        "{{{}-{}-{}-{}-{}}}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}
