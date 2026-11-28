//! Month and day names, and the day period, of the languages this crate
//! knows: English, French, German, Spanish, Italian, Portuguese, Dutch,
//! Japanese, Chinese, Korean, Russian, Polish, Swedish, Turkish, Catalan and
//! Arabic.
//!
//! A `[$-40C]` tag in a date code asks for the names of its language, as
//! Excel shows them; Word's date controls show the names of the language of
//! `w:lid`. Excel's names depend on the region as well as the language
//! (Argentine Spanish writes `ene`, Spanish `ene.`, Taiwan `9月` and China
//! `九月`), so each table is that of one tag Excel for Mac was asked for
//! (`tests/cases.tsv`, rounds 4 to 8); a region it was not asked for takes
//! the table of the language's most widespread tag. The tables are written
//! by `tests/excel/names.py` from those rows: a language is one round and
//! one entry there.
//!
//! The names do not change with the day: Excel writes `15 январь`, not
//! `15 января`.

mod tables;

pub use tables::{DUTCH, ENGLISH, FRENCH, GERMAN, GERMAN_AUSTRIA, ITALIAN, PORTUGUESE, SPANISH};

/// The names of one language and region.
#[derive(Debug, PartialEq, Eq)]
pub struct Names {
    months: [&'static str; 12],
    short_months: [&'static str; 12],
    /// Sunday first.
    days: [&'static str; 7],
    short_days: [&'static str; 7],
    day_periods: [&'static str; 2],
}

impl Names {
    /// The full name of a month, 1 for January.
    pub fn month(&self, month: u32) -> &'static str {
        self.months[month as usize - 1]
    }

    /// The short name of a month, 1 for January.
    pub fn short_month(&self, month: u32) -> &'static str {
        self.short_months[month as usize - 1]
    }

    /// The full name of a weekday, 0 for Sunday.
    pub fn weekday(&self, weekday: u32) -> &'static str {
        self.days[weekday as usize]
    }

    /// The short name of a weekday, 0 for Sunday.
    pub fn short_weekday(&self, weekday: u32) -> &'static str {
        self.short_days[weekday as usize]
    }

    /// The designator of the morning and of the afternoon.
    pub fn day_period(&self, afternoon: bool) -> &'static str {
        self.day_periods[usize::from(afternoon)]
    }
}

/// The language a locale tag names, as far as names go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    /// A language this crate holds the names of.
    Known(&'static Names),
    /// Any other: Excel shows its names, this crate shows English ones.
    Other,
}

impl Language {
    /// The language of an LCID (`40C`). Its low word is the language and
    /// region (the low ten bits are the primary language). Excel took the
    /// tag `1040C` for French, and `2040C` for English; a higher high word
    /// was not asked and counts as a language with no table. A tag with no
    /// primary language (`[$-400]`, `[$-0]`) names none, as before.
    ///
    /// A neutral LCID (`[$-A]`, `[$-16]`) takes the table of the text tag
    /// with no region (`[$-es]`, `[$-pt]`) where Excel showed that other than
    /// the language's default table: our choice, the neutral LCIDs were not
    /// asked, and no producer writes them.
    pub(crate) fn of_lcid(lcid: u32) -> Option<Language> {
        if lcid & 0x3FF == 0 {
            return None;
        }
        match lcid >> 16 {
            0 | 1 => {}
            2 => return Some(Language::Known(&ENGLISH)),
            _ => return Some(Language::Other),
        }
        let lcid = (lcid & 0xFFFF) as u16;
        let region = tables::REGIONS.iter().find(|(region, ..)| *region == lcid);
        let language = tables::LANGUAGES
            .iter()
            .find(|(primary, ..)| *primary == lcid & 0x3FF);
        Some(
            region
                .or(language)
                .map_or(Language::Other, |(.., names)| Language::Known(names)),
        )
    }

