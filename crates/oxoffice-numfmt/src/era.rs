//! The eras of the Japanese calendar, as Excel shows them under a Japanese tag (`[$-411]`).
//!
//! `ggg` is the era's name, `gg` its first character, `g` its initial letter in Latin script, and `e` its year
//! (`ee` with two digits): `[$-411]ggge"年"m"月"d"日"` is `令和6年9月4日` for 4 September 2024. The first year of an
//! era is 1, not 元: `令和1年5月1日`. Under any other tag these tokens show nothing, and `e` is the Gregorian year
//! (`src/date.rs`). Excel for Mac 16.115, `tests/cases.tsv`, round 9.

/// One era: its first day and its names.
struct Era {
    /// The first day, Gregorian.
    from: (i64, u32, u32),
    /// The Gregorian year before the era's first year: era year n is `n + offset`.
    offset: i64,
    name: &'static str,
    initial: &'static str,
}

/// From the oldest. A workbook holds no date before 1900, in Meiji 33: the first day of Meiji, 1868-01-25, is
/// kept only for the order of the table.
const ERAS: [Era; 5] = [
    Era {
        from: (1868, 1, 25),
        offset: 1867,
        name: "明治",
        initial: "M",
    },
    Era {
        from: (1912, 7, 30),
        offset: 1911,
        name: "大正",
        initial: "T",
    },
    Era {
        from: (1926, 12, 25),
        offset: 1925,
        name: "昭和",
        initial: "S",
    },
    Era {
        from: (1989, 1, 8),
        offset: 1988,
        name: "平成",
        initial: "H",
    },
    Era {
        from: (2019, 5, 1),
        offset: 2018,
        name: "令和",
        initial: "R",
    },
];

/// The era of a Gregorian date, and the year of the date in it. Excel's day 0 of the 1900 system (`1900-01-00`)
/// and its 1900-02-29 are Meiji 33 as well.
pub(crate) fn of(year: i64, month: u32, day: u32) -> (&'static str, &'static str, i64) {
    let era = ERAS
        .iter()
        .rev()
        .find(|era| (year, month, day) >= era.from)
        .unwrap_or(&ERAS[0]);
    (era.initial, era.name, year - era.offset)
}

/// The text of `g` (`count` 1), `gg` (2) or `ggg` (3).
pub(crate) fn text(count: usize, initial: &'static str, name: &'static str) -> &'static str {
    match count {
        1 => initial,
        2 => &name[..name.chars().next().map_or(0, char::len_utf8)],
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_era_begins_on_its_first_day() {
        for (date, expected) in [
            ((1900, 1, 0), ("M", "明治", 33)),
            ((1912, 7, 29), ("M", "明治", 45)),
            ((1912, 7, 30), ("T", "大正", 1)),
            ((1926, 12, 24), ("T", "大正", 15)),
            ((1926, 12, 25), ("S", "昭和", 1)),
            ((1989, 1, 7), ("S", "昭和", 64)),
            ((1989, 1, 8), ("H", "平成", 1)),
            ((2019, 4, 30), ("H", "平成", 31)),
            ((2019, 5, 1), ("R", "令和", 1)),
            ((2025, 12, 31), ("R", "令和", 7)),
        ] {
            assert_eq!(of(date.0, date.1, date.2), expected, "{date:?}");
        }
    }
}
