//! Word date-picker display formats (`w:date/w:dateFormat`).
//!
//! A date content control shows its date in the pattern of `w:dateFormat`, with
//! month and day names in the language of `w:lid`. The patterns are those Word
//! offers in the control's properties: `d dd ddd dddd`, `M MM MMM MMMM`,
//! `yy yyyy`, quoted `'literals'`, and the time tokens `h hh H HH m mm s ss`
//! with `AM/PM` or `am/pm`. A value without a time shows midnight, as Word
//! shows a date picked from its calendar. Names exist for English (full
//! and short) and, full names only, for French, German, Spanish, Italian,
//! Portuguese and Dutch: short names differ between Windows and Office
//! versions outside English. The day period is the locale's designator whatever
//! the token's case, known for United States English only (`AM`): other
//! English locales write `am`. Any other letter, or a name in another language,
//! is refused. On the `gregorianUs` calendar (Gregorian, US English) names are
//! English whatever the language; the day period stays the language's.

use crate::error::{OxmlError, Result};
use oxoffice_numfmt::names::{
    DUTCH, ENGLISH, FRENCH, GERMAN, GERMAN_AUSTRIA, ITALIAN, Names, PORTUGUESE, SPANISH,
};

/// The names a date control shows for a language, and whether the short ones
/// are known (English only).
struct WordNames {
    names: &'static Names,
    short: bool,
}

/// The primary language and region subtags of a `w:lid` value: a BCP 47 tag
/// (`fr-FR`) or a hexadecimal Windows language identifier (`040C`), `None`
/// for an identifier of another language.
fn subtags(language: &str) -> Option<(&'static str, String)> {
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
            let region = match lcid {
                0x0c07 => "at",
                0x0409 => "us",
                _ => "",
            };
            (primary, region.to_owned())
        }
        _ => {
            let mut parts = tag.split(['-', '_']);
            let primary = match parts.next().unwrap_or("") {
                "en" => "en",
                "fr" => "fr",
                "de" => "de",
                "es" => "es",
                "it" => "it",
                "pt" => "pt",
                "nl" => "nl",
                _ => return None,
            };
            let region = parts.find(|part| part.len() == 2).unwrap_or("");
            (primary, region.to_owned())
        }
    };
    Some((primary, region))
}

/// The names of a `w:lid` value. No `w:lid` reads as `en-US`.
fn names_of(language: Option<&str>) -> Option<WordNames> {
    let Some(language) = language else {
        return Some(WordNames {
            names: &ENGLISH,
            short: true,
        });
    };
    let (primary, region) = subtags(language)?;
    let names = match (primary, region.as_str()) {
        ("en", _) => &ENGLISH,
        ("fr", _) => &FRENCH,
        ("de", "at") => &GERMAN_AUSTRIA,
        ("de", _) => &GERMAN,
        ("es", _) => &SPANISH,
        ("it", _) => &ITALIAN,
        ("pt", _) => &PORTUGUESE,
        ("nl", _) => &DUTCH,
        _ => return None,
    };
    Some(WordNames {
        names,
        short: primary == "en",
    })
}

/// The morning and afternoon designators of a `w:lid` value, known for
/// `en-US` only.
fn day_periods_of(language: Option<&str>) -> Option<[&'static str; 2]> {
    let periods = Some(["AM", "PM"]);
    let Some(language) = language else {
        return periods;
    };
    if matches!(subtags(language), Some(("en", region)) if region == "us") {
        periods
    } else {
        None
    }
}

