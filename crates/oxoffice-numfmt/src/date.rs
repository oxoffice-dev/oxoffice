//! Dates and times from serial day numbers.

use crate::decimal::Decimal;
use crate::era;
use crate::names::{self, Language, Names};
use crate::parse::{self, Section, SystemFormat, Token, Unit};
use crate::{DateSystem, FormatOptions, Formatted, Locale, Out};

impl Locale {
    /// The names a code takes when no locale tag names a language.
    fn names(self) -> &'static Names {
        match self {
            Locale::EnUs => &names::ENGLISH,
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

/// A calendar date and time of day: what a serial day number stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DateTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millisecond: u32,
}

const MILLISECONDS_PER_DAY: i64 = 86_400_000;

impl DateSystem {
    /// The date and time `serial` stands for, rounded to the millisecond (a
    /// cell shows no finer time). `None` for a serial no calendar date
    /// names: one that is negative, past 9999-12-31 or not finite, and the
    /// 1900 system's serial 60, Excel's 1900-02-29. The 1900 system's day 0,
    /// which Excel shows as 1900-01-00, is 1899-12-31.
    pub fn to_datetime(self, serial: f64) -> Option<DateTime> {
        if !serial.is_finite() || serial < 0.0 {
            return None;
        }
        let units = (serial * MILLISECONDS_PER_DAY as f64).round() as i64;
        let day = units / MILLISECONDS_PER_DAY;
        if day > self.last_day() || (self == DateSystem::Date1900 && day == 60) {
            return None;
        }
        let (year, month, date) = match (self, day) {
            (DateSystem::Date1900, 0) => (1899, 12, 31),
            _ => {
                let (year, month, date, _) = self.civil(day);
                (year, month, date)
            }
        };
        let within = units % MILLISECONDS_PER_DAY;
        Some(DateTime {
            year: year as i32,
            month,
            day: date,
            hour: (within / 3_600_000) as u32,
            minute: (within / 60_000 % 60) as u32,
            second: (within / 1000 % 60) as u32,
            millisecond: (within % 1000) as u32,
        })
    }

    /// The serial of a date and time; `None` for one that is not a date
    /// (a 31 April, a 25th hour), before the system's day 0 (1899-12-31 in
    /// the 1900 system, 1904-01-01 in the 1904 one) or past 9999-12-31.
    pub fn to_serial(self, datetime: DateTime) -> Option<f64> {
        let DateTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            millisecond,
        } = datetime;
        let year = i64::from(year);
        if !(1..=12).contains(&month)
            || day == 0
            || hour > 23
            || minute > 59
            || second > 59
            || millisecond > 999
        {
            return None;
        }
        let next_month = if month == 12 {
            days(year + 1, 1, 1)
        } else {
            days(year, month + 1, 1)
        };
        if days(year, month, day) >= next_month {
            return None;
        }
        let serial_day = match self {
            DateSystem::Date1900 => {
                let after = days(year, month, day) - days(1899, 12, 31);
                // Excel counts a 1900-02-29: every day from 1 March is one
                // serial later.
                if after < 60 { after } else { after + 1 }
            }
            DateSystem::Date1904 => days(year, month, day) - days(1904, 1, 1),
        };
        if !(0..=self.last_day()).contains(&serial_day) {
            return None;
        }
        let time = i64::from(hour) * 3_600_000
            + i64::from(minute) * 60_000
            + i64::from(second) * 1000
            + i64::from(millisecond);
        Some(serial_day as f64 + time as f64 / MILLISECONDS_PER_DAY as f64)
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
    let exact = magnitude.as_ratio().and_then(|(numerator, denominator)| {
        let twice = numerator.checked_mul(2 * per_day as u128)?;
        Some((twice.checked_add(denominator)? / (2 * denominator)) as i64)
    });
    let units = exact.unwrap_or_else(|| (magnitude.to_f64() * per_day as f64).round() as i64);
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
    // The tag of the section names the language; `Other` is a language this
    // crate has no names for, shown in the locale's.
    let names = match section.language {
        Some(Language::Known(names)) => names,
        _ => options.locale.names(),
    };

