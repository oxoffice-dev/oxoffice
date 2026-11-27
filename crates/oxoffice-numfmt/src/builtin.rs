//! The built-in format ids: a cell style may name one with no `numFmt`
//! record, and Excel shows the code of its own language.

use crate::Locale;

/// The code of built-in format `id` in `locale`, as en-US Excel shows it
/// (seen in Excel for Mac 16.115): ids 0 to 22 and 37 to 49 of ECMA-376
/// (18.8.30), the Short Date for id 14 (`m/d/yy`), and the ids reserved to
/// East Asian and Thai Excel mapped to the en-US code Excel takes for them.
/// `None` for an id Excel shows as General (23 to 26, unknown ids).
pub fn builtin_code(id: u32, locale: Locale) -> Option<&'static str> {
    match locale {
        Locale::EnUs => en_us(id),
    }
}

fn en_us(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 | 59 => "0",
        2 | 60 => "0.00",
        3 | 61 => "#,##0",
        4 | 62 => "#,##0.00",
        5 | 63 => "\"$\"#,##0_);\\(\"$\"#,##0\\)",
        6 | 64 => "\"$\"#,##0_);[Red]\\(\"$\"#,##0\\)",
        7 | 65 => "\"$\"#,##0.00_);\\(\"$\"#,##0.00\\)",
        8 | 66 => "\"$\"#,##0.00_);[Red]\\(\"$\"#,##0.00\\)",
        9 | 67 => "0%",
        10 | 68 => "0.00%",
        11 => "0.00E+00",
        12 | 69 => "# ?/?",
        13 | 70 => "# ??/??",
        14 | 27..=31 | 36 | 50..=58 | 71 | 72 => "m/d/yy",
        15 | 73 => "d-mmm-yy",
        16 | 74 => "d-mmm",
        17 | 75 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 | 76 => "h:mm",
        21 | 32..=35 | 77 => "h:mm:ss",
        22 | 78 => "m/d/yy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        41 => "_(* #,##0_);_(* \\(#,##0\\);_(* \"-\"_);_(@_)",
        42 => "_(\"$\"* #,##0_);_(\"$\"* \\(#,##0\\);_(\"$\"* \"-\"_);_(@_)",
        43 => "_(* #,##0.00_);_(* \\(#,##0.00\\);_(* \"-\"??_);_(@_)",
        44 => "_(\"$\"* #,##0.00_);_(\"$\"* \\(#,##0.00\\);_(\"$\"* \"-\"??_);_(@_)",
        45 | 79 => "mm:ss",
        46 | 80 => "[h]:mm:ss",
        47 | 81 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}