/// A checked Gregorian date, with the time of day Word saves when one is
/// typed into a control whose format shows a time (midnight otherwise).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WordDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl WordDateTime {
    /// Read a value given to a date control: `YYYY-MM-DD`, or with a time
    /// `YYYY-MM-DDTHH:MM` or `YYYY-MM-DDTHH:MM:SS` (24-hour, no zone).
    pub fn parse_value(value: &str) -> Result<Self> {
        let invalid = || {
            OxmlError::InvalidValue(format!(
                "expected a Gregorian YYYY-MM-DD date, optionally with THH:MM or THH:MM:SS, got {value:?}"
            ))
        };
        let (date, time) = match value.split_once('T') {
            Some((date, time)) => (date, Some(time)),
            None => (value, None),
        };
        let (year, month, day) = civil_date(date).ok_or_else(invalid)?;
        let (hour, minute, second) = match time {
            None => (0, 0, 0),
            Some(time) => {
                let fields = time.split(':').collect::<Vec<_>>();
                let field = |index: usize, maximum: u8| {
                    fields
                        .get(index)
                        .filter(|field| {
                            field.len() == 2 && field.bytes().all(|b| b.is_ascii_digit())
                        })
                        .and_then(|field| field.parse::<u8>().ok())
                        .filter(|number| *number <= maximum)
                        .ok_or_else(invalid)
                };
                if !matches!(fields.len(), 2 | 3) {
                    return Err(invalid());
                }
                let second = if fields.len() == 3 { field(2, 59)? } else { 0 };
                (field(0, 23)?, field(1, 59)?, second)
            }
        };
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }

    /// Read a `w:fullDate` as Word writes it, `YYYY-MM-DDTHH:MM:SSZ`.
    pub fn parse_full_date(value: &str) -> Option<Self> {
        let value = value.strip_suffix('Z')?;
        (value.len() == 19)
            .then(|| Self::parse_value(value).ok())
            .flatten()
    }

    /// The `w:fullDate` of this value, `YYYY-MM-DDTHH:MM:SSZ`.
    pub fn full_date(&self) -> String {
        format!(
            "{}T{:02}:{:02}:{:02}Z",
            self.date(),
            self.hour,
            self.minute,
            self.second
        )
    }

    /// The date alone, `YYYY-MM-DD`.
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// The value as given to a setter: the date, followed by the time when
    /// it is not midnight.
    pub fn value(&self) -> String {
        if self.is_midnight() {
            self.date()
        } else {
            format!(
                "{}T{:02}:{:02}:{:02}",
                self.date(),
                self.hour,
                self.minute,
                self.second
            )
        }
    }

    pub fn is_midnight(&self) -> bool {
        (self.hour, self.minute, self.second) == (0, 0, 0)
    }
}

/// A Gregorian `YYYY-MM-DD` date that exists.
fn civil_date(value: &str) -> Option<(u16, u8, u8)> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return None;
    }
    let year: u16 = value[..4].parse().ok()?;
    let month: u8 = value[5..7].parse().ok()?;
    let day: u8 = value[8..].parse().ok()?;
    let leap = year.is_multiple_of(400) || year.is_multiple_of(4) && !year.is_multiple_of(100);
    let maximum = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (year != 0 && day != 0 && day <= maximum).then_some((year, month, day))
}

