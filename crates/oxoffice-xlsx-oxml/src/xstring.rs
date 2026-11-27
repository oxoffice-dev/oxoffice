//! The `_xHHHH_` escapes of `ST_Xstring`, the text of shared strings, inline
//! strings and cell values: a character XML cannot carry (a control
//! character) or would not keep (a carriage return) is stored as `_x000D_`,
//! and a literal `_xHHHH_` in the text as `_x005F_xHHHH_`.

use std::borrow::Cow;

/// Whether `bytes` holds an escape `_xHHHH_` at `at`.
fn escape_at(bytes: &[u8], at: usize) -> Option<u32> {
    let candidate = bytes.get(at..at + 7)?;
    if candidate[0] != b'_' || !candidate[1].eq_ignore_ascii_case(&b'x') || candidate[6] != b'_' {
        return None;
    }
    let digits = std::str::from_utf8(&candidate[2..6]).ok()?;
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// The text `stored` stands for: each `_xHHHH_` read as its character. An
/// escape of no character (a surrogate) is kept as written.
pub fn decode(stored: &str) -> Cow<'_, str> {
    if !stored.contains('_') {
        return Cow::Borrowed(stored);
    }
    let bytes = stored.as_bytes();
    let mut out = String::with_capacity(stored.len());
    let mut at = 0;
    while at < bytes.len() {
        if let Some(character) = escape_at(bytes, at).and_then(char::from_u32) {
            out.push(character);
            at += 7;
            continue;
        }
        let character = stored[at..]
            .chars()
            .next()
            .expect("a character at a boundary");
        out.push(character);
        at += character.len_utf8();
    }
    Cow::Owned(out)
}

/// `text` as stored: a character XML 1.0 cannot carry, and a carriage
/// return, as `_xHHHH_`; the `_` of a literal `_xHHHH_` as `_x005F_`.
pub fn encode(text: &str) -> Cow<'_, str> {
    let escaped =
        |character: char| character == '\r' || !oxoffice_core::xml::is_xml_1_0_character(character);
    let bytes = text.as_bytes();
    let literal = |at: usize| escape_at(bytes, at).is_some();
    if !text.chars().any(escaped) && !(0..bytes.len()).any(literal) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 7);
    for (at, character) in text.char_indices() {
        if escaped(character) {
            out.push_str(&format!("_x{:04X}_", u32::from(character)));
        } else if character == '_' && literal(at) {
            out.push_str("_x005F_");
        } else {
            out.push(character);
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_read_and_write_back() {
        for (text, stored) in [
            ("plain", "plain"),
            ("a\u{1}b", "a_x0001_b"),
            ("line\r\nnext", "line_x000D_\nnext"),
            ("_x0041_", "_x005F_x0041_"),
            ("tab\there", "tab\there"),
            ("_x12_", "_x12_"),
        ] {
            assert_eq!(encode(text), stored, "{text:?}");
            assert_eq!(decode(stored), text, "{stored:?}");
        }
        assert_eq!(decode("_x0041__xd800_"), "A_xd800_");
    }
}
