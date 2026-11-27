//! A1 references: a cell `B3`, an area `A1:C10`, whole columns `A:C` and rows
//! `2:5`, each part relative or absolute (`$`), and where the sheet prefix of
//! a reference token ends.

use std::ops::Range;

/// The rows of a worksheet: 1,048,576.
pub(crate) const MAX_ROWS: u32 = 1_048_576;
/// The columns of a worksheet: 16,384, `A` to `XFD`.
pub(crate) const MAX_COLUMNS: u32 = 16_384;

/// A column or a row of a reference: its 0-based index, whether it is
/// absolute, and where its letters or digits are in the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Part {
    pub index: u32,
    pub absolute: bool,
    pub span: Range<usize>,
}

/// What a reference token points at, after its prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Cell {
        column: Part,
        row: Part,
    },
    Area {
        first: (Part, Part),
        last: (Part, Part),
    },
    Columns(Part, Part),
    Rows(Part, Part),
    /// `#REF!`, a reference that was deleted.
    Deleted,
}

/// Whether `c` may continue a name: what makes `A1x` a name and not a cell.
pub(crate) fn is_name_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(c, '_' | '.' | '\\' | '?')
        || (!c.is_ascii() && !c.is_whitespace())
}

/// Whether `c` may start a name.
pub(crate) fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '_' | '\\') || (!c.is_ascii() && !c.is_whitespace())
}

fn char_at(text: &str, at: usize) -> Option<char> {
    text.get(at..)?.chars().next()
}

/// A column at `at`: `$`, then one to three letters within `XFD`.
fn column_at(text: &str, at: usize) -> Option<(usize, Part)> {
    let bytes = text.as_bytes();
    let absolute = bytes.get(at) == Some(&b'$');
    let start = at + usize::from(absolute);
    let letters = bytes[start.min(bytes.len())..]
        .iter()
        .take_while(|byte| byte.is_ascii_alphabetic())
        .count();
    if letters == 0 || letters > 3 {
        return None;
    }
    let mut number = 0u32;
    for byte in &bytes[start..start + letters] {
        number = number * 26 + u32::from(byte.to_ascii_uppercase() - b'A') + 1;
    }
    (number <= MAX_COLUMNS).then(|| {
        let span = start..start + letters;
        (
            span.end,
            Part {
                index: number - 1,
                absolute,
                span,
            },
        )
    })
}

/// A row at `at`: `$`, then the digits of a row within the grid.
fn row_at(text: &str, at: usize) -> Option<(usize, Part)> {
    let bytes = text.as_bytes();
    let absolute = bytes.get(at) == Some(&b'$');
    let start = at + usize::from(absolute);
    let digits = bytes[start.min(bytes.len())..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || digits > 7 {
        return None;
    }
    let number: u32 = text[start..start + digits].parse().ok()?;
    (1..=MAX_ROWS).contains(&number).then(|| {
        let span = start..start + digits;
        (
            span.end,
            Part {
                index: number - 1,
                absolute,
                span,
            },
        )
    })
}

/// A cell at `at`, which no name character or `(` follows.
fn cell_at(text: &str, at: usize) -> Option<(usize, (Part, Part))> {
    let (end, column) = column_at(text, at)?;
    let (end, row) = row_at(text, end)?;
    match char_at(text, end) {
        Some(c) if is_name_char(c) || c == '(' => None,
        _ => Some((end, (column, row))),
    }
}

/// The reference target that starts at `at`, and where it ends. An area
/// whose second cell a spill `#` follows is read as its first cell, so that
/// `A1:B2#` is `A1` to the spill of `B2`.
pub(crate) fn target_at(text: &str, at: usize) -> Option<(usize, Target)> {
    if let Some((end, first)) = cell_at(text, at) {
        if text[end..].starts_with(':')
            && let Some((last_end, last)) = cell_at(text, end + 1)
            && char_at(text, last_end) != Some('#')
        {
            return Some((last_end, Target::Area { first, last }));
        }
        return Some((
            end,
            Target::Cell {
                column: first.0,
                row: first.1,
            },
        ));
    }
    let ends_cleanly = |end: usize| match char_at(text, end) {
        Some(c) => !(is_name_char(c) || c == '(' || c == '$'),
        None => true,
    };
    if let Some((end, first)) = column_at(text, at)
        && text[end..].starts_with(':')
        && let Some((end, last)) = column_at(text, end + 1)
        && ends_cleanly(end)
    {
        return Some((end, Target::Columns(first, last)));
    }
    if let Some((end, first)) = row_at(text, at)
        && text[end..].starts_with(':')
        && let Some((end, last)) = row_at(text, end + 1)
        && ends_cleanly(end)
    {
        return Some((end, Target::Rows(first, last)));
    }
    None
}

/// Where the target of a reference token starts: after its prefix
/// (`Sheet1!`, `'Q1 data'!`, `Sheet1:Sheet3!`, `[1]Sheet1!`, `!`), at 0
/// without one.
pub(crate) fn target_start(token: &str) -> usize {
    let mut at = 0;
    if token.starts_with('\'') {
        at = 1;
        while let Some(found) = token[at..].find('\'') {
            at += found + 1;
            if token[at..].starts_with('\'') {
                at += 1;
            } else {
                break;
            }
        }
    }
    // `#REF!` alone is a target, not a prefix.
    if token[at..].starts_with('#') {
        return 0;
    }
    token[at..].find('!').map_or(0, |found| at + found + 1)
}

/// The target of a reference token.
pub(crate) fn target(token: &str) -> Option<Target> {
    let start = target_start(token);
    let rest = &token[start..];
    if rest.eq_ignore_ascii_case("#REF!") {
        return Some(Target::Deleted);
    }
    let (end, target) = target_at(token, start)?;
    (end == token.len()).then_some(target)
}

/// The letters of a 0-based column: `A` for 0, `XFD` for 16,383.
pub(crate) fn column_letters(index: u32) -> String {
    let mut letters = Vec::new();
    let mut rest = index + 1;
    while rest > 0 {
        letters.push(b'A' + ((rest - 1) % 26) as u8);
        rest = (rest - 1) / 26;
    }
    letters.reverse();
    String::from_utf8(letters).expect("ASCII letters")
}