/// Show the checked date and time `at` in the Word date pattern `pattern`,
/// with the names of the language `language` (`w:lid`), or English ones on
/// the `gregorianUs` calendar (`w:calendar`).
pub(crate) fn format_word_date(
    pattern: &str,
    language: Option<&str>,
    calendar: Option<&str>,
    at: WordDateTime,
) -> Result<String> {
    let names_language = if calendar == Some("gregorianUs") {
        Some("en-US")
    } else {
        language
    };
    let WordDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    } = at;
    let unsupported =
        |why: &str| OxmlError::InvalidValue(format!("unsupported date format {pattern:?}: {why}"));
    let names = |kind: &str| {
        names_of(names_language)
            .map(|word| word.names)
            .ok_or_else(|| {
                unsupported(&format!(
                    "{kind} in language {:?} cannot be shown, numeric formats can",
                    language.unwrap_or_default()
                ))
            })
    };
    let short = |kind: &str| {
        names(kind)?;
        names_of(names_language)
            .filter(|word| word.short)
            .map(|word| word.names)
            .ok_or_else(|| {
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
    let chars: Vec<char> = pattern.chars().collect();
    let mut output = String::new();
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        if current == '\'' {
            // A quoted literal; two quotes in a row are one quote, inside
            // a literal or outside.
            index += 1;
            if chars.get(index) == Some(&'\'') {
                output.push('\'');
                index += 1;
                continue;
            }
            loop {
                match (chars.get(index), chars.get(index + 1)) {
                    (None, _) => return Err(unsupported("unterminated quote")),
                    (Some('\''), Some('\'')) => {
                        output.push('\'');
                        index += 2;
                    }
                    (Some('\''), _) => {
                        index += 1;
                        break;
                    }
                    (Some(&literal), _) => {
                        output.push(literal);
                        index += 1;
                    }
                }
            }
            continue;
        }
        let rest: String = chars[index..chars.len().min(index + 5)].iter().collect();
        if rest.eq_ignore_ascii_case("AM/PM") {
            let periods = day_periods_of(language).ok_or_else(|| {
                unsupported(&format!(
                    "the day period in language {:?} cannot be shown, 24-hour times can",
                    language.unwrap_or_default()
                ))
            })?;
            output.push_str(periods[usize::from(hour >= 12)]);
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
            ('d', 3) => output.push_str(short("a short day name")?.short_weekday(weekday as u32)),
            ('d', 4) => output.push_str(names("a day name")?.weekday(weekday as u32)),
            ('M', 1) => output.push_str(&month.to_string()),
            ('M', 2) => output.push_str(&format!("{month:02}")),
            ('M', 3) => output.push_str(short("a short month name")?.short_month(u32::from(month))),
            ('M', 4) => output.push_str(names("a month name")?.month(u32::from(month))),
            ('y', 2) => output.push_str(&format!("{:02}", year % 100)),
            ('y', 4) => output.push_str(&format!("{year:04}")),
            ('h', 1) => output.push_str(&twelve_hour(hour).to_string()),
            ('h', 2) => output.push_str(&format!("{:02}", twelve_hour(hour))),
            ('H', 1) => output.push_str(&hour.to_string()),
            ('H', 2) => output.push_str(&format!("{hour:02}")),
            ('m', 1) => output.push_str(&minute.to_string()),
            ('m', 2) => output.push_str(&format!("{minute:02}")),
            ('s', 1) => output.push_str(&second.to_string()),
            ('s', 2) => output.push_str(&format!("{second:02}")),
            _ => {
                let token: String = chars[index..index + count].iter().collect();
                return Err(unsupported(&format!("unknown token {token:?}")));
            }
        }
        index += count;
    }
    Ok(output)
}

/// The hour on a 12-hour clock: midnight and noon are 12.
fn twelve_hour(hour: u8) -> u8 {
    match hour % 12 {
        0 => 12,
        hour => hour,
    }
}

/// Whether the date pattern `pattern` holds, outside quotes, one of `tokens`.
/// A day period `AM/PM`, in any case, is one token, `'A'`: its letters are
/// not hours or minutes.
fn pattern_holds(pattern: &str, tokens: &[char]) -> bool {
    let chars: Vec<char> = pattern.chars().collect();
    let mut quoted = false;
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        if current == '\'' {
            quoted = !quoted;
        } else if !quoted {
            let rest: String = chars[index..chars.len().min(index + 5)].iter().collect();
            if rest.eq_ignore_ascii_case("AM/PM") {
                if tokens.contains(&'A') {
                    return true;
                }
                index += 5;
                continue;
            }
            if tokens.contains(&current) {
                return true;
            }
        }
        index += 1;
    }
    false
}

/// Whether the date pattern `pattern` shows a time (an hour, minute, second
/// or day period token outside quotes).
pub fn word_date_format_shows_time(pattern: &str) -> bool {
    pattern_holds(pattern, &['h', 'H', 'm', 's', 'A'])
}

/// Refuse a time whose hours, minutes or seconds the pattern `pattern`
/// would not show: they would be saved in `w:fullDate` and a binding,
/// unseen in the control.
pub(crate) fn check_time_shown(pattern: &str, at: WordDateTime) -> Result<()> {
    if at.is_midnight() {
        return Ok(());
    }
    if !word_date_format_shows_time(pattern) {
        return Err(OxmlError::InvalidValue(format!(
            "date format {pattern:?} shows no time, give a date without one"
        )));
    }
    for (value, tokens, part) in [
        (at.hour, &['h', 'H'][..], "hours"),
        (at.minute, &['m'][..], "minutes"),
        (at.second, &['s'][..], "seconds"),
    ] {
        if value != 0 && !pattern_holds(pattern, tokens) {
            return Err(OxmlError::InvalidValue(format!(
                "date format {pattern:?} does not show the {part}, give a time without them"
            )));
        }
    }
    Ok(())
}

