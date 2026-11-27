//! Dates and times from serial day numbers.

use crate::decimal::Decimal;
use crate::parse::{self, Section, SystemFormat, Token, Unit};
use crate::{DateSystem, FormatOptions, Formatted, Locale, Out};

const MONTHS: [&str; 12] = [
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
];
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

impl Locale {
    fn month(self, month: u32) -> &'static str {
        match self {
            Locale::EnUs => MONTHS[month as usize - 1],
        }
    }

    fn weekday(self, weekday: u32) -> &'static str {
        match self {
            Locale::EnUs => DAYS[weekday as usize],
        }
    }

    /// The code `[$-F800]` and `[$-F400]` stand for.
    fn system_code(self, system: SystemFormat) -> &'static str {
        match (self, system) {
            (Locale::EnUs, SystemFormat::LongDate) => "dddd, mmmm d, yyyy",
            (Locale::EnUs, SystemFormat::Time) => "h:mm:ss AM/PM",
        }
    }
}

impl DateSystem {
    /// The last serial day: 9999-12-31.
    fn last_day(self) -> i64 {
        match self {
            DateSystem::Date1900 => 2_958_465,
            DateSystem::Date1904 => 2_957_003,
        }
    }

    /// Year, month, day and weekday (0 for Sunday) of a serial day.
    fn civil(self, day: i64) -> (i64, u32, u32, u32) {
        match self {
            DateSystem::Date1900 => {
                // Excel counts a 1900-02-29 (serial 60), and serial 0 is
                // 1900-01-00, a Saturday: serial 1 is a Sunday.
                let weekday = ((day + 6) % 7) as u32;
                match day {
                    0 => (1900, 1, 0, weekday),
                    60 => (1900, 2, 29, weekday),
                    _ => {
                        let after = if day < 60 { day } else { day - 1 };
                        let (year, month, date) = from_days(days(1899, 12, 31) + after);
                        (year, month, date, weekday)
                    }
                }
            }
            DateSystem::Date1904 => {
                let (year, month, date) = from_days(days(1904, 1, 1) + day);
                (year, month, date, ((day + 5) % 7) as u32)
            }
        }
    }
}

/// Days from 1970-01-01 (Howard Hinnant's algorithm).
fn days(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let month = i64::from(month);
    let of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_era_days - 719_468
}

fn from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day = (of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A date or time section. `sign` is false when the section shows
/// negatives without their minus sign.
pub(crate) fn format(
    section: &Section,
    value: f64,
    sign: bool,
    options: &FormatOptions,
) -> Formatted {
    let system;
    let tokens = match section.system {
        Some(format) => {
            system = parse::sections(options.locale.system_code(format))
                .expect("a system code reads")
                .remove(0);
            &system.tokens
        }
        None => &section.tokens,
    };
    let negative = value < 0.0;
    if negative && options.date_system == DateSystem::Date1900 {
        return Formatted::overflow(section.color);
    }
    let magnitude = Decimal::from_f64(value);
    let last_day = options.date_system.last_day();
    if magnitude.to_f64() >= (last_day + 1) as f64 {
        return Formatted::overflow(section.color);
    }
    // The value is rounded to the second, or to the decimals of the seconds
    // shown, before the date is taken: 23:59:59.6 is the next day.
    let decimals = tokens
        .iter()
        .filter_map(|token| match token {
            Token::SubSecond(count) => Some(*count as u32),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let per_second = 10i64.pow(decimals);
    let per_day = 86_400 * per_second;
    // In decimal: 12:00:00.4999999999968 is noon, though its double times
    // 86400 is 43200.5.
    let units = match magnitude.as_ratio() {
        Some((numerator, denominator)) => {
            let scaled = numerator * per_day as u128;
            ((2 * scaled + denominator) / (2 * denominator)) as i64
        }
        None => (magnitude.to_f64() * per_day as f64).round() as i64,
    };
    let day = units / per_day;
    if day > last_day {
        return Formatted::overflow(section.color);
    }
    let within = units % per_day;
    let (year, month, date, weekday) = options.date_system.civil(day);
    let hour = within / (3600 * per_second);
    let minute = within / (60 * per_second) % 60;
    let second = within / per_second % 60;
    let fraction = within % per_second;
    let twelve_hours = tokens
        .iter()
        .any(|token| matches!(token, Token::AmPm { .. }));
    let locale = options.locale;

    let mut out = Out::default();
    if negative && sign {
        out.push('-');
    }
    let padded = |out: &mut Out, value: i64, width: usize| {
        out.push_str(&format!("{value:0width$}"));
    };
    for token in tokens {
        match *token {
            Token::Year(count) if count <= 2 => padded(&mut out, year % 100, 2),
            Token::Year(_) => padded(&mut out, year, 4),
            Token::Month(1) => padded(&mut out, i64::from(month), 1),
            Token::Month(2) => padded(&mut out, i64::from(month), 2),
            Token::Month(3) => out.push_str(&locale.month(month)[..3]),
            Token::Month(5) => out.push_str(&locale.month(month)[..1]),
            Token::Month(_) => out.push_str(locale.month(month)),
            Token::Day(count @ (1 | 2)) => padded(&mut out, i64::from(date), count),
            Token::Day(3) => out.push_str(&locale.weekday(weekday)[..3]),
            Token::Day(_) => out.push_str(locale.weekday(weekday)),
            Token::Hour(count) => {
                let hour = if twelve_hours {
                    (hour + 11) % 12 + 1
                } else {
                    hour
                };
                padded(&mut out, hour, count.min(2));
            }
            Token::Minute(count) => padded(&mut out, minute, count.min(2)),
            Token::Second(count) => padded(&mut out, second, count.min(2)),
            Token::SubSecond(count) => {
                let digits = format!("{fraction:0width$}", width = decimals as usize);
                out.push('.');
                out.push_str(&digits[..count]);
            }
            Token::Elapsed(unit, count) => {
                let per_unit = match unit {
                    Unit::Hour => 3600,
                    Unit::Minute => 60,
                    Unit::Second => 1,
                } * per_second;
                padded(&mut out, units / per_unit, count);
            }
            Token::AmPm { short } => {
                let morning = hour < 12;
                match short {
                    Some((am, pm)) => out.push(if morning { am } else { pm }),
                    None => out.push_str(if morning { "AM" } else { "PM" }),
                }
            }
            ref token => {
                out.push_plain(token);
            }
        }
    }
    out.finish(section.color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_days_follow_excel_calendars() {
        let civil = |system: DateSystem, day| system.civil(day);
        assert_eq!(civil(DateSystem::Date1900, 45306), (2024, 1, 15, 1));
        assert_eq!(civil(DateSystem::Date1900, 59), (1900, 2, 28, 2));
        assert_eq!(civil(DateSystem::Date1900, 61), (1900, 3, 1, 4));
        assert_eq!(civil(DateSystem::Date1900, 2_958_465).0, 9999);
        assert_eq!(civil(DateSystem::Date1904, 0), (1904, 1, 1, 5));
        assert_eq!(civil(DateSystem::Date1904, 43844), (2024, 1, 15, 1));
    }
}
