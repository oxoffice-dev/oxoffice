//! Month and day names, and the day period, of the languages this crate
//! knows.
//!
//! A `[$-40C]` tag in a date code asks for the names of its language, as
//! Excel shows them; Word's date controls show the names of the language of
//! `w:lid`. Excel's names depend on the region as well as the language
//! (Argentine Spanish writes `ene`, Spanish `ene.`), so each table is that
//! of one tag Excel for Mac was asked for (`tests/cases.tsv`, rounds 4 and
//! 5); a region it was not asked for takes the table of the language's most
//! widespread tag, its full names being the same in every region seen.

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
    Other {
        /// Excel shows the day period as `AM` and `PM` in this language, as
        /// this crate does (Russian and Polish: `PM` was seen).
        english_day_period: bool,
    },
}

impl Language {
    /// A language with no table whose day period Excel did not show as ours.
    const OTHER: Language = Language::Other {
        english_day_period: false,
    };

    /// The language of an LCID (`40C`). Its low word is the language and
    /// region (the low ten bits are the primary language). Excel took the
    /// tag `1040C` for French, and `2040C` for English; a higher high word
    /// was not asked and counts as a language with no table. A tag with no
    /// primary language (`[$-400]`, `[$-0]`) names none, as before.
    ///
    /// A neutral LCID (`[$-A]`, `[$-16]`) takes the table of the text tag
    /// with no region (`[$-es]`, `[$-pt]`), which Excel showed other than the
    /// table of Spain or of Brazil: our choice, the neutral LCIDs were not
    /// asked, and no producer writes them.
    pub(crate) fn of_lcid(lcid: u32) -> Option<Language> {
        if lcid & 0x3FF == 0 {
            return None;
        }
        match lcid >> 16 {
            0 | 1 => {}
            2 => return Some(Language::Known(&ENGLISH)),
            _ => return Some(Language::OTHER),
        }
        let lcid = lcid & 0xFFFF;
        let names = match (lcid & 0x3FF, lcid) {
            (0x09, 0x0809 | 0x0C09 | 0x1809) => &ENGLISH_UK,
            (0x09, _) => &ENGLISH,
            (0x0C, 0x0C0C) => &FRENCH_CANADA,
            (0x0C, _) => &FRENCH,
            (0x07, 0x0C07) => &GERMAN_AUSTRIA,
            (0x07, _) => &GERMAN,
            (0x0A, 0x080A) => &SPANISH_MEXICO,
            (0x0A, 0x2C0A) => &SPANISH_ARGENTINA,
            (0x0A, 0x240A) => &SPANISH_COLOMBIA,
            (0x0A, 0x340A) => &SPANISH_CHILE,
            (0x0A, 0x540A) => &SPANISH_US,
            (0x0A, 0x000A) => &SPANISH_NO_REGION,
            (0x0A, _) => &SPANISH,
            (0x10, _) => &ITALIAN,
            (0x16, 0x0816) => &PORTUGUESE_PORTUGAL,
            (0x16, 0x0016) => &PORTUGUESE_NO_REGION,
            (0x16, _) => &PORTUGUESE,
            (0x13, _) => &DUTCH,
            // Russian and Polish: `PM` was seen.
            (0x19 | 0x15, _) => {
                return Some(Language::Other {
                    english_day_period: true,
                });
            }
            _ => return Some(Language::OTHER),
        };
        Some(Language::Known(names))
    }

    /// The language of a text tag in lower case (`fr-fr`, `fr`). `None` for
    /// no tag.
    pub(crate) fn of_tag(tag: &str) -> Option<Language> {
        if tag.is_empty() {
            return None;
        }
        let mut parts = tag.split(['-', '_']);
        let primary = parts.next().unwrap_or_default();
        let region = parts.next();
        let names = match (primary, region) {
            ("en", Some("gb" | "au" | "ie")) => &ENGLISH_UK,
            ("en", _) => &ENGLISH,
            ("fr", Some("ca")) => &FRENCH_CANADA,
            ("fr", _) => &FRENCH,
            ("de", Some("at")) => &GERMAN_AUSTRIA,
            ("de", _) => &GERMAN,
            ("es", Some("mx")) => &SPANISH_MEXICO,
            ("es", Some("ar")) => &SPANISH_ARGENTINA,
            ("es", Some("co")) => &SPANISH_COLOMBIA,
            ("es", Some("cl")) => &SPANISH_CHILE,
            ("es", Some("us")) => &SPANISH_US,
            ("es", None) => &SPANISH_NO_REGION,
            ("es", _) => &SPANISH,
            ("it", _) => &ITALIAN,
            ("pt", Some("pt")) => &PORTUGUESE_PORTUGAL,
            ("pt", None) => &PORTUGUESE_NO_REGION,
            ("pt", _) => &PORTUGUESE,
            ("nl", _) => &DUTCH,
            ("ru" | "pl", _) => {
                return Some(Language::Other {
                    english_day_period: true,
                });
            }
            _ => return Some(Language::OTHER),
        };
        Some(Language::Known(names))
    }
}

