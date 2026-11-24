//! The canonical identity of a part name, which every comparison of part
//! names in the embedded-content and package-signature checks uses.
//!
//! A relationship target is resolved against its source part, each segment
//! is percent-decoded, dot and empty segments are collapsed and ASCII case
//! is folded, as OPC compares part names. A target that cannot be read that
//! way names no part and is refused by the checks.

use std::fmt;

/// Why a relationship target names no part.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnresolvableTarget {
    /// The target is empty: it names its own source.
    Empty,
    /// The first segment of a relative target reads as a URI scheme.
    Scheme,
    /// A `%` is not followed by two hexadecimal digits.
    InvalidEscape,
    /// The decoded bytes are not UTF-8.
    InvalidUtf8,
    /// An escape decodes to `/` or `\`.
    EncodedSeparator,
    /// A segment holds a control character, written or escaped.
    ControlCharacter,
    /// A `..` segment climbs above the package root.
    EscapesRoot,
}

impl fmt::Display for UnresolvableTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "it is empty",
            Self::Scheme => "it starts with a URI scheme",
            Self::InvalidEscape => "it holds an invalid percent escape",
            Self::InvalidUtf8 => "it decodes to invalid UTF-8",
            Self::EncodedSeparator => "it encodes a path separator",
            Self::ControlCharacter => "it holds a control character",
            Self::EscapesRoot => "it climbs above the package root",
        })
    }
}

/// The identity of an absolute part name as stored in a package.
///
/// A stored name that cannot be decoded keeps the identity opc's lookup
/// gives it (dot segments collapsed, ASCII case folded). A decoded target
/// can still name it: `a%25ZZ.png` decodes to `a%ZZ.png`, the identity of
/// a stored `a%ZZ.png`. That errs on the safe side only: such a part counts
/// as reachable and is kept, and two stored names that meet this way are
/// refused as ambiguous.
pub(crate) fn stored_part_identity(part_name: &str) -> String {
    resolve("/", part_name).unwrap_or_else(|_| crate::package::part_identity(part_name))
}

/// Every identity a relationship target can name from its source part.
///
/// A target with `\`, `?` or `#` has more than one reading: as written,
/// with `\` read as `/`, and without its query or fragment. Each reading
/// must resolve; the identities they name are returned without duplicates.
pub(crate) fn target_identities(
    source_part: &str,
    target: &str,
) -> Result<Vec<String>, UnresolvableTarget> {
    let mut readings = vec![target.to_owned()];
    if target.contains('\\') {
        readings.push(target.replace('\\', "/"));
    }
    if let Some(end) = target.find(['?', '#']) {
        let stripped = &target[..end];
        readings.push(stripped.to_owned());
        if stripped.contains('\\') {
            readings.push(stripped.replace('\\', "/"));
        }
    }
    let mut identities = Vec::with_capacity(readings.len());
    for reading in readings {
        let identity = resolve(source_part, &reading)?;
        if !identities.contains(&identity) {
            identities.push(identity);
        }
    }
    Ok(identities)
}

fn resolve(source_part: &str, target: &str) -> Result<String, UnresolvableTarget> {
    if target.is_empty() {
        return Err(UnresolvableTarget::Empty);
    }
    let mut segments = Vec::new();
    if !target.starts_with('/') {
        if target
            .split('/')
            .next()
            .is_some_and(|segment| segment.contains(':'))
        {
            return Err(UnresolvableTarget::Scheme);
        }
        let directory = source_part
            .rsplit_once('/')
            .map_or("", |(directory, _)| directory);
        for segment in directory.split('/') {
            push_segment(&mut segments, segment)?;
        }
    }
    for segment in target.split('/') {
        push_segment(&mut segments, segment)?;
    }
    let mut identity = String::with_capacity(target.len() + source_part.len());
    for segment in &segments {
        identity.push('/');
        identity.push_str(segment);
    }
    if identity.is_empty() {
        identity.push('/');
    }
    identity.make_ascii_lowercase();
    Ok(identity)
}

fn push_segment(segments: &mut Vec<String>, raw: &str) -> Result<(), UnresolvableTarget> {
    let segment = percent_decode_segment(raw)?;
    match segment.as_str() {
        "" | "." => {}
        ".." => {
            segments.pop().ok_or(UnresolvableTarget::EscapesRoot)?;
        }
        _ => segments.push(segment),
    }
    Ok(())
}