/// Refuse a pattern Word date controls cannot show in `language` on
/// `calendar`.
pub fn check_word_date_format(
    pattern: &str,
    language: Option<&str>,
    calendar: Option<&str>,
) -> Result<()> {
    // A Wednesday in September: every token and name kind is exercised.
    format_word_date(
        pattern,
        language,
        calendar,
        WordDateTime::parse_value("2026-09-30").expect("a valid date"),
    )
    .map(|_| ())
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

    fn at(value: &str) -> WordDateTime {
        WordDateTime::parse_value(value).unwrap()
    }

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
            // Word shows the locale's designator whatever the token's case.
            ("M/d/yyyy h:mm:ss am/pm", "1/2/2015 12:00:00 AM"),
            ("h 'o''clock' am/pm", "12 o'clock AM"),
            ("''d''", "'2'"),
            ("HH:mm", "00:00"),
            ("H:m:s", "0:0:0"),
            ("ddd dd/MM/yyyy", "Fri 02/01/2015"),
            ("'Day' d 'of' MMMM", "Day 2 of January"),
        ] {
            assert_eq!(
                format_word_date(pattern, Some("en-US"), None, at("2015-01-02")).unwrap(),
                shown,
                "{pattern}"
            );
        }
    }

    #[test]
    fn a_time_of_day_is_shown_in_the_tokens_of_the_format() {
        for (pattern, value, shown) in [
            (
                "M/d/yyyy h:mm AM/PM",
                "2015-01-02T15:05",
                "1/2/2015 3:05 PM",
            ),
            (
                "M/d/yyyy h:mm am/pm",
                "2015-01-02T00:30",
                "1/2/2015 12:30 AM",
            ),
            ("hh:mm:ss AM/PM", "2015-01-02T12:07:09", "12:07:09 PM"),
            ("hh AM/PM", "2015-01-02T09:00", "09 AM"),
            ("HH:mm:ss", "2015-01-02T15:05:09", "15:05:09"),
            ("H:m:s", "2015-01-02T08:05:09", "8:5:9"),
            ("h:mm", "2015-01-02T23:59", "11:59"),
            ("d MMMM yyyy HH:mm", "2015-01-02", "2 January 2015 00:00"),
        ] {
            assert_eq!(
                format_word_date(pattern, Some("en-US"), None, at(value)).unwrap(),
                shown,
                "{pattern} {value}"
            );
        }
    }

    #[test]
    fn names_on_the_us_gregorian_calendar_are_english_whatever_the_language() {
        assert_eq!(
            format_word_date(
                "dddd d MMMM yyyy",
                Some("fr-FR"),
                Some("gregorianUs"),
                at("2015-01-02")
            )
            .unwrap(),
            "Friday 2 January 2015"
        );
        assert_eq!(
            format_word_date(
                "ddd d MMM",
                Some("ja-JP"),
                Some("gregorianUs"),
                at("2015-01-02")
            )
            .unwrap(),
            "Fri 2 Jan"
        );
        // The day period is the language's, not the calendar's.
        assert!(
            format_word_date(
                "h AM/PM",
                Some("fr-FR"),
                Some("gregorianUs"),
                at("2015-01-02")
            )
            .is_err()
        );
        assert_eq!(
            format_word_date(
                "dddd d MMMM",
                Some("fr-FR"),
                Some("gregorian"),
                at("2015-01-02")
            )
            .unwrap(),
            "vendredi 2 janvier"
        );
    }

    #[test]
    fn a_time_part_the_format_does_not_show_is_refused() {
        for (pattern, value) in [
            ("M/d/yyyy h:mm AM/PM", "2015-01-02T15:05"),
            ("M/d/yyyy h:mm AM/PM", "2015-01-02T15:05:00"),
            ("HH:mm:ss", "2015-01-02T15:05:09"),
            ("d MMMM yyyy", "2015-01-02T00:00"),
        ] {
            assert!(
                check_time_shown(pattern, at(value)).is_ok(),
                "{pattern} {value}"
            );
        }
        for (pattern, value, reason) in [
            ("d MMMM yyyy", "2015-01-02T15:05", "shows no time"),
            ("M/d/yyyy h:mm AM/PM", "2015-01-02T15:05:09", "the seconds"),
            ("h AM/PM", "2015-01-02T15:05", "the minutes"),
            ("h am/pm", "2015-01-02T15:05", "the minutes"),
            ("s am/pm", "2015-01-02T15:00:09", "the hours"),
            ("mm:ss", "2015-01-02T15:05:09", "the hours"),
            ("'hh:mm' d", "2015-01-02T15:05", "shows no time"),
        ] {
            let error = check_time_shown(pattern, at(value))
                .unwrap_err()
                .to_string();
            assert!(error.contains(reason), "{pattern} {value}: {error}");
        }
    }

    #[test]
    fn a_value_is_a_date_with_an_optional_time_and_round_trips_through_full_date() {
        let value = at("2015-01-02T15:05:09");
        assert_eq!(value.full_date(), "2015-01-02T15:05:09Z");
        assert_eq!(
            WordDateTime::parse_full_date(&value.full_date()),
            Some(value)
        );
        assert_eq!(value.value(), "2015-01-02T15:05:09");
        assert_eq!(at("2015-01-02T15:05").full_date(), "2015-01-02T15:05:00Z");
        assert_eq!(at("2015-01-02").full_date(), "2015-01-02T00:00:00Z");
        assert_eq!(at("2015-01-02T00:00").value(), "2015-01-02");
        for invalid in [
            "2015-02-29",
            "2015-01-02T",
            "2015-01-02T24:00",
            "2015-01-02T12:60",
            "2015-01-02T12:00:60",
            "2015-01-02T1:05",
            "2015-01-02T12:05:00Z",
            "2015-01-02T12:05:00.5",
            "2015-01-02 12:05",
        ] {
            assert!(WordDateTime::parse_value(invalid).is_err(), "{invalid}");
        }
        for invalid in ["2015-01-02T15:05:09", "2015-01-02Z", "2015-01-02T15:05Z"] {
            assert_eq!(WordDateTime::parse_full_date(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn a_pattern_shows_a_time_when_it_holds_a_time_token_outside_quotes() {
        for pattern in ["M/d/yyyy h:mm am/pm", "HH:mm", "d 's' s", "am/pm"] {
            assert!(word_date_format_shows_time(pattern), "{pattern}");
        }
        for pattern in ["d MMMM yyyy", "dddd, d' de 'MMMM", "'hh:mm' d"] {
            assert!(!word_date_format_shows_time(pattern), "{pattern}");
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
            ("es-ES", "viernes 2 enero"),
            ("it-IT", "venerdì 2 gennaio"),
            ("pt-BR", "sexta-feira 2 janeiro"),
            ("nl-NL", "vrijdag 2 januari"),
            ("en-GB", "Friday 2 January"),
        ] {
            assert_eq!(
                format_word_date("dddd d MMMM", Some(language), None, at("2015-01-02")).unwrap(),
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
            (
                "h:mm AM/PM",
                Some("fr-FR"),
                "day period in language \"fr-FR\"",
            ),
            (
                "h:mm am/pm",
                Some("en-GB"),
                "day period in language \"en-GB\"",
            ),
            ("h:mm am/pm", Some("0809"), "day period"),
            ("y", None, "unknown token \"y\""),
            ("yyy", None, "unknown token \"yyy\""),
            ("ddddd", None, "unknown token"),
            ("MMMMM", None, "unknown token"),
            ("d 'of MMMM", None, "unterminated quote"),
            ("d 'o''", None, "unterminated quote"),
            ("gg yyyy", None, "unknown token \"gg\""),
            ("", None, "empty"),
        ] {
            let error = check_word_date_format(pattern, language, None)
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
