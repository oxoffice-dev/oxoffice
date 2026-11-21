//! Word date-picker display formats (`w:date/w:dateFormat`).
//!
//! A date content control shows its date in the pattern of `w:dateFormat`, with
//! month and day names in the language of `w:lid`. The patterns are those Word
//! offers in the control's properties: `d dd ddd dddd`, `M MM MMM MMMM`,
//! `yy yyyy`, quoted `'literals'`, and the time tokens `h hh H HH m mm s ss`
//! with `AM/PM` or `am/pm`. The value is a date, so a time shows midnight, as
//! Word shows a date picked from its calendar. Names exist for English (full
//! and short) and, full names only, for French, German, Spanish, Italian,
//! Portuguese and Dutch: short names and day periods differ between Windows
//! and Office versions outside English. Any other letter, or a name in another
//! language, is refused.

use crate::error::{OxmlError, Result};

struct Names {
    months: [&'static str; 12],
    /// Sunday first.
    days: [&'static str; 7],
    short: Option<ShortNames>,
}

struct ShortNames {
    months: [&'static str; 12],
    days: [&'static str; 7],
    am: &'static str,
}

const ENGLISH: Names = Names {
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
    days: [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ],
    short: Some(ShortNames {
        months: [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ],
        days: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
        am: "AM",
    }),
};

const FRENCH: Names = Names {
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
    days: [
        "dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi",
    ],
    short: None,
};

const GERMAN: Names = Names {
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
    days: [
        "Sonntag",
        "Montag",
        "Dienstag",
        "Mittwoch",
        "Donnerstag",
        "Freitag",
        "Samstag",
    ],
    short: None,
};

/// Austrian German names January "Jänner".
const AUSTRIAN_GERMAN: Names = Names {
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
    days: GERMAN.days,
    short: None,
};

const SPANISH: Names = Names {
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
    days: [
        "domingo",
        "lunes",
        "martes",
        "miércoles",
        "jueves",
        "viernes",
        "sábado",
    ],
    short: None,
};

const ITALIAN: Names = Names {
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
    days: [
        "domenica",
        "lunedì",
        "martedì",
        "mercoledì",
        "giovedì",
        "venerdì",
        "sabato",
    ],
    short: None,
};

const PORTUGUESE: Names = Names {
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
    days: [
        "domingo",
        "segunda-feira",
        "terça-feira",
        "quarta-feira",
        "quinta-feira",
        "sexta-feira",
        "sábado",
    ],
    short: None,
};

const DUTCH: Names = Names {
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
    days: [
        "zondag",
        "maandag",
        "dinsdag",
        "woensdag",
        "donderdag",
        "vrijdag",
        "zaterdag",
    ],
    short: None,
};

/// The names of a `w:lid` value: a BCP 47 tag (`fr-FR`) or a hexadecimal
/// Windows language identifier (`040C`). No `w:lid` reads as `en-US`.
fn names_of(language: Option<&str>) -> Option<&'static Names> {
    let Some(language) = language else {
        return Some(&ENGLISH);
    };
    let tag = language.to_ascii_lowercase();
    let (primary, region) = match u16::from_str_radix(&tag, 16) {
        Ok(lcid) if tag.len() == 4 => {
            let primary = match lcid & 0x3ff {
                0x09 => "en",
                0x0c => "fr",
                0x07 => "de",
                0x0a => "es",
                0x10 => "it",
                0x16 => "pt",
                0x13 => "nl",
                _ => return None,
            };
            (primary, if lcid == 0x0c07 { "at" } else { "" })
        }
        _ => {
            let mut parts = tag.split(['-', '_']);
            let primary = parts.next().unwrap_or("");
            (primary, parts.find(|part| part.len() == 2).unwrap_or(""))
        }
    };
    Some(match (primary, region) {
        ("en", _) => &ENGLISH,
        ("fr", _) => &FRENCH,
        ("de", "at") => &AUSTRIAN_GERMAN,
        ("de", _) => &GERMAN,
        ("es", _) => &SPANISH,
        ("it", _) => &ITALIAN,
        ("pt", _) => &PORTUGUESE,
        ("nl", _) => &DUTCH,
        _ => return None,
    })
}

/// Show the Gregorian date `year-month-day` in the Word date pattern `pattern`,
/// with the names of the language `language` (`w:lid`).
pub fn format_word_date(
    pattern: &str,
    language: Option<&str>,
    year: u16,
    month: u8,
    day: u8,
) -> Result<String> {
    let unsupported =
        |why: &str| OxmlError::InvalidValue(format!("unsupported date format {pattern:?}: {why}"));
    let names = |kind: &str| {
        names_of(language).ok_or_else(|| {
            unsupported(&format!(
                "{kind} in language {:?} cannot be shown, numeric formats can",
                language.unwrap_or_default()
            ))
        })
    };
    let short = |kind: &str| {
        names(kind)?.short.as_ref().ok_or_else(|| {
            unsupported(&format!(
                "{kind} in language {:?} cannot be shown, full names can",
                language.unwrap_or_default()
            ))
        })
    };
    if pattern.is_empty() {
        return Err(unsupported("empty"));
    }
    let weekday = weekday(year, month, day);
    let month_index = usize::from(month - 1);
    let chars: Vec<char> = pattern.chars().collect();
    let mut output = String::new();
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        if current == '\'' {
            let Some(length) = chars[index + 1..].iter().position(|&c| c == '\'') else {
                return Err(unsupported("unterminated quote"));
            };
            output.extend(&chars[index + 1..index + 1 + length]);
            index += length + 2;
            continue;
        }
        let rest: String = chars[index..chars.len().min(index + 5)].iter().collect();
        if rest == "AM/PM" || rest == "am/pm" {
            let am = short("a day period")?.am;
            if rest == "am/pm" {
                output.push_str(&am.to_lowercase());
            } else {
                output.push_str(am);
            }
            index += 5;
            continue;
        }
        if !current.is_ascii_alphabetic() {
            output.push(current);
            index += 1;
            continue;
        }
        let count = chars[index..].iter().take_while(|&&c| c == current).count();
        match (current, count) {
            ('d', 1) => output.push_str(&day.to_string()),
            ('d', 2) => output.push_str(&format!("{day:02}")),
            ('d', 3) => output.push_str(short("a short day name")?.days[weekday]),
            ('d', 4) => output.push_str(names("a day name")?.days[weekday]),
            ('M', 1) => output.push_str(&month.to_string()),
            ('M', 2) => output.push_str(&format!("{month:02}")),
            ('M', 3) => output.push_str(short("a short month name")?.months[month_index]),
            ('M', 4) => output.push_str(names("a month name")?.months[month_index]),
            ('y', 2) => output.push_str(&format!("{:02}", year % 100)),
            ('y', 4) => output.push_str(&format!("{year:04}")),
            ('h', 1) => output.push_str("12"),
            ('h' | 'H', 2) => output.push_str(if current == 'h' { "12" } else { "00" }),
            ('H' | 'm' | 's', 1) => output.push('0'),
            ('m' | 's', 2) => output.push_str("00"),
            _ => {
                let token: String = chars[index..index + count].iter().collect();
                return Err(unsupported(&format!("unknown token {token:?}")));
            }
        }
        index += count;
    }
    Ok(output)
}

/// Refuse a pattern [`format_word_date`] cannot show in `language`.
pub fn check_word_date_format(pattern: &str, language: Option<&str>) -> Result<()> {
    // A Wednesday in September: every token and name kind is exercised.
    format_word_date(pattern, language, 2026, 9, 30).map(|_| ())
}

/// Day of the week of a Gregorian date, Sunday 0 (Sakamoto's method).
fn weekday(year: u16, month: u8, day: u8) -> usize {
    const OFFSETS: [u32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let year = u32::from(year) - u32::from(month < 3);
    let value = year + year / 4 - year / 100
        + year / 400
        + OFFSETS[usize::from(month - 1)]
        + u32::from(day);
    (value % 7) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weekday_is_the_calendar_one() {
        assert_eq!(weekday(2015, 1, 2), 5);
        assert_eq!(weekday(2026, 11, 21), 6);
        assert_eq!(weekday(2000, 2, 29), 2);
        assert_eq!(weekday(1, 1, 1), 1);
    }

    #[test]
    fn every_format_word_offers_in_english_is_shown() {
        // Word's list for an en-US date control, on Friday 2 January 2015.
        for (pattern, shown) in [
            ("M/d/yyyy", "1/2/2015"),
            ("dddd, MMMM d, yyyy", "Friday, January 2, 2015"),
            ("MMMM d, yyyy", "January 2, 2015"),
            ("M/d/yy", "1/2/15"),
            ("yyyy-MM-dd", "2015-01-02"),
            ("d-MMM-yy", "2-Jan-15"),
            ("M.d.yyyy", "1.2.2015"),
            ("MMM. d, yy", "Jan. 2, 15"),
            ("d MMMM yyyy", "2 January 2015"),
            ("MMMM yy", "January 15"),
            ("MMM-yy", "Jan-15"),
            ("M/d/yyyy h:mm AM/PM", "1/2/2015 12:00 AM"),
            ("M/d/yyyy h:mm:ss am/pm", "1/2/2015 12:00:00 am"),
            ("HH:mm", "00:00"),
            ("H:m:s", "0:0:0"),
            ("ddd dd/MM/yyyy", "Fri 02/01/2015"),
            ("'Day' d 'of' MMMM", "Day 2 of January"),
        ] {
            assert_eq!(
                format_word_date(pattern, Some("en-US"), 2015, 1, 2).unwrap(),
                shown,
                "{pattern}"
            );
        }
    }

    #[test]
    fn a_language_is_read_from_its_tag_or_its_windows_identifier() {
        for (language, shown) in [
            ("fr-FR", "vendredi 2 janvier"),
            ("FR-be", "vendredi 2 janvier"),
            ("040C", "vendredi 2 janvier"),
            ("0407", "Freitag 2 Januar"),
            ("0C07", "Freitag 2 Jänner"),
            ("it-IT", "venerdì 2 gennaio"),
            ("pt-BR", "sexta-feira 2 janeiro"),
            ("nl-NL", "vrijdag 2 januari"),
            ("en-GB", "Friday 2 January"),
        ] {
            assert_eq!(
                format_word_date("dddd d MMMM", Some(language), 2015, 1, 2).unwrap(),
                shown,
                "{language}"
            );
        }
    }

    #[test]
    fn a_pattern_that_cannot_be_shown_is_refused_with_its_reason() {
        for (pattern, language, reason) in [
            (
                "d MMMM yyyy",
                Some("ja-JP"),
                "month name in language \"ja-JP\"",
            ),
            ("dddd", Some("0411"), "day name in language \"0411\""),
            (
                "d MMM yyyy",
                Some("fr-FR"),
                "short month name in language \"fr-FR\"",
            ),
            ("ddd d", Some("de-DE"), "short day name"),
            ("h:mm AM/PM", Some("fr-FR"), "day period"),
            ("y", None, "unknown token \"y\""),
            ("yyy", None, "unknown token \"yyy\""),
            ("ddddd", None, "unknown token"),
            ("MMMMM", None, "unknown token"),
            ("d 'of MMMM", None, "unterminated quote"),
            ("gg yyyy", None, "unknown token \"gg\""),
            ("", None, "empty"),
        ] {
            let error = check_word_date_format(pattern, language)
                .unwrap_err()
                .to_string();
            assert!(error.contains(reason), "{pattern}: {error}");
            assert!(
                error.contains(&format!("{pattern:?}")),
                "{pattern}: {error}"
            );
        }
    }
}