    /// The language of a text tag in lower case (`fr-fr`, `fr`). `None` for
    /// no tag.
    pub(crate) fn of_tag(tag: &str) -> Option<Language> {
        if tag.is_empty() {
            return None;
        }
        let mut parts = tag.split(['-', '_']);
        let primary = parts.next().unwrap_or_default();
        // The keys of a region table, the most exact first: `es` with no
        // region, `es-mx` with one, `zh-hans-cn` and then `zh-hans` with a
        // script (four letters) as BCP 47 producers write it.
        let mut keys = Vec::new();
        match (parts.next(), parts.next()) {
            (Some(script), region) if script.len() == 4 => {
                if let Some(region) = region {
                    keys.push(format!("{primary}-{script}-{region}"));
                }
                keys.push(format!("{primary}-{script}"));
            }
            (Some(region), _) => keys.push(format!("{primary}-{region}")),
            (None, _) => keys.push(primary.to_owned()),
        }
        let region = keys
            .iter()
            .find_map(|key| tables::REGIONS.iter().find(|(_, tag, _)| tag == key));
        let language = tables::LANGUAGES.iter().find(|(_, tag, _)| *tag == primary);
        Some(
            region
                .or(language)
                .map_or(Language::Other, |(.., names)| Language::Known(names)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table a tag gives, by identity: the tables of two regions can hold
    /// the same words.
    fn table(language: Option<Language>) -> Option<&'static Names> {
        match language {
            Some(Language::Known(names)) => Some(names),
            _ => None,
        }
    }

    #[test]
    fn a_text_tag_reads_as_the_lcid_of_its_region() {
        for (tag, lcid) in [
            ("en-us", 0x0409),
            ("en-gb", 0x0809),
            ("en-au", 0x0C09),
            ("en-ca", 0x1009),
            ("en-ie", 0x1809),
            ("en", 0x0009),
            ("fr-fr", 0x040C),
            ("fr-ca", 0x0C0C),
            ("fr-be", 0x080C),
            ("fr", 0x000C),
            ("de-de", 0x0407),
            ("de-at", 0x0C07),
            ("de-ch", 0x0807),
            ("de", 0x0007),
            ("es-es", 0x0C0A),
            ("es-mx", 0x080A),
            ("es-ar", 0x2C0A),
            ("es-co", 0x240A),
            ("es-cl", 0x340A),
            ("es-us", 0x540A),
            // A neutral LCID reads as the tag with no region, which Excel
            // showed other than Spain's and Brazil's (our choice).
            ("es", 0x000A),
            ("it-it", 0x0410),
            ("it", 0x0010),
            ("pt-br", 0x0416),
            ("pt-pt", 0x0816),
            ("pt", 0x0016),
            ("nl-nl", 0x0413),
            ("nl", 0x0013),
            ("ja-jp", 0x0411),
            ("ja", 0x0011),
            ("zh-cn", 0x0804),
            ("zh-tw", 0x0404),
            ("zh-hk", 0x0C04),
            ("zh-sg", 0x1004),
            ("zh-mo", 0x1404),
            ("zh-hant", 0x7C04),
            ("zh", 0x7804),
            // A script and a region, as BCP 47 producers write it: Excel read
            // them as the LCID of the pair (round 10).
            ("zh-hans-cn", 0x0804),
            ("zh-hans-sg", 0x1004),
            ("zh-hant-hk", 0x0C04),
            ("zh-hant-tw", 0x0404),
            ("zh-hant-mo", 0x1404),
            ("zh-hans-hk", 0x0804),
            ("zh-hans-tw", 0x7804),
            ("zh-hant-cn", 0x7C04),
            ("zh-hant_sg", 0x7C04),
            ("zh-hans", 0x0004),
            ("ko-kr", 0x0412),
            ("ko", 0x0012),
            ("ru-ru", 0x0419),
            ("ru-md", 0x0819),
            ("ru", 0x0019),
            ("pl-pl", 0x0415),
            ("pl", 0x0015),
            ("sv-se", 0x041D),
            ("sv-fi", 0x081D),
            ("sv", 0x001D),
            ("tr-tr", 0x041F),
            ("tr", 0x001F),
            ("ca-es", 0x0403),
            ("ca-es-valencia", 0x0803),
            ("ca", 0x0003),
            ("ar-sa", 0x0401),
            ("ar-eg", 0x0C01),
            ("ar-ma", 0x1801),
            ("ar-iq", 0x0801),
            ("ar-sy", 0x2801),
            ("ar-jo", 0x2C01),
            ("ar-lb", 0x3001),
        ] {
            let by_tag = table(Language::of_tag(tag));
            assert!(by_tag.is_some(), "{tag}");
            assert!(
                by_tag
                    .zip(table(Language::of_lcid(lcid)))
                    .is_some_and(|(a, b)| std::ptr::eq(a, b)),
                "{tag} and {lcid:#x}"
            );
        }
        // The tables differ where Excel showed them differ: the tags above
        // are not all one table.
        for (one, other) in [
            ("es-ar", "es-co"),
            ("zh-cn", "zh-tw"),
            ("zh-tw", "zh-hk"),
            ("zh-cn", "zh"),
            ("ar-sa", "ar-jo"),
        ] {
            assert!(
                !std::ptr::eq(
                    table(Language::of_tag(one)).unwrap(),
                    table(Language::of_tag(other)).unwrap()
                ),
                "{one} and {other}"
            );
        }
    }

    #[test]
    fn the_neutral_arabic_lcid_shows_english_as_excel_does() {
        // `[$-ar]` shows Arabic names, `[$-1]` English ones.
        let months = |language: Option<Language>| table(language).map(|names| names.month(1));
        assert_eq!(months(Language::of_tag("ar")), Some("يناير"));
        assert_eq!(months(Language::of_lcid(0x1)), Some("January"));
    }

    #[test]
    fn a_tag_with_no_table_names_no_language_or_one_without_names() {
        // No primary language, as before the tables: no language.
        for lcid in [0x0, 0x400, 0x800, 0xC00, 0x1_0400] {
            assert_eq!(Language::of_lcid(lcid), None, "{lcid:#x}");
        }
        // `[$-2040C]` showed English names: English.
        assert_eq!(
            table(Language::of_lcid(0x2_040C)).map(|names| names.short_month(1)),
            Some("Jan")
        );
        // A high word nobody asked, and languages with no table (Hebrew,
        // Greek, Thai).
        for lcid in [0x3_040C, 0x40D, 0x408, 0x41E] {
            assert_eq!(Language::of_lcid(lcid), Some(Language::Other), "{lcid:#x}");
        }
        for tag in ["he-il", "el", "th-th"] {
            assert_eq!(Language::of_tag(tag), Some(Language::Other), "{tag}");
        }
        assert_eq!(Language::of_tag(""), None);
    }
}