/// United States English, also Canada, India, New Zealand and South Africa.
pub static ENGLISH: Names = Names {
    months: [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
    short_months: [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    days: [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ],
    short_days: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    day_periods: ["AM", "PM"],
};

/// British English, also Australia and Ireland: the day period in lower case.
static ENGLISH_UK: Names = Names {
    day_periods: ["am", "pm"],
    ..ENGLISH
};

/// French of France, also Belgium, Switzerland and Luxembourg.
pub static FRENCH: Names = Names {
    months: [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ],
    short_months: [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ],
    days: [
        "dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi",
    ],
    short_days: ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."],
    day_periods: ["AM", "PM"],
};

/// Canadian French: `juill.` and `a.m.`.
static FRENCH_CANADA: Names = Names {
    short_months: [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juill.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ],
    day_periods: ["a.m.", "p.m."],
    ..FRENCH
};

/// German of Germany, also Switzerland and Luxembourg.
pub static GERMAN: Names = Names {
    months: [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ],
    short_months: [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ],
    days: [
        "Sonntag",
        "Montag",
        "Dienstag",
        "Mittwoch",
        "Donnerstag",
        "Freitag",
        "Samstag",
    ],
    short_days: ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"],
    day_periods: ["AM", "PM"],
};

/// Austrian German names January `Jänner`.
pub static GERMAN_AUSTRIA: Names = Names {
    months: [
        "Jänner",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ],
    short_months: [
        "Jän", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ],
    ..GERMAN
};

/// Spanish of Spain, the table of a region of Spanish not asked for.
pub static SPANISH: Names = Names {
    months: [
        "enero",
        "febrero",
        "marzo",
        "abril",
        "mayo",
        "junio",
        "julio",
        "agosto",
        "septiembre",
        "octubre",
        "noviembre",
        "diciembre",
    ],
    short_months: [
        "ene.", "feb.", "mar.", "abr.", "may.", "jun.", "jul.", "ago.", "sep.", "oct.", "nov.",
        "dic.",
    ],
    days: [
        "domingo",
        "lunes",
        "martes",
        "miércoles",
        "jueves",
        "viernes",
        "sábado",
    ],
    short_days: ["do.", "lu.", "ma.", "mi.", "ju.", "vi.", "sá."],
    day_periods: ["a.\u{202f}m.", "p.\u{202f}m."],
};

/// Mexican Spanish.
static SPANISH_MEXICO: Names = Names {
    short_days: ["dom.", "lun.", "mar.", "mié.", "jue.", "vie.", "sáb."],
    day_periods: ["a. m.", "p. m."],
    ..SPANISH
};

/// Argentine Spanish.
static SPANISH_ARGENTINA: Names = Names {
    short_months: [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic",
    ],
    short_days: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    ..SPANISH
};

/// Colombian Spanish.
static SPANISH_COLOMBIA: Names = Names {
    short_days: ["dom.", "lun.", "mar.", "mié.", "jue.", "vie.", "sáb."],
    day_periods: ["a.\u{a0}m.", "p.\u{a0}m."],
    ..SPANISH
};

/// Chilean Spanish.
static SPANISH_CHILE: Names = Names {
    short_months: [
        "ene.", "feb.", "mar.", "abr.", "may.", "jun.", "jul.", "ago.", "sept.", "oct.", "nov.",
        "dic.",
    ],
    short_days: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    day_periods: ["a.\u{a0}m.", "p.\u{a0}m."],
    ..SPANISH
};

/// Spanish of the United States.
static SPANISH_US: Names = Names {
    short_months: [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic",
    ],
    short_days: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    day_periods: ["a.m.", "p.m."],
    ..SPANISH
};

/// The tag `es`, with no region.
static SPANISH_NO_REGION: Names = Names {
    short_months: [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "set", "oct", "nov", "dic",
    ],
    short_days: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    day_periods: ["a. m.", "p. m."],
    ..SPANISH
};

/// Italian, also Switzerland.
pub static ITALIAN: Names = Names {
    months: [
        "gennaio",
        "febbraio",
        "marzo",
        "aprile",
        "maggio",
        "giugno",
        "luglio",
        "agosto",
        "settembre",
        "ottobre",
        "novembre",
        "dicembre",
    ],
    short_months: [
        "gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic",
    ],
    days: [
        "domenica",
        "lunedì",
        "martedì",
        "mercoledì",
        "giovedì",
        "venerdì",
        "sabato",
    ],
    short_days: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
    day_periods: ["AM", "PM"],
};

/// Portuguese of Brazil, the table of a region of Portuguese not asked for.
pub static PORTUGUESE: Names = Names {
    months: [
        "janeiro",
        "fevereiro",
        "março",
        "abril",
        "maio",
        "junho",
        "julho",
        "agosto",
        "setembro",
        "outubro",
        "novembro",
        "dezembro",
    ],
    short_months: [
        "jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.",
        "dez.",
    ],
    days: [
        "domingo",
        "segunda-feira",
        "terça-feira",
        "quarta-feira",
        "quinta-feira",
        "sexta-feira",
        "sábado",
    ],
    short_days: ["dom.", "seg.", "ter.", "qua.", "qui.", "sex.", "sáb."],
    day_periods: ["AM", "PM"],
};

/// Portuguese of Portugal.
static PORTUGUESE_PORTUGAL: Names = Names {
    short_months: [
        "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
    ],
    short_days: [
        "domingo", "segunda", "terça", "quarta", "quinta", "sexta", "sábado",
    ],
    ..PORTUGUESE
};

/// The tag `pt`, with no region.
static PORTUGUESE_NO_REGION: Names = Names {
    short_months: [
        "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
    ],
    ..PORTUGUESE
};

/// Dutch, also Belgium.
pub static DUTCH: Names = Names {
    months: [
        "januari",
        "februari",
        "maart",
        "april",
        "mei",
        "juni",
        "juli",
        "augustus",
        "september",
        "oktober",
        "november",
        "december",
    ],
    short_months: [
        "jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec",
    ],
    days: [
        "zondag",
        "maandag",
        "dinsdag",
        "woensdag",
        "donderdag",
        "vrijdag",
        "zaterdag",
    ],
    short_days: ["zo", "ma", "di", "wo", "do", "vr", "za"],
    day_periods: ["AM", "PM"],
};

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
        assert!(!std::ptr::eq(
            table(Language::of_tag("es-ar")).unwrap(),
            table(Language::of_tag("es-co")).unwrap()
        ));
    }

    #[test]
    fn a_tag_with_no_table_is_known_only_where_excel_showed_what_we_show() {
        let other = |english_day_period| Some(Language::Other { english_day_period });
        // No primary language, as before the tables: no language.
        for lcid in [0x0, 0x400, 0x800, 0xC00, 0x1_0400] {
            assert_eq!(Language::of_lcid(lcid), None, "{lcid:#x}");
        }
        // `[$-2040C]` showed English names: English, with its day period.
        assert_eq!(
            table(Language::of_lcid(0x2_040C)).map(|names| names.short_month(1)),
            Some("Jan")
        );
        // A high word nobody asked, and languages with no table.
        for lcid in [0x3_040C, 0x411, 0x804, 0x41F] {
            assert_eq!(Language::of_lcid(lcid), other(false), "{lcid:#x}");
        }
        // Russian and Polish showed `PM`.
        for (lcid, tag) in [(0x419, "ru-ru"), (0x415, "pl")] {
            assert_eq!(Language::of_lcid(lcid), other(true), "{lcid:#x}");
            assert_eq!(Language::of_tag(tag), other(true), "{tag}");
        }
        assert_eq!(Language::of_tag("ja-jp"), other(false));
        assert_eq!(Language::of_tag(""), None);
    }
}