    // Under a Japanese tag the era of the date; under any other, none.
    let era = names.is_japanese().then(|| era::of(year, month, date));
    let mut out = Out::default();
    if negative && sign {
        out.push('-');
    }
    // Zeros pushed by hand: a `[hhh...]` as wide as the code allows is no
    // width `format!` takes.
    let padded = |out: &mut Out, value: i64, width: usize| {
        let digits = value.to_string();
        for _ in digits.len()..width {
            out.push('0');
        }
        out.push_str(&digits);
    };
    for token in tokens {
        match *token {
            Token::Year(count) if count <= 2 => padded(&mut out, year % 100, 2),
            Token::Year(_) => padded(&mut out, year, 4),
            Token::EraYear(count) => match era {
                Some((_, _, era_year)) => padded(&mut out, era_year, count.min(2)),
                None => padded(&mut out, year, 4),
            },
            Token::Era(count) => {
                if let Some((initial, name, _)) = era {
                    out.push_str(era::text(count, initial, name));
                }
            }
            Token::Month(1) => padded(&mut out, i64::from(month), 1),
            Token::Month(2) => padded(&mut out, i64::from(month), 2),
            Token::Month(3) => out.push_str(names.short_month(month)),
            Token::Month(5) => {
                out.push_str(&names.month(month).chars().take(1).collect::<String>())
            }
            Token::Month(_) => out.push_str(names.month(month)),
            Token::Day(count @ (1 | 2)) => padded(&mut out, i64::from(date), count),
            Token::Day(3) => out.push_str(names.short_weekday(weekday)),
            Token::Day(_) => out.push_str(names.weekday(weekday)),
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
                if section.language.is_some() {
                    // Under a language tag, every spelling (`A/P`, `am/pm`) shows
                    // the designator of the language.
                    out.push_str(names.day_period(!morning));
                } else {
                    match short {
                        Some((am, pm)) => out.push(if morning { am } else { pm }),
                        None => out.push_str(names.day_period(!morning)),
                    }
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

    #[test]
    fn serials_and_dates_convert_both_ways() {
        let at = |year, month, day, hour, minute, second, millisecond| DateTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            millisecond,
        };
        let (d1900, d1904) = (DateSystem::Date1900, DateSystem::Date1904);
        for (system, serial, datetime) in [
            (d1900, 46347.0, at(2026, 11, 21, 0, 0, 0, 0)),
            (d1900, 46_346.333_333_333_34, at(2026, 11, 20, 8, 0, 0, 0)),
            (d1900, 0.5, at(1899, 12, 31, 12, 0, 0, 0)),
            (d1900, 59.0, at(1900, 2, 28, 0, 0, 0, 0)),
            (d1900, 61.0, at(1900, 3, 1, 0, 0, 0, 0)),
            (d1900, 1.0625, at(1900, 1, 1, 1, 30, 0, 0)),
            (d1900, 2_958_465.0, at(9999, 12, 31, 0, 0, 0, 0)),
            (d1904, 0.0, at(1904, 1, 1, 0, 0, 0, 0)),
            (d1904, 0.573263888888889, at(1904, 1, 1, 13, 45, 30, 0)),
            (d1904, 44885.0, at(2026, 11, 21, 0, 0, 0, 0)),
        ] {
            assert_eq!(system.to_datetime(serial), Some(datetime), "{serial}");
            let back = system.to_serial(datetime).expect("a date of the system");
            assert!((back - serial).abs() < 1e-8, "{datetime:?}: {back}");
        }
        for (system, serial) in [(d1900, 60.0), (d1900, -1.0), (d1900, 2_958_466.0)] {
            assert_eq!(system.to_datetime(serial), None, "{serial}");
        }
        for (system, datetime) in [
            (d1900, at(1899, 12, 30, 0, 0, 0, 0)),
            (d1904, at(1903, 12, 31, 0, 0, 0, 0)),
            (d1900, at(2026, 4, 31, 0, 0, 0, 0)),
            (d1900, at(2026, 2, 29, 0, 0, 0, 0)),
            (d1900, at(2026, 1, 1, 24, 0, 0, 0)),
        ] {
            assert_eq!(system.to_serial(datetime), None, "{datetime:?}");
        }
    }
}