fn percent_decode_segment(raw: &str) -> Result<String, UnresolvableTarget> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let (Some(high), Some(low)) = (
                bytes.get(index + 1).copied().and_then(hex_digit),
                bytes.get(index + 2).copied().and_then(hex_digit),
            ) else {
                return Err(UnresolvableTarget::InvalidEscape);
            };
            let byte = (high << 4) | low;
            if matches!(byte, b'/' | b'\\') {
                return Err(UnresolvableTarget::EncodedSeparator);
            }
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| UnresolvableTarget::InvalidUtf8)?;
    if decoded.chars().any(char::is_control) {
        return Err(UnresolvableTarget::ControlCharacter);
    }
    Ok(decoded)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: &str = "/word/document.xml";
    const ORIGIN: &str = "/_xmlsignatures/origin.sigs";

    fn one(source_part: &str, target: &str) -> String {
        match target_identities(source_part, target).as_deref() {
            Ok([identity]) => identity.clone(),
            other => panic!("{target}: expected one identity, got {other:?}"),
        }
    }

    #[test]
    fn a_target_names_its_part_whatever_its_case() {
        assert_eq!(one(MAIN, "../_XMLSIGNATURES/ORIGIN.SIGS"), ORIGIN);
        assert_eq!(one("/", "_xmlsignatures/Origin.sigs"), ORIGIN);
        assert_eq!(stored_part_identity("/_XmlSignatures/origin.SIGS"), ORIGIN);
    }

    #[test]
    fn an_escaped_unreserved_character_names_the_same_part() {
        assert_eq!(one(MAIN, "../%5Fxmlsignatures/origin.sigs"), ORIGIN);
        assert_eq!(one(MAIN, "../%5fxmlsignatures/origin%2Esigs"), ORIGIN);
        assert_eq!(one(MAIN, "media/image%201.png"), "/word/media/image 1.png");
        assert_eq!(
            stored_part_identity("/word/media/image%201.png"),
            stored_part_identity("/word/media/image 1.png")
        );
    }

    #[test]
    fn an_escaped_separator_is_refused() {
        for target in ["..%2F_xmlsignatures/origin.sigs", "media%2fimage.png"] {
            assert_eq!(
                target_identities(MAIN, target),
                Err(UnresolvableTarget::EncodedSeparator),
                "{target}"
            );
        }
        assert_eq!(
            target_identities(MAIN, "media%5Cimage.png"),
            Err(UnresolvableTarget::EncodedSeparator)
        );
    }

    #[test]
    fn an_escape_that_is_not_utf8_or_not_an_escape_is_refused() {
        assert_eq!(
            target_identities(MAIN, "media/image%FF.png"),
            Err(UnresolvableTarget::InvalidUtf8)
        );
        assert_eq!(
            target_identities(MAIN, "media/image%C3.png"),
            Err(UnresolvableTarget::InvalidUtf8)
        );
        assert_eq!(
            one(MAIN, "media/caf%C3%A9.png"),
            "/word/media/caf\u{e9}.png"
        );
        for target in ["media/image%2.png", "media/image%G1.png", "media/image%"] {
            assert_eq!(
                target_identities(MAIN, target),
                Err(UnresolvableTarget::InvalidEscape),
                "{target}"
            );
        }
    }

    #[test]
    fn a_control_character_written_or_escaped_is_refused() {
        for target in [
            "media/image\u{1}.png",
            "media/image%0A.png",
            "media/image%C2%85.png",
        ] {
            assert_eq!(
                target_identities(MAIN, target),
                Err(UnresolvableTarget::ControlCharacter),
                "{target:?}"
            );
        }
    }

    #[test]
    fn dot_and_empty_segments_collapse() {
        assert_eq!(one(MAIN, "./media//image.png"), "/word/media/image.png");
        assert_eq!(
            one(MAIN, "media/../media/./image.png"),
            "/word/media/image.png"
        );
        assert_eq!(one(MAIN, "%2E%2E/_xmlsignatures/origin.sigs"), ORIGIN);
        assert_eq!(one("/", "/word/./document.xml"), MAIN);
    }

    #[test]
    fn a_climb_above_the_package_root_is_refused() {
        for (source_part, target) in [
            (MAIN, "../../_xmlsignatures/origin.sigs"),
            ("/", "../origin.sigs"),
            (MAIN, "/../origin.sigs"),
            (MAIN, "%2E%2E/%2E%2E/origin.sigs"),
        ] {
            assert_eq!(
                target_identities(source_part, target),
                Err(UnresolvableTarget::EscapesRoot),
                "{source_part} {target}"
            );
        }
    }

    #[test]
    fn an_empty_target_or_a_scheme_is_refused() {
        assert_eq!(target_identities(MAIN, ""), Err(UnresolvableTarget::Empty));
        assert_eq!(
            target_identities(MAIN, "http:image.png"),
            Err(UnresolvableTarget::Scheme)
        );
    }

    #[test]
    fn a_backslash_query_or_fragment_is_read_every_way() {
        assert_eq!(
            target_identities(MAIN, "..\\_xmlsignatures\\origin.sigs").unwrap(),
            [
                "/word/..\\_xmlsignatures\\origin.sigs".to_owned(),
                ORIGIN.to_owned()
            ]
        );
        assert_eq!(
            target_identities(MAIN, "../_xmlsignatures/origin.sigs?x=1").unwrap(),
            [
                "/_xmlsignatures/origin.sigs?x=1".to_owned(),
                ORIGIN.to_owned()
            ]
        );
        assert_eq!(
            target_identities(MAIN, "../_xmlsignatures/origin.sigs#part").unwrap(),
            [
                "/_xmlsignatures/origin.sigs#part".to_owned(),
                ORIGIN.to_owned()
            ]
        );
        assert!(
            target_identities(MAIN, "..\\_xmlsignatures\\origin.sigs#a\\b")
                .unwrap()
                .contains(&ORIGIN.to_owned())
        );
        // Every reading must resolve: stripped of its query, this one is empty.
        assert_eq!(
            target_identities(MAIN, "?image.png"),
            Err(UnresolvableTarget::Empty)
        );
        // Read with `\` as `/`, this one climbs above the root.
        assert_eq!(
            target_identities(MAIN, "..\\..\\origin.sigs"),
            Err(UnresolvableTarget::EscapesRoot)
        );
    }

    #[test]
    fn a_stored_name_that_cannot_be_decoded_keeps_its_lookup_identity() {
        assert_eq!(
            stored_part_identity("/word/Media/a%ZZ.png"),
            "/word/media/a%zz.png"
        );
    }
}
